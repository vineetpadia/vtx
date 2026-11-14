VTX 0.2.2 brings body-colored section faces and clearance-based assembly
explosion to Windows, macOS, and Linux.

Windows: download VTX-Setup.exe for automatic updates. Checks start after the
first view appears, downloads run in the background, and verified packages
install when the last main-app window closes. Use ... > Check for updates to
check manually or restart immediately. No GitHub account, GitHub CLI, login,
or access token is required. Existing v0.2.0 users can run this installer once
to switch to the public updater, or update using their existing authenticated
installation. The app does not require Rust,
Python or .NET. Windows builds remain unsigned.

- Pure white background, CAD shading/outlines, supported SolidWorks colors,
  transparency, modeled labels and limited image decal projection.
- SolidWorks-style middle-button rotation, Ctrl+middle pan, wheel zoom and Fit.
- Hover/click recognized lines and whole curves; Ctrl-click a second edge for
  gap, lengths and straight-line angle. Point distance, angle and snapping.
- Diameter/radius on circular rims and recognized cylindrical walls.
- Hatched X/Y/Z sections with body colors, flip controls, and a view straight
  into the cut. Closed meshes retain bores and cavities.
- Explode slider with a fixed reference body, estimated assembly directions,
  and an Assemble button.
- Anchored annotation notes with saved camera, section, and exploded views.
- DXF linework, curves and nested blocks; Open-with registration and a new icon.

SolidWorks values use saved display meshes and are approximate. Exact native
CAD/BREP topology, some appearances/decals and some edges remain unsupported.
DXF circles/arcs preserve source-defined geometry. Unknown source units must
be selected explicitly. Open meshes can produce incomplete section faces.

The main-app installer preserves existing file defaults. It does not install,
replace or register Explorer preview handlers. Existing previews remain
independent. Portable archives and third-party notices are also provided.

[Installation and limitations](https://github.com/vineetpadia/vtx/blob/main/docs/INSTALL.md)
