"""Exercise real CAD conversion, bounds, holes and explicit failure (no CAD app)."""
import argparse
import json
import os
import queue
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import threading

ROOT = Path(__file__).resolve().parents[1]


def fixtures(folder):
    from OCP.BRepAlgoAPI import BRepAlgoAPI_Cut
    from OCP.BRepPrimAPI import BRepPrimAPI_MakeBox, BRepPrimAPI_MakeCylinder
    from OCP.BRepTools import BRepTools
    from OCP.gp import gp_Ax2, gp_Dir, gp_Pnt
    from OCP.IGESCAFControl import IGESCAFControl_Writer
    from OCP.TDocStd import TDocStd_Document
    from OCP.TCollection import TCollection_ExtendedString
    from OCP.XCAFDoc import XCAFDoc_DocumentTool, XCAFDoc_ColorGen, XCAFDoc_ColorSurf
    from OCP.Quantity import Quantity_Color, Quantity_TOC_sRGB
    from OCP.TopExp import TopExp_Explorer
    from OCP.TopAbs import TopAbs_FACE
    box = BRepPrimAPI_MakeBox(20, 10, 6).Shape()
    drill = BRepPrimAPI_MakeCylinder(gp_Ax2(gp_Pnt(10, 5, -1), gp_Dir(0, 0, 1)), 2, 8).Shape()
    cut = BRepAlgoAPI_Cut(box, drill)
    cut.Build()
    assert cut.IsDone()
    shape = cut.Shape()
    brep = folder / 'drilled-block.brep'
    assert BRepTools.Write_s(shape, str(brep))
    iges = folder / 'drilled-block.iges'
    document = TDocStd_Document(TCollection_ExtendedString('VTX test'))
    shapes = XCAFDoc_DocumentTool.ShapeTool_s(document.Main())
    colors = XCAFDoc_DocumentTool.ColorTool_s(document.Main())
    label = shapes.AddShape(shape, False)
    colors.SetColor(label, Quantity_Color(0.1, 0.3, 0.9, Quantity_TOC_sRGB), XCAFDoc_ColorGen)
    walker = TopExp_Explorer(shape, TopAbs_FACE)
    face = shapes.AddSubShape(label, walker.Current())
    colors.SetColor(face, Quantity_Color(0.9, 0.1, 0.2, Quantity_TOC_sRGB), XCAFDoc_ColorSurf)
    writer = IGESCAFControl_Writer()
    writer.SetColorMode(True)
    assert writer.Transfer(document)
    assert writer.Write(str(iges))
    # Exercise material counts large enough to expose whole-model vertex
    # duplication. The source geometry is authored here, not a user model.
    import colorsys
    palette = folder / 'palette.iges'
    document = TDocStd_Document(TCollection_ExtendedString('VTX palette test'))
    shapes = XCAFDoc_DocumentTool.ShapeTool_s(document.Main())
    colors = XCAFDoc_DocumentTool.ColorTool_s(document.Main())
    for i in range(32):
        box = BRepPrimAPI_MakeBox(gp_Pnt((i % 8) * 2, (i // 8) * 2, 0), 1, 1, 1).Shape()
        label = shapes.AddShape(box, False)
        colors.SetColor(label, Quantity_Color(*colorsys.hsv_to_rgb(i / 32, 0.8, 0.9), Quantity_TOC_sRGB), XCAFDoc_ColorGen)
    writer = IGESCAFControl_Writer()
    writer.SetColorMode(True)
    assert writer.Transfer(document)
    assert writer.Write(str(palette))
    return brep, iges, palette


def document(path):
    data = path.read_bytes()
    magic, version, size, length, kind = struct.unpack_from('<5I', data)
    assert magic == 0x46546c67 and version == 2 and size == len(data)
    assert kind == 0x4e4f534a
    return json.loads(data[20:20 + length])


def environment_for(isolated):
    environment = os.environ.copy()
    if isolated:
        for key in ['PYTHONHOME', 'PYTHONPATH', 'VIRTUAL_ENV', 'CONDA_PREFIX',
                    'VTX_CAD_PYTHON', 'LD_LIBRARY_PATH', 'DYLD_LIBRARY_PATH']:
            environment.pop(key, None)
        environment['PATH'] = ''
    return environment


def invoke(command, isolated=False):
    # Match VTX's held parent pipe, including normal interpreter shutdown.
    child = subprocess.Popen([*command, '--parent-pipe'], stdin=subprocess.PIPE, env=environment_for(isolated))
    try:
        return child.wait(timeout=120)
    finally:
        if child.poll() is None:
            child.kill()
            child.wait()
        child.stdin.close()


def server_checks(command, sources, folder, isolated):
    child = subprocess.Popen([*command, '--serve'], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                             stderr=subprocess.DEVNULL, env=environment_for(isolated))
    replies = queue.Queue()

    def read_replies():
        try:
            while line := child.stdout.readline(4097):
                if len(line) > 4096:
                    raise ValueError('Oversized server reply')
                replies.put(json.loads(line))
        except Exception as error:
            replies.put(error)

    reader = threading.Thread(target=read_replies, daemon=True)
    reader.start()
    try:
        # A failed model must not poison the next conversion in the same process.
        invalid = folder / 'server-invalid.x_b'
        invalid.write_bytes(b'invalid binary')
        for number, source in enumerate([*sources, invalid, sources[0]]):
            output, report = folder / f'server-{number}.glb', folder / f'server-{number}.json'
            request = dict(id=number, source=str(source), output=str(output), report=str(report))
            child.stdin.write(json.dumps(request).encode() + b'\n')
            child.stdin.flush()
            reply = replies.get(timeout=120)
            assert isinstance(reply, dict) and reply['id'] == number, reply
            result = json.loads(report.read_text())
            assert reply['code'] == (2 if source == invalid else 0), result
            assert result['success'] == (source != invalid), result
            if source != invalid:
                document(output)
            assert child.poll() is None
        child.stdin.close()
        assert child.wait(timeout=5) in (0, 125)
    finally:
        if child.poll() is None:
            child.kill()
            child.wait()
        if not child.stdin.closed:
            child.stdin.close()
        reader.join(timeout=5)
        assert not reader.is_alive()
        child.stdout.close()
    # Idle workers must exit while the parent is still alive and holds stdin.
    idle = subprocess.Popen([*command, '--serve', '--idle-seconds', '0.2'], stdin=subprocess.PIPE,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=environment_for(isolated))
    try:
        assert idle.wait(timeout=10) == 0
    finally:
        if idle.poll() is None:
            idle.kill()
            idle.wait()
        idle.stdin.close()
    print('Reusable worker: multiple models, error recovery, parent EOF and idle shutdown passed.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--helper', type=Path)
    parser.add_argument('--parasolid', type=Path, action='append', default=[])
    args = parser.parse_args()
    command = [str(args.helper.resolve())] if args.helper else [sys.executable, '-I', str(ROOT / 'native/cad-importer/convert.py')]
    with tempfile.TemporaryDirectory(prefix='vtx-cad-test-') as temporary:
        folder = Path(temporary)
        generated = fixtures(folder)
        sources = [*generated, *args.parasolid]
        for source in sources:
            output = folder / (source.name + '.glb')
            report = folder / (source.name + '.json')
            code = invoke([*command, str(source), str(output), '--report', str(report)], isolated=bool(args.helper))
            result = json.loads(report.read_text())
            assert code == 0, result
            assert result['schema'] == 1 and result['success'] and result['complete']
            assert result['triangles'] > 12 and result['faces'] > 0
            doc = document(output)
            positions = [doc['accessors'][p['attributes']['POSITION']] for p in doc['meshes'][0]['primitives']]
            assert sum(p['count'] for p in positions) == result['vertices'], 'Material groups repeat whole-model vertices'
            if source in generated:
                span = [max(p['max'][i] for p in positions) - min(p['min'][i] for p in positions) for i in range(3)]
                palette = source.name == 'palette.iges'
                expected = [15, 7, 1] if palette else [20, 10, 6]
                assert all(abs(a - b) < 0.001 for a, b in zip(span, expected)), span
                assert result['faces'] >= 7  # Cylinder wall must survive the import.
                assert result['meters_per_unit'] == (0.001 if source.suffix == '.iges' else None)
                if source.suffix == '.iges':
                    assert result['materials'] >= 2, 'IGES face/body colors were lost'
                if palette:
                    assert result['materials'] == 32 and result['vertices'] == 768
                    assert result['triangles'] == 384
            print(json.dumps(dict(file=source.name, **result)))
        bad = folder / 'invalid.x_b'
        bad.write_bytes(b'not a Parasolid file')
        output, report = folder / 'invalid.glb', folder / 'invalid.json'
        code = invoke([*command, str(bad), str(output), '--report', str(report)], isolated=bool(args.helper))
        result = json.loads(report.read_text())
        assert code != 0 and not result['success'] and not result['complete']
        assert not output.exists()
        print('Invalid binary input rejected without a partial model.')
        from OCP.BRepBuilderAPI import BRepBuilderAPI_MakeEdge
        from OCP.BRepTools import BRepTools
        from OCP.gp import gp_Pnt
        wire = folder / 'curve-only.brep'
        assert BRepTools.Write_s(BRepBuilderAPI_MakeEdge(gp_Pnt(0, 0, 0), gp_Pnt(1, 0, 0)).Shape(), str(wire))
        code = invoke([*command, str(wire), str(output), '--report', str(report)], isolated=bool(args.helper))
        result = json.loads(report.read_text())
        assert code != 0 and not result['success'] and 'curves or wires' in result['error'], result
        assert not output.exists()
        print('Unsupported wire geometry rejected explicitly.')
        server_checks(command, sources, folder, isolated=bool(args.helper))


if __name__ == '__main__':
    main()
