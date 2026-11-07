# Drawing load performance

The drawing path uses a native ASCII DXF reader for a conservative subset of visible modelspace content: LINE, CIRCLE, ARC, and LWPOLYLINE (including bulges). It preserves layer visibility/colors, original handles, and unit metadata. Curves are tessellated with a maximum sagitta of 0.01 source units before float32 conversion.

Unsupported visible content, nondefault extrusion/thickness, transparency, noncontinuous linetypes, polyline widths, unsupported colors/units, binary DXF, and other unsupported cases use the existing ezdxf frontend for the **entire drawing**. Content is not dropped to obtain a faster preview. A preflight rejects unsupported visible entity types before allocating or tessellating native geometry.

DWG still uses the separate LibreDWG `dwg2dxf` converter. The converted file goes through the native reader when eligible, avoiding Python startup; other converted drawings use ezdxf without converting a second time. Conversion errors/warnings remain visible and the 120-second conversion timeout is retained.

All drawing paths batch adjacent primitive kinds for GPU upload, retaining per-vertex entity IDs and colors. A drawing containing 100,000 line entities can use one GPU batch while queries still return the original handles and layers. Batches are bounded to about one million vertices; an individual entity larger than that still uses its original allocation limits. Original ordering is retained for coplanar drawing content. `source_info.gpu_batches`, `source_info.entities` (import entity groups), `source_info.reader`, and `source_info.load_ms` expose the selected path and load cost.

The Python fallback transports positions as little-endian float32 arrays with a bounded JSON metadata header instead of encoding every coordinate as JSON text. Entity metadata uses typed records; repeated native entity type/layer names are shared, and root block paths are synthesized only when serialized. No persistent parsed-geometry cache is used.

## Debugger-driven optimization

The investigation repeatedly interrupted a release build loading a generated 100,000-line drawing in GDB, then inspected the import and main-thread stacks. Samples exposed `malloc`/boxed allocations in the native reader and standard-library floating-point parsing. Replacing per-entity JSON maps with compact typed metadata and using the existing fast-float decoder reduced observed native import time from roughly 214 ms to 105 ms on that workload. These two observations are diagnostic timings, not a broad benchmark claim.

Later samples predominantly caught the import thread waiting in `UploadTarget::get` while the main thread initialized the Vulkan backend. This distinguishes remaining driver/GPU startup latency from geometry loading. Debugger samples identify candidates; all quoted benchmark times below were taken **without a debugger**.

Generate test drawings with the CAD Python environment (`scripts/generate_cad_benchmark.py --counts 1000 10000`; use `--counts 100000 --columns 1000` for the larger sampling workload). Reproduce the stack sampling on Linux with GDB installed:

```sh
python3 scripts/profile_cad_gdb.py large.dxf --samples 16 \
  --binary native/target/release/examples/vtx --output dist/cad-performance/gdb.txt
```

## Benchmark method

Use `scripts/benchmark_cad.py` to alternate old/new binaries, run `--check` to completion, and save every measurement as JSON:

```sh
python3 scripts/benchmark_cad.py --before /path/to/old-vtx \
  --after native/target/release/examples/vtx --runs 5 drawing.dxf drawing.dwg
```

Wall time includes fresh process startup, import, an offscreen GPU frame/readback, and shutdown. `first_frame_ms` separately records time through GPU frame completion. The operating system's file cache and driver caches are warm. There is no VTX parsed-geometry cache. This does not measure desktop input-to-display latency and does not compare against other CAD applications.

The final comparison on November 7, 2026 used Linux x86-64, an Intel Core i9-9900K, an NVIDIA RTX 3070, seven alternating runs, the previous `4b43b4e` implementation, and the optimized working build:

| Workload | Previous wall median | Optimized wall median | Speedup |
|---|---:|---:|---:|
| Generated DXF, 1,000 LINE entities | 477.3 ms | 158.5 ms | 3.01× |
| Generated DXF, 10,000 LINE entities | 1,168.4 ms | 158.4 ms | 7.38× |
| Small DXF with text (ezdxf fallback) | 391.3 ms | 380.7 ms | 1.03× |
| LibreDWG 2018 `Line.dwg` fixture | 388.4 ms | 157.3 ms | 2.47× |

The 10,000-line native reader itself took about 9 ms; fresh GPU/process startup dominates the remaining total. The existing desktop viewer retains its GPU device across subsequent file opens. Text/hatch/block-heavy drawings can still be limited by Python startup and frontend work. Large DWG files may be limited by LibreDWG conversion. These results establish improvements over VTX's previous implementation, not a claim to be faster than every other program.

## Correctness checks

Native/ezdxf comparison tests cover colors, hidden layers, bounds, curved polylines, entity handles, batched pixel picking, and whole-drawing fallback for text. Binary transport tests compare decoded positions/metadata against the JSON bridge and reject truncated/trailing data. Existing model rendering, geometry queries, units, measurements, DXF/DWG bridge, and GPU tests remain applicable.

```sh
~/.local/share/vtx/cad-venv/bin/python scripts/test_cad_fast.py \
  --binary native/target/release/examples/vtx
```

`VTX_CAD_FORCE_FALLBACK=1` disables the native path for diagnosis and comparison; it does not disable GPU batching or binary transport.
