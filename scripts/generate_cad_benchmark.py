"""Generate linework DXFs for benchmark_cad.py / profile_cad_gdb.py (requires ezdxf)."""
import argparse
from pathlib import Path
import ezdxf


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--counts',type=int,nargs='+',default=[1000,10000])
    parser.add_argument('--columns',type=int,default=100)
    parser.add_argument('--output',type=Path,default=Path('dist/cad-performance'))
    args=parser.parse_args()
    if args.columns<1 or any(count<1 for count in args.counts):parser.error('counts/columns must be positive')
    args.output.mkdir(parents=True,exist_ok=True)
    for count in args.counts:
        doc=ezdxf.new('R2000');doc.units=4;model=doc.modelspace()
        for i in range(count):
            x=i%args.columns;y=i//args.columns
            model.add_line((x,y),(x+.8,y+.7))
        path=args.output/f'lines-{count}.dxf'
        doc.saveas(path)
        print(path)


if __name__=='__main__':main()
