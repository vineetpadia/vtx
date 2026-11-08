# VTX

A fast, local 3D viewer for Windows, macOS, and Linux. Open large models in a standalone GPU-rendered app.

## Download

| Platform | Download |
|---|---|
| Windows x64 | [Windows ZIP](https://github.com/vineetpadia/vtx/releases/latest/download/vtx-windows-x64.zip) |
| Mac — Apple silicon | [Mac ZIP](https://github.com/vineetpadia/vtx/releases/latest/download/vtx-macos-arm64.zip) |
| Mac — Intel | [Mac ZIP](https://github.com/vineetpadia/vtx/releases/latest/download/vtx-macos-x64.zip) |
| Linux x64 | [Linux archive](https://github.com/vineetpadia/vtx/releases/latest/download/vtx-linux-x64.tar.gz) |

**Windows:** use [VTX-Setup.exe](https://github.com/vineetpadia/vtx/releases/latest/download/VTX-Setup.exe) for automatic updates, or extract the portable archive. On Mac, drag **VTX.app** into Applications. No Rust or Python needed.

The Windows installer checks for updates after the first view appears, downloads verified packages in the background, and applies them when the last VTX window closes. **… → Check for updates** offers a manual check/restart. Existing file defaults and separate Explorer previews are preserved. This repository's releases are private: automatic downloads use an existing GitHub CLI login with repository access; tokens are not bundled or saved by VTX. See [installation details](docs/INSTALL.md).

Preview builds are unsigned; your OS may ask for approval on first launch. [Installation help and system requirements](docs/INSTALL.md) · [All releases](https://github.com/vineetpadia/vtx/releases)

## Use

Drop a model into the window, or press **Ctrl+O** (**⌘O** on Mac). Middle-drag to rotate, **Ctrl+middle-drag** to pan, **Shift+middle-drag** to zoom, and **F** to fit. Scroll to zoom toward the pointer; **R** resets the view.

**Formats:** STL, OBJ, PLY, glTF/GLB, FBX, STEP, 3MF, IFC, and SolidWorks saved display meshes. IGES is available through an optional extension. Keep model textures and companion files together.

The current source build also opens DXF drawings in a fitted 2D view with pan, zoom and antialiased linework. The light viewer background is pure white. This DXF support is newer than the published v0.1.1 download. See [installation and format limits](docs/INSTALL.md).

The main app has a compact **Open / Measure / Section / Views** toolbar. Hover and click recognized lines/curves to select them; **Ctrl-click** a second edge for minimum gap, lengths and straight-line angle. Smooth curves select as whole paths. Measure also supports point distances, XYZ offsets, three-point angles, endpoint/center snaps, and **diameter/radius** on circular rims or cylindrical walls. Results copy in mm/inches when source units are known or specified. Feature analysis runs on demand; hover uses a cache and asynchronous visibility checks. Mesh-derived values are labelled approximate; native DXF circles/arcs preserve source geometry. Section along X/Y/Z with a movable plane; use standard and orthographic views. Panels appear only while needed. Unknown source units are never assumed. These tools are in the source build, not the older published download.

The source-build [Explorer preview handler](native/windows-preview/README.md) shows a saved/cached image first, then a navigable GPU view in the same pane. Middle-drag to rotate, Ctrl+middle-drag to pan, wheel to zoom and F to fit; DXF drawings pan in 2D. Preview registration is independent of the default app used for double-clicking.

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

DXF decoding uses [dxf-rs](https://github.com/ixmilia/dxf-rs), under the MIT license. The Windows icon is an original vector asset in [assets/vtx.svg](assets/vtx.svg).
