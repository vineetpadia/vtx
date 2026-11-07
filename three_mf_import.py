"""Read 3MF core meshes and build placements into VTX indexed frames."""

import argparse
from array import array
from dataclasses import dataclass, field
import math
import os
import sys
from time import perf_counter
from urllib.parse import unquote, urlsplit
from xml.parsers import expat
from zipfile import BadZipFile, ZipFile

from mesh_frames import FRAME_BYTES, peak_memory, write_frame


CORE = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02}"
MODEL, RESOURCES, OBJECT, VERTICES, VERTEX, TRIANGLES, TRIANGLE, COMPONENTS, COMPONENT, BUILD, ITEM = (
    CORE + tag for tag in ("model", "resources", "object", "vertices", "vertex",
                           "triangles", "triangle", "components", "component", "build", "item")
)
START_REL = "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"
UNITS = {"micron": .001, "millimeter": 1, "centimeter": 10, "inch": 25.4,
         "foot": 304.8, "meter": 1000}
IDENTITY = (1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0.)


def finite(value):
    number = float(value)
    if not math.isfinite(number):
        raise ValueError("Non-finite 3MF coordinate or transform")
    return number


def placement(attrs):
    matrix = tuple(map(finite, attrs["transform"].split())) if "transform" in attrs else IDENTITY
    if len(matrix) != 12:
        raise ValueError("A 3MF transform must have 12 coefficients")
    if any(key.endswith("}path") for key in attrs):
        raise ValueError("Cross-document 3MF component references are unsupported")
    return int(attrs["objectid"]), matrix


def compose(local, parent):
    if local == IDENTITY:
        return parent
    if parent == IDENTITY:
        return local
    return tuple(
        local[row] * parent[col] + local[row+1] * parent[col+3] +
        local[row+2] * parent[col+6] + (parent[col+9] if row == 9 else 0)
        for row in (0, 3, 6, 9) for col in range(3)
    )


def determinant(matrix):
    a, b, c, d, e, f, g, h, i = matrix[:9]
    return a * (e*i-f*h) - b * (d*i-f*g) + c * (d*h-e*g)


@dataclass
class ModelObject:
    points: array = field(default_factory=lambda: array("d"))
    indices: array = field(default_factory=lambda: array("I"))
    components: list = field(default_factory=list)


def parse_xml(source, start, end=None):
    parser = expat.ParserCreate(namespace_separator="}")
    parser.StartElementHandler = start
    parser.EndElementHandler = end

    def reject_doctype(*_args):
        raise ValueError("DTDs are unsupported in 3MF")

    parser.StartDoctypeDeclHandler = reject_doctype
    while chunk := source.read(64 * 1024):
        parser.Parse(chunk, False)
    parser.Parse(b"", True)


def model_part(archive):
    info = archive.getinfo("_rels/.rels")
    if info.file_size > 64 * 1024:
        raise ValueError("3MF relationship file is too large")
    targets = []

    def relationship(tag, attrs):
        if tag.endswith("}Relationship") and attrs.get("Type") == START_REL:
            if attrs.get("TargetMode", "Internal") != "Internal":
                raise ValueError("External 3MF model relationships are unsupported")
            targets.append(attrs["Target"])

    with archive.open(info) as source:
        parse_xml(source, relationship)
    if len(targets) != 1:
        raise ValueError("Expected exactly one 3MF start model")
    uri = urlsplit(targets[0])
    name = unquote(uri.path).removeprefix("/")
    if (uri.scheme or uri.netloc or uri.query or uri.fragment or "\\" in name
            or any(part in ("", ".", "..") for part in name.split("/"))):
        raise ValueError("Invalid 3MF model part name")
    return archive.getinfo(name)


