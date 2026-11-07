"""Compare complete process-to-first-offscreen-frame CAD loads; no VTX geometry cache.

python scripts/benchmark_cad.py --before OLD_VTX --after NEW_VTX model.dxf model.dwg
Runs alternate executables to reduce timing-order bias. Source files are never modified.
"""
import argparse
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import time


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('models',nargs='+',type=Path)
    parser.add_argument('--before',required=True,type=Path)
    parser.add_argument('--after',required=True,type=Path)
    parser.add_argument('--runs',type=int,default=5)
    parser.add_argument('--output',type=Path,default=Path('dist/cad-performance/comparison.json'))
    args=parser.parse_args()
    if args.runs<1:parser.error('--runs must be positive')
    records=[]
    for model in args.models:
        results={'before':[],'after':[]}
        for index in range(args.runs):
            for name in (['before','after'] if index%2==0 else ['after','before']):
                binary=getattr(args,name).resolve()
                start=time.perf_counter_ns()
                process=subprocess.run([str(binary),'--check',str(model.resolve())],capture_output=True,text=True,timeout=180)
                elapsed=(time.perf_counter_ns()-start)/1e6
                if process.returncode:
                    raise RuntimeError(f'{name} failed on {model}: {process.stderr}')
                metrics={}
                for token in process.stdout.split():
                    if '=' in token:
                        key,value=token.split('=',1)
                        try:metrics[key]=float(value)
                        except ValueError:metrics[key]=value
                results[name].append({'wall_ms':elapsed,'metrics':metrics,'stderr':process.stderr})
        before=statistics.median(r['wall_ms'] for r in results['before'])
        after=statistics.median(r['wall_ms'] for r in results['after'])
        record={'model':str(model.resolve()),'bytes':model.stat().st_size,'before_median_wall_ms':before,'after_median_wall_ms':after,'speedup':before/after,'runs':results}
        records.append(record)
        print(f'{model.name}: {before:.1f} -> {after:.1f} ms ({before/after:.2f}x)',flush=True)
    report={'system':platform.platform(),'cpu_count':os.cpu_count(),'method':'Median fresh-process wall time through --check first offscreen GPU frame and shutdown; OS file/driver caches are warm. No VTX parsed-geometry cache. Not a desktop input-to-display measurement or a comparison against other CAD applications.','results':records}
    args.output.parent.mkdir(parents=True,exist_ok=True);args.output.write_text(json.dumps(report,indent=2))

if __name__=='__main__':main()
