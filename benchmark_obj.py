"""Compare release mesh readers and verify the packed triangle positions."""

import argparse
import csv
import hashlib
from pathlib import Path
import statistics
import subprocess
import tempfile
import time


def run(executable, model, output=None):
    command = [str(executable), str(model)]
    if output is not None:
        command.append(str(output))
    start = time.perf_counter()
    result = subprocess.run(command, capture_output=True, text=True, check=True)
    wall = (time.perf_counter() - start) * 1000
    values = dict(field.split("=") for field in result.stdout.split())
    return float(values["total_ms"]), wall, int(values["triangles"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("vtx", type=Path)
    parser.add_argument("model", type=Path)
    parser.add_argument("--runs", type=int, default=12)
    parser.add_argument("--csv", type=Path, required=True)
    parser.add_argument("--baseline-name", default="RapidOBJ")
    parser.add_argument("--warm-cache", action="store_true",
                        help="Read the input before each timed process to prime the OS cache")
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")
    engines = [(args.baseline_name, args.baseline.resolve()), ("VTX", args.vtx.resolve())]
    model = args.model.resolve()
    checks = []
    with tempfile.TemporaryDirectory(prefix="vtx-compare-") as directory:
        for index, (label, exe) in enumerate(engines):
            output = Path(directory) / f"{index}.packed"
            _, _, count = run(exe, model, output)
            if output.stat().st_size != count * 36:
                raise SystemExit(f"{label} emitted an invalid packed triangle file")
            digest = hashlib.sha256()
            with output.open("rb") as source:
                while chunk := source.read(1024 * 1024):
                    digest.update(chunk)
            checks.append((count, digest.hexdigest()))
    if checks[0] != checks[1]:
        raise SystemExit(f"Triangle geometry or order differs: {checks}")
    print(f"Exact ordered triangle match: {checks[0][0]:,}; SHA-256 {checks[0][1]}", flush=True)

    rows = []
    warm_buffer = bytearray(4 * 1024 * 1024) if args.warm_cache else None
    for trial in range(args.runs):
        for label, exe in (engines if trial % 2 == 0 else reversed(engines)):
            if warm_buffer is not None:
                with model.open("rb", buffering=0) as source:
                    while source.readinto(warm_buffer):
                        pass
            total, wall, count = run(exe, model)
            if count != checks[0][0]:
                raise SystemExit(f"{label} produced a different triangle count during timing")
            rows.append((trial + 1, label, total, wall))
    with args.csv.open("w", encoding="utf-8", newline="") as output:
        writer = csv.writer(output)
        writer.writerow(("run", "engine", "read_to_triangles_ms", "process_wall_ms"))
        writer.writerows(rows)
    for label, _ in engines:
        samples = [row for row in rows if row[1] == label]
        total = statistics.median(row[2] for row in samples)
        wall = statistics.median(row[3] for row in samples)
        print(f"{label}: {total:.2f} ms read to triangles; {wall:.2f} ms process wall")


if __name__ == "__main__":
    main()
