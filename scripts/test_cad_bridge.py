"""Run with CAD Python: python scripts/test_cad_bridge.py. DWG test needs dxf2dwg/dwg2dxf."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

import ezdxf
from cad_bridge import load_drawing


class DrawingTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="vtx-cad-test-")
        self.root = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def fixture(self, binary=False):
        doc = ezdxf.new("R2000")
        doc.units = 4
        doc.layers.new("outline", dxfattribs={"color": 1})
        block = doc.blocks.new("bracket")
        block.add_line((0, 0), (20, 0), dxfattribs={"layer": "outline"})
        block.add_circle((10, 5), 2)
        doc.modelspace().add_blockref("bracket", (100, 200), dxfattribs={"xscale": 2, "yscale": 2})
        doc.modelspace().add_text("O", dxfattribs={"height": 3, "insert": (100, 190)})
        hatch = doc.modelspace().add_hatch()
        hatch.paths.add_polyline_path([(100, 220), (110, 220), (110, 230), (100, 230)], is_closed=True)
        hatch.paths.add_polyline_path([(102, 222), (108, 222), (108, 228), (102, 228)], is_closed=True, flags=0)
        path = self.root / "drawing.dxf"
        doc.saveas(path, fmt="bin" if binary else "asc")
        return path

    def test_ascii_and_binary_dxf_geometry_and_handles(self):
        for binary in (False, True):
            result = load_drawing(self.fixture(binary))
            self.assertEqual(result["source_info"]["insunits"], 4)
            self.assertAlmostEqual(result["source_info"]["meters_per_unit"],0.001)
            self.assertEqual(result["source_info"]["warnings"], [])
            lines = [p for p in result["parts"] if p["metadata"]["entity_type"] == "LINE"]
            self.assertEqual(len(lines), 1)
            self.assertEqual(lines[0]["positions"], [[100, 200, 0], [140, 200, 0]])
            self.assertEqual(lines[0]["metadata"]["layer"], "outline")
            self.assertTrue(lines[0]["metadata"]["block_path"])
            hatches = [p for p in result["parts"] if p["metadata"]["entity_type"] == "HATCH"]
            self.assertTrue(hatches)
            area = 0
            for part in hatches:
                p = part["positions"]
                for i in range(0, len(p), 3):
                    a,b,c = p[i:i+3]
                    area += abs((b[0]-a[0])*(c[1]-a[1])-(b[1]-a[1])*(c[0]-a[0]))/2
            self.assertAlmostEqual(area, 100-36)  # holes must stay empty

    def test_invalid_drawing_exits_without_json(self):
        source = self.root / "invalid.dxf"
        source.write_text("not a DXF")
        result = subprocess.run([os.sys.executable, str(Path(__file__).with_name("cad_bridge.py")), str(source)], capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(result.stdout)
        self.assertIn(b"VTX CAD import", result.stderr)

    def test_dwg_conversion(self):
        converter = os.environ.get("VTX_DWG2DXF") or shutil.which("dwg2dxf")
        writer = str(Path(converter).with_name("dxf2dwg")) if converter else None
        if not writer or not Path(writer).is_file():
            self.skipTest("requires LibreDWG dxf2dwg and dwg2dxf")
        dwg = self.root / "drawing.dwg"
        result = subprocess.run([writer, "--as", "r2000", "-o", str(dwg), str(self.fixture())], capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr.decode(errors="replace"))
        result = load_drawing(dwg)
        self.assertEqual(result["source_info"]["format"], "dwg")
        self.assertTrue(result["parts"])
        self.assertEqual(result["source_info"]["conversion"]["tool"], "LibreDWG dwg2dxf")


if __name__ == "__main__":
    unittest.main()
