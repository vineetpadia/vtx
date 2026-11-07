# DXF and DWG drawings

The native VTX desktop app and `--inspect` support **2D modelspace drawing previews**. DXF uses [ezdxf's drawing frontend](https://ezdxf.readthedocs.io/en/stable/addons/drawing.html). DWG is converted locally to DXF by the separate [GNU LibreDWG `dwg2dxf` executable](https://www.gnu.org/software/libredwg/). No drawings are uploaded to a service.

## Install optional dependencies

Python 3.10+ is required for the drawing bridge. From the repository:

```sh
python3 scripts/install_cad_support.py
# Linux/macOS: also compile and install LibreDWG 0.14 locally:
python3 scripts/install_cad_support.py --build-libredwg
```

The second command requires a C compiler, `make`, `pkg-config`, and the build dependencies required by LibreDWG. It downloads a pinned source archive, verifies its SHA-256, and installs in `~/.local/share/vtx/cad-tools`. It does not need administrator access. LibreDWG is GPL-3.0-or-later and remains a separate executable, with its own installed attribution/licenses; no LibreDWG code is linked into VTX.

On Windows, install a LibreDWG `dwg2dxf.exe` separately and set `VTX_DWG2DXF` to its full path, or put it on PATH. Python's default environment is installed under `~/.local/share/vtx/cad-venv`; set `VTX_CAD_PYTHON` to its `Scripts/python.exe` on Windows. On Linux the native viewer detects that default location automatically. For custom installs, set `VTX_CAD_PYTHON` to the venv Python and `VTX_DWG2DXF` to the converter executable. macOS can use the same environment variables or adjacent `cad-venv`/`cad-tools` directories.

The bridge is embedded in the native binary. The environment provides ezdxf 1.4.3 and Pillow 12.3.0. Non-drawing formats continue to run without these optional dependencies. Released binaries predating these changes need to be rebuilt.

## Use

```sh
vtx drawing.dxf
vtx drawing.dwg
vtx --inspect drawing.dxf --output drawing-inspection
vtx --inspect drawing.dwg --output dwg-inspection --views plan --size 1024
```

The file picker accepts both extensions. Drawings open in an orthographic plan view; R restores that view. Inspection defaults to `plan.png`, looking from +Z onto XY. Other standard views can be explicitly selected, but side views of a 2D projection are usually edge-on. Region framing and pixel queries work with drawings; a line hit returns an entity handle/layer when available and a null surface normal.

Supported preview content includes linework, arcs/circles, polylines and their curves, block insert transforms, filled paths/hatches (with holes), and text/dimensions supported by ezdxf. ASCII and binary DXF are accepted. Layer colors and visibility are resolved by ezdxf. Fonts can be substituted; text and curves are tessellated. Linework is rendered at one pixel rather than physical plot lineweights.

## Scope and diagnostics

This is a **2D projection onto XY**: imported Z is zero. It does not reconstruct DWG/DXF ACIS/3DSOLID geometry or offer 3D solid operations. Paperspace layouts, raster underlays, and external references are not loaded as complete drawings. Unsupported entities and converter/auditor warnings are retained in `source_info`; the image footer reports the import issue count. Do not assume a preview is complete when warnings are present. The browser viewer does not yet support the drawing bridge.

The drawing header's `$INSUNITS` supplies the label when nonzero; unitless files stay in model units. VTX does not convert coordinates. Explicit `--unit` labels override the header and carry `user_override` provenance.

DWG compatibility depends on LibreDWG's entity/version support. A nonzero converter exit status is treated as a failed import, even if partial DXF output exists. Conversion has a 120-second timeout. Neither DXF nor DWG source files are modified; DWG conversion uses an isolated temporary directory.

Entity metadata includes handles, types, layers, and nested block paths. Virtual block entities may not have their own persistent handle; use the block path and import `part_id` together, and do not treat IDs as stable across edits.

## Tests

```sh
~/.local/share/vtx/cad-venv/bin/python scripts/test_cad_bridge.py
# Include a real DWG round trip through LibreDWG:
VTX_DWG2DXF=~/.local/share/vtx/cad-tools/bin/dwg2dxf \
  ~/.local/share/vtx/cad-venv/bin/python scripts/test_cad_bridge.py
```

Tests cover ASCII/binary DXF, transformed block inserts, layer metadata, hatch holes, invalid-file errors, and DWG conversion when LibreDWG's writer is installed. Native GPU tests separately verify scale, visible-surface queries, misses, opposite-side depth/normal orientation, detail framing, and section consistency.