def read_model(path):
    start_time = perf_counter()
    objects = {}
    build = []
    stack = []
    current = None
    unit = 1

    def start(name, attrs):
        nonlocal current, unit
        parent = stack[-1] if stack else ""
        if name == VERTEX and parent == VERTICES and current is not None:
            x, y, z = float(attrs["x"]), float(attrs["y"]), float(attrs["z"])
            if not (math.isfinite(x) and math.isfinite(y) and math.isfinite(z)):
                raise ValueError("Non-finite 3MF coordinate")
            current.points.extend((x, y, z))
        elif name == TRIANGLE and parent == TRIANGLES and current is not None:
            current.indices.extend((int(attrs["v1"]), int(attrs["v2"]), int(attrs["v3"])))
        elif not stack:
            if name != MODEL:
                raise ValueError("Unsupported 3MF model namespace")
            if attrs.get("requiredextensions", "").strip():
                raise ValueError("This 3MF model requires extensions outside the core mesh reader")
            unit = UNITS[attrs.get("unit", "millimeter")]
        elif name == OBJECT and parent == RESOURCES:
            key = int(attrs["id"])
            if key <= 0 or key in objects:
                raise ValueError("Invalid or duplicate 3MF object ID")
            current = objects[key] = ModelObject()
        elif name == COMPONENT and parent == COMPONENTS and current is not None:
            current.components.append(placement(attrs))
        elif name == ITEM and parent == BUILD:
            build.append(placement(attrs))
        stack.append(name)
        if len(stack) > 128:
            raise ValueError("3MF XML nesting exceeds the decode limit")

    def end(_name):
        nonlocal current
        if stack.pop() == OBJECT:
            current = None

    with ZipFile(path) as archive:
        info = model_part(archive)
        if info.file_size > 512 * 1024 * 1024:
            raise ValueError("Expanded 3MF model exceeds the decode limit")
        located = perf_counter()
        with archive.open(info) as source:
            parse_xml(source, start, end)

    for obj in objects.values():
        if obj.indices and (obj.components or max(obj.indices) >= len(obj.points) // 3):
            raise ValueError("Invalid 3MF mesh indices or components")
    return objects, build, unit, located - start_time, perf_counter() - located


def instances(objects, build):
    pending = [(key, matrix, frozenset()) for key, matrix in reversed(build)]
    while pending:
        key, matrix, ancestors = pending.pop()
        if key in ancestors or len(ancestors) >= 128:
            raise ValueError("Cyclic or excessively nested 3MF components")
        obj = objects.get(key)
        if obj is None:
            raise ValueError(f"Missing 3MF object {key}")
        if obj.indices:
            yield key, obj, matrix
        else:
            visited = ancestors | {key}
            pending.extend((child, compose(local, matrix), visited)
                           for child, local in reversed(obj.components))


def world_points(values, matrix, unit):
    if matrix == IDENTITY:
        return array("f", values if unit == 1 else (value * unit for value in values))
    a,b,c,d,e,f,g,h,i,x0,y0,z0 = matrix
    points = array("f")
    coordinates = iter(values)
    for x, y, z in zip(coordinates, coordinates, coordinates):
        points.extend(((a*x+d*y+g*z+x0)*unit, (b*x+e*y+h*z+y0)*unit, (c*x+f*y+i*z+z0)*unit))
    return points


def write_array(output, kind, data):
    if sys.byteorder != "little":
        data = data[:]
        data.byteswap()
    view = memoryview(data).cast("B")
    for offset in range(0, len(view), FRAME_BYTES):
        write_frame(output, kind, view[offset:offset + FRAME_BYTES])


def stream(path, output):
    objects, build, unit, package, parse = read_model(path)
    emit_start = perf_counter()
    write_frame(output, "meta", read_ms=round((package + parse) * 1000, 3))
    count = batches = payload = draw_batches = 0
    placements = {}
    for number, (key, _, matrix) in enumerate(instances(objects, build), 1):
        if number > 100_000:
            raise ValueError("Too many 3MF placements")
        placements.setdefault(key, []).append(matrix)
    for key, matrices in placements.items():
        obj = objects[key]
        shared = len(matrices) > 1
        matrix = IDENTITY if shared else matrices[0]
        points = world_points(obj.points, matrix, 1 if shared else unit)
        indices = obj.indices
        if determinant(matrix) < 0:
            indices = indices[:]
            indices[1::3], indices[2::3] = indices[2::3], indices[1::3]
        triangles = len(indices) // 3
        bounds = {}
        if shared:
            limits = []
            for axis in range(3):
                coordinates = points[axis::3]
                limits.append((min(coordinates), max(coordinates)))
            minimum, maximum = map(list, zip(*limits))
            bounds = {"shared": True, "minimum": minimum, "maximum": maximum}
        write_frame(output, "indexed", vertices=len(points) // 3, triangles=triangles, **bounds)
        write_array(output, "positions", points)
        write_array(output, "indices", indices)
        if shared:
            groups = {False: array("f"), True: array("f")}
            for matrix in matrices:
                groups[determinant(matrix) < 0].extend(
                    matrix[col + row * 3] * unit for col in range(3) for row in range(4)
                )
            for mirrored, values in groups.items():
                if sys.byteorder != "little":
                    values.byteswap()
                data = memoryview(values).cast("B")
                for offset in range(0, len(data), FRAME_BYTES):
                    chunk = data[offset:offset + FRAME_BYTES]
                    write_frame(output, "instances", chunk, count=len(chunk) // 48, mirrored=mirrored)
                    draw_batches += 1
                payload += len(data)
        else:
            draw_batches += 1
        count += triangles * len(matrices)
        batches += 1
        payload += (len(points) + len(indices)) * 4
    if not count:
        raise ValueError("3MF build has no mesh triangles")
    emitted = perf_counter() - emit_start
    write_frame(output, "done", triangles=count, total_ms=round((package + parse + emitted) * 1000, 3))
    output.flush()
    if os.environ.get("STEPFLASH_PROFILE"):
        print(f"3MF phases: package={package*1000:.2f} ms, xml_geometry={parse*1000:.2f} ms, "
              f"placement_and_stream={emitted*1000:.2f} ms; triangles={count}, batches={batches}, "
              f"draw_batches={draw_batches}, payload_bytes={payload}, "
              f"peak_working_set_bytes={peak_memory()}", file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("path")
    args = parser.parse_args()
    try:
        stream(args.path, sys.stdout.buffer)
    except (BadZipFile, expat.ExpatError, KeyError, OSError, OverflowError, ValueError) as error:
        print(f"3MF import: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
