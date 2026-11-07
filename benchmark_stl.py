"""Compare VTX's STL loader and pyvista-stl on a shared binary grid."""

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
import pyvista_stl


def write_grid(path):
    """A 1000-by-1000 grid with 2M triangles and 1,002,001 vertices."""
    x = np.arange(1000, dtype=np.float32)
    records = np.zeros(2000, dtype=[
        ("normal", "<f4", (3,)), ("positions", "<f4", (3, 3)), ("attribute", "<u2"),
    ])
    vertices = records["positions"]
    records["normal"][:, 2] = 1
    for slot, offset in enumerate((0, 1, 1)):
        vertices[::2, slot, 0] = x + offset
    for slot, offset in enumerate((0, 1, 0)):
        vertices[1::2, slot, 0] = x + offset
    with path.open("wb") as output:
        output.write(bytes(80) + struct.pack("<I", 2_000_000))
        for y in range(1000):
            vertices[::2, :, 1] = (y, y, y + 1)
            vertices[1::2, :, 1] = (y, y + 1, y + 1)
            output.write(records.tobytes())


NODE_BENCHMARK = """
import { readStl } from './stl.mjs';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
const [path, count] = process.argv.slice(1);
const samples = [];
let digest;
for (let trial = -2; trial < Number(count); trial++) {
    global.gc();
    const start = performance.now();
    const bytes = readFileSync(path);
    const loaded = performance.now();
    const positions = readStl(bytes);
    const elapsed = performance.now();
    if (trial >= 0) samples.push({total: elapsed - start, decode: elapsed - loaded});
    if (!digest) digest = createHash('sha256').update(Buffer.from(positions.buffer,
        positions.byteOffset, positions.byteLength)).digest('hex');
}
console.log(JSON.stringify({samples, digest}));
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--node", default="node")
    parser.add_argument("--runs", type=int, default=7)
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")
    with tempfile.TemporaryDirectory(prefix="vtx-stl-") as temp:
        path = Path(temp) / "grid.stl"
        write_grid(path)
        vtx = json.loads(subprocess.check_output([
            args.node, "--expose-gc", "--input-type=module", "-e", NODE_BENCHMARK,
            str(path), str(args.runs),
        ], text=True, cwd=Path(__file__).resolve().parent))
        indexed, packed = [], []
        for trial in range(-2, args.runs):
            start = time.perf_counter()
            vertices, indices = pyvista_stl.read(path, threads=0)
            read = time.perf_counter()
            expanded = vertices[indices]
            end = time.perf_counter()
            if trial == -2:
                digest = hashlib.sha256(memoryview(expanded).cast("B")).hexdigest()
                if digest != vtx["digest"]:
                    raise SystemExit("Triangle positions or winding differ")
                print(f"Exact ordered position match: {len(indices):,} triangles, SHA-256 {digest}")
            if trial >= 0:
                indexed.append((read - start) * 1000)
                packed.append((end - start) * 1000)
            del vertices, indices, expanded
    print(f"VTX file read + packed positions: {statistics.median(s['total'] for s in vtx['samples']):.2f} ms")
    print(f"VTX decode from memory: {statistics.median(s['decode'] for s in vtx['samples']):.2f} ms")
    print(f"pyvista-stl file read + indexed mesh: {statistics.median(indexed):.2f} ms")
    print(f"pyvista-stl file read + indexed mesh + expand: {statistics.median(packed):.2f} ms")


if __name__ == "__main__":
    main()
