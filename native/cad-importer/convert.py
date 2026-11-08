"""Offline, out-of-process CAD conversion. No CAD app, network or credentials."""
from __future__ import annotations

import argparse
import array
import json
import math
import os
import queue
from pathlib import Path
import struct
import sys
import threading
import time

MAX_INPUT = 512 * 1024 * 1024
MAX_VERTICES = 16_000_000
MAX_TRIANGLES = 16_000_000
MAX_FACES = 250_000
MAX_OUTPUT = 512 * 1024 * 1024
SUPPORTED = {'.x_b', '.x_t', '.xb', '.xt', '.xmt_bin', '.xmt_txt', '.igs', '.iges', '.brep'}


def convert_shape(path: Path):
    suffix = path.suffix.lower()
    if suffix in {'.x_b', '.x_t', '.xb', '.xt', '.xmt_bin', '.xmt_txt'}:
        from parasolid_kit import read_brep
        from parasolid_kit.interop.occt import to_occt
        source_format = 'x-b' if suffix in {'.x_b', '.xb', '.xmt_bin'} else 'x-t'
        parsed = read_brep(path, source_format=source_format)
        if not parsed.complete:
            raise ValueError('This Parasolid file contains geometry not covered by the standalone reader. No partial model was displayed.')
        # Source units are deliberately unspecified in VTX. Equal conversion
        # units preserve numeric coordinates; this does not infer millimeters.
        result = to_occt(parsed.brep, source_unit='mm', target_unit='mm', require_complete=True, place_instances=True)
        return result.shape, None, 'Parasolid geometry with assembly placements and supported face/body RGB. Source units unspecified; textures and transparency are not imported.', parasolid_colors(parsed.brep, result)
    from OCP.BRep import BRep_Builder
    from OCP.TopoDS import TopoDS_Shape
    if suffix == '.brep':
        from OCP.BRepTools import BRepTools
        shape = TopoDS_Shape()
        if not BRepTools.Read_s(shape, str(path), BRep_Builder()):
            raise ValueError('Open CASCADE could not read this BREP file.')
        return shape, None, 'Open CASCADE BREP geometry; source units unspecified.', lambda face: None
    from OCP.IGESCAFControl import IGESCAFControl_Reader
    from OCP.IFSelect import IFSelect_RetDone
    from OCP.TDocStd import TDocStd_Document
    from OCP.TCollection import TCollection_ExtendedString
    from OCP.TDF import TDF_LabelSequence
    from OCP.XCAFDoc import XCAFDoc_DocumentTool
    from OCP.TopoDS import TopoDS_Compound
    reader = IGESCAFControl_Reader()
    reader.SetColorMode(True)
    if reader.ReadFile(str(path)) != IFSelect_RetDone:
        raise ValueError('Open CASCADE could not read this IGES file.')
    roots = reader.NbRootsForTransfer()
    if roots == 0 or reader.TransferRoots() != roots:
        raise ValueError('An IGES root could not be converted. No partial model was displayed.')
    checks = reader.WS().TransferReader().TransientProcess().CheckList(True)
    checks.Start()
    while checks.More():
        if checks.Value().HasFailed():
            raise ValueError('IGES transfer reported a failed entity. No partial model was displayed.')
        checks.Next()
    document = TDocStd_Document(TCollection_ExtendedString('VTX'))
    if not reader.Transfer(document):
        raise ValueError('The IGES file has no transferable geometry.')
    shapes = XCAFDoc_DocumentTool.ShapeTool_s(document.Main())
    labels = TDF_LabelSequence()
    shapes.GetFreeShapes(labels)
    compound = TopoDS_Compound()
    builder = BRep_Builder()
    builder.MakeCompound(compound)
    for i in range(1, labels.Length() + 1):
        builder.Add(compound, shapes.GetShape_s(labels.Value(i)))
    colors = iges_colors(compound, XCAFDoc_DocumentTool.ColorTool_s(document.Main()))
    return compound, 0.001, 'IGES geometry and supported surface RGB; normalized to millimeters. Textures and transparency are not imported.', colors


