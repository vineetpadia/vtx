# Install VTX

[Download the latest release](https://github.com/vineetpadia/vtx/releases/latest). Choose your platform under **Assets**. Downloads are public; no GitHub account is needed. No Rust, Python, browser, or separate CAD application is needed to run VTX.

## Windows

For automatic updates, download and run **VTX-Setup.exe**. This per-user installer needs no administrator access and installs the main app under `%LOCALAPPDATA%\VTXDesktop`. It preserves file-default identities and leaves the separate Explorer preview installation alone. The older manual installation is retained as a fallback.

Updates are checked after the first view appears and at six-hour intervals while VTX is open. Downloads run in the background, are hash-verified, and apply when the last window of that main app closes. Open **… → Check for updates** and choose **Check now** for an immediate check; **Restart to update** reopens the current model. A previous executable is retained in the installation's `rollback` folder. Offline or failed checks do not prevent viewing files. The rollback executable can be opened separately for recovery; updates are disabled for that copy. Reinstall a previous release to restore a complete older installation.

From v0.2.1, updates download from the public GitHub release feed without GitHub CLI, a login, or an access token. If you have v0.2.0, run the latest installer once to switch to this public updater; its existing authenticated updater can also install the new version. The updater-enabled portable ZIP also supports updates, but the plain **vtx-windows-x64.zip** is a manual-update archive: extract it and open **VTX.exe**.

Requires Windows 10 or newer with a Vulkan or Direct3D 12 capable GPU.

The local/source main-app build can be installed separately with `native/windows-app/install.ps1`. It installs `%LOCALAPPDATA%\VTX\VTX.exe`, a Start-menu shortcut and the VTX Open-with command. It preserves file defaults and leaves the Explorer preview handler and its worker executable unchanged. Run its installed `install.ps1 -Uninstall` to restore the previous app registrations; application files are retained.

Download the separate [Windows Explorer preview package](https://github.com/vineetpadia/vtx/releases/download/preview-v0.2.1/vtx-explorer-preview-windows-x64.zip), extract it, and double-click **Install.cmd**. [Preview details](../native/windows-preview/README.md). Select a model with the Preview pane enabled (**Alt+P**) to see the saved SolidWorks image immediately, followed by a navigable model in the same pane. Middle-drag to rotate, **Ctrl+middle-drag** to pan, wheel to zoom, and **F** to fit; DXFs use drag-to-pan. A 1920 × 1440 image is cached for future selections, and background work is cancelled when selection changes. Files without a saved image load directly or use the cache. Registration is per user, preserves double-click associations, and includes a restore command. This handler is installed separately and is not bundled with the main-app installer.

Preview builds are unsigned. Windows may show an unknown-publisher prompt. Check that the download came from this repository before choosing **More info → Run anyway**, if your Windows policy allows it.

## macOS

Download **vtx-macos-arm64.zip** for Apple silicon (M-series), or **vtx-macos-x64.zip** for Intel. Unzip, drag **VTX.app** into **Applications**, then open it. Requires macOS 12 or newer.

Preview builds are not notarized. If macOS blocks the first launch, try opening the app once, then use **System Settings → Privacy & Security → Open Anyway**. [Apple's instructions](https://support.apple.com/en-us/102445). VTX does not change Gatekeeper settings.

## Linux

Download **vtx-linux-x64.tar.gz**, extract it, and open the executable **VTX** inside the extracted folder. Requires an x86-64 desktop with glibc 2.35 or newer (such as Ubuntu 22.04+), X11 or Wayland, and a working Vulkan driver. The file chooser uses your desktop's XDG portal.

If your file manager asks how to open it, choose **Run**. You can also launch it from a terminal:

```sh
./VTX
```

## Open a model

Drop a model into the VTX window, or press **Ctrl+O** (**⌘O** on Mac). Middle-drag to rotate, **Ctrl+middle-drag** to pan, **Shift+middle-drag** to zoom, and **F** or double-middle-click to fit the current view. **Alt+middle-drag** rolls the view. Scroll to zoom toward the pointer; **R** resets the camera. Saved SolidWorks mouse rotation speed, reverse-wheel direction and zoom-about-center preferences are read on startup when available, without launching SolidWorks. The normal SolidWorks wheel direction is backward/toward you to zoom in. Keep OBJ/MTL textures and glTF companion files beside their model.

DXF drawings open in a fitted front view. **Drag to pan**, scroll to zoom, and press **R** to fit again. Lines, arcs, circles, polylines (including curved segments), ellipses, supported splines and nested block inserts are displayed as antialiased linework. Off layers and invisible entities stay hidden. Text, dimensions, hatch fills, external references, some layer flags and advanced entity types are incomplete; skipped entities are reported in **I** details.

The Windows app includes the new [folded-V icon](../assets/vtx.svg) in its executable, taskbar/window and Open-with registration. The editable SVG and multi-resolution ICO are in `assets/`; `python scripts/build_icon.py` regenerates the PNG/ICO assets with Pillow.

Press **I**, or use **… → Model info**, for model details, including the names and original local paths of unresolved SolidWorks parts.

In the main app, hover over a recognized edge for an orange preselection highlight, then **left-click** to select it and open the compact Measure card. **Ctrl-click** adds a second edge; Ctrl-click an already selected edge to remove it. Plain-click selects a new edge, and plain-clicking empty space clears the selection. Selected edges use blue and green. One edge shows length (and Ø/R for circles); two edges show their finite minimum gap, individual/total lengths, and the acute angle for two straight lines. Two circles also show center distance. Curved-edge gaps are approximate and use bounded segment searches. Middle-button rotation and Ctrl+middle pan are unchanged; left-drag still pans DXF while Measure is closed.

**Measure** (**M**) also has a selector for **Point distance** (two points and XYZ offsets), **Angle** (three points, with the middle point as vertex), and **Diameter / radius** (circular rim or recognized cylindrical wall). Endpoint, midpoint and circle-center snapping can be toggled for point modes. **Edges** is the default mode. Copy, Clear and per-edge removal stay in the same temporary card; no sidebar is needed. **Esc** clears selections and leaves the tools.

Feature recognition starts on a cancellable background worker after canvas interaction, or when Measure needs it. Opening does not wait for it. A screen-space cache is reused for hover, rebuilt after a view change, and validated with asynchronous GPU visibility queries. Pointer work is throttled and stops while idle or dragging. Recognized smooth noncircular curves remain whole selectable paths rather than individual tessellation segments. Circles are validated for planarity, radial fit and angular support; sparse, distorted or unrecognized geometry may not be selectable. Cylindrical wall recognition requires matching circular rims. Smooth tangent seams and curves split across appearance groups can be missed. SolidWorks and other tessellated formats report **approximate mesh geometry**, not exact native CAD/BREP dimensions. DXF circle/arc entities retain their source-defined geometry through supported uniform block transforms; ellipses remain curves, not circles.

SolidWorks/3MF reader coordinates are normalized to millimeters; glTF coordinates use meters. For unitless/unspecified formats, choose source units explicitly or keep results in model units. Displayed decimal places do not imply CAD manufacturing accuracy.

**Section** (**S**) exposes an X/Y/Z plane, position slider and flip control. It is a visual clipping operation; cut faces are not capped. **Views** contains standard orientations, orthographic projection, edge outlines and Fit. **Esc** closes tools and clears the cut. **… → Model info** shows geometry statistics and import details. These controls appear only in the main window; the installed Explorer build remains separate.

The default view uses a pure white background, CAD shading, antialiased edge outlines, and closer initial framing. **E** toggles outlines, **B** switches between light and dark backgrounds, and **R** resets and fits the view. Edge/background choices carry over when opening another model. Outlines are derived from visible surface direction and depth, so planar tessellation diagonals remain hidden; these are display outlines, not selectable CAD edges. Tiny features and boundaries behind transparent layers may not all receive outlines.

STL, OBJ, PLY, glTF/GLB, FBX, STEP, 3MF, IFC/IFCZIP, and SolidWorks saved display meshes are included. IGES requires the separately built Open CASCADE extension; see [development setup](DEVELOPMENT.md#optional-iges-extension).

SolidWorks parts retain supported saved part, body, and face RGB colors and opacity. Assemblies retain their cached geometry and placements. Persistent surface references transfer matching body/face appearances from companion SLDPRT files; ambiguous matches are left unresolved. Component RGB/opacity overrides are supported for a uniquely identified saved display state and component context. Transparent surfaces use weighted blended rendering so intersecting surfaces do not depend on triangle order; this approximates transparency without refraction.

Modeled labels and logos retain supported imported-body appearances. For image decals, Explorer initially keeps the saved SolidWorks image at its original resolution; click the filename to switch to navigable geometry and back to saved appearances. Decals can be incomplete in 3D, and approximate decals do not replace the saved image cache. The no-thumbnail geometry fallback remains available. Its limited projection decoder uses embedded or sibling PNG/JPEG images; keep linked images beside the part.

Keep the parts beside the assembly; existing original local references are also checked. Network references are not followed. Multiple display-state selection, nested or ambiguous component overrides, some feature appearances, material textures, masked/mirrored/fitted or curved decals, and SolidWorks lighting/edge styling remain incomplete. Changing colors in SolidWorks requires saving the source files before reopening them in VTX.
