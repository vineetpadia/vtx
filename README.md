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

**Formats:** STL, OBJ, PLY, glTF/GLB, FBX, STEP, 3MF, IFC, and SolidWorks saved display meshes. IGES is available through an optional extension. Keep model textures and companion files together.

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