def iges_colors(shape, tool):
    from OCP.TopTools import TopTools_IndexedMapOfShape
    from OCP.TopoDS import TopoDS_Iterator
    from OCP.TopAbs import TopAbs_FACE
    from OCP.Quantity import Quantity_Color, Quantity_TOC_sRGB
    from OCP.XCAFDoc import XCAFDoc_ColorSurf, XCAFDoc_ColorGen
    faces = TopTools_IndexedMapOfShape()
    colors = {}
    stack = [(shape, None)]
    visits = 0
    while stack:
        current, inherited = stack.pop()
        visits += 1
        if visits > MAX_FACES * 32:
            raise ValueError('CAD topology traversal limit exceeded.')
        color = Quantity_Color()
        if tool.GetColor(current, XCAFDoc_ColorSurf, color) or tool.GetColor(current, XCAFDoc_ColorGen, color):
            inherited = color.Values(Quantity_TOC_sRGB)
        if current.ShapeType() == TopAbs_FACE:
            if inherited is not None:
                colors[faces.Add(current)] = inherited
            continue
        iterator = TopoDS_Iterator(current)
        while iterator.More():
            stack.append((iterator.Value(), inherited))
            iterator.Next()
    return lambda face: colors.get(faces.FindIndex(face))


def parasolid_colors(brep, converted):
    from OCP.TopTools import TopTools_IndexedMapOfShape
    from OCP.TopAbs import TopAbs_FACE
    from OCP.TopExp import TopExp_Explorer
    targets = {item.key: item.shape for item in converted.subshapes}
    faces = TopTools_IndexedMapOfShape()
    colors = {}
    # Body colors first; an explicitly attached face color takes precedence.
    for kind in ['body', 'face']:
        for relation in converted.source_map.relations:
            if relation.source.kind.value != kind:
                continue
            color = brep.colour_of(kind, relation.source.entity_id)
            if color is None or not all(math.isfinite(c) and 0 <= c <= 1 for c in color):
                continue
            shape = targets.get(relation.target_key)
            if shape is None:
                continue
            if shape.ShapeType() == TopAbs_FACE:
                colors[faces.Add(shape)] = color
            else:
                walker = TopExp_Explorer(shape, TopAbs_FACE)
                while walker.More():
                    colors[faces.Add(walker.Current())] = color
                    walker.Next()
    return lambda face: colors.get(faces.FindIndex(face))


