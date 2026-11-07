#!/usr/bin/env python3
"""Optional DXF/DWG 2D-modelspace bridge; stdout is a VTX scene JSON document.

Dependencies: ezdxf==1.4.3, Pillow>=10,<13. DWG additionally uses dwg2dxf.
Geometry is explicitly projected onto world XY; this is not a 3D-solid reader.
"""
import contextlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


def load_drawing(filename):
    try:
        import ezdxf
        from ezdxf.addons.drawing import Frontend, RenderContext
        from ezdxf.addons.drawing.backend import Backend
        from ezdxf.addons.drawing.config import Configuration
        from ezdxf.path import triangulate
        from ezdxf.math.triangulation import mapbox_earcut_2d
    except ImportError as exc:
        raise RuntimeError("CAD dependencies missing. Run scripts/install_cad_support.py or set VTX_CAD_PYTHON to a Python with ezdxf==1.4.3 and Pillow installed.") from exc

    warnings = []
    parts = {}

    class SceneBackend(Backend):
        def set_background(self, color):
            pass

        def group(self, properties, kind):
            entity = self.current_entity
            block_path = [str(e.dxf.handle or e.dxftype()) for e, _ in self.entity_stack]
            handle = str(entity.dxf.handle or "virtual") if entity else "unknown"
            key = (tuple(block_path), handle, properties.color, kind)
            if key not in parts:
                color = properties.color.lstrip("#")
                rgba = [int(color[i:i+2], 16) for i in (0, 2, 4)] + [255]
                parts[key] = {"positions": [], "kind": kind, "color": rgba,
                    "metadata": {"handle": handle, "entity_type": entity.dxftype() if entity else None,
                                 "layer": entity.dxf.get("layer", "0") if entity else "0", "block_path": block_path,
                                 "text": entity.dxf.text if entity is not None and entity.dxf.hasattr("text") else None}}
            return parts[key]["positions"]

        def draw_point(self, pos, properties):
            self.group(properties, "points").append([pos.x, pos.y, 0.0])

        def draw_line(self, start, end, properties):
            self.group(properties, "lines").extend([[start.x, start.y, 0.0], [end.x, end.y, 0.0]])

        def draw_filled_polygon(self, points, properties):
            target = self.group(properties, "triangles")
            for triangle in mapbox_earcut_2d(points.vertices()):
                target.extend([[p.x, p.y, 0.0] for p in triangle])

        def draw_filled_paths(self, paths, properties):
            # Preserve nested rings, including holes in text and hatch boundaries.
            target = self.group(properties, "triangles")
            for triangle in triangulate((p.to_path() for p in paths), max_sagitta=self.config.max_flattening_distance):
                target.extend([[p.x, p.y, 0.0] for p in triangle])

        def draw_image(self, image_data, properties):
            entity = self.current_entity
            warnings.append({"handle": entity.dxf.handle if entity else None, "reason": "Raster image underlay is not imported"})

        def clear(self):
            parts.clear()

    class ReportingFrontend(Frontend):
        def skip_entity(self, entity, msg):
            # Hidden layers/entities are deliberately excluded from the visible drawing.
            if "invisible" not in msg.lower():
                warnings.append({"handle": entity.dxf.get("handle"), "entity_type": entity.dxftype(), "reason": msg})

        def log_message(self, message):
            warnings.append({"reason": str(message)})

    with tempfile.TemporaryDirectory(prefix="vtx-drawing-") as tmp:
        source = Path(filename).resolve()
        converted = source
        conversion = None
        if source.suffix.lower() == ".dwg":
            converter = os.environ.get("VTX_DWG2DXF") or shutil.which("dwg2dxf")
            if not converter:
                raise RuntimeError("DWG support requires LibreDWG dwg2dxf. Install it on PATH or set VTX_DWG2DXF to the executable.")
            converted = Path(tmp) / "drawing.dxf"
            result = subprocess.run([converter, "-o", str(converted), str(source)], capture_output=True, text=True, errors="replace", timeout=120)
            # LibreDWG can produce partial output with a nonzero status: never silently accept it.
            if result.returncode != 0 or not converted.is_file():
                raise RuntimeError(f"DWG conversion failed (exit {result.returncode}): {result.stderr[-4000:]} {result.stdout[-1000:]}")
            conversion = {"tool": "LibreDWG dwg2dxf", "diagnostics": (result.stderr + result.stdout)[-8000:]}
            if "warning" in (result.stderr + result.stdout).lower():
                warnings.append({"reason": "DWG converter reported warnings; inspect conversion diagnostics"})
        document = ezdxf.readfile(converted)
        audit = document.audit()
        if audit.has_errors or audit.has_fixes:
            warnings.append({"reason": "DXF auditor reported issues", "errors": len(audit.errors), "fixes": len(audit.fixes)})
        backend = SceneBackend()
        frontend = ReportingFrontend(RenderContext(document), backend, config=Configuration(pdsize=1))
        with contextlib.redirect_stdout(sys.stderr):
            frontend.draw_layout(document.modelspace(), finalize=True)
        output = [p for p in parts.values() if p["positions"]]
        if not output:
            raise RuntimeError("Drawing contains no supported visible modelspace geometry (paperspace-only drawings are not imported)")
        try:
            meters_per_unit = ezdxf.units.conversion_factor(document.units, ezdxf.units.M) if document.units else None
        except (ValueError, TypeError):
            meters_per_unit = None
        return {"parts": output, "source_info": {"kind": "drawing_2d", "projection": "modelspace projected onto XY; Z set to zero",
            "format": source.suffix.lower()[1:], "dxf_version": document.dxfversion,
            "insunits": int(document.header.get("$INSUNITS", 0)), "meters_per_unit": meters_per_unit, "units_label": ezdxf.units.unit_name(document.units),
            "conversion": conversion, "warnings": warnings,
            "limitations": ["2D modelspace only; paperspace layouts are not imported", "ACIS/3DSOLID bodies and raster underlays are not reconstructed", "Lines render at one pixel; font substitutions and curve tessellation may affect appearance"]}}


if __name__ == "__main__":
    try:
        with contextlib.redirect_stdout(sys.stderr):
            drawing = load_drawing(sys.argv[1])
        print(json.dumps(drawing, allow_nan=False, separators=(",", ":")))
    except Exception as exc:
        print(f"VTX CAD import: {exc}", file=sys.stderr)
        sys.exit(1)
