# Units and surface measurements

VTX measurements use 3D positions returned by the visible-surface query pass. They are measurements of tessellated/display geometry at selected pixels, not exact CAD topology or automatic feature recognition. The commands never estimate a distance from a count of image pixels.

## Units and provenance

The inspection manifest (schema version 3) includes `units.label`, `units.source`, `units.status`, `units.meters_per_model_unit`, and supporting `units.evidence`. Coordinates and geometry remain unchanged by unit reporting.

| Input | Rendered coordinate units | Provenance |
|---|---|---|
| STEP/STP/P21 | Context-assigned SI length units or conversion-based units | `step_context`; conversion references are followed, not guessed from names |
| glTF/GLB | Meters, as specified by [glTF 2.0](https://registry.khronos.org/glTF/specs/2.0/glTF-2.0.html#coordinate-system-and-units) | `format_standard`; node transforms have already been applied |
| 3MF | Millimeters, after the existing importer applies source unit and placement scaling | `importer_normalization` |
| DXF/DWG | Drawing `$INSUNITS` when a supported physical scale is available | `drawing_header`; modelspace projection does not change XY scale |
| STL/OBJ/PLY and other unverified importers | Model units | `unspecified`; no inference from filename, size, or common export conventions |

STEP files may contain more than one representation context. VTX only reports a physical scale when all resolved context length scales agree and no assigned length unit remains unresolved. It conservatively considers every assigned context, including unused ones. Conflicting or unresolved contexts yield `status: ambiguous` and a null physical scale. The current STEP geometry importer does not normalize mixed-unit contexts; VTX therefore cannot give reliable physical dimensions for such assemblies. Missing unit metadata yields `status: unknown`.

`--unit mm|cm|m|in|ft` remains a **label override**, not a geometry conversion. It records `source: user_override`, retains detected metadata under `evidence.detected`, and adds a warning note if the override conflicts with the detected scale. Use it only when you know the imported coordinate values use that unit. `--unit model` retains automatic detection.

## Select points and measure

Every `--query view,x,y` gets a stable zero-based index in **command-line order**, independently of export view order. The JSON `queries` array follows that order and records `query_index`. X/Y refer to individual view PNG pixels, excluding footers.

```sh
# Two views can select surfaces on opposite sides of a part.
vtx --inspect part.step --output measured --views front,back \
  --query front,300,300 --query back,300,300 --distance 0,1

# Display lengths in inches while keeping all source coordinates unchanged.
vtx --inspect part.step --output measured-inches --views front,back \
  --query front,300,300 --query back,300,300 --distance 0,1 --measure-unit in

# Included sample STEP: two pixels 80 pixels apart on the planar plate,
# away from the central through-hole.
vtx --inspect sample.step --output sample-measured --views front \
  --query front,220,300 --query front,300,300 --distance 0,1
```

Measurements are returned under `measurements` in both `manifest.json` and stdout JSON. Referenced query points are marked `Q0`, `Q1`, etc. on the PNGs. These crosshairs are annotation overlays; queries still intersect the underlying clean model image. Repeated measurement flags are supported, with a limit of 128 measurements and 1024 queries per inspection.

- `--distance i,j`: 3D Euclidean distance between two selected surface points, including depth differences. This is not shortest surface-path distance or an automatic wall-thickness measurement.
- `--angle i,j,k`: included 3D angle in degrees, with the **middle point j as vertex**. A zero-length leg yields an unavailable result.
- `--diameter i,j,k,...`: planar circle fit through 3–256 distinct selected query indices. Select points distributed around the circular boundary of interest, not arbitrary points on a cylindrical surface. Three noncollinear points construct a circle but cannot validate circularity.

Example diameter invocation (replace the pixels with locations on your circle):

```sh
vtx --inspect drawing.dxf --output fitted --views plan \
  --query plan,400,300 --query plan,450,350 --query plan,400,400 --query plan,350,350 \
  --diameter 0,1,2,3 --measure-unit mm
```

Length results contain `value`, `unit`, `value_model_units`, and `value_meters`. With unknown units, the model-coordinate value is still available and the meter value is null. `--measure-unit mm|cm|m|in|ft` performs an actual **numeric conversion for measurement output only**, using the recorded physical scale. It does not change coordinates, scale bars, or PNG framing. Conversion is refused when units are unknown/ambiguous; supply a verified `--unit` override or omit `--measure-unit`.

## Missing points and rejected fits

A background/through-hole/clipped query produces `status: unavailable` for measurements that reference it; VTX never substitutes a zero position. Other valid measurements are still reported. An unavailable/rejected measurement is a successful inspection with an explicit result status, so callers must check each status.

Diameter fits include center, plane normal, maximum radial residual, maximum planarity residual, and angular coverage. Nonplanar/noncircular fits exceeding **1% of fitted radius**, coverage below 90 degrees, and degenerate/ill-conditioned point sets are rejected without a diameter value. Three-point fits use `status: constructed_circle`; four or more passing points use `status: ok`. A passing fit does not establish that the selected points belong to a hole or that the underlying CAD feature is circular.

Surface sampling, tessellation, source import errors, and float32 coordinate precision affect accuracy. Residuals measure consistency of the selected samples; they are not a certified tolerance or an uncertainty bound. Unit metadata declares physical scale but does not prove the file was authored correctly. Check `units`, importer warnings, and measurement/fit status before using a result for engineering decisions.
