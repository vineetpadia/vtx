"""Native DXF/fallback equivalence and binary-bridge tests (requires ezdxf + VTX + GPU).
Run using CAD Python: python scripts/test_cad_fast.py --binary native/target/release/examples/vtx
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

import ezdxf
from PIL import Image

BINARY='native/target/release/examples/vtx'
class FastDrawingTests(unittest.TestCase):
    def test_native_geometry_matches_fallback_and_preserves_handles(self):
        with tempfile.TemporaryDirectory(prefix='vtx-fast-test-') as temporary:
            root=Path(temporary);doc=ezdxf.new('R2000');doc.units=4
            doc.layers.new('red',dxfattribs={'color':1})
            doc.layers.new('hidden',dxfattribs={'color':-7})
            m=doc.modelspace()
            m.add_line((0,0),(20,0),dxfattribs={'layer':'red'})
            m.add_line((0,10),(20,10),dxfattribs={'color':5})
            m.add_circle((10,5),2)
            m.add_arc((10,5),4,0,90)
            m.add_lwpolyline([(0,0,1),(4,0,0),(4,4,0)],format='xyb',close=True)
            m.add_line((1000,1000),(2000,2000),dxfattribs={'layer':'hidden'})
            path=root/'mixed.dxf';doc.saveas(path)
            pixels=[(128,171),(128,85)]
            def inspect(name,fallback=False):
                env=dict(os.environ)
                if fallback:env['VTX_CAD_FORCE_FALLBACK']='1'
                query=[v for x,y in pixels for v in ('--query',f'plan,{x},{y}')]
                p=subprocess.run([BINARY,'--inspect',str(path),'--output',str(root/name),'--size','257',*query],capture_output=True,text=True,env=env)
                self.assertEqual(p.returncode,0,p.stderr)
                return json.loads(p.stdout)
            fast=inspect('overview')
            ppu=fast['views'][0]['pixels_per_model_unit'];center=fast['bounds']['center']
            image=Image.open(root/'overview'/'plan.png')
            pixels=[]
            for world_y,target in [(0,(255,0,0)),(10,(0,0,255))]:
                x=128.5+(10-center[0])*ppu;y=128.5-(world_y-center[1])*ppu
                choices=[(px,py) for px in range(int(x)-2,int(x)+3) for py in range(int(y)-2,int(y)+3) if image.getpixel((px,py))[:3]==target]
                self.assertTrue(choices)
                pixels.append(min(choices,key=lambda p:(p[0]-x)**2+(p[1]-y)**2))
            fast=inspect('fast');slow=inspect('fallback',True)
            self.assertEqual(fast['source_info']['reader'],'native_ascii_dxf')
            self.assertGreater(fast['source_info']['load_ms'], 0)
            self.assertEqual(slow['source_info']['reader'],'ezdxf_fallback')
            self.assertEqual(fast['source_info']['gpu_batches'],1)
            self.assertEqual(fast['source_info']['entities'],5)
            for a,b in zip(fast['bounds']['size_xyz'],slow['bounds']['size_xyz']):self.assertAlmostEqual(a,b,delta=.02)
            self.assertEqual({p['handle'] for p in fast['parts']},{p['handle'] for p in slow['parts']})
            # Line picking remains per-entity even though all entities share one GPU batch.
            self.assertTrue(fast['queries'][0]['hit']);self.assertTrue(fast['queries'][1]['hit'])
            self.assertNotEqual(fast['queries'][0]['part']['handle'],fast['queries'][1]['part']['handle'])
            self.assertEqual(fast['queries'][0]['part']['layer'],'red')
            for a,b in zip(fast['queries'],slow['queries']):
                self.assertEqual(a['part']['handle'],b['part']['handle'])
                for x,y in zip(a['position'],b['position']):self.assertAlmostEqual(x,y,delta=.001)
            # Unsupported visible content triggers whole-drawing fallback, not an incomplete native preview.
            m.add_text('Full frontend',dxfattribs={'height':1,'insert':(1,4)})
            doc.saveas(path);full=inspect('text')
            self.assertEqual(full['source_info']['reader'],'ezdxf_fallback')
            self.assertTrue(any(p['entity_type']=='TEXT' for p in full['parts']))

    def test_binary_bridge_has_identical_geometry(self):
        with tempfile.TemporaryDirectory(prefix='vtx-wire-test-') as temporary:
            path=Path(temporary)/'text.dxf';doc=ezdxf.new('R2000');doc.modelspace().add_text('VTX',dxfattribs={'height':2});doc.saveas(path)
            bridge=Path(__file__).with_name('cad_bridge.py')
            plain=subprocess.run([os.sys.executable,str(bridge),str(path)],capture_output=True,check=True)
            binary=subprocess.run([os.sys.executable,str(bridge),str(path)],capture_output=True,check=True,env={**os.environ,'VTX_CAD_BINARY':'1'})
            import struct
            self.assertEqual(binary.stdout[:8],b'VTXCAD01')
            length=struct.unpack_from('<Q',binary.stdout,8)[0];header=json.loads(binary.stdout[16:16+length]);offset=16+length
            expected=json.loads(plain.stdout)
            self.assertEqual(len(header['parts']),len(expected['parts']))
            for part,reference in zip(header['parts'],expected['parts']):
                self.assertEqual(part['metadata'],reference['metadata']);self.assertEqual(part['vertex_count'],len(reference['positions']))
                for point in reference['positions']:
                    actual=struct.unpack_from('<fff',binary.stdout,offset);offset+=12
                    for a,b in zip(actual,point):self.assertAlmostEqual(a,b,delta=1e-5)
            self.assertEqual(offset,len(binary.stdout))

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--binary',default=BINARY);args,rest=parser.parse_known_args();BINARY=str(Path(args.binary).resolve());unittest.main(argv=[__file__,*rest])