def tessellate(shape, face_color=lambda face: None):
    from OCP.BRep import BRep_Tool
    from OCP.BRepBndLib import BRepBndLib
    from OCP.BRepCheck import BRepCheck_Analyzer
    from OCP.BRepMesh import BRepMesh_IncrementalMesh
    from OCP.Bnd import Bnd_Box
    from OCP.TopAbs import TopAbs_EDGE, TopAbs_FACE, TopAbs_REVERSED
    from OCP.TopExp import TopExp_Explorer
    from OCP.TopoDS import TopoDS
    from OCP.TopLoc import TopLoc_Location
    if shape.IsNull() or not BRepCheck_Analyzer(shape).IsValid():
        raise ValueError('CAD conversion did not produce valid geometry.')
    if TopExp_Explorer(shape, TopAbs_EDGE, TopAbs_FACE).More():
        raise ValueError('This CAD file includes standalone curves or wires, which this face importer cannot display. No partial model was displayed.')
    bounds = Bnd_Box()
    BRepBndLib.Add_s(shape, bounds)
    box = bounds.Get()
    span = max(box[i + 3] - box[i] for i in range(3))
    if not math.isfinite(span) or span <= 0:
        raise ValueError('Invalid CAD bounds.')
    deflection = max(1e-7, span * 0.0005)
    mesher = BRepMesh_IncrementalMesh(shape, deflection, False, 0.35, True)
    mesher.Perform()
    if not mesher.IsDone():
        raise ValueError('CAD tessellation failed.')
    positions = array.array('f')
    groups = {}
    triangles = 0
    faces = 0
    walker = TopExp_Explorer(shape, TopAbs_FACE)
    while walker.More():
        faces += 1
        if faces > MAX_FACES:
            raise ValueError('CAD face limit exceeded.')
        face = TopoDS.Face_s(walker.Current())
        location = TopLoc_Location()
        mesh = BRep_Tool.Triangulation_s(face, location)
        if mesh is None or mesh.NbTriangles() == 0:
            raise ValueError('A CAD face could not be tessellated. No partial model was displayed.')
        base = len(positions) // 3
        if base + mesh.NbNodes() > MAX_VERTICES or triangles + mesh.NbTriangles() > MAX_TRIANGLES:
            raise ValueError('CAD mesh limit exceeded.')
        color = tuple(round(c * 255) for c in (face_color(face) or (0.75, 0.77, 0.80)))
        if color not in groups:
            if len(groups) >= 4096:
                raise ValueError('CAD material limit exceeded.')
            groups[color] = array.array('I')
        indices = groups[color]
        transform = location.Transformation()
        for i in range(1, mesh.NbNodes() + 1):
            p = mesh.Node(i).Transformed(transform)
            xyz = (p.X(), p.Y(), p.Z())
            if not all(math.isfinite(v) and abs(v) < 1e30 for v in xyz):
                raise ValueError('CAD mesh contains invalid coordinates.')
            positions.extend(xyz)
        for i in range(1, mesh.NbTriangles() + 1):
            a, b, c = mesh.Triangle(i).Get()
            if min(a, b, c) < 1 or max(a, b, c) > mesh.NbNodes():
                raise ValueError('CAD mesh contains invalid indices.')
            if face.Orientation() == TopAbs_REVERSED:
                b, c = c, b
            indices.extend((base + a - 1, base + b - 1, base + c - 1))
        triangles += mesh.NbTriangles()
        walker.Next()
    if not triangles:
        raise ValueError('CAD file contains no drawable faces.')
    return positions, groups, deflection, faces


