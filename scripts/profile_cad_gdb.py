"""Sample a VTX load by repeatedly interrupting it in GDB (not a timing run).

python3 scripts/profile_cad_gdb.py --binary native/target/release/examples/vtx drawing.dxf
"""
import argparse
import re
import subprocess
import time
from pathlib import Path


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('model')
    parser.add_argument('--binary',default='native/target/release/examples/vtx')
    parser.add_argument('--output',default='dist/cad-performance/gdb-samples.txt')
    parser.add_argument('--samples',type=int,default=16)
    args=parser.parse_args()
    debugger=subprocess.Popen(['gdb','--nx','--quiet','--interpreter=mi2','--args',args.binary,'--check',args.model],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,bufsize=1)
    trace=[];sequence=0
    def command(value):
        nonlocal sequence
        sequence+=1;token=str(sequence)
        debugger.stdin.write(token+value+'\n');debugger.stdin.flush()
        lines=[]
        while True:
            line=debugger.stdout.readline()
            if not line:break
            trace.append(line);lines.append(line)
            if line.startswith(token+'^'):break
        return ''.join(lines)
    try:
        command('-gdb-set pagination off')
        command('-gdb-set mi-async on')
        command('-gdb-set print thread-events off')
        command('-gdb-set debuginfod enabled off')
        command('-exec-run')
        for sample in range(args.samples):
            time.sleep(0.025)
            interrupted=command('-exec-interrupt --all')
            if '*stopped' not in interrupted:
                while True:
                    line=debugger.stdout.readline()
                    if not line:break
                    trace.append(line)
                    if line.startswith('*stopped') or 'thread-group-exited' in line:break
            info=command('-thread-info')
            if 'No registers' in info or not re.search(r'id="\d+"',info):break
            trace.append(f'\nSAMPLE {sample}\n')
            found=False
            for entry in info.split('},{'):
                if 'name="vtx import"' in entry:
                    match=re.search(r'id="(\d+)"',entry)
                    if match:
                        command('-thread-select '+match[1]);frames=command('-stack-list-frames 0 10');trace.append('IMPORT STACK '+frames);found=True
            if not found:trace.append('IMPORT THREAD NOT PRESENT\n')
            command('-thread-select 1');frames=command('-stack-list-frames 0 8');trace.append('MAIN STACK '+frames)
            if '^error' in command('-exec-continue --all'):break
    finally:
        try:command('-gdb-exit')
        except (BrokenPipeError,OSError):pass
        debugger.wait(timeout=10)
    output=Path(args.output);output.parent.mkdir(parents=True,exist_ok=True);output.write_text(''.join(trace))
    print(output)

if __name__=='__main__':main()
