# VTX

A fast, local 3D viewer for Windows, macOS, and Linux. Open large models in a standalone GPU-rendered app.

## Download

| Platform | Download |
|---|---|
| Windows x64 | [Windows ZIP](https://github.com/vineetpadia/vtx/releases/latest/download/vtx-windows-x64.zip) |
| Mac — Apple silicon | [Mac ZIP](https://github.com/vineetpadia/vtx/releases/latest/download/vtx-macos-arm64.zip) |
| Mac — Intel | [Mac ZIP](https://github.com/vineetpadia/vtx/releases/latest/download/vtx-macos-x64.zip) |
| Linux x64 | [Linux archive](https://github.com/vineetpadia/vtx/releases/latest/download/vtx-linux-x64.tar.gz) |

**Download → extract → open VTX.** On Mac, drag **VTX.app** into Applications. No Rust or Python needed.

Preview builds are unsigned; your OS may ask for approval on first launch. [Installation help and system requirements](docs/INSTALL.md) · [All releases](https://github.com/vineetpadia/vtx/releases)

## Use

Drop a model into the window, or press **Ctrl+O** (**⌘O** on Mac). Drag to orbit, scroll to zoom, **R** to reset.

**Formats:** STL, OBJ, PLY, glTF/GLB, FBX, STEP, 3MF, IFC, and SolidWorks saved display meshes. Common ASCII DXF linework is built in. DXF/DWG drawing previews, planar ACIS solids, and IGES are available through optional extensions. Drawings support disclosed recovery and a verified local cache for repeat opens. See [drawing setup and limitations](docs/DRAWINGS.md). Keep model textures and companion files together.

## Inspect with an LLM

Export standard orthographic views, a labeled contact sheet with scale bars and XYZ extents, and a JSON manifest without opening a window:

```sh
vtx --inspect model.step --output model-inspection
```

Use `--region` for detail views, `--section` for section cuts, and repeated `--query view,x,y` options for visible surface positions, normals, and part IDs. DXF/DWG default to a plan view.

Use `--distance`, `--angle`, and `--diameter` to measure selected 3D query points; `--measure-unit` converts reported lengths. [Measurement guide](docs/MEASUREMENTS.md).

Units are detected for STEP, glTF, 3MF, and drawing headers with explicit provenance. Other formats remain in model units; `--unit mm` is a user override, not a geometry conversion. See [agent inspection workflow](docs/INSPECTION.md) for conventions, options, and limitations. Build from source to use this feature.

For DXF/DWG, see [drawing load benchmarks and profiling](docs/CAD_PERFORMANCE.md). Common ASCII linework uses a native path; complex content falls back to the full drawing frontend.

The [public CAD test corpus](docs/CAD_CORPUS.md) provides 38 checksum-verified online DXF/DWG samples and a runner that saves previews, timing, warnings, and failures.

## Performance

Same PC, same file, warm cache; median geometry-import times. These exclude app startup and rendering.

| Model | Compared with | Reference | VTX | Speedup |
|---|---|---:|---:|---:|
| Voron STEP, 6.43M triangles | Foxtrot | 4,415 ms | 3,182 ms | **1.39×** |
| Rungholt OBJ, 6.70M triangles | RapidOBJ | 196.85 ms | 60.55 ms | **3.25×** |
| A Beautiful Game GLB, 1.50M triangles | FastGLTF | 36.96 ms | 5.48 ms | **6.74×** |

Geometry was checked against each reference. Voron retains 666 tessellation errors in both readers. Reader feature coverage differs; see [all comparisons, methods, and limitations](docs/BENCHMARKS.md).

[Build from source](docs/DEVELOPMENT.md) · [Benchmark models](docs/BENCHMARKS.md#public-benchmark-models)

VTX's STEP core derives from [Formlabs Foxtrot](https://github.com/formlabs/foxtrot), under [MIT](native/LICENSE-MIT) / [Apache 2.0](native/LICENSE-APACHE). See [third-party attribution](vendor/sldprt/NOTICE.md) for the SolidWorks reader.
