# Build and develop VTX

For a ready-to-run app, use the [downloads](https://github.com/vineetpadia/vtx/releases/latest).

## Desktop app

Install stable Rust and the platform's C/C++ toolchain. On Windows, use Visual Studio Build Tools with **Desktop development with C++** and the MSVC Rust toolchain. On Mac, install the Xcode Command Line Tools. On Ubuntu, install `build-essential`, `pkg-config`, `libxkbcommon-dev`, `libwayland-dev`, and `libvulkan-dev`.

```sh
cargo build --manifest-path native/Cargo.toml --locked --release -p triangulate --features parallel,desktop --example vtx
./native/target/release/examples/vtx /path/to/model.step
```

Windows executable: `native\target\release\examples\vtx.exe`.

VTX uses Metal on Mac, Vulkan on Linux, and Vulkan with a Direct3D 12 fallback on Windows. `WGPU_BACKEND=dx12` selects Direct3D 12 on Windows. For GPU checks and diagnostic timings, use `vtx --check MODEL [SCREENSHOT.png]`.

For automated visual analysis, use `vtx --inspect MODEL --output DIR`. See [inspection exports](INSPECTION.md) for detail framing, sections, and pixel queries.

For optional DXF/DWG 2D drawing previews, see [drawing support](DRAWINGS.md). The Python bridge is embedded in the binary; ezdxf/Pillow and the separate LibreDWG converter are installed independently.

## Release packages

The [Desktop releases workflow](../.github/workflows/release.yml) builds Windows x64, Linux x64, Mac Apple silicon, and Mac Intel packages. Push a version tag, such as `v0.1.1`, or run the workflow with a new version. It publishes archives and SHA-256 files to GitHub Releases after all four builds succeed.

Windows builds include the C runtime statically. Mac builds are app bundles with an ad hoc signature; distribution signing and notarization require an Apple Developer identity. Linux builds use Ubuntu 22.04 as their base. Packages include third-party notices and dependency license files. The release workflow's packaging helper is `scripts/package_release.py`.

## Optional IGES extension

Install the Open CASCADE SDK and build the bridge with the matching C++ toolchain:

```sh
cmake -S native/iges_bridge -B native/target/iges_bridge -DCMAKE_PREFIX_PATH=/path/to/occt -DCMAKE_BUILD_TYPE=Release
cmake --build native/target/iges_bridge --config Release
cmake --install native/target/iges_bridge --config Release --prefix "$PWD/native/target/release/examples"
```

Place the SDK's runtime libraries beside the executable or on the platform's library path. Windows was tested with OCCT 7.9.3. `VTX_IGES_DEFLECTION` changes the default linear deflection of 0.25.

## Browser viewer

The optional browser viewer uses a local Python 3.10+ server and WebGL2:

```sh
cargo build --manifest-path native/Cargo.toml --locked --release -p triangulate --features parallel --example compare
python server.py
```

Open <http://127.0.0.1:8765>. For Open CASCADE mode, install `cadquery-ocp==7.7.2` and run `python server.py --occt`.

## Benchmarks and model corpus

See the [full measurements and geometry checks](BENCHMARKS.md), [public model corpus](BENCHMARKS.md#public-benchmark-models), and [download manifest](../benchmark_models.json). Model downloads require Python 3.11+:

```sh
python fetch_benchmark_models.py
python fetch_benchmark_models.py pine-tree-fbx voron-step a-beautiful-game rungholt
```

On Windows with the `x86_64-pc-windows-gnullvm` Rust toolchain, install its `rust-mingw` component, use `RUSTFLAGS=-C linker=rust-lld`, and keep that toolchain's `libunwind.dll` beside the executable or on PATH. Official Windows packages use MSVC.

## Inspection regression tests

The example test suite includes camera convention, scale-bar, and CLI validation tests. One explicitly ignored GPU integration test exports a translated 40 × 20 × 10 box at an odd pixel width, checks rendered dimensions against the scale, checks the manifest/contact sheet, and verifies overwrite refusal. It also verifies visible world positions, camera-facing normals from opposite sides, background misses, detail framing, and consistent section clipping between images and queries.

```sh
cargo test --manifest-path native/Cargo.toml --locked --release -p triangulate --features parallel,desktop --example vtx
# Include the GPU test on a machine with an available adapter:
cargo test --manifest-path native/Cargo.toml --locked --release -p triangulate --features parallel,desktop --example vtx -- --include-ignored
```
