VTX 0.2.0 adds compact CAD inspection tools and automatic Windows updates.

Windows: download VTX-Setup.exe for automatic updates. Checks start after the
first view appears, downloads run in the background, and verified packages
install when the last main-app window closes. Use ... > Check for updates to
check manually or restart immediately. This release is private: GitHub CLI and
an existing GitHub login with repository access are required for downloads.
No access token is bundled or saved by VTX. The app does not require Rust,
Python or .NET. Windows builds remain unsigned.

- Pure white background, CAD shading/outlines, supported SolidWorks colors,
  transparency, modeled labels and limited image decal projection.
- SolidWorks-style middle-button rotation, Ctrl+middle pan, wheel zoom and Fit.
- Hover/click recognized lines and whole curves; Ctrl-click a second edge for
  gap, lengths and straight-line angle. Point distance, angle and snapping.
- Diameter/radius on circular rims and recognized cylindrical walls.
- X/Y/Z section planes, standard orientations and orthographic views.
- DXF linework, curves and nested blocks; Open-with registration and a new icon.

SolidWorks values use saved display meshes and are approximate. Exact native
CAD/BREP topology, some appearances/decals and some edges remain unsupported.
DXF circles/arcs preserve source-defined geometry. Unknown source units must
be selected explicitly. Section cuts have no caps.

The main-app installer preserves existing file defaults. It does not install,
replace or register Explorer preview handlers. Existing previews remain
independent. Portable archives and third-party notices are also provided.

[Installation and limitations](https://github.com/vineetpadia/vtx/blob/main/docs/INSTALL.md)
