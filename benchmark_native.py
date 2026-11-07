"""Benchmark two native STEP cores and verify their oriented triangle output."""

import argparse
import csv
import hashlib
from pathlib import Path
import statistics
import struct
import subprocess
import tempfile
import time


def run(executable, step, output=None):
    command = [str(executable), str(step)]
    if output is not None:
        command.append(str(output))
    start = time.perf_counter()
    result = subprocess.run(command, capture_output=True, text=True, check=True)
    wall_ms = (time.perf_counter() - start) * 1000
    metrics = dict(pair.split("=") for pair in result.stdout.strip().split())
    return {"wall_ms": wall_ms, **metrics}


def canonical_triangle(data, start):
    a, b, c = (data[start + i:start + i + 12] for i in (0, 12, 24))
    # Canonicalize the starting vertex, preserving winding and duplicates.
    return min(a + b + c, b + c + a, c + a + b)


def geometry_digest(stl):
    data = stl.read_bytes()
    count, = struct.unpack_from("<I", data, 80)
    if len(data) != 84 + count * 50 or not count:
        raise ValueError(f"Invalid or empty binary STL: {stl}")
    triangles = [canonical_triangle(data, start) for start in range(96, len(data), 50)]
    digest = hashlib.sha256()
    for triangle in sorted(triangles):
        digest.update(triangle)
    return count, digest.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("foxtrot", type=Path)
    parser.add_argument("stepflash", type=Path)
    parser.add_argument("step", type=Path)
    parser.add_argument("--runs", type=int, default=12)
    parser.add_argument("--csv", type=Path, required=True)
    parser.add_argument("--baseline-name", default="Foxtrot")
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")
    engines = [(args.baseline_name, args.foxtrot.resolve()), ("STEPFlash", args.stepflash.resolve())]
    step = args.step.resolve()
    with tempfile.TemporaryDirectory(prefix="stepflash-compare-") as directory:
        fingerprints = []
        for index, (_, executable) in enumerate(engines):
            stl = Path(directory) / f"{index}.stl"
            run(executable, step, stl)
            fingerprints.append(geometry_digest(stl))
        if fingerprints[0] != fingerprints[1]:
            raise SystemExit(f"Geometry differs: {fingerprints}")

    rows = []
    for trial in range(args.runs):
        # Alternate order and start a new process on each trial.
        order = engines if trial % 2 == 0 else reversed(engines)
        for label, executable in order:
            metrics = run(executable, step)
            if int(metrics["triangles"]) != fingerprints[0][0]:
                raise SystemExit(f"{label} emitted a different triangle count during timing")
            rows.append({"run": trial + 1, "engine": label, **metrics})
    with args.csv.open("w", newline="", encoding="utf-8") as output:
        writer = csv.DictWriter(output, fieldnames=rows[0].keys())
        writer.writeheader()
        writer.writerows(rows)
    input_digest = hashlib.sha256()
    with step.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            input_digest.update(chunk)
    print(f"Input SHA-256: {input_digest.hexdigest()}")
    print(f"Identical oriented triangles: {fingerprints[0][0]:,}; SHA-256: {fingerprints[0][1]}")
    for label, _ in engines:
        samples = [row for row in rows if row["engine"] == label]
        for key in ("total_ms", "wall_ms"):
            values = [float(row[key]) for row in samples]
            print(f"{label} {key}: median {statistics.median(values):.2f}, "
                  f"range {min(values):.2f}–{max(values):.2f}")
    print(f"Raw timings: {args.csv}")


if __name__ == "__main__":
    main()
