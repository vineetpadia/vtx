"""Compare release mesh readers and verify the packed triangle positions."""

import argparse
import array
import csv
import hashlib
from pathlib import Path
import statistics
import subprocess
import sys
import tempfile
import time


def geometry_digest(path, mode):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(36 * 65536):
            if mode == "cyclic":
                words = array.array("I", chunk)
                if sys.byteorder != "little":
                    words.byteswap()
                for index, value in enumerate(words):
                    if value == 0x80000000:
                        words[index] = 0
                if sys.byteorder != "little":
                    words.byteswap()
                chunk = words.tobytes()
                for offset in range(0, len(chunk), 36):
                    triangle = chunk[offset:offset + 36]
                    digest.update(min(
                        triangle,
                        triangle[12:] + triangle[:12],
                        triangle[24:] + triangle[:24],
                    ))
            else:
                digest.update(chunk)
    return digest.hexdigest()


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
    parser.add_argument("--geometry-check", choices=("exact", "cyclic"), default="exact",
                        help="Cyclic mode normalizes signed zero and triangle starting vertices, preserving face order and winding")
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
            checks.append((count, geometry_digest(output, args.geometry_check)))
    if checks[0] != checks[1]:
        raise SystemExit(f"Triangle geometry or order differs: {checks}")
    label = "Exact ordered triangle match" if args.geometry_check == "exact" else "Cyclic ordered triangle match"
    print(f"{label}: {checks[0][0]:,}; SHA-256 {checks[0][1]}", flush=True)

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
        columns = ("run", "engine", "read_to_triangles_ms", "process_wall_ms")
        if args.geometry_check == "cyclic":
            writer.writerow((*columns, "geometry_check"))
            writer.writerows((*row, args.geometry_check) for row in rows)
        else:
            writer.writerow(columns)
            writer.writerows(rows)
    for label, _ in engines:
        samples = [row for row in rows if row[1] == label]
        total = statistics.median(row[2] for row in samples)
        wall = statistics.median(row[3] for row in samples)
        print(f"{label}: {total:.2f} ms read to triangles; {wall:.2f} ms process wall")


if __name__ == "__main__":
    main()
