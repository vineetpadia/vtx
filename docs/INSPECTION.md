# Inspect models and drawings with an LLM

VTX exports visual evidence without a window, browser, or display server. A working GPU backend (or software Vulkan driver) is required. The desktop viewer and inspection command share importers and materials. DXF/DWG require the optional [drawing dependencies](DRAWINGS.md); other inspection formats do not require Python.

```sh
vtx --inspect part.step --output overview
vtx --inspect part.stl --output part-mm --unit mm
vtx --inspect assembly.glb --output assembly-views --views front,right,top,iso --size 512
vtx --inspect drawing.dxf --output drawing-view
vtx --inspect drawing.dwg --output dwg-view
```

Use a new or empty output directory. Existing inspections are never overwritten. `--size` sets the square model viewport (256–2048 pixels; default 768), excluding annotation footers. Paths with spaces should be quoted. Errors return a nonzero exit status; failed exports can leave partial output, so retry in a new directory.

## Targeted detail views

Frame a world-coordinate bounding box with `--region xmin,ymin,zmin,xmax,ymax,zmax`:

```sh
vtx --inspect part.step --output bore-detail --views front,right,iso \
  --region 15,5,0,60,40,28
```

This changes the camera center and scale, preserving all source geometry. It does not isolate components or remove occluders. Region bounds must be ordered and have a nonzero diagonal. All exported views share the detail scale; the scale bar is recalculated. `MODEL XYZ` in the footer always describes the complete imported model, while `detail_region` and each view's `target_center` describe the requested framing.

## Section cuts

`--section nx,ny,nz,offset` retains the half-space `dot(normal, world_position) <= offset`. VTX normalizes the plane internally. The normal must be nonzero.

```sh
# Retain X <= 40; inspect the internal bore:
vtx --inspect part.step --output section --views front,iso --section 1,0,0,40
# Retain X >= 40:
vtx --inspect part.step --output opposite-section --views iso --section -1,0,0,-40
```

Cuts affect rendered images and pixel queries identically. Orange lines mark intersections on existing mesh surfaces. **Cuts are open: VTX does not generate solid caps or calculate wall thickness.** The manifest records the normalized plane and `caps: false`. Section clipping can be combined with detail framing. A plane can legitimately remove all visible geometry; this yields a blank view and query misses.

## Pixel-to-geometry queries

Repeat `--query view,x,y` to inspect up to 1024 pixels per invocation:

```sh
vtx --inspect part.step --output queried --views front,iso \
  --query front,384,384 --query iso,384,384 --query front,200,300
```

Coordinates are zero-based integer pixels in an individual view PNG: X increases rightward and Y downward. Queries sample pixel centers. Annotation footers and contact-sheet coordinates are not valid query inputs. The named view must be exported by the same command, and X/Y must lie inside its square viewport.

The manifest's `queries` array follows command-line query order and records a stable zero-based `query_index`. It returns:

- `hit: false` for background, through-holes, and geometry removed by the section plane.
- `hit: true` with imported-world `position`, a camera-facing geometric `normal`, one-based `part_id`, and zero-based GPU `instance_index`. Drawing hits also include entity/layer metadata where available.

The GPU query pass shares transforms, depth testing, clipping, and material alpha-cutoff tests with the image pass. Normals are null for lines and points. Normals describe the rendered mesh, not exact CAD surface derivatives. A part ID identifies an import draw group; it is not a CAD face/feature identifier and is not guaranteed stable across model edits or importer changes. GPU instance ordering may differ from source file node ordering. Position precision is the same float32 precision used for display geometry.

## Measurements

Use `--distance i,j`, `--angle i,j,k` (middle point is vertex), and `--diameter i,j,k,...` with zero-based `--query` indices. Results use resolved 3D surface coordinates. `--measure-unit mm|cm|m|in|ft` converts measurement output when physical units are known. See [units and measurement details](MEASUREMENTS.md) for provenance, examples, fit validation, and accuracy limits.

## Outputs and tool integration

- Seven standard views for 3D input: `front`, `back`, `left`, `right`, `top`, `bottom`, `iso`. Drawings default to one `plan` view; `--views` overrides the selection. Plan looks from +Z onto XY with +Y up.
- Each PNG has an orthographic projection, view label, scale bar, projected XYZ triad, complete-model XYZ bounding extents, and import issue count. Detail and cut views are marked in their labels.
- `contact-sheet.png` assembles the selected images into up to three columns. Unused cells are blank.
- `manifest.json` (schema version 3) records geometry bounds/counts, framing, camera bases, units/provenance, sections, queries, and importer diagnostics. `model_viewport` excludes the footer. `image_center_px`, `target_center`, and `pixels_per_model_unit` describe the image-plane mapping.

Standard output is one JSON object identical to `manifest.json`; diagnostics go to standard error. Give the LLM that JSON and the contact sheet. It can request individual images, choose a region, cut away an occluder, and query pixels in a subsequent invocation. Each invocation reimports the model; there is no persistent server or separate agent protocol.

## Coordinates and scale

3D views use **Y up**, front from **+Z**, right from **+X**, and top from **+Y**. Top has -Z toward the image top; bottom has +Z toward the top. Isometric is equally inclined to +X/+Y/+Z. Names describe coordinate views, not an inferred semantic front. Models authored Z-up may need `plan` for their natural top view. Every manifest view provides world vectors for screen right, screen up, and toward the camera.

Framing uses the full-model or detail-region diagonal to fit every orientation. Very slender objects can occupy little of a view. Orthographic scale bars measure lengths in the image plane; angled 3D edges are foreshortened, including in isometric images. The triad omits axes pointing directly toward or away from the camera.

STEP context units, glTF meters, 3MF importer-normalized millimeters, and DXF/DWG header units are detected with explicit provenance. Unverified formats remain in **model units**. Conflicting STEP contexts disable physical conversion. `--unit mm|cm|m|in|ft` overrides the coordinate label only, retaining detected metadata and recording conflicts. It never rescales the geometry. `--measure-unit` converts measurement values, independently of coordinate labels and framing. See [unit rules](MEASUREMENTS.md).

Check import warnings and drawing limitations before reasoning about completeness. Images alone do not establish hidden/internal geometry. Section views help expose interiors but do not reconstruct geometry missing from the importer.
