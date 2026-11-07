"""Repeatable STEPFlash engine benchmark for a local STEP file.

Measures cold *process* starts, not necessarily cold OS page-cache reads.
Browser rendering and upload are not measured.
"""
import argparse
import csv
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys
import time


def run_once(path, deflection):
    code = r'''
import json, sys, time
from engine import load_stream
start=time.perf_counter()
meta={}; meshes=0; parts=0; triangles=0; first=None; completed=None
for e in load_stream(sys.argv[1], float(sys.argv[2])):
    if e['type']=='meta':
        meta=e
        imported=(time.perf_counter()-start)*1000
    elif e['type']=='part':
        parts+=1
        meshes+=e['mesh_ms']
        triangles+=e['triangles']
        if first is None: first=(time.perf_counter()-start)*1000
    elif e['type']=='done':
        completed=(time.perf_counter()-start)*1000
print('BENCH_RESULT='+json.dumps({'read_ms':meta['read_ms'],'transfer_ms':meta['transfer_ms'],
  'import_ms':imported,'mesh_ms_sum':meshes,'first_part_ms':first,
  'total_ms':completed,'parts':parts,'triangles':triangles,'bytes':meta['size_bytes']}))
'''
    t = time.perf_counter()
    proc = subprocess.run([sys.executable, '-c', code, str(path), str(deflection)], cwd=Path(__file__).parent,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    if proc.returncode:
        raise RuntimeError(f'Viewer failed (exit {proc.returncode}):\n{proc.stderr[-3000:]}')
    for line in proc.stdout.splitlines():
        if line.startswith('BENCH_RESULT='):
            result=json.loads(line.partition('=')[2]); result['process_wall_ms']=(time.perf_counter()-t)*1000
            return result
    raise RuntimeError('Benchmark did not emit a result')


def main():
    ap=argparse.ArgumentParser(description='Measure STEPFlash on the original Foxtrot benchmark model')
    ap.add_argument('step_file',type=Path)
    ap.add_argument('--runs', type=int,default=5)
    ap.add_argument('--deflection', type=float,default=.25)
    ap.add_argument('--csv', type=Path,default=Path('benchmark_results.csv'))
    args=ap.parse_args()
    if not args.step_file.is_file(): ap.error(f'File not found: {args.step_file}')
    if args.runs < 1: ap.error('--runs must be >= 1')
    result=[]
    for i in range(args.runs):
        data=run_once(args.step_file.resolve(),args.deflection)
        data['run']=i+1
        result.append(data)
        print(f'Run {i+1}: read={data["read_ms"]:.1f} ms, transfer={data["transfer_ms"]:.1f} ms, '
              f'first part={data["first_part_ms"]:.1f} ms, complete={data["total_ms"]:.1f} ms, '
              f'process wall={data["process_wall_ms"]:.1f} ms, triangles={data["triangles"]}',flush=True)
    fields=['run','bytes','parts','triangles','read_ms','transfer_ms','import_ms','mesh_ms_sum','first_part_ms','total_ms','process_wall_ms']
    with args.csv.open('w',newline='') as fp:
        writer=csv.DictWriter(fp,fieldnames=fields);writer.writeheader();writer.writerows(result)
    print('Median engine time: %.1f ms; median total process wall time: %.1f ms' %
          (statistics.median(x['total_ms'] for x in result), statistics.median(x['process_wall_ms'] for x in result)))
    print('Results:',args.csv)

if __name__=='__main__': main()