def write_glb(path: Path, positions, groups):
    lows = [min(positions[i::3]) for i in range(3)]
    highs = [max(positions[i::3]) for i in range(3)]
    if sys.byteorder != 'little':
        positions.byteswap()
        for indices in groups.values():
            indices.byteswap()
    data = bytearray(positions.tobytes())
    views=[dict(buffer=0, byteOffset=0, byteLength=len(data), target=34962)]
    accessors=[dict(bufferView=0, componentType=5126, count=len(positions)//3, type='VEC3', min=lows, max=highs)]
    materials=[];primitives=[]
    def linear(c):
        v=c/255
        return v/12.92 if v <= 0.04045 else ((v+0.055)/1.055)**2.4
    for color, indices in groups.items():
        view=len(views);accessor=len(accessors);material=len(materials);encoded=indices.tobytes()
        views.append(dict(buffer=0, byteOffset=len(data), byteLength=len(encoded), target=34963))
        data.extend(encoded)
        accessors.append(dict(bufferView=view, componentType=5125, count=len(indices), type='SCALAR'))
        materials.append(dict(pbrMetallicRoughness=dict(baseColorFactor=[*(linear(c) for c in color),1], metallicFactor=0, roughnessFactor=0.7)))
        primitives.append(dict(attributes={'POSITION':0},indices=accessor,material=material))
    doc = dict(asset=dict(version='2.0', generator='VTX standalone CAD importer'), scene=0,
        scenes=[dict(nodes=[0])], nodes=[dict(mesh=0)],
        meshes=[dict(primitives=primitives)], materials=materials,
        buffers=[dict(byteLength=len(data))],
        bufferViews=views, accessors=accessors)
    metadata = json.dumps(doc, separators=(',', ':')).encode()
    metadata += b' ' * (-len(metadata) % 4)
    size = 28 + len(metadata) + len(data)
    if size > MAX_OUTPUT:
        raise ValueError('CAD output limit exceeded.')
    with path.open('xb') as f:
        f.write(struct.pack('<III', 0x46546c67, 2, size))
        f.write(struct.pack('<II', len(metadata), 0x4e4f534a)); f.write(metadata)
        f.write(struct.pack('<II', len(data), 0x004e4942)); f.write(data)


def convert(source: Path, output: Path, report_path: Path):
    start = time.perf_counter()
    try:
        if source.suffix.lower() not in SUPPORTED:
            raise ValueError('Unsupported standalone CAD input.')
        if source.stat().st_size > MAX_INPUT:
            raise ValueError('CAD input exceeds 512 MiB.')
        shape, units, notice, colors = convert_shape(source)
        converted = time.perf_counter()
        positions, groups, error, faces = tessellate(shape, colors)
        meshed = time.perf_counter()
        write_glb(output, positions, groups)
        exported = time.perf_counter()
        report = dict(schema=1, success=True, complete=True, meters_per_unit=units,
            notice=notice, vertices=len(positions)//3, triangles=sum(len(i)//3 for i in groups.values()), materials=len(groups),
            faces=faces, deflection=error, elapsed_ms=(exported-start)*1000,
            phases_ms=dict(read_convert=(converted-start)*1000, mesh=(meshed-converted)*1000, export=(exported-meshed)*1000))
    except Exception as error:
        # Do not include the input path or a native traceback in public diagnostics.
        message = str(error).replace(str(source), source.name)
        report = dict(schema=1, success=False, complete=False,
            error=f'{type(error).__name__}: {message[:1800]}')
        report_path.write_text(json.dumps(report), encoding='utf-8')
        return 2
    report_path.write_text(json.dumps(report), encoding='utf-8')
    return 0


def serve(idle_seconds):
    # Keep the protocol on a private duplicate. OCCT and other native readers
    # may write directly to stdout; those messages must never corrupt replies.
    with os.fdopen(os.dup(sys.stdout.fileno()), 'wb', buffering=0) as replies:
        with open(os.devnull, 'wb') as sink:
            os.dup2(sink.fileno(), sys.stdout.fileno())
        requests = queue.Queue(maxsize=1)

        def read_commands():
            pending = bytearray()
            while True:
                chunk = os.read(sys.stdin.fileno(), 8192)
                if not chunk:
                    os._exit(125)  # Also terminate when the parent disappears.
                pending.extend(chunk)
                while b'\n' in pending:
                    line, _, pending = pending.partition(b'\n')
                    if len(line) > 256 * 1024:
                        os._exit(2)
                    requests.put(line)
                if len(pending) > 256 * 1024:
                    os._exit(2)

        threading.Thread(target=read_commands, daemon=True).start()
        # Bound retained kernel caches; an idle worker exits without a service,
        # listening port, or background installation on the user's machine.
        for _ in range(16):
            try:
                line = requests.get(timeout=idle_seconds)
            except queue.Empty:
                return 0
            try:
                request = json.loads(line)
                if not isinstance(request, dict) or set(request) != {'id', 'source', 'output', 'report'}:
                    raise ValueError('Invalid conversion request')
                if type(request['id']) is not int or not all(isinstance(request[k], str) for k in ['source', 'output', 'report']):
                    raise ValueError('Invalid conversion request fields')
                code = convert(Path(request['source']), Path(request['output']), Path(request['report']))
                replies.write(json.dumps(dict(id=request['id'], code=code)).encode() + b'\n')
            except Exception:
                return 2  # Malformed protocol or unusable destination: respawn.
    return 0


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('source', type=Path, nargs='?')
    parser.add_argument('output', type=Path, nargs='?')
    parser.add_argument('--report', type=Path)
    parser.add_argument('--parent-pipe', action='store_true')
    parser.add_argument('--serve', action='store_true')
    parser.add_argument('--idle-seconds', type=float, default=45)
    args = parser.parse_args()
    if args.serve:
        if not 0 < args.idle_seconds <= 120:
            parser.error('Idle timeout must be greater than zero and at most 120 seconds')
        return serve(args.idle_seconds)
    if args.source is None or args.output is None or args.report is None:
        parser.error('source, output and --report are required for one conversion')
    if args.parent_pipe:
        def parent_watch():
            if not os.read(sys.stdin.fileno(), 1):
                os._exit(125)
        threading.Thread(target=parent_watch, daemon=True).start()
    return convert(args.source, args.output, args.report)


if __name__ == '__main__':
    raise SystemExit(main())
