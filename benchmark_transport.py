"""Compare native mesh streams, including byte-exact expanded geometry."""

import argparse
import hashlib
import json
from pathlib import Path
import statistics
import struct
import subprocess
import tempfile
import time

import numpy as np

from benchmark_native import canonical_triangle


def inspect(path, ignore_triangle_order=False):
    digest = hashlib.sha256()
    oriented = []
    draws = 0
    positions = bytearray()
    payload_bytes = 0
    triangles = 0
    index_width = 4

    def add_triangles(data):
        if ignore_triangle_order:
            if isinstance(data, np.ndarray):
                data = data.tobytes()
            oriented.extend(canonical_triangle(data, start) for start in range(0, len(data), 36))
        else:
            digest.update(data)

    with path.open("rb") as source:
        while header := source.read(4):
            size, = struct.unpack("<I", header)
            event = json.loads(source.read(size))
            length = event["payload_bytes"]
            payload = source.read(length)
            if len(payload) != length:
                raise ValueError("Truncated mesh frame")
            payload_bytes += length
            kind = event["type"]
            if kind == "indexed":
                positions = bytearray()
                index_width = event.get("index_width", 4)
                if index_width not in (2, 4):
                    raise ValueError("Invalid mesh index width")
                draws += 1
            elif kind == "positions":
                positions.extend(payload)
            elif kind == "indices":
                index = np.frombuffer(payload, dtype=f"<u{index_width}")
                point = np.frombuffer(positions, dtype="<f4").reshape(-1, 3)
                add_triangles(point[index])
                triangles += len(index) // 3
            elif kind == "part":
                add_triangles(payload)
                triangles += event["triangles"]
                draws += 1
            elif kind == "done":
                if event["triangles"] != triangles:
                    raise ValueError("Triangle count does not match")
                if ignore_triangle_order:
                    for triangle in sorted(oriented):
                        digest.update(triangle)
                return (triangles, digest.hexdigest()), (payload_bytes, draws)
    raise ValueError("Missing completion frame")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("packed", type=Path)
    parser.add_argument("indexed", type=Path)
    parser.add_argument("model", type=Path)
    parser.add_argument("--runs", type=int, default=7)
    parser.add_argument("--ignore-triangle-order", action="store_true",
                        help="Compare oriented triangles while allowing a different emission order")
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")
    builds = [("packed", args.packed.resolve()), ("indexed", args.indexed.resolve())]
    times = {name: [] for name, _ in builds}
    checks = {}
    with tempfile.TemporaryDirectory(prefix="vtx-stream-") as temp:
        for trial in range(args.runs):
            for name, exe in (builds if trial % 2 == 0 else reversed(builds)):
                path = Path(temp) / f"{name}.mesh"
                with path.open("wb") as output:
                    start = time.perf_counter()
                    subprocess.run([exe, args.model.resolve(), "--stream"], stdout=output, check=True)
                    times[name].append((time.perf_counter() - start) * 1000)
                if trial == 0:
                    checks[name] = inspect(path, args.ignore_triangle_order)
    if checks["packed"][0] != checks["indexed"][0]:
        raise SystemExit(f"Triangle geometry differs: {checks}")
    comparison = "oriented" if args.ignore_triangle_order else "ordered"
    print(f"Exact {comparison} triangle match: {checks['packed'][0]}")
    for name, _ in builds:
        payload, draws = checks[name][1]
        print(f"{name}: {payload:,} payload bytes, {draws:,} completed draw batches, "
              f"{statistics.median(times[name]):.2f} ms median process plus stream")


if __name__ == "__main__":
    main()
