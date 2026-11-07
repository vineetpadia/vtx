# Public DXF and DWG test corpus

The [download manifest](../cad_benchmark_models.json) records **38 online samples, 14,003,065 bytes**: 18 DXFs and 20 DWGs. Every original has its download URL, byte count, SHA-256, and coverage tags. GitHub samples use pinned commits. Downloads live under `.benchmark-models/cad/` and are excluded from Git.

| Source | Files | Coverage |
|---|---:|---|
| [Autodesk AutoCAD LT samples](https://www.autodesk.com/support/technical/article/caas/tsarticles/ts/01em4r6LLJgnQQVBlk5GqD.html) | 8 DWG | Floor plans, buildings, landscape, civil, mechanical, multileaders |
| [TrattoCAD samples](https://trattocad.com/samples) | 6 DXF, 3 DWG | Apartment, HVAC, flange, site plan; paired DXF/DWG and 3D solids |
| [ezdxf examples and regression data](https://github.com/mozman/ezdxf/tree/c498560bb491c357deda538360ed0c8fdb11f5a7) | 12 DXF, 3 DWG | Maps, terrain, text/fonts, nested blocks, clipping, hatches, binary DXF, malformed input, 3D faces/solids, dimensions |
| [LibreCAD regression data](https://github.com/LibreCAD/LibreCAD/tree/c20884671c5f4566e7494890c5060569cc8d8330/librecad/src/lib/filters/tests/testdata) | 6 DWG | R13/R14/R2000/R2018, layers, visibility, radial dimensions, hatches, MPOLYGON, multiline attributes |

TrattoCAD explicitly permits testing and teaching. Other sources have their own terms; a software repository license does not establish the rights to every contributed sample. The manifest records that distinction. Only the manifest and tooling are committed, not downloaded drawings.

## Download and test

Python 3.11+ downloads and verifies the corpus. Full drawing tests also need the [optional drawing dependencies](DRAWINGS.md) and a working GPU backend.

```sh
# List datasets, or fetch all four. Existing files are checksum-verified.
python3 fetch_benchmark_models.py --manifest cad_benchmark_models.json
python3 fetch_benchmark_models.py --manifest cad_benchmark_models.json --all

# Run every input, continuing after failures; keep JSON reports and plan PNGs.
python3 scripts/test_cad_corpus.py --output dist/cad-corpus

# Repeat a subset in a separate, new output folder.
python3 scripts/test_cad_corpus.py --dataset cad-autodesk --runs 3 \
  --timeout 60 --output dist/cad-corpus-autodesk-repeat
```

The runner verifies every input checksum, uses fresh VTX processes, and saves `results.json` after each input. Each run retains stderr, output artifacts, import warnings, geometry counts, units, reader choice, and load/conversion timing. A timeout stops the process group on POSIX, including converter/bridge children. Missing files, invalid JSON, failures, and timeouts produce a nonzero final exit status without discarding earlier results. Use a new/empty output directory to preserve prior evidence.

Timing is **complete `--inspect --views plan --size 512` export wall time**, including startup, import, GPU initialization, PNG/JSON output, and shutdown. It is not the `--check` first-frame metric in [CAD_PERFORMANCE.md](CAD_PERFORMANCE.md), nor desktop input-to-display latency. OS file and driver caches may be warm; there is no VTX parsed-geometry cache.

## Initial screening: November 7, 2026

VTX `b6d906a`, Linux x86-64, Intel Core i9-9900K, NVIDIA RTX 3070, LibreDWG 0.14, ezdxf 1.4.3. One run per file, 30-second outer timeout, no debugger. This is a coverage screen and bottleneck survey, not a statistically stable benchmark or a comparison against other CAD applications.

**29/38 files exported; 17 of those reported import warnings.** Three used the native DXF reader after DWG conversion; 26 used ezdxf. No timeout occurred. Successful export is not proof of visual completeness: the drawing importer projects modelspace onto XY, substitutes unavailable fonts, and omits paperspace and ACIS solids. Previews still require review; no visual reference comparison has been established for this corpus.

| Sample | Full export | Import | ezdxf parse/draw portion | Observation |
|---|---:|---:|---:|---|
| Autodesk `home_floor_plan.dwg` | 11,083 ms | 10,834 ms | 8,890 ms | Complex architectural target; converter/auditor warnings |
| Autodesk annotation/scaling/multileaders | 10,205 ms | 9,905 ms | 8,314 ms | Annotation/block-heavy target; warnings |
| Autodesk `landscape_plan.dwg` | 4,479 ms | 4,265 ms | 3,446 ms | Dense landscape drawing; warnings |
| Autodesk `building.dwg` | 3,906 ms | 3,705 ms | 2,775 ms | Building geometry; warnings |
| ezdxf `text_fonts.dxf` | 3,274 ms | 3,078 ms | 2,233 ms | Font/text target |
| TrattoCAD apartment DXF | 1,156 ms | 989 ms | 513 ms | Real floor plan; no reported import warnings |
| TrattoCAD HVAC DXF | 791 ms | 621 ms | 197 ms | MEP/layers; no reported import warnings |
| TrattoCAD flange DXF | 637 ms | 465 ms | 86 ms | Mechanical dimensions/hatching |
| ezdxf `807_1.dwg` | 198 ms | 39 ms | — | Native reader; converter warnings |
| LibreCAD layer flags R13 DWG | 168 ms | 5 ms | — | Native reader; converter warnings |

The next optimization target is the full drawing frontend, especially text, blocks, and dense architectural geometry. The two slowest Autodesk inputs spent 8.3–8.9 seconds in bridge parsing/drawing; their DWG conversions took approximately 0.43 and 0.05 seconds respectively. These real samples do not support a universal speed claim based on simple generated linework.

### Failures retained as regression targets

| Input | Observed failure |
|---|---|
| Autodesk `pumping_station.dwg` | Converted DXF rejected: missing ENDSEC. Inspect converter output before attributing this to the original DWG. |
| TrattoCAD 3D pulley DXF and DWG | No supported visible modelspace geometry; ACIS solids are currently unsupported. |
| TrattoCAD 3D house DXF and DWG | Same unsupported solid-model limitation. |
| ezdxf `3dsolids.dxf` | Same unsupported solid-model limitation. |
| ezdxf `recover01.dxf` | Deliberately malformed recovery fixture: ENDSEC without previous SECTION; no recovery reader is currently used. |
| LibreCAD `large_radial.dwg` | No supported visible modelspace geometry; conversion/rendering of this radial dimension needs investigation. |
| LibreCAD `oda_pattern_ac1015.dwg` | No supported visible modelspace geometry; MPOLYGON support/conversion needs investigation. |

Five failures involve solid models, two involve DXF structure/recovery, and two involve converted dimension/MPOLYGON content. All remain in the corpus rather than being excluded to improve the pass rate.
