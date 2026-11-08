"""Exercise real CAD conversion, bounds, holes and explicit failure (no CAD app)."""
import argparse
import json
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
    from OCP.IGESControl import IGESControl_Writer
    box = BRepPrimAPI_MakeBox(20, 10, 6).Shape()
    drill = BRepPrimAPI_MakeCylinder(gp_Ax2(gp_Pnt(10, 5, -1), gp_Dir(0, 0, 1)), 2, 8).Shape()
    cut = BRepAlgoAPI_Cut(box, drill)
    cut.Build()
    assert cut.IsDone()
    shape = cut.Shape()
    brep = folder / 'drilled-block.brep'
    assert BRepTools.Write_s(shape, str(brep))
    iges = folder / 'drilled-block.iges'
    writer = IGESControl_Writer('MM', 0)
    assert writer.AddShape(shape)
    writer.ComputeModel()
    assert writer.Write(str(iges))
    return brep, iges


def document(path):
    data = path.read_bytes()
    magic, version, size, length, kind = struct.unpack_from('<5I', data)
    assert magic == 0x46546c67 and version == 2 and size == len(data)
    assert kind == 0x4e4f534a
    return json.loads(data[20:20 + length])


def invoke(command):
    # Match VTX's held parent pipe, including normal interpreter shutdown.
    child = subprocess.Popen([*command, '--parent-pipe'], stdin=subprocess.PIPE)
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
            assert invoke([*command, str(source), str(output), '--report', str(report)]) == 0
            result = json.loads(report.read_text())
            assert result['schema'] == 1 and result['success'] and result['complete']
            assert result['triangles'] > 12 and result['faces'] > 0
            doc = document(output)
            position = doc['accessors'][0]
            if source.suffix in {'.brep', '.iges'}:
                span = [b - a for a, b in zip(position['min'], position['max'])]
                assert all(abs(a - b) < 0.001 for a, b in zip(span, [20, 10, 6])), span
                assert result['faces'] >= 7  # Cylinder wall must survive the import.
                assert result['meters_per_unit'] == (0.001 if source.suffix == '.iges' else None)
            print(json.dumps(dict(file=source.name, **result)))
        bad = folder / 'invalid.x_b'
        bad.write_bytes(b'not a Parasolid file')
        output, report = folder / 'invalid.glb', folder / 'invalid.json'
        code = invoke([*command, str(bad), str(output), '--report', str(report)])
        result = json.loads(report.read_text())
        assert code != 0 and not result['success'] and not result['complete']
        assert not output.exists()
        print('Invalid binary input rejected without a partial model.')


if __name__ == '__main__':
    main()
