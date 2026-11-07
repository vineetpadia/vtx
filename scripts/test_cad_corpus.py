"""Load public CAD samples, retain previews/diagnostics, and continue after failures.

Download first: python3 fetch_benchmark_models.py --manifest cad_benchmark_models.json --all
This measures full --inspect export time, not desktop or --check first-frame latency.
Successful imports still require visual review; warnings can indicate missing content.
"""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import signal
import statistics
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from fetch_benchmark_models import local_path, matches


def execute(command, timeout):
    start = time.perf_counter()
    process = subprocess.Popen(
        command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, errors="replace",
        start_new_session=os.name == "posix",
    )
    timed_out = False
    try:
        stdout, stderr = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        timed_out = True
        # Stop converter/Python children too, so one bad input cannot skew later runs.
        if os.name == "posix":
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass  # The process group exited at the timeout boundary.
        else:
            process.kill()
        stdout, stderr = process.communicate()
    return {
        "status": "timeout" if timed_out else "loaded" if process.returncode == 0 else "failed",
        "wall_ms": (time.perf_counter() - start) * 1000,
        "returncode": process.returncode,
        "stderr": stderr,
    }, stdout


def main():
    root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=root / "cad_benchmark_models.json")
    parser.add_argument("--directory", type=Path, default=root / ".benchmark-models")
    parser.add_argument("--binary", type=Path, default=root / "native/target/release/examples/vtx")
    parser.add_argument("--output", type=Path, default=root / "dist/cad-corpus")
    parser.add_argument("--dataset", action="append", help="Select a dataset ID; repeat as needed")
    parser.add_argument("--runs", type=int, default=1)
    parser.add_argument("--timeout", type=float, default=60)
    args = parser.parse_args()
    if args.runs < 1 or args.timeout <= 0:
        parser.error("--runs and --timeout must be positive")
    datasets = json.loads(args.manifest.read_text())["datasets"]
    known = {dataset["id"] for dataset in datasets}
    if set(args.dataset or []) - known:
        parser.error("Unknown dataset ID")
    if args.output.exists() and any(args.output.iterdir()):
        parser.error("--output must be new or empty so previous results are preserved")
    binary = args.binary.resolve(strict=True)
    args.output.mkdir(parents=True, exist_ok=True)
    report = {
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "system": platform.platform(), "binary": str(binary),
        "manifest": str(args.manifest.resolve()),
        "method": "Fresh processes, --inspect --views plan --size 512, including import, GPU startup, PNG and JSON export, and shutdown. OS caches may be warm. No VTX geometry cache. Not --check or desktop latency. Loaded does not establish visual completeness.",
        "results": [],
    }
    with binary.open("rb") as source:
        report["binary_sha256"] = hashlib.file_digest(source, "sha256").hexdigest()
    report["manifest_sha256"] = hashlib.sha256(args.manifest.read_bytes()).hexdigest()
    output = args.output.resolve()
    for dataset in datasets:
        if args.dataset and dataset["id"] not in args.dataset:
            continue
        for model in dataset["models"]:
            source = local_path(args.directory, model["path"])
            record = {"model": model["path"], "bytes": model["bytes"],
                      "sha256": model["sha256"], "tags": model.get("tags", []), "runs": []}
            if not matches(source, model):
                record["status"] = "missing_or_checksum_mismatch"
            else:
                for run in range(args.runs):
                    folder = output / "previews" / model["path"] / f"run-{run + 1}"
                    result, stdout = execute([
                        str(binary), "--inspect", str(source.resolve()),
                        "--output", str(folder), "--views", "plan", "--size", "512",
                    ], args.timeout)
                    result["artifacts"] = str(folder.relative_to(output))
                    if result["status"] == "loaded":
                        try:
                            manifest = json.loads(stdout)
                            report["gpu"] = manifest.get("gpu")
                            result.update({key: manifest.get(key) for key in
                                           ("source_info", "geometry", "units", "bounds")})
                        except ValueError:
                            result["status"] = "invalid_json"
                            result["stdout"] = stdout
                    record["runs"].append(result)
                    if result["status"] != "loaded":
                        break
                record["status"] = record["runs"][-1]["status"]
                if record["status"] == "loaded":
                    record["median_inspect_wall_ms"] = statistics.median(r["wall_ms"] for r in record["runs"])
                    record["has_import_warnings"] = any(
                        r["geometry"].get("import_issues", 0) or r["source_info"].get("warnings")
                        for r in record["runs"]
                    )
            report["results"].append(record)
            # Persist after each input, including failures, rather than losing partial work.
            (output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
            timing = record.get("median_inspect_wall_ms")
            detail = f" {timing:.0f} ms" if timing is not None else ""
            if record.get("has_import_warnings"):
                detail += " (import warnings; review preview)"
            print(f"{record['status']}: {model['path']}{detail}", flush=True)
    failed = sum(r["status"] != "loaded" for r in report["results"])
    print(f"{len(report['results']) - failed}/{len(report['results'])} loaded; report: {output / 'results.json'}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
