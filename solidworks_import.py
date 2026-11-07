"""Stream SolidWorks saved display geometry to VTX."""

import argparse
from array import array
from itertools import chain
import json
import os
from pathlib import Path
import struct
import sys
from time import perf_counter

from mesh_frames import FRAME_BYTES, peak_memory, write_frame
from vendor.sldprt.assembly import AssemblyError, read_assembly
from vendor.sldprt.blocks import find_block, read_blocks
from vendor.sldprt.mesh import Mesh
from vendor.sldprt.savedmesh import MESH_BLOCKS, iter_tables


MAX_BUNDLE_HEADER = 64 * 1024


def load_bundle(path):
    with path.open("rb") as source:
        prefix = source.read(4)
        if len(prefix) != 4:
            raise ValueError("Truncated SolidWorks bundle")
        length, = struct.unpack("<I", prefix)
        if not 0 < length <= MAX_BUNDLE_HEADER:
            raise ValueError("Invalid SolidWorks bundle header")
        entries = json.loads(source.read(length))
        if not isinstance(entries, list) or not 1 <= len(entries) <= 512:
            raise ValueError("Invalid SolidWorks bundle entries")
        main = None
        companions = {}
        remaining = path.stat().st_size - 4 - length
        for entry in entries:
            if not isinstance(entry, dict):
                raise ValueError("Invalid SolidWorks bundle entry")
            name, size = entry.get("name"), entry.get("size")
            if not isinstance(name, str) or not name or "/" in name or "\\" in name:
                raise ValueError("Invalid SolidWorks component name")
            if type(size) is not int or not 0 < size <= remaining:
                raise ValueError("Invalid SolidWorks component size")
            contents = source.read(size)
            if len(contents) != size:
                raise ValueError("Truncated SolidWorks component")
            remaining -= size
            name = name.casefold()
            if name.endswith(".sldasm") and main is None:
                main = contents
            elif name.endswith(".sldprt") and name not in companions:
                companions[name] = contents
            else:
                raise ValueError("Select one assembly and uniquely named part files")
        if remaining or main is None:
            raise ValueError("Invalid SolidWorks assembly bundle")
        return main, companions


def decode_display(blocks):
    for name in MESH_BLOCKS:
        block = find_block(blocks, name)
        if block is None:
            continue
        data = block.inflate()
        mesh = Mesh()
        vertex_ids = {}
        for table in iter_tables(data):
            start, _ = table.descriptors[1]
            for length in table.lengths:
                strip = []
                for x, y, z in struct.iter_unpack("<3f", memoryview(data)[start:start + length * 12]):
                    point = (x * 1000.0, y * 1000.0, z * 1000.0)
                    # Same weld grid and first occurrence as the reference reader.
                    px, py, pz = point
                    key = (round(px * 10_000.0), round(py * 10_000.0), round(pz * 10_000.0))
                    index = vertex_ids.get(key)
                    if index is None:
                        index = len(mesh.vertices)
                        vertex_ids[key] = index
                        mesh.vertices.append(point)
                    strip.append(index)
                start += length * 12
                for i in range(length - 2):
                    a, b, c = strip[i], strip[i + 1], strip[i + 2]
                    if a != b and a != c and b != c:
                        mesh.triangles.append((a, c, b) if i & 1 else (a, b, c))
        if mesh.triangles:
            return mesh
    return None


def import_mesh(path):
    start = perf_counter()
    extension = path.suffix.lower()
    if extension == ".sldbundle":
        data, companions = load_bundle(path)
    else:
        data, companions = path.read_bytes(), {}
    read = perf_counter()
    blocks = read_blocks(data)
    scanned = perf_counter()
    warning = ""
    if extension in (".sldasm", ".sldbundle"):
        resolve = (lambda name: companions.get(name.casefold())) if companions else None
        try:
            mesh, _, summary = read_assembly(blocks, resolve=resolve, decode_part=decode_display)
            warning = summary.summary()
            if not summary.complete:
                warning = "Partial assembly: " + warning
        except AssemblyError:
            mesh = decode_display(blocks)
            warning = "Saved assembly display snapshot. Component completeness is unknown."
    else:
        mesh = decode_display(blocks)
    if mesh is None:
        raise ValueError("No supported SolidWorks display geometry. Save the display mesh or export STEP.")
    return mesh, warning, (read - start, scanned - read, perf_counter() - scanned)


def stream(path, output):
    mesh, warning, phases = import_mesh(path)
    started = perf_counter()
    count = len(mesh.triangles)
    write_frame(output, "meta", read_ms=round(sum(phases[:2]) * 1000, 3))
    write_frame(output, "indexed", vertices=len(mesh.vertices), triangles=count)
    for kind, code, records in (("positions", "f", mesh.vertices), ("indices", "I", mesh.triangles)):
        step = FRAME_BYTES // 12
        for offset in range(0, len(records), step):
            values = array(code, chain.from_iterable(records[offset:offset + step]))
            if sys.byteorder != "little":
                values.byteswap()
            write_frame(output, kind, memoryview(values).cast("B"))
    write_frame(output, "done", triangles=count, total_ms=round(sum(phases) * 1000, 3),
                warning=warning)
    output.flush()
    if os.environ.get("STEPFLASH_PROFILE"):
        read, scan, geometry = (value * 1000 for value in phases)
        print(f"SolidWorks phases: file={read:.2f} ms, directory={scan:.2f} ms, "
              f"geometry={geometry:.2f} ms, stream={(perf_counter()-started)*1000:.2f} ms; "
              f"triangles={count}, peak_working_set_bytes={peak_memory()}", file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("file", type=Path)
    args = parser.parse_args()
    try:
        stream(args.file, sys.stdout.buffer)
    except (OSError, ValueError, AssemblyError, OverflowError) as error:
        print(f"SolidWorks import: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
