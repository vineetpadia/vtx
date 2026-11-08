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

## Release packages

The [Desktop releases workflow](../.github/workflows/release.yml) builds Windows x64, Linux x64, Mac Apple silicon, and Mac Intel packages. Update `VERSION` and the release notes, then push a matching version tag (for example `v0.2.0`), or run the workflow with a new version. `VTX_VERSION` embeds that exact version into the binary. It publishes archives, checksums, a Windows installer and the `releases.win.json`/full-package update feed after all four builds succeed. Publishing newer code under an existing version is rejected if its tag points to a different commit.

Windows uses Velopack 1.2.161. The pipeline runs Rust tests and `scripts/test_windows_updates.py`, which upgrades an isolated portable test edition, checks its new payload/rollback copy, and rejects a corrupt package. The real user's app registrations are excluded from the test edition. Install .NET SDK 8 and `dotnet tool install vpk --version 1.2.161` for local packaging; end users do not need .NET. Pass `--vpk PATH_TO_VPK` to `scripts/package_release.py` to generate the installer/feed alongside the manual ZIP.

Updates default to public `vineetpadia/vtx` releases, with no GitHub CLI, login, or token required. `VTX_UPDATE_REPO` and `VTX_UPDATE_PRIVATE` are build-time settings for an explicitly configured alternate download repository. Setting `VTX_UPDATE_PRIVATE=true` enables authentication through the user's existing standard Windows GitHub CLI keyring login. Tokens are never compiled into artifacts or passed on command lines.

`vtx --update-smoke LOCAL_FEED [--apply]` is an explicit Windows lifecycle diagnostic; `--update-smoke --github` checks the compiled GitHub source with the same authentication/download code. Tests must use isolated roots and packages. Automatic checks start only after first presentation and apply only on graceful exit of the last instance of that exact main executable. The independent Explorer worker is excluded. Fast installer hooks register Open-with commands and restore only their owned values on uninstall, without writing extension defaults, UserChoice or preview-handler registration.

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
