# Benchmarks and technical notes

VTX is a local 3D viewer. The standalone app opens STL, OBJ, PLY, glTF, GLB, FBX, STEP, 3MF, IFC, and SolidWorks display meshes and renders them directly on the GPU. An optional native extension adds IGES.

## Local SolidWorks and Explorer comparison, November 7, 2026

The label/decal update measured Pomona's first saved image at 2.32–2.90 ms in the warm COM-host trials, its first sharp replacement at 565 ms (including the 120 ms dwell), and cached replacements at 61 ms. In five alternating fresh-process HD-render trials per build, median complete process time was 376 ms with labels versus 382 ms before the fix. These are warm file-cache measurements on this machine, excluding Explorer selection/hosting and DLL activation; they are not a guarantee for every file. Initial handler/WIC startup measured about 16 ms. Projection texture alpha/clipping checks passed on Vulkan and DX12; 26 Rust tests, 11 CAD regression models, four format smoke checks, native lifecycle/cancellation/fallback/cache checks passed.

The current progressive Explorer build keeps the embedded thumbnail first (roughly 2.5–4.1 ms in warm COM-host trials), then replaces it in the same pane with a 1920 × 1440 render. Cached replacement measured about 30–76 ms; a first Pomona render took 679 ms including a 120 ms selection pause. Initial handler/WIC startup adds overhead. These intervals exclude Explorer hosting/selection and DLL activation. Work is cancelled on selection changes. Source-model visibility and appearance fixes now also prevent Pomona's hidden compression-part bodies from covering its second cutout. These measurements supersede the thumbnail-only handler figures below.

On Windows with an RTX 2070, the current CAD-style build generated a PNG from ASTM-E96_cup.SLDPRT (23.6 MB) in a median 394 ms versus 5,010 ms for already-running SolidWorks 2025. dHANDLEPart5.SLDPRT (44.8 MB) took 409 ms versus 7,745 ms, respectively: 12.7x and 18.9x. These are three alternating trials per reader with a warm file cache. VTX starts a fresh process with `--check`; SolidWorks silently opens the document read-only, fits/redraws, and exports a PNG-sized BMP. Times include image export; they are not cold-start or interactive-frame-rate measurements. Documents already resident in SolidWorks were excluded, and test documents were closed afterward.

The revised Explorer handler decodes embedded SolidWorks images directly. In its COM host test, image readiness was 2.1–2.8 ms across the cup, handle, NanoPowerAssem2, and Pomona files after initial WIC startup (9.8 ms for the first image). These tests used a DLL folder without VTX.exe, proving no helper process was needed. With every PreviewPNG block removed in a separate copy of the cup, the retained VTX render fallback took 398 ms for first readiness and 9.1–9.4 ms for cached repeats. Source files were untouched. The pane shows one static view with no extra launch button.

The initial helper-based version measured 43 ms for the cup's saved PNG, 443 ms for NanoPowerAssem2 including initial startup, and 460 ms for a STEP render. Explorer's own selection/hosting overhead and DLL activation are excluded from the COM host readiness figures. Actual Explorer integration was checked separately. SolidWorks images reflect the last saved view. These samples do not establish performance for every model or a comparison against F3D.

