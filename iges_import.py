"""Translate IGES surfaces and solids and stream indexed triangles to VTX."""

import argparse
from array import array
import math
import os
import sys
from time import perf_counter

from mesh_frames import FRAME_BYTES, peak_memory, write_frame

BATCH_BYTES = 768 * 1024


def read_shape(path):
    from OCP.IFSelect import IFSelect_RetDone
    from OCP.IGESControl import IGESControl_Reader

    start = perf_counter()
    reader = IGESControl_Reader()
    if reader.ReadFile(str(path)) != IFSelect_RetDone:
        raise ValueError("Open CASCADE could not read the IGES file")
    parsed = perf_counter()
    if reader.TransferRoots() == 0:
        raise ValueError("IGES file contains no transferable shapes")
    shape = reader.OneShape()
    if shape.IsNull():
        raise ValueError("IGES transfer produced no shape")
    return shape, parsed - start, perf_counter() - parsed


def stream(path, deflection, output):
    start = perf_counter()
    from OCP.BRep import BRep_Tool
    from OCP.BRepMesh import BRepMesh_IncrementalMesh
    from OCP.TopAbs import TopAbs_FACE, TopAbs_REVERSED
    from OCP.TopExp import TopExp_Explorer
    from OCP.TopLoc import TopLoc_Location
    from OCP.TopoDS import TopoDS
    imported = perf_counter()
    shape, read, transfer = read_shape(path)
    write_frame(output, "meta", read_ms=round(read * 1000, 2),
                transfer_ms=round(transfer * 1000, 2))
    mesh_start = perf_counter()
    # Explore all faces, including free surfaces mixed with solids.
    BRepMesh_IncrementalMesh(shape, deflection, False, 0.5, True)
    meshed = perf_counter()
    points = array("f")
    indices = array("I")
    written = batches = count = skipped = 0
    writing = 0.0

    def flush():
        nonlocal written, batches, count, writing
        if not indices:
            return
        before = perf_counter()
        vertices, triangles = len(points) // 3, len(indices) // 3
        write_frame(output, "indexed", vertices=vertices, triangles=triangles)
        for kind, values in (("positions", points), ("indices", indices)):
            if sys.byteorder != "little":
                values.byteswap()
            data = memoryview(values).cast("B")
            for offset in range(0, len(data), FRAME_BYTES):
                write_frame(output, kind, data[offset:offset + FRAME_BYTES])
            written += len(data)
            data.release()
            del values[:]
        count += triangles
        batches += 1
        writing += perf_counter() - before

    explorer = TopExp_Explorer(shape, TopAbs_FACE)
    while explorer.More():
        face = TopoDS.Face_s(explorer.Current())
        explorer.Next()
        location = TopLoc_Location()
        mesh = BRep_Tool.Triangulation_s(face, location)
        if not mesh or not mesh.NbTriangles():
            skipped += 1
            continue
        n, t = mesh.NbNodes(), mesh.NbTriangles()
        if (len(points) + len(indices) + 3 * (n + t)) * 4 > BATCH_BYTES:
            flush()
        # OCCT node numbers are one-based. Reuse each node within its face.
        base = len(points) // 3 - 1
        transform = None if location.IsIdentity() else location.Transformation()
        for i in range(1, n + 1):
            point = mesh.Node(i)
            if transform is not None:
                point = point.Transformed(transform)
            points.extend((point.X(), point.Y(), point.Z()))
        reversed_face = face.Orientation() == TopAbs_REVERSED
        for i in range(1, t + 1):
            a, b, c = mesh.Triangle(i).Get()
            if reversed_face:
                b, c = c, b
            indices.extend((base + a, base + b, base + c))
    flush()
    if not count:
        raise ValueError("IGES file contains no tessellated faces")
    finished = perf_counter()
    write_frame(output, "done", triangles=count, total_ms=round((finished - imported) * 1000, 2),
                warning=f"{skipped} faces could not be tessellated." if skipped else "")
    output.flush()
    if os.environ.get("STEPFLASH_PROFILE"):
        print(f"IGES phases: dependencies={(imported-start)*1000:.2f} ms, read={read*1000:.2f} ms, "
              f"transfer={transfer*1000:.2f} ms, meshing={(meshed-mesh_start)*1000:.2f} ms, "
              f"extraction={(finished-meshed-writing)*1000:.2f} ms, writing={writing*1000:.2f} ms; "
              f"triangles={count}, batches={batches}, payload_bytes={written}, "
              f"peak_working_set_bytes={peak_memory()}", file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("path")
    parser.add_argument("--deflection", type=float, default=0.25)
    args = parser.parse_args()
    if not math.isfinite(args.deflection) or args.deflection <= 0:
        parser.error("--deflection must be finite and positive")
    # OCCT can print diagnostics to stdout. Keep binary frames on the
    # original descriptor and send library output to stderr.
    with os.fdopen(os.dup(sys.stdout.fileno()), "wb") as output:
        os.dup2(sys.stderr.fileno(), sys.stdout.fileno())
        try:
            stream(args.path, args.deflection, output)
        except (ImportError, OSError, RuntimeError, ValueError) as error:
            print(f"IGES import: {error}", file=sys.stderr)
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
