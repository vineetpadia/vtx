# Install VTX

[Download the latest release](https://github.com/vineetpadia/vtx/releases/latest). Choose your platform under **Assets**. The repository is private, so sign in with an account that has access. No Rust, Python, browser, or separate CAD application is needed to run VTX.

## Windows

Download **vtx-windows-x64.zip**, choose **Extract all**, then double-click **VTX.exe** in the extracted folder. Keep the extracted files together. You can move that folder anywhere; no installer or administrator access is required.

Requires Windows 10 or newer with a Vulkan or Direct3D 12 capable GPU.

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

Drop a model into the VTX window, or press **Ctrl+O** (**⌘O** on Mac). Drag to orbit, scroll to zoom, and press **R** to reset the camera. Keep OBJ/MTL textures and glTF companion files beside their model.

STL, OBJ, PLY, glTF/GLB, FBX, STEP, 3MF, IFC/IFCZIP, and SolidWorks saved display meshes are included. IGES requires the separately built Open CASCADE extension; see [development setup](DEVELOPMENT.md#optional-iges-extension).
