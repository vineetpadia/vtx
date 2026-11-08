# Standalone CAD importer (experimental)

Runs offline in a separate process, without SOLIDWORKS, Fusion, a cloud service,
or an end-user Python installation. The existing mesh, STEP, DXF, IFC and
SolidWorks display-data readers remain independent of this helper.

| Input | Current behavior |
| --- | --- |
| Parasolid X_T / X_B and aliases | Experimental schema/geometry subset through parasolid-kit 0.3.15. Complete supported geometry only; assembly placements and supported face/body RGB. Units require user selection. Modern B3 binary encoding and some neutral PS binary schemas fail explicitly. This is **not universal Parasolid support**. |
| IGES / IGS | OCCT face tessellation and supported surface RGB, normalized to millimeters. Textures/transparency and wire-only geometry are not displayed. |
| Open CASCADE BREP | Face tessellation; source units require user selection. |
| Fusion F3D | Separate built-in saved-image fallback. Pan/zoom only; no native 3D geometry or measurement. |
| SolidWorks SLDDRW | Separate built-in saved-image fallback. Pan/zoom only; no drawing entities or measurement. |

SLDPRT/SLDASM continue using saved display data, without SOLIDWORKS installed.
They do not reconstruct every native feature or appearance. All-format,
all-version, full-fidelity proprietary CAD import is not implemented.

The helper is launched only for its supported CAD formats. Rapid opens reuse
one private converter process so Python and the CAD kernel are loaded once.
It exits after 45 seconds idle or 16 conversions, and closes with VTX; no
network service or background installation is used. Successful meshes
are cached against source path, size, modification time and engine version.
Conversion runs outside the UI, has input/output/face/mesh limits and a two-minute
timeout, and rejects incomplete or invalid geometry. Source files are read-only.
Cold conversion is slower than reopening a cached mesh; no universal timing is
claimed. A native conversion process is isolated for robustness, not sandboxed.

Material groups use compact vertex spans, so adding colors does not repeat the
whole model's vertex buffer for every material. The generated 32-color regression
fixture contains 768 vertices in total, rather than 768 vertices per color.

Real binary qualification includes two public part files from
[42006-Technic-Model](https://github.com/Himalia13/42006-Technic-Model), with
90/274 faces and 5,246/10,048 rendered triangles. Other real binary inputs
still fail explicitly; this is not an all-version X_B reader. CI downloads a
pinned, hash-checked binary into temporary storage; model assets and screenshots
are not included in the repository.

## Build and test

Use Python 3.12 in a fresh venv:

```text
python -m pip install -r native/cad-importer/requirements.txt
python scripts/test_cad_importer.py
python scripts/build_cad_importer.py --output helper-dist --work helper-build
python scripts/test_cad_importer.py --helper helper-dist/vtx-cad-import/vtx-cad-import
```

On Windows the helper has an `.exe` suffix. Copy the contents of the frozen
`vtx-cad-import` directory into `cad_import` beside VTX, or into
`VTX.app/Contents/Resources/cad_import` on macOS. Keep `_internal` and its shared
libraries intact. During development, `VTX_CAD_PYTHON` may explicitly point at
the build venv interpreter; normal installations never search PATH for Python.

The CI validation workflow tests Windows x64, Linux x64, macOS ARM64 and macOS
x64. It does not publish or update the installed stable app.

For successive-open GPU timing in a single viewer process:

```text
VTX --bench-open first.x_t second.x_b first.x_t second.x_b
```

This reports renderer startup separately, then model readiness and GPU frame
completion for each open. It validates that the rendered image is not uniform;
PNG encoding, window composition and screen presentation are excluded. The
`STEPFLASH_PROFILE=1` development setting also reports cache hits and converter
reuse. Delete no user files to benchmark cold loads: use copies at new paths.

## Dependencies and distribution status

- [parasolid-kit 0.3.15](https://github.com/monozukuri-ai/parasolid-kit/tree/v0.3.15): MIT AND Apache-2.0; reviewed format subset, not the licensed Siemens kernel.
- [CadQuery OCP no-VTK 7.9.3.1.1](https://github.com/CadQuery/ocp-build-system/tree/v7.9.3.1.1): Apache-2.0 bindings, Open CASCADE LGPL-2.1 with its exception, plus native dependencies.
- Python: PSF license; PyInstaller: GPL with its bootloader exception.

Public helper binaries are **not released yet**. Before redistribution, audit
the exact platform wheels and ship all bundled native dependency license texts
(including OCCT and image/parallel runtime libraries), source/rebuild information
for applicable copyleft libraries, and Python/package notices. The package
notice collector alone does not complete that audit. OCCT libraries remain
dynamically loaded and replaceable. No commercial translator is bundled.
