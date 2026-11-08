# Windows Explorer preview

An x64, per-user `IPreviewHandler` for the Preview pane (Alt+P). Double-click file associations stay unchanged. No administrator access or Explorer restart is needed.

The installer also registers **VTX** under **Open with → Choose another app** for supported model formats, including SLDPRT, SLDASM and DXF. It uses the installed `%LOCALAPPDATA%\VTXPreview\VTX.exe`, so the download/workspace location is not needed. Selecting VTX opens the interactive viewer. Existing defaults remain unchanged; Windows lets you choose whether to use VTX once or make it the default. Open-with registrations are covered by the same backup/uninstall as the preview handler. Optional IGES formats are not advertised by this installer. The folded-V icon is embedded in VTX.exe and copied as a content-hashed ICO for Windows' app chooser.

DXF previews use a navigable 2D view: middle-drag (or left-drag) to pan, wheel to zoom and F to fit. Native support covers common line/curve entities and block inserts. Text, dimension rendering, hatch fills and some layer flags remain incomplete. No external CAD app is launched.

The pane shows one view. SolidWorks parts and assemblies decode their embedded, last-saved PNG immediately, before any image-cache work, companion enumeration, or helper startup. The handler reads the last 256 KiB first, then up to the last 4 MiB if needed. CRC, decompressed size, PNG signature, IHDR, and image dimensions are checked before decoding.

After that first image is painted, the handler checks for a cached **1920 × 1440** image. With an image available, it waits 120 ms to avoid starting live viewers for files being scrolled past. One below-normal-priority VTX worker creates a real Windows child window inside the pane and presents its complete model before being revealed. Middle-drag to rotate, Ctrl+middle-drag to pan, Shift+middle-drag to zoom, and Alt+middle-drag to roll. Wheel zoom follows the pointer; F or a double-middle-click fits the current orientation. R also resets the camera. Saved SolidWorks rotation speed, reversed-wheel and zoom-about-center settings are read on worker startup, without launching SolidWorks. Rotation is free through both poles. The viewport renders at its actual size as the pane resizes, on a pure white background. Idle views do not continuously redraw.

The live renderer also captures a high-resolution image using its existing GPU; asynchronous readback and background PNG encoding avoid a second renderer startup. Future selections can show this image while geometry loads. If the worker fails or times out during startup, the existing image stays visible. Changing selection cancels timers and terminates the owned worker. A kill-on-close Windows job also prevents workers surviving a preview-host crash. Image scaling uses a cached WIC Fant filter.

Wheel zoom needs no click or modifier: move the pointer over the live model and scroll. Hover takes input focus only inside an already-active Explorer window, and wheel messages delivered to the preview-handler window are forwarded to its live child. Background Explorer windows are never activated by hovering. `preview_test.exe DLL --wheel FILE...` verifies this with a visible native host and checks the zoom of frames actually presented by the GPU, including in/out, one-time parent forwarding, header exclusion and inactive-window focus protection.

Files without a usable saved PNG use a cached image or load directly into the live view. There is no separate app window or launch button. Click the filename to switch between the saved/cached image and the live model. A generated view uses VTX's fitted camera, shading, supported saved appearances, and mesh geometry; its angle/style can differ from the embedded SolidWorks thumbnail.

Modeled lettering, label backing and logo bodies retain supported body appearances. Files with detected image decals initially keep their saved SolidWorks image: click the filename for navigable 3D, and click again to return to saved appearances. The geometry view can have incomplete decals. These files do not replace the image cache with approximate labels. Without a saved image, available geometry is displayed. The diagnostic renderer supports a narrow projection decoder with embedded/sibling PNG/JPEG, alpha and face clipping; masks, fitting, mirrors and curved mapping remain unsupported.

Build the Rust release example first, then build with Visual Studio 2022 C++ tools and the Windows SDK. The native build links the libdeflate already built by Cargo and runs its bounded-extraction tests:

```powershell
.\build.cmd
Copy-Item ..\target\release\examples\vtx.exe .\build\VTX.exe
.\install.ps1
```

The Rust executable must include `--explorer-preview PARENT_HWND MODEL [CACHE_PNG]`. It validates the parent window class, matches the parent's DPI context, creates a hidden child and reports readiness to the owner. The existing `--preview MODEL OUTPUT.png` remains 1920 × 1440, and `--check` remains 768 × 512. The DLL uses the Microsoft Visual C++ x64 runtime. Installation defaults to `%LOCALAPPDATA%\VTXPreview`; images are cached in `%USERPROFILE%\AppData\LocalLow\VTX\Previews` for the standard low-integrity preview host. Startup has a 30-second timeout; live navigation stays available until selection changes. Source CAD files are read only.

The installer backs up each changed registry value before first registration. DLL filenames include a content hash so updating does not overwrite code loaded by an existing preview host. Restore previous handlers with:

```powershell
& "$env:LOCALAPPDATA\VTXPreview\install.ps1" -Uninstall
```

Restoration skips associations changed by another installation after VTX. Files are retained to avoid unloading code from a live preview host. Supported extensions: SLDPRT, SLDASM, DXF, STEP/STP/P21, STL, OBJ, PLY, GLB/GLTF, FBX, 3MF, IFC/IFCZIP. Fusion `.f3d`, SolidWorks `.slddrw` and optional IGES are not supported by this handler.

Sharp-image cache keys include the handler revision/resolution, source path, size, and modification time, plus relevant companions in the same folder for assemblies and linked assets. Updating VTX.exe through the installer invalidates this revision's rendered-image cache so color/geometry fixes take effect. References outside that folder are not tracked. Saved SolidWorks images show the last saved view and appearances; save in SolidWorks to refresh them. Cache contents can be removed when no preview is loading.

`preview_test.exe DLL FILE...` exercises COM lifecycle, first-image/live/cache readiness, genuine child parenting, absence of app windows, resizing, navigation messages, unload, and repeated selections without registering anything. It prints submission, image-ready and live-ready timings; these exclude Explorer's own selection and hosting overhead. `--dpi` checks a per-monitor-aware host. `--saved` checks decal fallback switching without losing the live child. `--live` waits for navigation without requiring a new cache image.

The HD DLL/class supersedes earlier handlers without restarting Explorer. The installer migrates the original registry backup so restoration covers all versions. The local `research/test_progressive_preview.py` checks first-image/HD timing, cache invalidation, same-window replacement, missing-thumbnail fallback, cancellation before/after launch, retained thumbnails on helper failure, and output dimensions. `preview_test.exe DLL --instant FILE...` stops after the first image; `--retain` verifies a thumbnail stays visible for a second with the helper absent. Original CAD files remain untouched.
