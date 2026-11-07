"""Run with CAD Python: python scripts/test_cad_bridge.py. DWG test needs dxf2dwg/dwg2dxf."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import ezdxf
from cad_bridge import load_drawing, acis_triangles


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
            self.assertNotEqual(lines[0]["metadata"]["handle"], "LINE")
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

    def test_acis_cube_retains_xyz_transform_and_outward_volume(self):
        from ezdxf.acis import api
        from ezdxf.render import forms
        from ezdxf.math import Vec3
        mesh = forms.cube().scale(2, 3, 4).translate(10, 20, 30)
        doc = ezdxf.new("R2013")
        entity = doc.modelspace().add_3dsolid()
        api.export_dxf(entity, [api.body_from_mesh(mesh)])
        path = self.root / "solid.dxf"
        doc.saveas(path)
        drawing = load_drawing(path)
        self.assertEqual(drawing["source_info"]["kind"], "drawing_3d")
        self.assertEqual(drawing["source_info"]["solid_entities"], 1)
        self.assertEqual(drawing["source_info"]["warnings"], [])
        vertices = drawing["parts"][0]["positions"]
        self.assertEqual([min(p[i] for p in vertices) for i in range(3)], [min(p[i] for p in mesh.vertices) for i in range(3)])
        self.assertEqual([max(p[i] for p in vertices) for i in range(3)], [max(p[i] for p in mesh.vertices) for i in range(3)])
        volume = sum(Vec3(vertices[i]).dot(Vec3(vertices[i+1]).cross(Vec3(vertices[i+2]))) / 6 for i in range(0, len(vertices), 3))
        self.assertAlmostEqual(volume, 24)

    def test_acis_vertical_face_hole_stays_open(self):
        from ezdxf.acis import api
        from ezdxf.render import MeshBuilder
        from ezdxf.math import Vec3
        def body(radius):
            mesh = MeshBuilder()
            mesh.add_face([(3,-radius,-radius),(3,radius,-radius),(3,radius,radius),(3,-radius,radius)])
            return api.body_from_mesh(mesh)
        outer, inner = body(5), body(2)
        face = outer.lump.shell.face
        face.append_loop(inner.lump.shell.face.loop)
        with patch.object(api, "load_dxf", return_value=[outer]):
            vertices = acis_triangles(None)
        area = sum((Vec3(vertices[i+1])-Vec3(vertices[i])).cross(Vec3(vertices[i+2])-Vec3(vertices[i])).magnitude / 2 for i in range(0,len(vertices),3))
        self.assertAlmostEqual(area, 84)
        self.assertTrue(all(p[0] == 3 for p in vertices))
        # ACIS linked loops can list a hole before the exterior.
        outside, inside = face.loop, face.loop.next_loop
        outside.next_loop = inside.next_loop
        inside.next_loop = outside
        face.loop = inside
        with patch.object(api, "load_dxf", return_value=[outer]):
            reordered = acis_triangles(None)
        area = sum((Vec3(reordered[i+1])-Vec3(reordered[i])).cross(Vec3(reordered[i+2])-Vec3(reordered[i])).magnitude / 2 for i in range(0,len(reordered),3))
        self.assertAlmostEqual(area, 84)

    def test_external_reference_inventory_discloses_missing_content(self):
        doc = ezdxf.new("R2000")
        doc.modelspace().add_line((0,0), (1,1))
        block = doc.blocks.new("missing")
        block.block.dxf.flags = 4
        block.block.dxf.xref_path = "not-available.dwg"
        doc.modelspace().add_blockref("missing", (0,0))
        path = self.root/"xref.dxf"
        doc.saveas(path)
        result = load_drawing(path)
        inventory = result["source_info"]["inventory"]
        self.assertEqual(inventory["modelspace_entities"], 2)
        self.assertEqual(inventory["external_references"], [{"name":"missing","path":"not-available.dwg"}])
        self.assertTrue(any("External reference" in w["reason"] for w in result["source_info"]["warnings"]))

    def test_acis_rejects_whole_entity_when_a_face_is_curved(self):
        from ezdxf.acis import api
        from ezdxf.acis.entities import Surface
        from ezdxf.render import forms
        body = api.body_from_mesh(forms.cube())
        body.lump.shell.face.next_face.surface = Surface()
        with patch.object(api, "load_dxf", return_value=[body]):
            with self.assertRaisesRegex(ValueError, "Curved ACIS surfaces"):
                acis_triangles(None)

    def test_acis_nested_and_repeated_block_transforms_and_handles(self):
        from ezdxf.acis import api
        from ezdxf.render import forms
        from ezdxf.math import Vec3
        doc = ezdxf.new("R2013")
        inner = doc.blocks.new("cube")
        solid = inner.add_3dsolid()
        api.export_dxf(solid, [api.body_from_mesh(forms.cube())])
        outer = doc.blocks.new("nested")
        outer.add_blockref("cube", (5, 0, 0))
        doc.modelspace().add_blockref("nested", (10,20,30), dxfattribs={"rotation":90})
        doc.modelspace().add_blockref("cube", (50,60,70), dxfattribs={"xscale":-2})
        path = self.root/"nested-solid.dxf"
        doc.saveas(path)
        drawing = load_drawing(path)
        self.assertEqual(drawing["source_info"]["solid_entities"], 2)
        self.assertEqual(drawing["source_info"]["warnings"], [])
        self.assertEqual(len(drawing["parts"]), 2)
        for part, center, expected_volume in zip(drawing["parts"], [(10,25,30),(50,60,70)], [1,2]):
            vertices = part["positions"]
            for axis in range(3):
                self.assertAlmostEqual((max(p[axis] for p in vertices)+min(p[axis] for p in vertices))/2, center[axis])
            self.assertEqual(part["metadata"]["handle"], solid.dxf.handle)
            volume = sum(Vec3(vertices[i]).dot(Vec3(vertices[i+1]).cross(Vec3(vertices[i+2]))) / 6 for i in range(0,len(vertices),3))
            self.assertAlmostEqual(volume, expected_volume)
        self.assertNotEqual(drawing["parts"][0]["metadata"]["block_path"], drawing["parts"][1]["metadata"]["block_path"])

    def test_structural_recovery_is_disclosed(self):
        path = self.fixture()
        text = path.read_text()
        path.write_text("  0\nENDSEC\n" + text)
        drawing = load_drawing(path)
        self.assertTrue(drawing["parts"])
        self.assertEqual(drawing["source_info"]["recovery"]["reader"], "ezdxf.recover")
        self.assertTrue(any("recovered" in w["reason"] for w in drawing["source_info"]["warnings"]))

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

    def test_converter_error_diagnostic_is_reported_even_on_exit_zero(self):
        valid_dxf = self.fixture()
        def convert(args, **kwargs):
            shutil.copyfile(valid_dxf, args[2])
            return subprocess.CompletedProcess(args, 0, stdout="", stderr="ERROR: omitted an entity\n")
        with patch.dict(os.environ, {"VTX_DWG2DXF":"/fake/converter"}), patch("cad_bridge.subprocess.run", side_effect=convert):
            drawing = load_drawing(self.root/"input.dwg")
        self.assertTrue(drawing["parts"])
        self.assertTrue(any("converter reported" in w["reason"] for w in drawing["source_info"]["warnings"]))


if __name__ == "__main__":
    unittest.main()
