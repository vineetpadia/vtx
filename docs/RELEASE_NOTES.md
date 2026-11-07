Native VTX desktop preview for Windows, macOS, and Linux.

This update accelerates large FBX imports with a bounded worker pool and preserves serial error diagnostics. The packages include an original colored cube (`demo.glb`) and offline testing instructions (`TESTING.txt`), so you can send the archive directly to a tester without GitHub access.

Download the archive for your computer, extract it, and open **VTX.exe**, **VTX.app**, or **VTX**. No Rust or Python installation is required. On Mac, drag the app into Applications. [Installation help](https://github.com/vineetpadia/vtx/blob/main/docs/INSTALL.md).

| Computer | Download |
|---|---|
| Windows x64 | `vtx-windows-x64.zip` |
| Mac with Apple silicon | `vtx-macos-arm64.zip` |
| Mac with Intel | `vtx-macos-x64.zip` |
| Linux x64 | `vtx-linux-x64.tar.gz` |

Drop a model into the window, or press **Ctrl+O** (**⌘O** on Mac). Drag to orbit, scroll to zoom, **R** to reset.

Supports STL, OBJ, PLY, glTF/GLB, FBX, STEP, 3MF, IFC/IFCZIP, and SolidWorks saved display meshes. IGES needs the optional Open CASCADE extension and is not bundled.

Preview builds are unsigned on Windows and not notarized on Mac; your OS may ask for approval on first launch. Linux needs glibc 2.35+, an X11/Wayland session, and a Vulkan driver. Each archive has a companion SHA-256 file and includes third-party license notices.

Reader coverage is still evolving: some STEP surfaces fail tessellation, some FBX materials need work, and advanced material appearance is incomplete. See [benchmark coverage and verification](https://github.com/vineetpadia/vtx/blob/main/docs/BENCHMARKS.md).
