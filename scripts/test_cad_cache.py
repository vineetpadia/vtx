"""Integration tests for geometry cache integrity and CAD 3D views; requires CAD Python + GPU."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

import ezdxf

BINARY = "native/target/release/examples/vtx"


class CacheTests(unittest.TestCase):
    def test_content_change_same_size_mtime_and_corruption(self):
        with tempfile.TemporaryDirectory(prefix="vtx-cache-test-") as directory:
            root = Path(directory)
            source = root / "drawing.dxf"
            doc = ezdxf.new("R2000")
            m = doc.modelspace()
            for x in range(1000):
                m.add_line((x, 0), (x, 10))
            m.add_text("CACHE AAA", dxfattribs={"height": 2})
            doc.saveas(source)
            environment = dict(os.environ, VTX_CAD_CACHE="1", VTX_CAD_CACHE_DIR=str(root / "cache"))
            def inspect(name, disabled=False):
                env = dict(environment)
                if disabled:
                    env["VTX_CAD_CACHE"] = "0"
                p = subprocess.run([BINARY,"--inspect",str(source),"--output",str(root/name),"--size","256"],capture_output=True,text=True,env=env,timeout=30)
                self.assertEqual(p.returncode, 0, p.stderr)
                return json.loads(p.stdout)
            first, hit = inspect("first"), inspect("hit")
            self.assertEqual(first["source_info"]["cache"]["status"], "miss")
            self.assertTrue(first["source_info"]["cache"]["stored"])
            self.assertEqual(hit["source_info"]["cache"]["status"], "hit")
            self.assertEqual((root/"first/plan.png").read_bytes(), (root/"hit/plan.png").read_bytes())
            self.assertEqual(first["parts"], hit["parts"])
            stat = source.stat()
            data = source.read_bytes().replace(b"CACHE AAA", b"CACHE BBB")
            self.assertEqual(len(data), stat.st_size)
            source.write_bytes(data)
            os.utime(source, ns=(stat.st_atime_ns, stat.st_mtime_ns))
            changed = inspect("changed")
            self.assertEqual(changed["source_info"]["cache"]["status"], "miss")
            self.assertTrue(any(p["text"] == "CACHE BBB" for p in changed["parts"]))
            for entry in (root/"cache").glob("*.vtxcad"):
                data = bytearray(entry.read_bytes())
                data[-1] ^= 1
                entry.write_bytes(data)
            repaired = inspect("repaired")
            self.assertEqual(repaired["source_info"]["cache"]["status"], "miss")
            disabled = inspect("disabled", disabled=True)
            self.assertEqual(disabled["source_info"]["cache"]["status"], "disabled")
            self.assertEqual(repaired["parts"], disabled["parts"])
            self.assertEqual((root/"repaired/plan.png").read_bytes(), (root/"disabled/plan.png").read_bytes())

    def test_z_up_solid_defaults_to_seven_views_and_queries_keep_xyz(self):
        from ezdxf.acis import api
        from ezdxf.render import forms
        with tempfile.TemporaryDirectory(prefix="vtx-solid-test-") as directory:
            root = Path(directory)
            doc = ezdxf.new("R2013")
            entity = doc.modelspace().add_3dsolid()
            api.export_dxf(entity, [api.body_from_mesh(forms.cube().scale(2, 3, 4).translate(10, 20, 30))])
            source = root/"solid.dxf"
            doc.saveas(source)
            p = subprocess.run([BINARY,"--inspect",str(source),"--output",str(root/"views"),"--size","256","--views","front,top,iso","--query","front,128,128"],capture_output=True,text=True,timeout=30)
            self.assertEqual(p.returncode, 0, p.stderr)
            m = json.loads(p.stdout)
            q = m["queries"][0]
            self.assertTrue(q["hit"])
            self.assertAlmostEqual(q["position"][1], 18.5, places=3)
            self.assertTrue(28 <= q["position"][2] <= 32)
            front = m["views"][0]
            self.assertEqual(front["screen_up"], [0.0, 0.0, 1.0])
            self.assertEqual(front["toward_camera"], [0.0, -1.0, 0.0])
            p = subprocess.run([BINARY,"--inspect",str(source),"--output",str(root/"defaults"),"--size","256","--query","front,128,128"],capture_output=True,text=True,timeout=30)
            self.assertEqual(p.returncode, 0, p.stderr)
            self.assertEqual(len(json.loads(p.stdout)["views"]), 7)
            self.assertTrue(json.loads(p.stdout)["queries"][0]["hit"])


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=BINARY)
    args, remaining = parser.parse_known_args()
    BINARY = str(Path(args.binary).resolve())
    unittest.main(argv=[__file__, *remaining])
