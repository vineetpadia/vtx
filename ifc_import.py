"""IfcOpenShell worker for VTX's indexed mesh stream."""

import argparse
import os
import sys
from time import perf_counter

from mesh_frames import FRAME_BYTES, peak_memory, write_frame


BATCH_BYTES = 768 * 1024
def stream(path, workers, output):
    start = perf_counter()
    import ifcopenshell
    import ifcopenshell.geom
    import numpy as np
    dependencies = perf_counter()
    model = ifcopenshell.open(path)
    opened = perf_counter()
    settings = ifcopenshell.geom.settings()
    # Keep world placement and Open CASCADE's opening subtraction.
    settings.set(settings.USE_WORLD_COORDS, True)
    iterator = ifcopenshell.geom.iterator(settings, model, workers, exclude=["IfcOpeningElement"])
    if not iterator.initialize():
        raise ValueError("IFC contains no supported polygon geometry")
    initialized = perf_counter()
    write_frame(output, "meta", read_ms=round((opened - dependencies) * 1000, 3))
    positions = bytearray()
    indices = bytearray()
    batches = triangles = shapes = payload_bytes = 0
    packing = writing = 0.0

    def flush():
        nonlocal batches, triangles, payload_bytes, writing
        if not indices:
            return
        before = perf_counter()
        write_frame(output, "indexed", vertices=len(positions) // 12, triangles=len(indices) // 12)
        for kind, buffer in (("positions", positions), ("indices", indices)):
            data = memoryview(buffer)
            for offset in range(0, len(data), FRAME_BYTES):
                write_frame(output, kind, data[offset:offset + FRAME_BYTES])
            data.release()
        triangles += len(indices) // 12
        payload_bytes += len(positions) + len(indices)
        batches += 1
        positions.clear()
        indices.clear()
        writing += perf_counter() - before

    while True:
        before = perf_counter()
        shape = iterator.get()
        geometry = shape.geometry
        vertices = np.frombuffer(geometry.verts_buffer, dtype=np.float64)
        faces = np.frombuffer(geometry.faces_buffer, dtype=np.int32)
        if len(faces):
            if len(vertices) % 3 or len(faces) % 3:
                raise ValueError("IFC geometry contains incomplete triangles")
            if len(positions) + len(indices) + (len(vertices) + len(faces)) * 4 > BATCH_BYTES:
                packing += perf_counter() - before
                flush()
                before = perf_counter()
            base = len(positions) // 12
            positions.extend(memoryview(vertices.astype("<f4")).cast("B"))
            indices.extend(memoryview(np.add(faces, base, dtype="<u4", casting="unsafe")).cast("B"))
            shapes += 1
        packing += perf_counter() - before
        if not iterator.next():
            break
    flush()
    if not triangles:
        raise ValueError("IFC contains no triangle faces")
    elapsed = perf_counter()
    write_frame(output, "done", triangles=triangles,
                total_ms=round((elapsed - dependencies) * 1000, 3))
    output.flush()
    if os.environ.get("STEPFLASH_PROFILE"):
        print(f"IFC phases: dependencies={(dependencies-start)*1000:.2f} ms, "
              f"parse={(opened-dependencies)*1000:.2f} ms, "
              f"initial_geometry={(initialized-opened)*1000:.2f} ms, "
              f"remaining_geometry={(elapsed-initialized-packing-writing)*1000:.2f} ms, "
              f"packing={packing*1000:.2f} ms, writing={writing*1000:.2f} ms; "
              f"workers={workers}, shapes={shapes}, triangles={triangles}, "
              f"batches={batches}, payload_bytes={payload_bytes}, "
              f"peak_working_set_bytes={peak_memory()}", file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("path")
    parser.add_argument("--workers", type=int, default=min(8, os.cpu_count() or 1))
    args = parser.parse_args()
    if args.workers < 1:
        parser.error("--workers must be positive")
    try:
        stream(args.path, args.workers, sys.stdout.buffer)
    except (ImportError, OSError, RuntimeError, ValueError) as error:
        print(f"IFC import: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