The native core is a fork of [Formlabs Foxtrot](https://github.com/formlabs/foxtrot/tree/69fb37688892246e670fc08eb60b7d3a92d70d3f). The upstream MIT and Apache 2.0 licenses are in [native](../native/). The fork retains Foxtrot’s tessellation settings and supported geometry. On the Raspberry Pi benchmark both builds reported 14 counted tessellation errors, so their matching meshes share gaps. Open CASCADE mode has separate coverage and quality settings.

The following comparisons are recorded development benchmarks on the same Windows Core i9-12900K with a warm OS file cache. Each row compares the same input file through the listed output. The benchmarks span different VTX revisions, and each speedup uses its own paired measurements. Times are medians in milliseconds. The format sections below give trial counts, reader settings, and geometry verification details.

| Format and fixture | Compared project | Reference, ms | VTX, ms | Speedup | Timed output |
|---|---|---:|---:|---:|---|
| [STEP, Raspberry Pi 3](../step_indexed_foxtrot.csv) | [Foxtrot](https://github.com/formlabs/foxtrot/tree/69fb37688892246e670fc08eb60b7d3a92d70d3f) | 258.17 | 125.07 | 2.06× | Tessellated geometry |
| [STEP, Voron 2.4r2 assembly](../voron_foxtrot_benchmark.csv) | [Foxtrot](https://github.com/formlabs/foxtrot/tree/69fb37688892246e670fc08eb60b7d3a92d70d3f) | 4,415.09 | 3,181.74 | 1.39× | Tessellated geometry, 666 shared errors |
| [STL, 2-million-triangle grid](../benchmark_stl.py) | [pyvista-stl 0.5.3](https://github.com/pyvista/pyvista-stl) with NumPy expansion | 111.06 | 49.65 | 2.24× | Packed triangle positions |
| [OBJ, Rungholt geometry](../obj_benchmark_current.csv) | [RapidOBJ](https://github.com/guybrush77/rapidobj/tree/ccca1622432a3aeb2fb85ff9085219d008d11afc) | 196.85 | 60.55 | 3.25× | Triangulated geometry |
| [OBJ, Rungholt with materials](../obj_material_benchmark.csv) | [RapidOBJ](https://github.com/guybrush77/rapidobj/tree/ccca1622432a3aeb2fb85ff9085219d008d11afc) | 117.56 | 81.95 | 1.43× | Geometry and material assignments |
| [OBJ, San Miguel high, 1.14 GB](../san_miguel_benchmark.csv) | [RapidOBJ](https://github.com/guybrush77/rapidobj/tree/ccca1622432a3aeb2fb85ff9085219d008d11afc) | 632.89 | 214.41 | 2.95× | Triangulated geometry, cyclic vertex check |
| [PLY, 6-million-point RGB cloud](../ply_benchmark.csv) | [Tinyply 3.0](https://github.com/ddiakopoulos/tinyply/tree/c9bb690dfe5e9105961e9e28120c48c9ae084bc6) | 185.84 | 16.96 | 10.95× | Positions and RGB bytes |
| [GLB, 2-million-triangle grid](../gltf_benchmark.csv) | [FastGLTF](https://github.com/spnda/fastgltf/tree/7ed89f6a2e3119bf6721149bfee7ec54658a90b5) | 37.88 | 17.70 | 2.14× | Indexed geometry and scene transforms |
| [GLB, A Beautiful Game](../beautiful_game_benchmark.csv) | [FastGLTF](https://github.com/spnda/fastgltf/tree/7ed89f6a2e3119bf6721149bfee7ec54658a90b5) | 36.96 | 5.48 | 6.74× | Geometry and scene transforms, signed zero normalized |
| [IFC4.3, buildingSMART bridge](../ifc_native_benchmark.csv) | [IFClite 28.2.0](https://crates.io/crates/ifc-lite-processing/28.2.0) default full pass | 21.03 | 12.24 | 1.72× | Geometry and draw plan |
| [3MF, 500,000-triangle textured grid](../three_mf_xml_benchmark.csv) | [lib3mf 2.5.0](https://github.com/3MFConsortium/lib3mf/releases/tag/v2.5.0) | 641.38 | 260.41 | 2.46× | Placed geometry and material data |
| [3MF, eight-part grid, 4 million triangles](../three_mf_xml_benchmark.csv) | [lib3mf 2.5.0](https://github.com/3MFConsortium/lib3mf/releases/tag/v2.5.0) | 2,399.98 | 114.80 | 20.91× | Placed geometry |
| [SLDPRT, NIST CTC-02](../solidworks_native_benchmark.csv) | [sldprt-export](https://github.com/XRTC5/sldprt-export/tree/31d3b5a9b115d9cbfc19bbf10906fcc29e8e3c57) | 43.51 | 2.87 | 15.14× | Saved display mesh |

The speedups use unrounded medians. These intervals cover file reading and import through the listed output. Viewer startup, GPU upload, and first-frame measurements appear in the standalone viewer section. Linked CSVs contain individual trials, and the STL link contains its reproduction harness. The STL row uses VTX's JavaScript decoder under Node.js. Its reference includes expansion from pyvista-stl's indexed mesh into the same packed triangle layout. pyvista-stl's indexed result alone took 60.66 ms in that run.

Reader coverage also differs. RapidOBJ's default loader reads authored normals and other OBJ attributes beyond VTX's geometry path. VTX uses IFClite for IFC geometry, with metadata filtering, worker settings, and draw planning optimized for viewing. VTX uses ufbx for FBX import. The Raspberry Pi STEP comparison retains the same 14 counted tessellation errors in both builds. The per-format sections describe the matching geometry checks and supported features.

The Voron, San Miguel, and A Beautiful Game comparisons each use 12 alternating fresh processes per reader with a warm file cache. Voron matched 6,429,086 oriented triangles, with 666 tessellation errors in both readers. San Miguel matched after rotating the starting vertex of ten triangles. A Beautiful Game's initial byte differences were 438 positive/negative zero coordinates. `benchmark_obj.py --geometry-check cyclic` normalizes signed zero and triangle starting vertices while retaining exact coordinates, face order, and winding. Its default check remains byte-exact. [Geometry checks](../public_model_geometry_checks.json) · [Public model corpus](#public-benchmark-models)

## Run the standalone viewer

Build with Rust, Cargo, and the platform's C and C++ toolchain. On macOS, install the Xcode Command Line Tools.

```sh
cargo build --manifest-path native/Cargo.toml --locked --release -p triangulate --features parallel,desktop --example vtx
./native/target/release/examples/vtx "/path/to/model.step"
```

On Windows, run `native\target\release\examples\vtx.exe`. VTX uses Metal on macOS and Vulkan on Linux. On Windows it tries Vulkan first and falls back to Direct3D 12 if Vulkan initialization fails. Set `WGPU_BACKEND=dx12` to select Direct3D 12. On Linux, run in an X11 or Wayland session with a Vulkan driver. The file chooser uses the desktop's XDG portal.

Drop a model into the window, pass its path on the command line, or press **Ctrl+O** (**⌘O** on macOS). Drag to orbit, scroll to zoom, and press **R** to reset the camera. Press **I** for model details and import diagnostics. Keep glTF companion buffers and images relative to the model file. For textured OBJ, keep the MTL and images in the model folder or its subfolders. The viewer displays PLY vertex colors, STEP colors, supported SolidWorks saved RGB appearances, and OBJ, glTF, 3MF, and FBX colors and PNG/JPEG textures. It displays other imported meshes with geometric shading. See [SolidWorks appearance coverage](INSTALL.md#open-a-model) for remaining limitations.

When VTX launches with a model, the import worker starts during GPU initialization and waits when it first needs the renderer. On Windows, IGES waits before loading the optional Open CASCADE DLLs because concurrent DLL loading increased startup time in the benchmark.

Texture decoding can also start before the renderer is ready. VTX decodes one image at a time, checks its dimensions against the selected GPU, and uploads it before decoding another. Image decoding retains its 256 MiB allocation limit. OBJ prepares referenced textures before uploading geometry so that the first image can overlap GPU setup.

For an offscreen GPU check, run `vtx --check MODEL [SCREENSHOT.png]`. It reports triangle and point counts, import warnings, and timings in milliseconds. `first_frame_ms` includes GPU initialization through completion of the first frame and excludes pixel readback and PNG export. `ready_ms`, `submit_ms`, and `gpu_done_ms` measure from the end of GPU initialization. `upload_ms` counts CPU preparation and upload calls, including work performed during GPU initialization. `gpu_wait_ms` reports how long the import worker waited for the renderer.

On Windows with the RTX 3080 Ti, debugger stack samples found Direct3D driver work during startup on runs that selected Vulkan. Initializing Vulkan first reduced median GPU setup from about 309 ms to 133 ms. The following medians cover GPU initialization through completion of the first 768 × 512 offscreen frame, with 12 alternating fresh processes per build and a warm file cache.

| Model | Vulkan and Direct3D initialization | Vulkan first |
|---|---:|---:|
| Rungholt OBJ, 6.7 million triangles | 420.3 ms | 230.4 ms |
| GLB grid, 2 million triangles | 328.8 ms | 152.8 ms |
| RGB PLY, 6 million points | 351.8 ms | 179.7 ms |

VTX also copies OBJ index batches directly into the mapped GPU upload buffer, eliminating an 80 MB intermediate array on Rungholt. With Vulkan selected explicitly in both builds, Rungholt's file-to-GPU-completion median after initialization fell from 112.6 ms to 95.6 ms. Peak process commit was about 442 MiB in both builds, and the rendered images matched byte for byte.

The next benchmark used 12 alternating fresh processes per build on the Core i9-12900K and RTX 3080 Ti, with Vulkan and a warm OS file cache. The medians include GPU initialization, import, upload, and completion of the first 768 × 512 offscreen frame.

| Model | Import after GPU setup | Overlapped startup |
|---|---:|---:|
| Raspberry Pi STEP | 276.9 ms | 177.9 ms |
| Textured Rungholt OBJ | 300.4 ms | 238.4 ms |
| RGB PLY, 6 million points | 180.1 ms | 159.5 ms |
| GLB grid, 2 million triangles | 153.8 ms | 143.1 ms |
| Instanced FBX, 5 million triangles | 142.4 ms | 135.7 ms |
| Instanced 3MF, 4 million triangles | 283.3 ms | 156.5 ms |
| Mapped IFC grid | 205.5 ms | 139.8 ms |
| Binary STL sphere, 20 triangles | 133.0 ms | 133.5 ms |
| Mixed IGES | 159.8 ms | 159.3 ms |

Render checks covered all nine benchmark files plus a SolidWorks part and assembly. Triangle, point, and warning counts matched. Ten rendered PNGs matched byte for byte. The Raspberry Pi STEP produced the same set of images in repeated runs of both builds, with two depth-tied pixels varying between runs.

VTX fills GPU buffers of 1 MiB or larger through queue staging. This removes the initial CPU zero-fill that wgpu performs for buffers mapped at creation. In another set of 12 alternating fresh Vulkan processes per build on the same PC, median CPU upload preparation fell from 52.2 to 48.8 ms for textured Rungholt OBJ and from 19.9 to 17.9 ms for the 6-million-point RGB PLY. Median first-frame times fell from 242.5 to 237.0 ms and from 159.6 to 158.5 ms, respectively.

Moving image decoding before the first GPU wait reduced first-frame time on textured models. The following medians use 12 alternating fresh Vulkan processes per build on the Core i9-12900K and RTX 3080 Ti with a warm file cache. Each timing includes GPU initialization through completion of a 768 × 512 frame.

| Model | Previous VTX | Image decoding during GPU setup |
|---|---:|---:|
| Textured Rungholt OBJ | 249.8 ms | 231.6 ms |
| DamagedHelmet GLB | 155.8 ms | 139.5 ms |
| Multiple Textures 3MF | 180.3 ms | 145.0 ms |
| Embedded textures FBX fixture | 134.0 ms | 133.3 ms |
| Textured grid 3MF | 294.5 ms | 292.9 ms |

The OBJ loader also releases CPU position and UV arrays after copying them to GPU staging. Rungholt's peak working set fell from 564.2 to 535.2 MiB, while peak process commit changed from 817.6 to 818.9 MiB. Multiple Textures' peak commit stayed around 272 MiB. Sixty-one render comparisons across Vulkan and Direct3D 12 matched pixel for pixel. Five malformed or oversized image cases preserved their error diagnostics. [Raw texture startup timings](../texture_startup_benchmark.csv)

## Run the browser viewer

Build the native release executable with Rust, Cargo, a C compiler for ufbx, and a C++ compiler for the bundled [meshoptimizer 1.3 codec](https://github.com/zeux/meshoptimizer/tree/v1.3). Then start the local Python 3.10+ server.

```powershell
cargo build --manifest-path native/Cargo.toml --locked --release -p triangulate --features parallel --example compare
python server.py
```

On Windows with the Rust `x86_64-pc-windows-gnullvm` toolchain, install its `rust-mingw` component, set `RUSTFLAGS=-C linker=rust-lld` for the build, and keep the toolchain’s `libunwind.dll` on PATH or next to the executable.

Open <http://127.0.0.1:8765> in a WebGL2 browser. On Windows with Tkinter, **Open from disk** opens one model through the Windows file picker and reads it in place. Use the browser file chooser for glTF or SolidWorks models that need selected companion files.

The server uses the native core when the release executable is present. Native mode uses fixed tessellation settings and displays meshing errors when the core reports them. For Open CASCADE mode, install `cadquery-ocp==7.7.2` and run `python server.py --occt`. That mode supports adjustable deflection. STEP files submitted through the browser chooser can reuse completed meshes by file hash and meshing settings. Direct disk loads read the selected file on each open.

Select an STL file to use the browser-only mesh decoder. STL positions and winding are preserved in file order. The viewer shades the mesh from its geometry.

## STL benchmark

On the Core i9-12900K, both loaders read the same synthetic binary grid with 1,002,001 unique points and 2,000,000 triangles. Across 15 timed runs per loader with a warm OS file cache, VTX took **49.65 ms** to read the file and produce packed triangle positions. [pyvista-stl](https://github.com/pyvista/pyvista-stl) 0.5.3 with automatic threading took **60.66 ms** to read and produce its deduplicated indexed mesh, or **111.06 ms** including NumPy expansion to the same packed layout. VTX decoding alone took **21.99 ms**. The expanded output matched VTX byte for byte, including triangle order and winding. These are parser measurements in Node.js and Python, with browser drawing excluded.

```text
python -m pip install numpy pyvista-stl
python benchmark_stl.py --runs 15
```

## OBJ benchmark

The VTX importer parses independent ranges concurrently, scans face indices eight bytes at a time where possible, and resolves absolute and relative references. A range containing faces of the same size stores that size once. Triangle and quad ranges then go directly to indexed triangles. On Windows the importer maps regular files larger than 4 MiB through a read-only handle that excludes writers. If Windows cannot grant that handle, the importer uses buffered reads.

The standalone OBJ viewer reads `usemtl` assignments, diffuse colors (`Kd`), and opaque PNG and JPEG diffuse maps (`map_Kd`) with UV offset, scale, and clamp. It falls back to material color and reports a warning when a texture cannot be used. It uses geometric shading. Authored normals, opacity, lines, and points are outside the current coverage. The browser's OBJ path imports polygon geometry.

On the Core i9-12900K with the RTX 3080 Ti, the standalone importer loaded the 275.7 MB Rungholt OBJ and MTL through triangulated, material-assigned geometry in a median 82.0 ms. RapidOBJ took 117.6 ms through parse and triangulation on the same files. The VTX first offscreen frame with both texture atlases took 298.2 ms including Vulkan startup, down from 1,179.9 ms in the initial textured implementation. The GPU reads separate OBJ position and UV indices directly, removing CPU vertex remapping. The test used 12 fresh processes per variant with a warm file cache.

On the Core i9-12900K, both release builds read the same [Rungholt OBJ linked by RapidOBJ](https://github.com/guybrush77/rapidobj/blob/master/docs/BENCHMARKS.md). The downloaded OBJ was 275,675,419 bytes with SHA-256 `15ac8bf4f0e35d2bd4802202cf4a9d8d66b5b5873592a2a3d1cc1a8cabc5ba37`.

| Median file read through triangulated mesh | Runs per engine | RapidOBJ | VTX | Speedup |
|---|---:|---:|---:|---:|
| Rungholt, 276 MB | 24 | 196.85 ms | 60.55 ms | 3.25× |
| Positions-only grid, 62 MB | 24 | 51.87 ms | 23.22 ms | 2.23× |
| Sponza | 10 | 9.53 ms | 3.01 ms | 3.17× |

Both builds produced byte-identical ordered triangle coordinates on all three files. The counts were 6,704,264 for Rungholt, 2,880,000 for the grid, and 66,450 for Sponza. Rungholt’s expanded output had SHA-256 `de2126466a1747c8d6fa110bbbbd53d913409ffbc7d99aa8ae9a61f60957b664`. The harness alternates the run order and primes the OS file cache before each process. The timed work ends before export, transport, and rendering. Rungholt’s fresh-process medians were 215.50 ms for RapidOBJ and 76.21 ms for VTX. RapidOBJ uses unbuffered file I/O on Windows and its default loader also reads OBJ normals, UVs, and the material library when present. The grid contains only positions and faces. [Raw Rungholt timings](../obj_benchmark_current.csv)

In nine alternating Windows process probes on Rungholt, the latest parser reduced VTX’s read and decode median from 53.73 to 50.84 ms, and its triangulation median from 8.60 to 6.86 ms. Peak commit fell from 161 to 144 MiB. `STEPFLASH_PROFILE=1` reports those stages and the input mode. Chrome on the RTX 3080 Ti submitted the full indexed Rungholt draw during a viewer smoke test.

For reproduction, build [RapidOBJ at ccca162](https://github.com/guybrush77/rapidobj/tree/ccca1622432a3aeb2fb85ff9085219d008d11afc) with the provided harness. The measured build used Zig C++ 0.17.1 and the native VTX release build described above. Keep `rungholt.mtl` alongside the OBJ for RapidOBJ’s default loader.

```text
zig c++ -std=c++17 -O3 -DNDEBUG -ffp-contract=off -I <rapidobj-checkout>/include benchmark_rapidobj.cpp -o rapid_bench.exe
python benchmark_obj.py rapid_bench.exe native/target/release/examples/compare.exe <rungholt.obj> --runs 24 --warm-cache --csv obj_benchmark_current.csv
```

## PLY meshes and point clouds

VTX opens ASCII, little-endian binary, and big-endian binary PLY 1.0. It displays vertex-only files as points, preserves optional 8-bit RGB vertex colors, and triangulates face lists with the original winding. Gaussian splat files display as point clouds. The importer reads up to 32 million vertices and emits up to 32 million triangles.

| Synthetic binary PLY | Generic VTX reader | Optimized VTX | Tinyply 3.0 |
|---|---:|---:|---:|
| Two-million-triangle grid, 38 MB | 60.47 ms | 21.71 ms | 71.48 ms |
| Six-million-point RGB cloud, 90 MB | 152.06 ms | 16.96 ms | 185.84 ms |
| Six-million-point XYZ cloud, 72 MB | 90.29 ms | 13.34 ms | 121.63 ms |

These are file-to-geometry medians over 12 fresh processes per reader on the Core i9-12900K. Each trial primed the OS file cache. Tinyply used its example memory-stream path and a list-size hint of three for triangle records. On the same files, vertex positions, ordered face indices, and RGB bytes matched the generated reference arrays and Tinyply. The timings exclude geometry export, HTTP, and drawing. [Raw trials](../ply_benchmark.csv)

`STEPFLASH_PROFILE=1` reports file setup, vertex decoding, face decoding, and polygon triangulation. Windows uses a read-only mapping for large files when available. The mapped input faults during decoding, so `file_ms` on that path measures setup only. Large fixed-width XYZ and RGB layouts decode in parallel. The RGB vertex stage fell from 127.17 to 9.40 ms. The generic reader also passed reordered big-endian properties and an ASCII concave polygon area and winding check. [Tinyply revision](https://github.com/ddiakopoulos/tinyply/tree/c9bb690dfe5e9105961e9e28120c48c9ae084bc6)

All 69,451 expanded triangles in the Stanford Bunny fixture matched Tinyply byte for byte. Chrome rendered the point fixture's RGB values and the big-endian mesh. In one hardware-accelerated profile on the RTX 3080 Ti, direct disk opening of the six-million-point RGB cloud submitted its first draw in 111 ms and its full draw in 241 ms. A fixed test selection excluded the file dialog. The browser CPU trace attributed 44 ms to expanding camera bounds over incoming positions.

```text
zig c++ -std=c++17 -O3 -DNDEBUG -I <tinyply-checkout>/source benchmark_tinyply.cpp -o tinyply_bench.exe
tinyply_bench.exe input.ply reference
native/target/release/examples/compare.exe input.ply
```

## glTF and GLB

VTX opens GLB, glTF with embedded base64 buffers, and glTF with separately selected `.bin` buffers. Select the model, buffers, and any PNG or JPEG base-color images together. VTX matches relative references to selected basenames and rejects ambiguous names, parent traversal, and network URLs. It loads the default scene (or the first scene), keeps mesh instances, composes parent transforms, and preserves facing for mirrored instances. It handles static triangle lists, strips, and fans with base-color factors, PNG/JPEG base-color textures, sampler settings, alpha masks, and UV transforms.

`STEPFLASH_PROFILE=1` prints file, document, buffer, and geometry timings for the native CLI. On a 36 MB synthetic GLB with two million triangles, removing the temporary index array reduced median file-to-geometry preparation time from 37.87 ms to 30.81 ms over 12 alternating fresh-process runs per build with a warm OS cache. The packed triangle output matched byte for byte. These timings stop before expanded triangle serialization and browser rendering.

In a separate 12-run comparison of encodings for the same two-million-triangle grid, GLB took 29.52 ms, glTF with a selected binary buffer took 29.77 ms, and base64 glTF took 61.66 ms. The external path borrows buffer slices from the upload, while the base64 path spends additional time parsing the larger JSON and decoding the buffer. All three produced byte-identical packed triangles. The [Khronos BoxInterleaved](https://github.com/KhronosGroup/glTF-Sample-Assets/tree/main/Models/BoxInterleaved) external-buffer sample also matched its GLB counterpart and loaded in the browser.

VTX sends geometry before encoded texture frames and decodes images asynchronously in the browser. Instances sharing a glTF texture share one GPU upload. In one headless Chrome probe on [Khronos DamagedHelmet](https://github.com/KhronosGroup/glTF-Sample-Assets/tree/main/Models/DamagedHelmet), the 936 KB base-color image took 21.5 ms to decode and 9.2 ms for the WebGL texture upload call. This measures the call, before full GPU completion. Across five alternating runs, adding texture and UV frames increased native process-plus-stream median from 6.80 ms to 7.63 ms and payload from 360,096 to 1,412,173 bytes. The 15,452 ordered triangles remained byte-identical. The external PNG test checked rendered pixels and one texture upload across two instances.

### Embedded glTF decoding

Large literal data URIs stay borrowed from the input while the glTF validator reads the rest of the document. Escaped strings use the ordinary JSON path. VTX decodes large base64 payloads in parallel with runtime SIMD detection and checks padding across block boundaries. On Windows, large `.gltf` files can use a read-only mapping. The companion-file path also copies packed accessors directly when their offset in the bundle is unaligned. [base64-simd API](https://docs.rs/base64-simd/0.8.0/base64_simd/)

| Same two-million-triangle model | Prior VTX | Current VTX |
|---|---:|---:|
| Embedded base64 glTF | 50.83 ms | 34.07 ms |
| glTF with a selected binary companion | 29.78 ms | 19.04 ms |
| GLB | 17.59 ms | 17.55 ms |

These file-to-geometry medians use 12 fresh processes per build and a primed OS cache on the Core i9-12900K. The embedded buffer stage fell from 14.24 to 2.51 ms. A separate nine-run Windows probe found that median peak process commit fell from 161.32 to 71.03 MiB and peak resident memory fell from 164.50 to 119.57 MiB. For mapped input, the reported read stage measures mapping setup and page faults are charged to the code that touches the pages. [Raw trials](../gltf_embedded_benchmark.csv)

In a separate 12-run comparison on the embedded file, FastGLTF took 56.00 ms and VTX took 34.24 ms. All two million ordered triangles matched byte for byte. Across ten fixtures, the new and previous VTX builds also produced matching geometry, UV, and texture payloads. These included mixed embedded and external buffers, escaped URIs, and mirrored instances. In eight direct disk trials per build on the RTX 3080 Ti, Chrome submitted the full embedded mesh in a median 130.0 → 104.6 ms, using a fixed picker response.

### Meshopt and packed geometry

The static geometry path reads `EXT_meshopt_compression` and the release-candidate `KHR_meshopt_compression`, including quantized positions and texture coordinates. VTX checks compressed ranges and filter layouts before decoding and caps decompressed views at 512 MiB per import. Aligned packed float positions and 32-bit triangle indices borrow the backing bytes through serialization. Interleaved, sparse, and unaligned data use their conversion paths. On Windows, regular GLBs larger than 16 MiB use a read-only mapping when available, with a buffered fallback. [EXT specification](https://github.com/KhronosGroup/glTF/blob/main/extensions/2.0/Vendor/EXT_meshopt_compression/README.md) · [KHR specification](https://github.com/KhronosGroup/glTF/blob/main/extensions/2.0/Khronos/KHR_meshopt_compression/README.md)

| Two-million-triangle grid | File size | Previous VTX | Current VTX |
|---|---:|---:|---:|
| Uncompressed GLB | 36.02 MB | 17.95 ms | 9.09 ms |
| Lossless EXT meshopt GLB | 7.27 MB | 17.43 ms | 11.09 ms |
| Lossless EXT meshopt glTF with embedded data | 9.69 MB | 21.24 ms | 13.91 ms |
| Quantized KHR meshopt GLB from gltfpack `-cz` | 3.02 MB | 18.56 ms | 10.11 ms |

Each result is the median of 12 alternating fresh processes per build on the Core i9-12900K with a primed OS file cache. Timing stops before mesh serialization and drawing. Windows process counters measured a peak commit median of 77.93 → 43.54 MiB on the EXT GLB. `STEPFLASH_PROFILE=1` reports decode time and borrowed geometry bytes. For mapped input, the read phase measures setup and subsequent stages include page faults. [Raw timings](../gltf_meshopt_benchmark.csv)

The lossless compressed grid produced the same stream payloads as the original GLB. The quantized grid and two compressed DamagedHelmet exports matched independent upstream JavaScript decodes of the exported data. Twelve regression fixtures matched the previous VTX output, including a signed normalized mesh and a textured glTF with a separate binary buffer. Chrome's first draw stayed near 69 ms in the direct-disk grid trials. Full-draw timing for the EXT grid varied across the two trial batches. The Chrome DevTools traces included GPU buffer upload calls and browser stream handling.

In a separate same-file comparison on the uncompressed grid, FastGLTF took 37.85 ms and VTX took 9.12 ms to read and prepare the geometry. Fresh-process medians were 46.01 and 18.11 ms respectively. All two million ordered triangles matched byte for byte.

### Draco geometry

VTX reads `KHR_draco_mesh_compression` in the static glTF and GLB scene path. It uses Draco's decoded point and face order, expands the position and texture coordinate mappings, and applies the ordinary scene transforms and material settings. Each import caps Draco scene geometry at 512 MiB and applies limits inside the codec before allocating decoded data. Select any external binary and base-color image with a Draco glTF file. [Khronos Draco extension](https://github.com/KhronosGroup/glTF/blob/main/extensions/2.0/Khronos/KHR_draco_mesh_compression/README.md) · [draco-core 2.3.3](https://docs.rs/draco-core/2.3.3/draco_core/)

On the Core i9-12900K, a 0.86 MB Draco GLB with two million triangles imported in a median 98.81 ms, down from 105.43 ms in the initial Draco implementation. Each build ran in 16 alternating fresh processes with a primed OS file cache. Attribute expansion and geometry validation fell from 16.09 to 9.05 ms. The codec itself took 89.12 ms, including 30.05 ms in connectivity and 57.47 ms in attribute decoding. Windows counters recorded 129.73 MiB of median peak commit. Set `STEPFLASH_PROFILE=1` for VTX stages and `DECODE_PHASES=1` for the codec breakdown. [Raw timings](../gltf_draco_benchmark.csv)

The synthetic grid and the Khronos BoomBox and Cesium Milk Truck samples matched geometry and appearance payloads from Google's Draco 1.5.7 WebAssembly decoder. BoomBox imported in 3.17 ms, and a Chrome DevTools check verified matching rendered pixels against the uncompressed reference. In that check, decoding its base-color PNG took 35.5 ms and the WebGL texture upload call took 9 ms. [Khronos sample assets](https://github.com/KhronosGroup/glTF-Sample-Assets)

## Indexed transport and drawing

OBJ, glTF, and FBX send positions and triangle indices in bounded binary frames. WebGL uploads those frames to vertex and element buffers and draws each completed indexed mesh with one draw call. The renderer schedules frames for geometry, camera, and viewport changes and stays idle between them. `STEPFLASH_PROFILE=1` also reports indexed payload bytes, mesh count, and serialization time for direct native `--stream` runs.

| Model | Expanded to indexed payload | Completed draw batches | Median native process plus stream |
|---|---:|---:|---:|
| Rungholt OBJ | 241.35 MB → 110.79 MB | 819 → 1 | 217.61 ms → 144.32 ms |
| Two-million-triangle GLB grid | 72.00 MB → 36.02 MB | 245 → 1 | 96.14 ms → 51.19 ms |

Seven alternating runs per build wrote native output to temporary files. Times include process startup, import, and serialization and exclude HTTP and rendering. Expanding the indexed stream reproduced the original triangle bytes, order, and winding for Rungholt, the GLB grid, and a glTF scene with transformed and mirrored instances. The browser was checked separately for indexed and packed draws and idle behavior. [Comparison harness](../benchmark_transport.py)

The native writer now streams slices of decoded geometry directly on little-endian hosts. It preserves the bounded frame sizes across importer chunks and reverses index winding when a transform mirrors a mesh. In 10 alternating fresh-process trials per build, median serialization with output redirected to Windows `NUL` fell from 33.53 to 1.79 ms for Rungholt and from 6.98 to 0.48 ms for the GLB grid. Native process wall time with Python draining the pipe fell from 133.52 to 109.59 ms and from 38.95 to 34.94 ms respectively. Geometry, UV, and texture bytes matched across nine fixtures.

The HTTP handoff now reads each payload into a reusable packet buffer. With the same native worker, the median Rungholt stream after upload fell from 203.14 to 190.41 ms across 12 alternating localhost trials per build. The viewer uploads UV and index bytes directly and refreshes the triangle counter during animation frames and at completion. Chrome on the RTX 3080 Ti recorded 95.94 → 66.22 MB of JavaScript array copies for Rungholt and 29.44 → 13.66 MB for the GLB grid. These counts cover instrumented `Uint8Array.set` calls. In eight alternating browser trials per build, Open-to-full-draw submission medians were 1,007 → 1,005 ms for Rungholt and 190 → 201 ms for the grid. Rungholt spent about 677 ms in the browser’s upload request on the new build.

Fragmented geometry frames now share a reusable browser buffer during their synchronous WebGL upload. Texture frames retain their own storage for asynchronous image decoding. The viewer checks geometry upload errors before texture uploads and at stream completion, allowing progressive uploads to proceed without an immediate GPU round trip after each allocation.

| Direct disk on the RTX 3080 Ti | First draw submitted | Full draw submitted | Frame buffer allocation volume |
|---|---:|---:|---:|
| Two-million-triangle GLB | 67.80 → 51.45 ms | 86.65 → 85.75 ms | 5.51 → 0.20 MB |
| Rungholt OBJ | 156.70 → 157.35 ms | 245.80 → 231.20 ms | 19.37 → 0.20 MB |
| Six-million-point RGB PLY | 67.70 → 51.10 ms | 191.85 → 189.75 ms | 18.02 → 0.20 MB |

Each median uses 16 alternating Chrome trials per build with the same native worker and a fixed picker response. Allocation volume counts distinct JavaScript backing buffers of at least 16 KiB written through `Uint8Array.set` before the full draw. Browser checks read back identical GPU buffer bytes on DamagedHelmet and the RGB cloud. An injected geometry error during texture decoding cleared the partial model, and reopening the file succeeded. [Raw browser trials](../browser_handoff_benchmark.json)

### Camera bounds in indexed streams

The native worker includes exact bounds on each position batch. The browser merges those bounds as batches arrive and sends position bytes directly to WebGL. Older streams and packed meshes use a browser scan with separate XYZ accumulators. The native bounds scan took 6.24 ms for the RGB cloud, 2.57 ms for Rungholt, and 1.02 ms for the GLB grid in nine warm-cache runs per build with output redirected to Windows `NUL`.

| Direct disk load, median of eight Chrome trials per build | Full draw submission before → after | JavaScript array copies before → after |
|---|---:|---:|
| Six-million-point RGB cloud | 241.10 → 195.15 ms | 74.36 → 20.48 MB |
| Rungholt OBJ | 275.05 → 257.40 ms | 38.20 → 19.27 MB |
| Two-million-triangle GLB | 104.00 → 102.05 ms | 16.84 → 10.22 MB |

Trials rotated build order and used a fixed picker response on the RTX 3080 Ti. One Chrome CPU trace attributed 37 ms to the old cloud scan. The new build recorded zero samples in that scan. Array copy counts cover `Uint8Array.set` calls before full draw submission. The final camera center and scale matched on all three timed models. Eight fixtures also matched frame payloads byte for byte, and their declared bounds matched independent float32 extrema.

## Direct disk opening on Windows

The desktop picker returns a one-time load ID to the local viewer. Native importers read the selected file in place, eliminating the browser upload and temporary model copy. STL still decodes in the browser. When selected through the desktop picker, the local server reads the STL and sends its bytes to the decoder.

| Median over nine alternating trials per path | Browser file chooser | Open from disk |
|---|---:|---:|
| Rungholt OBJ, first draw submission | 858.0 ms | 195.7 ms |
| Rungholt OBJ, full draw submission | 975.3 ms | 295.1 ms |
| Two-million-triangle GLB, full draw submission | 158.1 ms | 101.6 ms |

The comparison uses Chrome on the RTX 3080 Ti. It times the load after file choice, with an automated picker response in the timed trials. A separate Windows dialog test opened the sample STEP and displayed all 520 triangles. The uploaded and direct paths produced identical mesh payloads for Rungholt, the GLB grid, and the STEP sample. The direct Rungholt path eliminated the 276 MB upload, which took a median 697 ms in the browser chooser path.

## FBX

The native FBX importer uses ufbx for binary and ASCII files. The standalone viewer displays opaque base colors and base-color textures from embedded PNG/JPEG images or companion images inside the model folder. It uses face material slots and per-node material assignments, and preserves UV seams, named UV sets, and texture transforms. A mesh with the same material assignment shares GPU geometry across placements. Animation playback, deformers, and layered textures are outside this path. [ufbx mesh and material reference](https://ufbx.github.io/elements/meshes/)

Debugger stack samples on Poly Haven's 648 MB Pine Tree FBX repeatedly reached `ufbxi_inflate_block_fast`. VTX now supplies a bounded worker pool through [ufbx's task interface](https://ufbx.github.io/reference). Files smaller than 4 MiB use the serial path. Larger files use up to eight workers, capped by available processors; `VTX_FBX_THREADS=1` selects serial loading. Failed threaded imports retry serially to preserve coverage and useful decoder diagnostics.

The following medians use 12 alternating fresh processes per build on the Core i9-12900K and RTX 3080 Ti, Vulkan, and a warm OS file cache. First frame includes GPU initialization, import, upload, and completion of a 768 × 512 render. [Individual trials](../fbx_parallel_benchmark.csv) · [Verification](../fbx_parallel_checks.json)

| Model and measurement | Previous VTX | Parallel FBX loading |
|---|---:|---:|
| Pine Tree, 17.18M triangles: first frame | 4,519.26 ms | 2,334.04 ms |
| Pine Tree: FBX read and parse | 2,813.34 ms | 1,247.65 ms |
| Pine Tree: peak process commit | 4,003.47 MiB | 4,003.78 MiB |
| Pine Tree: peak working set | 3,306.57 MiB | 3,168.60 MiB |
| Pine Tree: process CPU time | 4,500.00 ms | 4,187.50 ms |
| Instanced FBX, 5M triangles: first frame | 136.94 ms | 136.87 ms |

The Pine Tree first-frame speedup is 1.94×. Sixteen render cases matched byte for byte, including padded ASCII fixtures that exercise the larger-file path. Serial and parallel geometry exports matched all 17,182,252 ordered triangles exactly. Both viewers retain Pine Tree's three texture warnings and white foliage. Parallel parsing uses additional temporary buffers: parser-only peak commit rose from about 2,171 to 2,906 MiB in the worker-count probe, while the full viewer's later allocation peak stayed essentially unchanged.

Reproduce first-frame comparisons with `python benchmark_viewer.py BEFORE_EXE AFTER_EXE MODEL --runs 12 --backend vulkan --warm-cache --verify-images --csv results.csv`. To isolate thread settings within one build, pass the same executable twice and add `--before-fbx-threads 1 --after-fbx-threads 8`. Image verification is an additional untimed pass; the harness also checks triangle, point, and warning counts in every timed process. Full-coordinate export checks are recorded separately in the verification JSON.

On a geometry-only FBX with 40 placements and five million triangles, sharing vertex and index buffers reduced their combined upload from 90.2 MB to 2.3 MB. Median file open through the first completed Vulkan frame after GPU initialization fell from 28.4 ms to 8.2 ms. CPU-side upload work fell from 10.9 ms to 0.34 ms, and peak process commit fell from 316 MiB to 200 MiB. Both builds used the same file and RTX 3080 Ti across 12 alternating fresh processes per build with a warm file cache. The rendered frames and all five million ordered triangles matched byte for byte. On the Blender instancing fixture, float32 GPU placement coordinates differed from the double-precision CPU reference by at most 0.00000024 model units.

The packed VTX output matched a direct C ufbx reference byte for byte across five official fixtures with polygons, instances, and geometry transforms. The eight-instance Suzanne fixture produced 7,744 triangles. Its indexed output used 141,600 payload bytes versus 278,784 bytes as packed triangles. VTX's native file-and-parse probe reported a 0.43 ms median across 12 fresh-process runs on this fixture with a warm OS cache. On the Barbarian fixture, disabling animation and embedded-content loading reduced ufbx's reported temporary allocations from 329,454 to 256,096 bytes with the same triangle output. That compares loader settings within ufbx.

Set `STEPFLASH_PROFILE=1` for native FBX parsing, triangulation, ufbx allocation, and indexed serialization metrics. To reproduce the position comparison, compile [the reference harness](../benchmark_fbx_reference.c) with the `ufbx.c` and `ufbx.h` distributed by the Rust `ufbx` crate version 0.11.5. Run `reference full input.fbx reference.packed` and `compare input.fbx vtx.packed` and compare the two hashes. [Official FBX fixtures](https://github.com/ufbx/ufbx/tree/5955c5c0b042ac2dc6f32955ab206aaec0620cfc/data)

## IFC

The standalone viewer opens IFC2X3, IFC4, and IFC4.3 STEP files and IFC ZIP archives containing one `.ifc` file. It uses the native [IFClite 28.2.0 geometry pipeline](https://crates.io/crates/ifc-lite-processing/28.2.0), licensed under MPL-2.0, to apply placements and subtract openings. It draws the resulting surfaces in their IFC colors. The standalone view uses opaque shading.

VTX skips property and layer metadata during import. It groups flat surfaces by color, writes geometry directly to mapped GPU buffers, and uses GPU instancing for eligible repeated IFC representations. Debugger samples showed substantial idle-worker time on the bridge. VTX uses up to four geometry workers for files up to 2 MiB and eight for larger files. Set `RAYON_NUM_THREADS` to override the limit and `STEPFLASH_PROFILE=1` for file, geometry, and draw counts.

| File-to-draw-plan median | IFClite default | IFClite with VTX worker count | VTX |
|---|---:|---:|---:|
| buildingSMART IFC4.3 bridge | 21.04 ms | 20.66 ms | 12.24 ms |
| Synthetic grid of 12,500 mapped columns | 98.05 ms | 117.84 ms | 67.08 ms |

The reference uses IFClite's native full pass with flat geometry. VTX uses the same tessellation quality and opening settings. Each result is the median of 12 fresh processes on the Core i9-12900K with a warm file cache. On the RTX 3080 Ti the 1.75-million-triangle grid went from 12,500 draw calls to one. The first completed offscreen frame including Vulkan startup fell from 353.6 ms to 203.8 ms versus the initial native viewer. Peak process commit fell from 384 to 232 MiB. Both builds rendered the same pixels on the grid, bridge, and two architecture fixtures. Expanded triangle coordinates matched the flat reference byte for byte on those four inputs, the wall-opening fixture, and the reflected-instance fixture. The georeferenced fixture matched within 0.48 μm.

For IFC in the browser, install `ifcopenshell` into the Python environment that starts VTX. The worker uses the IfcOpenShell geometry iterator to apply world placements and subtract openings. It reads the geometry buffers directly and groups elements into bounded indexed batches.

```text
python -m pip install ifcopenshell
python server.py
```

`STEPFLASH_PROFILE=1` reports dependency startup, file parsing, geometry generation, packing, writing, and peak process memory. The default uses up to eight geometry workers. Start the server with `--ifc-workers 1` for a smaller working set.

On the buildingSMART IFC 4.3 bridge, three fresh-process runs per worker count produced these median import-and-stream times and peak working sets. Time starts after loading dependencies and ends before HTTP and rendering. The OS file cache was warm.

| Workers | Time | Peak working set |
|---|---:|---:|
| 1 | 2,091 ms | 100.7 MiB |
| 4 | 842 ms | 192.8 MiB |
| 8 | 744 ms | 200.3 MiB |
| 12 | 755 ms | 201.9 MiB |

All worker counts produced the same 13,240 oriented triangles after sorting for element completion order. The IFC4 architecture fixture also matched across worker counts. The wall-opening fixture matched direct IfcOpenShell world-space triangulation for the wall and window, and loaded as 44 triangles in the browser. [IfcOpenShell geometry iterator](https://docs.ifcopenshell.org/ifcopenshell/geometry_iterator.html) · [buildingSMART fixtures](https://github.com/buildingSMART/Certification-datasets/tree/80d976a9b193a26a8e928c3e79bff67af1de68a8)

## IGES

The standalone viewer opens `.igs` and `.iges` through a native extension built against the [Open CASCADE Technology SDK](https://github.com/Open-Cascade-SAS/OCCT/releases). Install the SDK for your platform and use the matching C++ toolchain. The Windows build was tested with OCCT 7.9.3.

```sh
cmake -S native/iges_bridge -B native/target/iges_bridge -DCMAKE_PREFIX_PATH="/path/to/occt" -DCMAKE_BUILD_TYPE=Release
cmake --build native/target/iges_bridge --config Release
cmake --install native/target/iges_bridge --config Release --prefix "$PWD/native/target/release/examples"
```

On Windows, put the matching OCCT runtime DLLs and their dependencies beside the VTX executable. On Linux and macOS, the extension uses the SDK library path and can also find shared libraries placed beside it. The extension loads on the first IGES import. Set `VTX_IGES_DEFLECTION` to change the default linear deflection of 0.25. The importer includes free surfaces alongside solids, applies placements and face winding, and sends indexed triangles directly to the GPU. It displays surfaces with geometric shading. Wire-only geometry is outside this path.

`STEPFLASH_PROFILE=1` reports library startup, read, shape transfer, meshing, and indexed extraction. The 38.5 MB Raspberry Pi export and the mixed solid/free-surface fixture matched direct OCCT output from the same SDK byte for byte, including triangle order and winding.

| Median dependency startup through indexed mesh | Python worker | Standalone VTX |
|---|---:|---:|
| Mixed solid and free surface, 14 triangles | 425.35 ms | 29.34 ms |
| Raspberry Pi, 130,285 triangles | 12.68 s | 10.06 s |

The Core i9-12900K trials used the same files, deflection, and OCCT version 7.9.3 with a warm file cache. Each path used its own OCCT binary distribution. The medians cover 12 alternating fresh processes per path for the small file and five for the Pi. They exclude serialization and rendering. Pi mesh extraction fell from 1,184 ms to 5.6 ms. Shape transfer remains the largest stage. On the Pi, median peak working set was 611 MiB for the Python worker and 1,430 MiB for the native SDK path.

For IGES in the browser, install `cadquery-ocp==7.7.2` in the Python environment running VTX. The browser viewer exposes the linear meshing deflection and streams the indexed geometry through its worker.

## 3MF

The standalone reader resolves the package's start-model relationship and streams compressed XML into indexed meshes. It applies model units and nested component placements. Repeated placements share vertex data and use GPU instancing. Mirrored placements draw with reversed winding. The reader displays base-material colors, per-corner color groups, and opaque PNG/JPEG texture groups. It preserves per-corner UVs, tile styles, and nearest or linear filtering. Multiple samplers referencing the same image share a single GPU upload. [3MF Core specification](https://github.com/3MFConsortium/spec_core) · [Materials extension](https://github.com/3MFConsortium/spec_materials)

Production packages can place objects from child model parts through build items and components. VTX follows the root model's relationships to the parts used by the build, and resolves each child's object IDs, colors, and textures within that child. Large sets of child parts decode in parallel from one shared archive. Layered material properties and alternative model representations are outside the current coverage. [Production extension](https://github.com/3MFConsortium/spec_production)

On the Core i9-12900K, an eight-part Production package with four million triangles imported in 178 ms with the default worker pool, 1,139 ms with one worker, and 2,396 ms with lib3mf 2.5.0. The consortium's 17-part Cube Gears Production sample took 3.2 ms with the default worker pool and 36.5 ms with lib3mf. These are medians over 12 interleaved fresh processes per variant with a warm file cache. Both packages produced byte-identical placed triangle coordinates and winding in VTX and lib3mf. In a separate 12-run desktop comparison, GPU initialization through the first completed offscreen frame for the eight-part package fell from 1.39 s with one worker to 0.34 s with the default pool on the RTX 3080 Ti. [3MF Consortium Production samples](https://github.com/3MFConsortium/3mf-samples/tree/master/examples/production)

Debugger stack samples on a 500,000-triangle grid showed time spent in XML attribute iteration and normalization. Parsing numeric attributes directly reduced median file-to-mesh preparation from 191.3 ms to 144.6 ms. On the same file, lib3mf's `ReadFromFile` took 296.3 ms and the browser's Python importer took 918.2 ms. Each median uses 12 alternating fresh processes on the Core i9-12900K with a warm file cache. Timings exclude output serialization and drawing. Ordered triangle coordinates, including winding, matched byte for byte. [lib3mf 2.5.0](https://github.com/3MFConsortium/lib3mf/releases/tag/v2.5.0)

The reader now avoids rescanning attributes for namespace declarations on empty geometry elements that contain no declarations. Plain triangle records with three ordered decimal indices use a checked numeric decoder. Records with material properties, escaped values, or other attribute layouts use the XML attribute iterator.

The next comparison used 12 interleaved fresh processes per importer on the same Core i9-12900K with a warm OS file cache. File-to-mesh timings exclude serialization and GPU rendering.

| 3MF package | Previous VTX | Current VTX | lib3mf 2.5.0 |
|---|---:|---:|---:|
| Grid, 500,000 triangles | 152.7 ms | 90.1 ms | 295.7 ms |
| Textured grid, 500,000 triangles | 309.3 ms | 260.4 ms | 641.4 ms |
| Multiple Textures sample | 67.9 ms | 57.7 ms | 147.1 ms |
| Eight-part Production grid, 4 million triangles | 188.2 ms | 114.8 ms | 2,400.0 ms |
| Cube Gears Production sample | 3.33 ms | 2.81 ms | 36.43 ms |

Including GPU initialization through completion of a 768 × 512 Vulkan frame on the RTX 3080 Ti, the textured grid fell from 334.4 to 292.8 ms and the eight-part Production grid fell from 204.6 to 165.9 ms. Multiple Textures stayed around 181 ms. Peak parser commit was approximately unchanged for these packages. Against the previous build, 36 packages produced byte-identical placed geometry, 18 Vulkan render checks matched pixel for pixel, and 98 XML syntax and numeric cases preserved their results and diagnostics. [Raw 3MF XML timings](../three_mf_xml_benchmark.csv)

On the textured version of the grid, debugger samples identified hashing in the vertex-to-UV remap. A direct lookup for common corners reduced median placement time from 50.4 ms to 11.4 ms and peak parser commit from 62.8 MiB to 54.9 MiB. Optimized import took 309 ms versus 640 ms for lib3mf on the same package, with byte-identical ordered triangles. The first completed Vulkan frame after GPU initialization fell from 374 ms to 329 ms on the RTX 3080 Ti. On the consortium's Multiple Textures sample, VTX imported in 67.8 ms and lib3mf imported in 146.9 ms. Each median uses 12 interleaved fresh runs per variant with a warm file cache. [3MF material samples](https://github.com/3MFConsortium/3mf-samples/tree/master/examples/material)

Use `STEPFLASH_PROFILE=1` for package, XML and geometry, and placement timings. The browser uses a separate Core geometry reader and reports placement, stream, and peak memory metrics. The native and browser readers produced byte-identical ordered triangles on four consortium samples and three generated placement fixtures. One fixture has four million placed triangles with eight placements of the same mesh. After GPU initialization, its first completed offscreen frame fell from a median 193.1 ms to 154.5 ms on the RTX 3080 Ti over 12 alternating Vulkan runs per build. [3MF Consortium samples](https://github.com/3MFConsortium/3mf-samples/tree/master/examples/core)

## SolidWorks

The standalone viewer opens saved display geometry from `.SLDPRT` and `.SLDASM`. Open the assembly with any companion `.SLDPRT` files beside it. VTX uses cached component meshes or matching sibling parts and uploads each reused mesh once. It opens the last saved configuration and labels partial assemblies and saved snapshots. In the browser, select the assembly and its companion parts together. Parts containing only Parasolid B-rep data require a saved display mesh or a STEP export.

The native reader checks expanded sizes and CRCs and welds strip vertices directly into indexed triangles. Debugger stack samples identified DEFLATE as the largest remaining cost after the Rust port. Switching the block inflater to libdeflate reduced the stress file's median read-to-mesh time from 31.45 to 27.06 ms. Set `STEPFLASH_PROFILE=1` for file, directory, block expansion, and geometry timings.

| File | Triangles | sldprt-export | VTX Python | Standalone VTX |
|---|---:|---:|---:|---:|
| Single-cylinder engine piston | 4,578 | 9.11 ms | 4.81 ms | 0.70 ms |
| NIST CTC-02 | 21,232 | 43.51 ms | 24.89 ms | 2.87 ms |
| Piston strips repeated 100 times | 457,800 | 804.71 ms | 325.13 ms | 27.06 ms |

These are file-to-indexed-mesh medians from 12 fresh processes per reader on the same Core i9-12900K with a warm file cache. Timings exclude process startup, export, and rendering. The native reader matched upstream vertex and index arrays byte for byte on all 11 NIST parts, all six engine parts, and the stress file. The assembled engine matched all 12,434 triangles within 0.000016 mm after placement. [NIST CAD test models](https://www.nist.gov/ctl/smart-connected-systems-division/smart-connected-manufacturing-systems-group/mbe-pmi-0) · [Single-cylinder engine model](https://github.com/Suyog1-18/Single_Cylinder_Engine_CAD) · [Reader provenance](../vendor/sldprt/NOTICE.md)

## FastGLTF comparison

An earlier optimization switched tightly packed position and 32-bit triangle accessors to bulk copies after checking their byte ranges. On the 2M-triangle GLB, the geometry phase in one profile dropped from 19.10 ms to 7.05 ms. Median read-to-geometry time across 12 alternating fresh-process trials fell from 30.03 ms to 17.47 ms. The current loader borrows those aligned accessors as described above.

The [FastGLTF harness](../benchmark_fastgltf.cpp) uses FastGLTF's file parser and accessor helpers, builds indexed positions and triangle lists, checks finite positions and index bounds, and records scene transforms. Both readers expand and hash geometry outside the timed interval. The test uses triangle-list fixtures, warm OS file caches, and the same PC. Geometry, triangle order, and winding matched byte for byte on all five inputs.

| Fixture | FastGLTF file to geometry | VTX file to geometry |
|---|---:|---:|
| Two-million-triangle GLB | 37.88 ms | 17.70 ms |
| DamagedHelmet GLB | 3.18 ms | 1.41 ms |
| BoxInterleaved GLB | 0.25 ms | 0.18 ms |
| Two-instance GLB | 0.22 ms | 0.16 ms |
| Two-million-triangle base64 glTF | 56.08 ms | 50.94 ms |

FastGLTF's process wall time was slightly lower on the two smallest fixtures. These are static geometry measurements before serialization, transport, and rendering. The comparator was [FastGLTF at 7ed89f6](https://github.com/spnda/fastgltf/tree/7ed89f6a2e3119bf6721149bfee7ec54658a90b5), built with Zig C++ 0.17.1, `-O3 -DNDEBUG -ffp-contract=off`, simdjson 5.1.0 and simdutf 9.3.3. Compile the harness against that release build, then run `python benchmark_obj.py fastgltf_bench.exe native/target/release/examples/compare.exe model.glb --baseline-name FastGLTF --runs 12 --csv results.csv`. [Raw trials](../gltf_benchmark.csv)

## Progressive HTTP handoff

The server forwards complete mesh frames directly from importer subprocesses. It holds the completion frame until the importer exits successfully. Failed or truncated streams return an error and the viewer clears partial geometry. `STEPFLASH_PROFILE=1` logs time to the first drawable frame, total HTTP mesh stream time, and bytes sent. Those server timings start when it launches the importer.

On the same PC, seven alternating localhost trials per build measured the HTTP response from the stream request, after upload. Both builds delivered identical mesh payload bytes. The comparison uses the same importers and changes only the handoff from an intermediate mesh file to a pipe.

| Model | First drawable batch, staged → pipe | Full response, staged → pipe |
|---|---:|---:|
| Rungholt OBJ | 235.01 ms → 117.70 ms | 384.79 ms → 206.83 ms |
| Two-million-triangle GLB | 133.33 ms → 73.21 ms | 165.59 ms → 121.28 ms |

In a separate test using the same frontend and five alternating trials per build, headless Chrome with software rendering submitted the GLB's first draw in 259.5 ms versus 208.3 ms from the Open click. Submission of the complete mesh fell from 533.6 ms to 262.4 ms. Draw submission includes file upload and precedes screen presentation.

## Browser mesh-frame decoding

Chrome DevTools CPU sampling showed array copies during the receipt of large meshes. The browser now reads headers once and fills each fragmented payload in one buffer. An aligned, contiguous payload can go directly to a typed-array view for WebGL.

With the same server transport and a two-million-triangle GLB, seven alternating headless Chrome loads reduced median JavaScript typed-array copy volume from 72.09 MB to 28.31 MB, counted through `Uint8Array.set` and `Uint8Array.slice`. Median time from clicking Open to submitting the complete draw fell from 303.5 ms to 272.2 ms in that test. The copy count excludes copies inside Chromium and the GPU. The timing includes upload and uses software rendering.

Run `node test_mesh_reader.mjs` to check fragmented frames, unaligned typed-array views, and truncation.

## Original Raspberry Pi comparison with Foxtrot

Both release builds ran on the same Core i9-12900K with Rust 1.99.1 and the parallel features enabled. The input was `Raspberry Pi 3.STEP` from the supplied ZIP (27,270,137 bytes, SHA-256 `ec40e28cc4f1d2d4f8e0c8b13bdcb0d043a68afeb0fb01dc2fbfca0d0ed73703`).

| Median measurement | Upstream Foxtrot | STEPFlash |
|---|---:|---:|
| File read, parse and mesh | 259.70 ms | 209.27 ms |
| Fresh process wall time | 281.26 ms | 231.71 ms |
| Open click to first mesh draw submission | 680.3 ms | 619.1 ms |
| Open click to complete mesh draw submission | 758.6 ms | 695.5 ms |

The CLI measurements used 12 alternating runs per engine, each in a fresh process. The OS file cache was warm. The browser measurements used seven alternating runs per engine through the same STEPFlash viewer in headless Chrome with software rendering. Each load uploaded the original file and launched a fresh core process. Draw submission is measured in the viewer and precedes screen presentation. The browser numbers use Foxtrot’s native core with the common harness and viewer. [CLI timings](../native_benchmark.csv) · [Browser timings](../native_browser_benchmark.json)

The two binary STL exports had the same 327,175 oriented triangles. The geometry check compares the exact float32 position triples, preserving winding and duplicate triangles while allowing triangle order and the starting vertex of each triangle to vary. The canonical SHA-256 was `0eb3cd9e93d514f63f1c75884a30c8098e8dd13a87207581e56bbc1f6eaf8e5b`.

To reproduce the CLI comparison, check out the pinned Foxtrot revision, copy `benchmark_foxtrot_step.rs` into its `triangulate/examples/compare.rs`, and build that example with the same toolchain and release flags. Set `CARGO_PROFILE_RELEASE_DEBUG=false` for both builds to match the recorded run. Then run:

```text
python benchmark_native.py <foxtrot-exe> <stepflash-exe> <input.step> --runs 12 --csv results.csv
```

The harness verifies the triangle geometry before running the timed trials. It writes the read, scan, parse and meshing times for each run. To measure Open CASCADE separately, run `python benchmark.py <input.step> --runs 12 --csv occt.csv`.

## Direct entity indexing

On the same PC and file, the next parser revision reduced median parse time from 94.57 ms to 31.27 ms and median read-to-mesh time from 207.51 ms to 136.39 ms. Median fresh process wall time fell from 229.78 ms to 156.88 ms. Twelve alternating trials per build used fresh processes and a warm OS file cache. The oriented triangle hash matched the original comparison. The Light Version and bundled sample also matched their prior geometry. [Raw timings](../native_profile_benchmark.csv)

In seven alternating runs per build through the same headless Chrome viewer, median click to complete mesh draw submission fell from 652.8 ms to 586.9 ms.

Set `STEPFLASH_PROFILE=1` when running the native executable directly to print declaration splitting, ID indexing, and entity decoding timings to stderr. Clear it for benchmark runs.

## Tessellation profiling pass

The same profile flag also prints worker elapsed times summed across faces for surface setup, contour construction, projection with refinement points, and constrained triangulation. These sums can exceed wall time because faces are processed concurrently. Projection and constrained triangulation were the largest measured stages. The optimized core uses inline scratch storage for common spline degrees and derivative orders, avoids copying points for the triangulation retry loop, and schedules four-face chunks for shells with at least 16 faces.

On the same Core i9-12900K and `Raspberry Pi 3.STEP`, 16 alternating fresh-process runs per engine gave these medians:

| Measurement | Foxtrot | STEPFlash |
|---|---:|---:|
| Mesh stage | 127.58 ms | 75.48 ms |
| File read through mesh | 259.63 ms | 123.07 ms |
| Process wall time | 280.27 ms | 144.57 ms |

In a separate paired run against the prior STEPFlash build, the mesh stage fell from 94.40 ms to 75.27 ms. The exact oriented triangle hash matched upstream Foxtrot and the prior build. Both continued to report 14 counted tessellation errors. These runs used a warm OS file cache and excluded browser display. [Foxtrot comparison](../native_tessellation_foxtrot.csv) · [Prior STEPFlash comparison](../native_tessellation_before.csv)

## Indexed STEP transport

The native STEP viewer carries authored RGB surface colors on the indexed mesh. Face styles take precedence over inherited shell, solid, and representation styles. On the Raspberry Pi 3 sample, the colored stream contains 27 RGB values and the same 327,175 oriented triangles as the previous stream. Geometry and face-style precedence were also checked on a small AP214 fixture.

VTX sends the first 8,192 triangles of a large STEP model as a packed preview before building the vertex remap. It compacts the remaining triangles into batches of up to 32,768 and sends 16-bit indices. Each indexed batch uses at most 65,535 vertices and reserves index 65,535 for WebGL primitive restart. Batches where indexing would save no payload bytes use packed positions. The mesh stream preserves triangle winding and carries the tessellator’s error count.

| Same PC and STEP file | Mesh payload before → after | First draw before → after | Full draw before → after |
|---|---:|---:|---:|
| Raspberry Pi 3 | 11.78 → 6.26 MB | 156.7 → 157.2 ms | 202.3 → 173.4 ms |
| Raspberry Pi 3 Light Version | 9.97 → 5.28 MB | 209.2 → 211.1 ms | 229.0 → 228.0 ms |

Nine browser trials per build rotated order in hardware-accelerated Chrome on the RTX 3080 Ti, using direct disk loads with a fixed picker response. Full-frame draw calls fell from 40 to 11 for Raspberry Pi 3 and from 34 to 10 for the Light Version. Both files kept the exact oriented triangle sets and camera framing. They continued to report 14 and 24 tessellation errors respectively. A separate five-run native probe on Raspberry Pi 3 measured 3.15 ms for remapping and 0.29 ms for bounds with output redirected to Windows `NUL`. [Browser trials](../step_indexed_browser_benchmark.json)

For Raspberry Pi 3, the pinned Foxtrot core feeding the same VTX viewer reached the full draw in 333.2 ms. In a separate 12-run comparison, native file-to-mesh medians were 258.17 ms for Foxtrot and 125.07 ms for VTX. Both produced the same 327,175 oriented triangles. [Core trials](../step_indexed_foxtrot.csv)

Reproduce the geometry and transport check with `python benchmark_transport.py <packed-exe> <indexed-exe> <input.step> --ignore-triangle-order --runs 9`. The option allows the STEP engine's existing variation in solid emission order and retains each triangle's exact float32 coordinates, winding, and multiplicity. `STEPFLASH_PROFILE=1` logs the time spent building local index batches and computing their bounds.

## Public benchmark models

Earlier tests used public Rungholt, Sponza, Stanford Bunny, Khronos, buildingSMART, NIST, ufbx, and 3MF Consortium fixtures alongside generated grids. The following corpus adds larger authored scenes and a mechanical assembly. File sizes describe the model input, with archives and texture companions recorded separately in the manifest.

| Model | Format | Input size | Imported triangles | License | Coverage |
|---|---|---:|---:|---|---|
| [Pine Tree 01](https://polyhaven.com/a/pine_tree_01) | FBX | 648.5 MB | 17,182,252 | CC0-1.0 | New, 3 import warnings |
| [Voron 2.4r2 assembly](https://github.com/VoronDesign/Voron-2/tree/7b32e405f0719881cca782500c822e01e3b16bf2/CAD) | STEP | 241.5 MB | 6,429,086 | GPL-3.0 repository license | New, 666 errors shared with Foxtrot |
| [A Beautiful Game](https://github.com/KhronosGroup/glTF-Sample-Assets/tree/8f492390ddc7a2ae221a1369ef560589cf8c70fb/Models/ABeautifulGame) | GLB | 43.0 MB | 1,499,072 | CC-BY-4.0 | New, geometry comparison passed |
| [San Miguel high](https://casual-effects.com/data/) | OBJ | 1,143.0 MB | 9,980,699 | Conflicting notices, research fixture | New, geometry comparison passed |
| [San Miguel low](https://casual-effects.com/data/) | OBJ | 628.0 MB | 5,617,451 | Conflicting notices, research fixture | New, rendered with zero import warnings |
| [Rungholt](https://casual-effects.com/data/) | OBJ | 275.7 MB | 6,704,264 | CC-BY-3.0 | Previously benchmarked |

The [manifest](../benchmark_models.json) records source URLs, author credits, licenses, input sizes, archive members, and SHA-256 checksums for model and companion files. The downloaded models stay in the selected local directory. The default `.benchmark-models` directory is ignored by Git.

```sh
python fetch_benchmark_models.py
python fetch_benchmark_models.py pine-tree-fbx voron-step a-beautiful-game rungholt
python fetch_benchmark_models.py pine-tree-fbx --directory /path/to/VTX_Benchmark_Models
```

The first command lists available datasets and download sizes. The downloader verifies cached and downloaded files, extracts archives, and checks model hashes. The Pine Tree FBX package includes its 1K texture companions.

Pine Tree's screening render has white foliage and three appearance/import warnings. Voron retains the same tessellation gaps as Foxtrot. A Beautiful Game exercises transmission and volume materials, whose full appearance equivalence remains unverified. The [screening record](../public_models_screening.json) contains single-load observations, including runs made while downloads were active.

San Miguel's archive index lists CC BY 3.0, while the bundled `license.txt` grants research and educational use with attribution. Both notices are recorded in the manifest. It is retained as a research fixture with redistribution rights requiring clarification. The archive is credited to Guillermo M. Leal Llaguno, with improvements by Morgan McGuire and collaborators. [Archive and attribution](https://casual-effects.com/data/)
