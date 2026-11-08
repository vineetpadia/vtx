"""Exercise real CAD conversion, bounds, holes and explicit failure (no CAD app)."""
import argparse
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile

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
    return brep, iges


def document(path):
    data = path.read_bytes()
    magic, version, size, length, kind = struct.unpack_from('<5I', data)
    assert magic == 0x46546c67 and version == 2 and size == len(data)
    assert kind == 0x4e4f534a
    return json.loads(data[20:20 + length])


def invoke(command, isolated=False):
    # Match VTX's held parent pipe, including normal interpreter shutdown.
    environment = os.environ.copy()
    if isolated:
        for key in ['PYTHONHOME', 'PYTHONPATH', 'VIRTUAL_ENV', 'CONDA_PREFIX',
                    'VTX_CAD_PYTHON', 'LD_LIBRARY_PATH', 'DYLD_LIBRARY_PATH']:
            environment.pop(key, None)
        environment['PATH'] = ''
    child = subprocess.Popen([*command, '--parent-pipe'], stdin=subprocess.PIPE, env=environment)
    try:
        return child.wait(timeout=120)
    finally:
        if child.poll() is None:
            child.kill()
            child.wait()
        child.stdin.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--helper', type=Path)
    parser.add_argument('--parasolid', type=Path, action='append', default=[])
    args = parser.parse_args()
    command = [str(args.helper.resolve())] if args.helper else [sys.executable, '-I', str(ROOT / 'native/cad-importer/convert.py')]
    with tempfile.TemporaryDirectory(prefix='vtx-cad-test-') as temporary:
        folder = Path(temporary)
        for source in [*fixtures(folder), *args.parasolid]:
            output = folder / (source.name + '.glb')
            report = folder / (source.name + '.json')
            code = invoke([*command, str(source), str(output), '--report', str(report)], isolated=bool(args.helper))
            result = json.loads(report.read_text())
            assert code == 0, result
            assert result['schema'] == 1 and result['success'] and result['complete']
            assert result['triangles'] > 12 and result['faces'] > 0
            doc = document(output)
            position = doc['accessors'][0]
            if source.suffix in {'.brep', '.iges'}:
                span = [b - a for a, b in zip(position['min'], position['max'])]
                assert all(abs(a - b) < 0.001 for a, b in zip(span, [20, 10, 6])), span
                assert result['faces'] >= 7  # Cylinder wall must survive the import.
                assert result['meters_per_unit'] == (0.001 if source.suffix == '.iges' else None)
                if source.suffix == '.iges':
                    assert result['materials'] >= 2, 'IGES face/body colors were lost'
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


if __name__ == '__main__':
    main()
