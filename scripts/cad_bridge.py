#!/usr/bin/env python3
"""Optional DXF/DWG modelspace bridge; stdout is a VTX scene JSON document.

Dependencies: ezdxf==1.4.3, Pillow>=10,<13. DWG additionally uses dwg2dxf.
Drawing entities project onto XY. Planar ACIS faces retain their XYZ coordinates.
"""
from array import array
from collections import Counter
import struct
import time
import contextlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


def acis_triangles(entity, world_transform=None):
    """Import planar ACIS faces with straight edges, preserving holes and 3D.

    Reject an entire entity if any face is unsupported. Displaying just the flat
    faces of a curved solid would present an incomplete solid as a complete one.
    """
    from ezdxf.acis import api
    from ezdxf.math.triangulation import mapbox_earcut_3d

    def linked(first, next_name, circular=False):
        seen = set()
        current = first
        while not current.is_none:
            if id(current) in seen:
                if circular and current is first:
                    return
                raise ValueError("Invalid cyclic ACIS topology")
            seen.add(id(current))
            yield current
            current = getattr(current, next_name)
        if circular:
            raise ValueError("Open ACIS boundary")

    bodies = api.load_dxf(entity)
    triangles = []
    for body in bodies:
        if not body.wire.is_none:
            raise ValueError("ACIS wire bodies are unsupported")
        transform = None if body.transform.is_none else body.transform.matrix
        mirrored = transform is not None and transform.determinant() < 0
        for lump in linked(body.lump, "next_lump"):
            for shell in linked(lump.shell, "next_shell"):
                if not shell.subshell.is_none or not shell.wire.is_none:
                    raise ValueError("ACIS subshell/wire topology is unsupported")
                for face in linked(shell.face, "next_face"):
                    if face.surface.type != "plane-surface":
                        raise ValueError("Curved ACIS surfaces require an external tessellation kernel")
                    rings = []
                    for loop in linked(face.loop, "next_loop"):
                        ring = []
                        for coedge in linked(loop.coedge, "next_coedge", circular=True):
                            edge = coedge.edge
                            if edge.is_none or edge.curve.type != "straight-curve":
                                raise ValueError("Curved ACIS edges require an external tessellation kernel")
                            vertex = edge.end_vertex if coedge.sense else edge.start_vertex
                            ring.append(vertex.point.location)
                        if len(ring) < 3:
                            raise ValueError("Degenerate ACIS boundary")
                        rings.append(ring)
                    if not rings:
                        raise ValueError("Unbounded ACIS face")
                    normal = -face.surface.normal if face.sense else face.surface.normal
                    # Linked-loop order need not identify the outer boundary.
                    def ring_area(ring):
                        origin = ring[0]
                        return abs(sum((a-origin).cross(b-origin).dot(normal) for a,b in zip(ring, ring[1:]+ring[:1])))
                    exterior = max(range(len(rings)), key=lambda i: ring_area(rings[i]))
                    rings[0], rings[exterior] = rings[exterior], rings[0]
                    face_triangles = list(mapbox_earcut_3d(rings[0], rings[1:]))
                    if not face_triangles:
                        raise ValueError("Degenerate ACIS face")
                    for triangle in face_triangles:
                        a, b, c = triangle
                        if (b-a).cross(c-a).dot(normal) < 0:
                            triangle = (a, c, b)
                        if transform is not None:
                            triangle = tuple(transform.transform_vertices(triangle))
                            if mirrored:
                                triangle = (triangle[0], triangle[2], triangle[1])
                        if world_transform is not None:
                            triangle = tuple(world_transform.transform_vertices(triangle))
                            if world_transform.determinant() < 0:
                                triangle = (triangle[0], triangle[2], triangle[1])
                        triangles.extend([list(p) for p in triangle])
    if not triangles:
        raise ValueError("ACIS entity has no supported faces")
    return triangles


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

    imported=time.perf_counter()
    warnings = []
    parts = {}
    solid_entities = 0
    recovery = None

    class SceneBackend(Backend):
        def __init__(self):
            super().__init__()
            self.group_cache = {}

        def enter_entity(self, entity, properties):
            super().enter_entity(entity, properties)
            self.group_cache.clear()

        def exit_entity(self, entity):
            super().exit_entity(entity)
            self.group_cache.clear()

        def set_background(self, color):
            pass

        def group(self, properties, kind):
            cached = self.group_cache.get((properties.color, kind))
            if cached is not None:
                return cached
            entity = self.current_entity
            def entity_handle(e):
                original = e.origin_of_copy
                return str(e.dxf.handle or (original.dxf.handle if original is not None else None) or e.dxftype())
            block_path = [entity_handle(e) for e, _ in self.entity_stack]
            handle = entity_handle(entity) if entity is not None else "unknown"
            key = (tuple(block_path), handle, properties.color, kind)
            if key not in parts:
                color = properties.color.lstrip("#")
                rgba = [int(color[i:i+2], 16) for i in (0, 2, 4)] + [255]
                if len(color) == 8 and color[6:8].lower() != "ff":
                    warnings.append({"handle": handle, "reason": "Drawing transparency is approximated as opaque"})
                text = None
                if entity is not None and entity.dxf.hasattr("text"):
                    text = entity.plain_text() if hasattr(entity, "plain_text") else entity.dxf.text
                parts[key] = {"positions": [], "kind": kind, "color": rgba,
                    "metadata": {"handle": handle, "entity_type": entity.dxftype() if entity else None,
                                 "layer": entity.dxf.get("layer", "0") if entity else "0", "block_path": block_path,
                                 "text": text}}
            target = parts[key]["positions"]
            self.group_cache[(properties.color, kind)] = target
            return target

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
            for triangle in triangulate((p.to_path() for p in paths), max_sagitta=self.config.max_flattening_distance, min_segments=4):
                target.extend([[p.x, p.y, 0.0] for p in triangle])

        def draw_solid_lines(self, lines, properties):
            target = self.group(properties, "lines")
            for start, end in lines:
                if end.isclose(start):
                    self.draw_point(start, properties)
                else:
                    target.extend([[start.x, start.y, 0.0], [end.x, end.y, 0.0]])

        def draw_path(self, path, properties):
            if not len(path):
                return
            points = iter(path.flattening(distance=self.config.max_flattening_distance))
            start = next(points)
            target = self.group(properties, "lines")
            for end in points:
                target.extend([[start.x, start.y, 0.0], [end.x, end.y, 0.0]])
                start = end

        def draw_image(self, image_data, properties):
            entity = self.current_entity
            warnings.append({"handle": entity.dxf.handle if entity else None, "reason": "Raster image underlay is not imported"})

        def clear(self):
            parts.clear()

    class ReportingFrontend(Frontend):
        def _build_dispatch_table(self):
            dispatch = super()._build_dispatch_table()
            for kind in ("3DSOLID", "BODY", "REGION"):
                dispatch[kind] = self.draw_acis_entity
            return dispatch

        def draw_acis_entity(self, entity, properties):
            nonlocal solid_entities
            try:
                from ezdxf.xclip import XClip
                ancestors = [e for e, _ in backend.entity_stack[:-1] if e.dxftype() == "INSERT"]
                if any(e.mcount > 1 for e in ancestors):
                    raise ValueError("ACIS inside multiple-insert arrays is unsupported")
                if any(XClip(e).has_clipping_path and XClip(e).is_clipping_enabled for e in ancestors):
                    raise ValueError("ACIS inside clipped block inserts is unsupported")
                # ezdxf's virtual ACIS copies share deferred transforms. Use the
                # nearest insert's WCS matrix, rather than those mutable copies.
                matrix = ancestors[-1].matrix44() if ancestors else None
                triangles = acis_triangles(entity, matrix)
            except Exception as exc:
                self.skip_entity(entity, f"ACIS entity omitted: {exc}")
                return
            backend.group(properties, "triangles").extend(triangles)
            solid_entities += 1

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
            diagnostics = (result.stderr + result.stdout).lower()
            if "warning" in diagnostics or "error" in diagnostics:
                warnings.append({"reason": "DWG converter reported warnings or errors; inspect conversion diagnostics"})
        try:
            document = ezdxf.readfile(converted)
            audit = document.audit()
        except ezdxf.DXFStructureError as exc:
            from ezdxf import recover
            document, audit = recover.readfile(converted)
            recovery = {"reader": "ezdxf.recover", "original_error": str(exc)}
            warnings.append({"reason": "DXF structure recovered; content may have been repaired or discarded", **recovery})
        if audit.has_errors or audit.has_fixes:
            warnings.append({"reason": "DXF auditor reported issues", "errors": len(audit.errors), "fixes": len(audit.fixes)})
        backend = SceneBackend()
        inventory = {
            "modelspace_entities": len(document.modelspace()),
            "entity_types": dict(Counter(e.dxftype() for e in document.modelspace())),
            "layers": [{"name": layer.dxf.name, "off": layer.is_off(), "frozen": layer.is_frozen(), "aci_color": abs(layer.dxf.color)} for layer in document.layers],
            "layouts": [{"name": layout.name, "entities": len(layout)} for layout in document.layouts],
            "external_references": [{"name": block.name, "path": block.block.dxf.get("xref_path", "")} for block in document.blocks if block.block.is_xref],
        }
        if inventory["external_references"]:
            warnings.append({"reason": "External reference files are not resolved; drawing may be incomplete", "references": inventory["external_references"]})
        frontend = ReportingFrontend(RenderContext(document), backend, config=Configuration(pdsize=1))
        with contextlib.redirect_stdout(sys.stderr):
            frontend.draw_layout(document.modelspace(), finalize=True)
        output = [p for p in parts.values() if p["positions"]]
        if not output:
            detail = "; ".join(str(w.get("reason", "")) for w in warnings[:4])
            raise RuntimeError("Drawing contains no supported visible modelspace geometry. " + detail)
        try:
            meters_per_unit = ezdxf.units.conversion_factor(document.units, ezdxf.units.M) if document.units else None
        except (ValueError, TypeError):
            meters_per_unit = None
        return {"parts": output, "source_info": {"kind": "drawing_3d" if solid_entities else "drawing_2d",
            "projection": "Planar ACIS faces retain XYZ; other drawing entities project onto XY" if solid_entities else "modelspace projected onto XY; Z set to zero",
            "solid_entities": solid_entities, "recovery": recovery,
            "inventory": inventory,
            "bridge_parse_draw_ms": (time.perf_counter()-imported)*1000,
            "format": source.suffix.lower()[1:], "dxf_version": document.dxfversion,
            "insunits": int(document.header.get("$INSUNITS", 0)), "meters_per_unit": meters_per_unit, "units_label": ezdxf.units.unit_name(document.units),
            "conversion": conversion, "warnings": warnings,
            "limitations": ["Modelspace only; paperspace layouts are not imported", "Only planar ACIS faces with straight boundaries are supported; curved solids are omitted as whole entities with warnings", "Non-ACIS entities project onto XY; raster underlays are omitted", "Lines render at one pixel; font substitutions and curve tessellation may affect appearance"]}}


if __name__ == "__main__":
    try:
        with contextlib.redirect_stdout(sys.stderr):
            drawing = load_drawing(sys.argv[1])
        if os.environ.get("VTX_CAD_BINARY") == "1":
            payloads=[]
            for part in drawing["parts"]:
                points=part.pop("positions")
                part["vertex_count"]=len(points)
                floats=array("f",(coordinate for point in points for coordinate in point))
                if sys.byteorder != "little": floats.byteswap()
                payloads.append(floats)
            metadata=json.dumps(drawing,allow_nan=False,separators=(",", ":")).encode("utf-8")
            stream=sys.stdout.buffer
            stream.write(struct.pack("<8sQ",b"VTXCAD01",len(metadata)))
            stream.write(metadata)
            for payload in payloads: stream.write(payload.tobytes())
        else:
            print(json.dumps(drawing, allow_nan=False, separators=(",", ":")))
    except Exception as exc:
        print(f"VTX CAD import: {exc}", file=sys.stderr)
        sys.exit(1)
