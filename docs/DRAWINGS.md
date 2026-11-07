# DXF and DWG drawings

The native VTX desktop app and `--inspect` support **modelspace drawing previews and planar ACIS solids in 3D**. Common ASCII DXF linework uses a native reader; other content uses [ezdxf's drawing frontend](https://ezdxf.readthedocs.io/en/stable/addons/drawing.html) and VTX's conservative planar ACIS tessellator. DWG is converted locally to DXF by the separate [GNU LibreDWG `dwg2dxf` executable](https://www.gnu.org/software/libredwg/). No drawings are uploaded to a service.

## Install optional dependencies

The native linework path needs no Python. Python 3.10+ is required for the full drawing fallback. From the repository:

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

See [performance measurements and profiling](CAD_PERFORMANCE.md) for the native path, batching, and reproducible benchmarks.

The file picker accepts both extensions. 2D drawings open in an orthographic plan view; R restores that view. Their inspection defaults to `plan.png`, looking from +Z onto XY. Drawings with supported ACIS solids open upright with Z up and default to seven inspection views: front looks from -Y, right from +X, top from +Z. Their explicit `plan` view also looks from +Z. The camera rotation preserves original source XYZ coordinates for sections, queries and measurements. R restores the appropriate camera. Region framing and pixel queries work with drawings; a line hit returns an entity handle/layer when available and a null surface normal.

Supported preview content includes linework, arcs/circles, polylines and their curves, block insert transforms, filled paths/hatches (with holes), and text/dimensions supported by ezdxf. ASCII and binary DXF are accepted. Layer colors and visibility are resolved by ezdxf. Fonts can be substituted; text and curves are tessellated. Linework is rendered at one pixel rather than physical plot lineweights.

## Scope and diagnostics

Non-ACIS drawing entities remain a **2D projection onto XY**. ACIS `3DSOLID`, `BODY` and `REGION` entities are accepted only when all faces are planar with straight, closed boundaries. VTX triangulates inner loops as holes, applies body and nested/repeated insert transforms (including mirrors), and preserves XYZ and outward triangle winding. ACIS inside clipped inserts or multiple-insert arrays is currently rejected with warnings. A curved/unsupported face rejects the **entire entity**, with a warning; flat fragments of a curved solid are not presented as a complete body. This is display geometry, not a solid modeling kernel. Mixed documents may contain actual 3D solids and projected 2D entities.

Paperspace layouts and raster underlays are not imported. External references are listed but not resolved, with a warning. Unsupported entities and converter/auditor warnings remain in `source_info`; the image footer reports the issue count. Transparent drawing colors are approximated as opaque with a warning. Do not assume a preview is complete when warnings are present. The browser viewer does not yet support the drawing bridge.

Strict DXF structure errors trigger `ezdxf.recover`. Recovery is disclosed in `source_info.recovery` and warnings, including the original error and auditor counts. Recovering an empty or unsupported drawing still fails. Source files are never repaired in place. Fatal DWG bridge errors now include the converter diagnostics.

The full bridge's `source_info.inventory` lists modelspace entity counts/types, layers with visibility flags, layout names/entity counts, and external references. Entity text metadata uses plain text where ezdxf provides it, supporting manifest searches without OCR. Virtual block entities retain their originating DXF handle when available, with the insert path distinguishing instances.

## Repeat-open geometry cache

Complex drawings can reuse local parsed geometry. Simple native DXFs bypass the cache because parsing is cheaper; files smaller than 64 KiB also skip it. The default directory is `$XDG_CACHE_HOME/vtx/drawings`, `$LOCALAPPDATA/vtx/drawings`, or `~/.cache/vtx/drawings`.

Cache keys include source SHA-256 content identity and path, importer code, Python environment/dependency metadata, converter identity for DWG, system/user font metadata, and the ezdxf font-manager cache identity. Unsupported/custom global Python or configuration/search paths disable caching. Venv dependencies and fonts are tracked by path, size and modification time; deliberately replacing a dependency while preserving those metadata requires clearing/disabling the cache.

Entries embed their key and a SHA-256 payload checksum. Corrupt, truncated, oversized or incompatible entries are misses. Writes are atomic, failed imports are not stored, and input/dependency changes during import prevent storing the result. Entries are limited to 256 MiB, with a 512 MiB disk budget and older-entry eviction. Cache I/O failure does not fail a drawing import. These limits bound retained data, not peak uncached-import memory.

Use `VTX_CAD_CACHE=0` to disable caching, or `VTX_CAD_CACHE_DIR=/path/to/cache` to choose another directory. `VTX_CAD_FORCE_FALLBACK=1` also disables it. `source_info.cache` reports `hit`, `miss`, `disabled` or `bypassed_native`; `load_ms` measures the current load including cache work. On a hit, bridge/conversion fields describe the original import, and `cache.original_import_ms` preserves its time. Cache-hit timings must be reported separately from first imports.

The drawing header's `$INSUNITS` supplies the label when nonzero; unitless files stay in model units. VTX does not convert coordinates. Explicit `--unit` labels override the header and carry `user_override` provenance.

DWG compatibility depends on LibreDWG's entity/version support. A nonzero converter exit status is treated as a failed import, even if partial DXF output exists. Conversion has a 120-second timeout. Neither DXF nor DWG source files are modified; DWG conversion uses an isolated temporary directory.

Entity metadata includes handles, types, layers, and nested block paths. Virtual block entities may not have their own persistent handle; use the block path and import `part_id` together, and do not treat IDs as stable across edits.

## Tests

```sh
~/.local/share/vtx/cad-venv/bin/python scripts/test_cad_bridge.py
~/.local/share/vtx/cad-venv/bin/python scripts/test_cad_cache.py --binary native/target/release/examples/vtx
# Include a real DWG round trip through LibreDWG:
VTX_DWG2DXF=~/.local/share/vtx/cad-tools/bin/dwg2dxf \
  ~/.local/share/vtx/cad-venv/bin/python scripts/test_cad_bridge.py
```

Tests cover ASCII/binary DXF, transformed blocks, layers, hatch holes, ACIS volume/winding/transforms and vertical face holes, whole-entity rejection for unsupported surfaces, disclosed recovery, cache invalidation despite unchanged source size/time, corruption fallback, identical cached/uncached images and metadata, upright CAD views/world queries, and DWG round trips. Native GPU tests also verify scale, queries, misses, opposite-side depth/normal orientation, detail framing, and sections.
