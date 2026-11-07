"""STEPFlash: prototype STEP-to-progressive-mesh pipeline using Open CASCADE.
This is a correctness-oriented baseline, NOT a demonstrated improvement over Foxtrot.
"""
import json, time, os
from OCP.STEPControl import STEPControl_Reader
from OCP.IFSelect import IFSelect_RetDone
from OCP.BRepMesh import BRepMesh_IncrementalMesh
from OCP.BRep import BRep_Tool
from OCP.TopExp import TopExp_Explorer
from OCP.TopAbs import TopAbs_SOLID, TopAbs_FACE
from OCP.TopLoc import TopLoc_Location
from OCP.TopoDS import TopoDS
from OCP.TopAbs import TopAbs_REVERSED
from OCP.BRepBndLib import BRepBndLib
from OCP.Bnd import Bnd_Box


def solids(shape):
    it = TopExp_Explorer(shape, TopAbs_SOLID)
    out = []
    while it.More():
        out.append(it.Current())
        it.Next()
    return out if out else [shape]


def triangles_for_solid(shape, linear_deflection=0.25, angular_deflection=0.5):
    # Open CASCADE can use multiple threads internally for the meshing operation.
    BRepMesh_IncrementalMesh(shape, linear_deflection, False, angular_deflection, True)
    vertices = []
    faces = 0
    exp = TopExp_Explorer(shape, TopAbs_FACE)
    while exp.More():
        face = TopoDS.Face_s(exp.Current())
        loc = TopLoc_Location()
        tri = BRep_Tool.Triangulation_s(face, loc)
        if tri:
            transform = loc.Transformation()
            nodes = tri.NbNodes()
            # Per-face coordinates, which avoids expensive mesh-wide de-duplication.
            points = [None]
            for idx in range(1, nodes + 1):
                p = tri.Node(idx).Transformed(transform)
                points.append((p.X(), p.Y(), p.Z()))
            reversed_face = face.Orientation() == TopAbs_REVERSED
            for idx in range(1, tri.NbTriangles() + 1):
                t = tri.Triangle(idx)
                a, b, c = t.Get()
                if reversed_face:
                    b, c = c, b
                vertices.extend((*points[a], *points[b], *points[c]))
                faces += 1
        exp.Next()
    return vertices, faces


def load_stream(path, linear_deflection=0.25, angular_deflection=0.5):
    start = time.perf_counter()
    reader = STEPControl_Reader()
    status = reader.ReadFile(str(path))
    if status != IFSelect_RetDone:
        raise ValueError('Open CASCADE could not read this STEP file')
    parsed = time.perf_counter()
    if reader.TransferRoots() == 0:
        raise ValueError('STEP file contains no transferable shapes')
    shape = reader.OneShape()
    transferred = time.perf_counter()
    parts = solids(shape)
    yield {'type': 'meta', 'parts': len(parts), 'read_ms': round((parsed-start)*1000,2),
           'transfer_ms': round((transferred-parsed)*1000,2), 'size_bytes': os.path.getsize(path)}
    total_tri = 0
    for i, part in enumerate(parts):
        t = time.perf_counter()
        vertices, count = triangles_for_solid(part, linear_deflection, angular_deflection)
        total_tri += count
        yield {'type':'part','index':i,'triangles':count,'mesh_ms':round((time.perf_counter()-t)*1000,2),
               'vertices':vertices}
    yield {'type':'done','triangles':total_tri,'total_ms':round((time.perf_counter()-start)*1000,2)}


def make_sample(path):
    from OCP.BRepPrimAPI import BRepPrimAPI_MakeBox, BRepPrimAPI_MakeCylinder
    from OCP.BRepAlgoAPI import BRepAlgoAPI_Fuse, BRepAlgoAPI_Cut
    from OCP.gp import gp_Pnt, gp_Ax2, gp_Dir
    from OCP.STEPControl import STEPControl_Writer, STEPControl_AsIs
    from OCP.IFSelect import IFSelect_RetDone
    block = BRepPrimAPI_MakeBox(75, 45, 12).Shape()
    boss = BRepPrimAPI_MakeCylinder(gp_Ax2(gp_Pnt(37.5, 22.5, 12), gp_Dir(0,0,1)), 12, 16).Shape()
    combined = BRepAlgoAPI_Fuse(block, boss).Shape()
    bore = BRepPrimAPI_MakeCylinder(gp_Ax2(gp_Pnt(37.5,22.5,0),gp_Dir(0,0,1)), 5, 30).Shape()
    final = BRepAlgoAPI_Cut(combined,bore).Shape()
    writer = STEPControl_Writer()
    writer.Transfer(final, STEPControl_AsIs)
    if writer.Write(str(path)) != IFSelect_RetDone:
        raise RuntimeError('Could not write sample STEP')

if __name__ == '__main__':
    import argparse
    p=argparse.ArgumentParser()
    p.add_argument('file')
    p.add_argument('--deflection',type=float,default=.25)
    args=p.parse_args()
    for event in load_stream(args.file,args.deflection):
        if event['type']=='part': event.pop('vertices')
        print(json.dumps(event))
