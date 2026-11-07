"""Local VTX viewer server and native model importer."""
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler
from dataclasses import dataclass
from pathlib import Path
import json, tempfile, uuid, os, hashlib, struct, subprocess, sys
from importlib.util import find_spec
from array import array
from stat import S_ISREG
from threading import Lock
from time import perf_counter
from urllib.parse import urlparse, parse_qs

ROOT=Path(__file__).resolve().parent
NATIVE_EXE = ROOT / 'native' / 'target' / 'release' / 'examples' / (
    'compare.exe' if os.name == 'nt' else 'compare'
)
JOBS={}
MAX_UPLOAD=250 * 1024 * 1024
MAX_NATIVE_UPLOAD = 512 * 1024 * 1024
CACHE=Path(tempfile.gettempdir())/'stepflash-mesh-cache-v1'
CACHE.mkdir(exist_ok=True)
SOLIDWORKS_FORMATS = ('sldprt', 'sldasm', 'sldbundle')
UPLOAD_FORMATS = ('step', 'iges', '3mf', 'obj', 'ply', 'gltf', 'glb', 'gltfpack', 'fbx', 'ifc', *SOLIDWORKS_FORMATS)
DESKTOP_FORMATS = {
    ext: ext for ext in ('step', 'iges', '3mf', 'obj', 'ply', 'gltf', 'glb', 'fbx', 'ifc', 'sldprt', 'sldasm', 'stl')
}
DESKTOP_FORMATS.update(stp='step', p21='step', igs='iges')
PICKER_LOCK = Lock()


@dataclass(frozen=True)
class Job:
    path: Path
    model_format: str
    temporary: bool
    digest: str | None = None


def pick_file():
    command = [sys.executable, ROOT / 'desktop_picker.py', *(f'*.{ext}' for ext in DESKTOP_FORMATS)]
    dialog = subprocess.run(command, capture_output=True, text=True, encoding='utf-8',
                            creationflags=subprocess.CREATE_NO_WINDOW, timeout=300, check=True)
    return json.loads(dialog.stdout)

def frame(event):
    """Length-prefixed JSON metadata followed by optional float32 positions."""
    event = event.copy()
    vertices = event.pop('vertices', None)
    payload = b''
    if vertices is not None:
        values = array('f', vertices)
        if sys.byteorder != 'little': values.byteswap()
        payload = values.tobytes()
    event['payload_bytes'] = len(payload)
    header = json.dumps(event, separators=(',', ':')).encode('utf-8')
    return struct.pack('<I', len(header)) + header + payload

class Handler(BaseHTTPRequestHandler):
    def local_host(self):
        port = self.server.server_port
        host = self.headers.get('Host', '').lower()
        suffix = f':{port}' if port != 80 else ''
        return host if host in (f'127.0.0.1{suffix}', f'localhost{suffix}') else None

    def json_response(self, data, status=200):
        body = json.dumps(data).encode('utf-8')
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Cache-Control', 'no-store')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def format_error(self, model_format):
        if model_format == 'ifc' and not self.server.ifc_available:
            return 'Install ifcopenshell to load IFC'
        if model_format == 'iges' and not self.server.occt_available:
            return 'Install cadquery-ocp to load IGES'
        if model_format not in ('step', 'stl', 'iges', '3mf', 'ifc', *SOLIDWORKS_FORMATS) and self.server.native_engine is None:
            return 'This format requires the native VTX importer'
        return None

    def choose_file(self):
        host = self.local_host()
        if host is None or self.headers.get('Origin') != f'http://{host}':
            self.send_error(403, 'Open the picker from the local VTX page')
            return
        if not self.server.desktop_picker:
            self.json_response({'error': 'Native file selection requires Windows with Tkinter'}, 501)
            return
        if not PICKER_LOCK.acquire(blocking=False):
            self.json_response({'error': 'The native file picker is already open'}, 409)
            return
        try:
            picked = pick_file()
        except (subprocess.SubprocessError, OSError, ValueError):
            self.json_response({'error': 'Unable to open the native file picker'}, 500)
            return
        finally:
            PICKER_LOCK.release()
        if not picked:
            self.send_response(204)
            self.end_headers()
            return
        path = Path(picked)
        model_format = DESKTOP_FORMATS.get(path.suffix.lower()[1:])
        if model_format is None:
            self.json_response({'error': 'Unsupported file extension'}, 415)
            return
        if error := self.format_error(model_format):
            self.json_response({'error': error}, 415)
            return
        try:
            info = path.stat()
            if not S_ISREG(info.st_mode) or not info.st_size:
                self.json_response({'error': 'Select a nonempty model file'}, 400)
                return
            if model_format == 'stl' and info.st_size > MAX_UPLOAD:
                self.json_response({'error': 'STL file exceeds 250 MiB'}, 413)
                return
        except OSError:
            self.json_response({'error': 'Unable to access the selected model'}, 400)
            return
        key = uuid.uuid4().hex
        JOBS[key] = Job(path, model_format, temporary=False)
        try:
            self.json_response({'id': key, 'name': path.name, 'format': model_format})
        except (BrokenPipeError, ConnectionResetError):
            JOBS.pop(key, None)

    def do_GET(self):
        url=urlparse(self.path)
        if url.path == '/sample.step':
            data=(ROOT/'sample.step').read_bytes()
            self.send_response(200);self.send_header('Content-Type','application/octet-stream');self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
        elif url.path in ('/stl.mjs', '/mesh_reader.mjs', '/texture_bank.mjs'):
            data = (ROOT / url.path[1:]).read_bytes()
            self.send_response(200)
            self.send_header('Content-Type', 'text/javascript; charset=utf-8')
            self.send_header('Content-Length', str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        elif url.path in ('/','/index.html'):
            data=(ROOT/'index.html').read_bytes()
            attrs = b' data-engine="native"' if self.server.native_engine else b''
            if self.server.ifc_available:
                attrs += b' data-ifc="enabled"'
            if self.server.occt_available:
                attrs += b' data-occt="enabled"'
            if self.server.desktop_picker:
                attrs += b' data-desktop="enabled"'
            data = data.replace(b'<body>', b'<body' + attrs + b'>', 1)
            self.send_response(200)
            self.send_header('Content-Type', 'text/html; charset=utf-8')
            self.send_header('Content-Length', str(len(data)))
            self.send_header('X-Frame-Options', 'DENY')
            self.end_headers()
            self.wfile.write(data)
        elif url.path.startswith('/api/stream/'):
            if self.local_host() is None or self.headers.get('Sec-Fetch-Site') not in (None, 'same-origin'):
                self.send_error(403, 'Open the model from the local VTX page')
                return
            key=url.path.split('/')[-1]
            job=JOBS.pop(key,None)
            if job is None: self.send_error(404,'Unknown or consumed model');return
            path = job.path
            try:
                if job.model_format == 'stl':
                    self.stream_stl(path)
                    return
                q=parse_qs(url.query)
                d=float(q.get('deflection',['0.25'])[0]);d=max(.005,min(10.0,d))
                native = self.server.native_engine
                extension = job.model_format
                ifc = extension == 'ifc'
                iges = extension == 'iges'
                three_mf = extension == '3mf'
                solidworks = extension in SOLIDWORKS_FORMATS
                cached = None
                if job.digest is not None:
                    cache_key = hashlib.sha256(f'{job.digest}:{d}:0.5:{self.server.ocp_version}'.encode()).hexdigest()
                    cached = CACHE / (cache_key + '.bin')
                hit = cached is not None and cached.is_file()
                self.send_response(200)
                self.send_header('Content-Type','application/octet-stream')
                self.send_header('Cache-Control','no-store')
                self.send_header('X-STEPFlash-Cache', 'hit' if hit else 'miss')
                engine = 'ifc' if ifc else 'iges' if iges else '3mf' if three_mf else 'solidworks' if solidworks else 'native' if native else 'occt'
                self.send_header('X-STEPFlash-Engine', engine)
                self.end_headers()
                try:
                    if ifc:
                        command = [sys.executable, ROOT / 'ifc_import.py', path]
                        if self.server.ifc_workers is not None:
                            command.extend(['--workers', str(self.server.ifc_workers)])
                        self.stream_worker(command)
                    elif solidworks:
                        self.stream_worker([sys.executable, ROOT / 'solidworks_import.py', path])
                    elif iges:
                        self.stream_worker([sys.executable, ROOT / 'iges_import.py', path, '--deflection', str(d)])
                    elif three_mf:
                        self.stream_worker([sys.executable, ROOT / 'three_mf_import.py', path])
                    elif native:
                        self.stream_worker([native, path, '--stream'])
                    elif hit:
                        with cached.open('rb') as source:
                            while chunk := source.read(1024*1024): self.wfile.write(chunk)
                    elif cached is None:
                        for event in self.server.load_stream(path,d):
                            self.wfile.write(frame(event))
                    else:
                        fd, tmp = tempfile.mkstemp(dir=CACHE, suffix='.tmp')
                        try:
                            with os.fdopen(fd, 'wb') as output:
                                for event in self.server.load_stream(path,d):
                                    data = frame(event)
                                    output.write(data)
                                    self.wfile.write(data); self.wfile.flush()
                            os.replace(tmp, cached)
                        finally:
                            if os.path.exists(tmp): os.remove(tmp)
                except Exception as e:
                    self.wfile.write(frame({'type':'error','message':str(e)}));self.wfile.flush()
            except (BrokenPipeError,ConnectionResetError):pass
            finally:
                if job.temporary:
                    try:path.unlink()
                    except OSError:pass
        else:self.send_error(404,'Not found')

    def stream_stl(self, path):
        try:
            source = path.open('rb')
        except OSError:
            self.send_error(404, 'Unable to open the selected STL')
            return
        with source:
            size = os.fstat(source.fileno()).st_size
            if not 0 < size <= MAX_UPLOAD:
                self.send_error(413, 'Select an STL file up to 250 MiB')
                return
            self.send_response(200)
            self.send_header('Content-Type', 'application/octet-stream')
            self.send_header('Content-Length', str(size))
            self.send_header('Cache-Control', 'no-store')
            self.send_header('X-Content-Type-Options', 'nosniff')
            self.end_headers()
            while size and (chunk := source.read(min(size, 1024 * 1024))):
                self.wfile.write(chunk)
                size -= len(chunk)

    def stream_worker(self, command):
        # Read complete frames so a failed worker cannot leave a partial
        # payload ahead of the server's error frame. Hold back completion
        # until the worker has exited successfully.
        started = perf_counter()
        first_mesh = None
        sent = 0
        completion = None
        shared_mesh = False
        point_frame = None
        packet = bytearray()
        with tempfile.TemporaryFile() as diagnostics:
            with subprocess.Popen(command, stdout=subprocess.PIPE, stderr=diagnostics) as worker:
                try:
                    while prefix := worker.stdout.read(4):
                        if completion is not None:
                            raise ValueError('Worker sent data after its completion frame')
                        if len(prefix) != 4:
                            break
                        size, = struct.unpack('<I', prefix)
                        if not 0 < size <= 64 * 1024:
                            raise ValueError('Invalid worker frame header')
                        header = worker.stdout.read(size)
                        if len(header) != size:
                            break
                        event = json.loads(header)
                        length = event['payload_bytes']
                        if type(length) is not int or not 0 <= length <= 1024 * 1024:
                            raise ValueError('Invalid worker frame payload')
                        payload_start = 4 + size
                        packet_size = payload_start + length
                        if packet_size > len(packet):
                            packet = bytearray(packet_size)
                        packet[:4] = prefix
                        packet[4:payload_start] = header
                        frame_view = memoryview(packet)[:packet_size]
                        if worker.stdout.readinto(frame_view[payload_start:]) != length:
                            break
                        kind = event['type']
                        if kind == 'done':
                            completion = bytes(frame_view)
                        else:
                            self.wfile.write(frame_view)
                            sent += packet_size
                            if kind == 'indexed':
                                shared_mesh = bool(event.get('shared'))
                                point_frame = None
                                if event.get('mode') == 'points':
                                    point_frame = 'colors' if event.get('vertex_colors') else 'positions'
                            drawable = (kind in ('part', 'instances') or kind == point_frame or
                                        (kind == 'indices' and not shared_mesh))
                            if first_mesh is None and drawable:
                                first_mesh = perf_counter()
                    code = worker.wait()
                finally:
                    if worker.poll() is None:
                        worker.kill()

            size = diagnostics.seek(0, os.SEEK_END)
            diagnostics.seek(max(0, size - 8192))
            message = diagnostics.read().decode('utf-8', errors='replace').strip()
        if code:
            raise ValueError(message[-500:] or f'Native import failed ({code})')
        if completion is None:
            raise ValueError('Worker stream ended before completion')
        self.wfile.write(completion)
        if os.environ.get('STEPFLASH_PROFILE'):
            if message:
                print(message, file=sys.stderr, flush=True)
            first_ms = f'{(first_mesh-started)*1000:.2f}' if first_mesh is not None else 'n/a'
            print(f'HTTP mesh stream: first_mesh_ms={first_ms}, '
                  f'total_ms={(perf_counter()-started)*1000:.2f}, bytes={sent+len(completion)}',
                  file=sys.stderr, flush=True)

    def do_POST(self):
        if self.path == '/api/pick':
            self.choose_file()
            return
        if self.path!='/api/upload':self.send_error(404,'Not found');return
        model_format = self.headers.get('X-VTX-Format', 'step')
        if model_format not in UPLOAD_FORMATS:
            self.send_error(415, 'Unsupported upload format')
            return
        if error := self.format_error(model_format):
            self.send_error(415, error)
            return
        try: length=int(self.headers.get('Content-Length','0'))
        except ValueError:length=0
        limit = MAX_NATIVE_UPLOAD if model_format != 'step' else MAX_UPLOAD
        if length <= 0 or length > limit:
            self.send_error(413, f'File must be between 1 byte and {limit // (1024 * 1024)} MiB')
            return
        fd,path=tempfile.mkstemp(suffix='.' + model_format,prefix='vtx_')
        key = uuid.uuid4().hex
        try:
            digest = hashlib.sha256() if model_format == 'step' and self.server.native_engine is None else None
            with os.fdopen(fd,'wb') as f:
                remaining=length
                while remaining:
                    block=self.rfile.read(min(1024*1024,remaining))
                    if not block:raise ConnectionError('Upload interrupted')
                    f.write(block)
                    if digest is not None:
                        digest.update(block)
                    remaining -= len(block)
            JOBS[key] = Job(Path(path), model_format, temporary=True, digest=digest.hexdigest() if digest is not None else None)
            self.json_response({'id': key})
        except Exception:
            JOBS.pop(key, None)
            try:os.remove(path)
            except OSError:pass
            self.send_error(500,'Upload failed')
    def log_message(self, fmt, *args):
        print('%s %s'%(self.address_string(),fmt%args))

def main():
    import argparse
    p = argparse.ArgumentParser()
    p.add_argument('--port', type=int, default=8765)
    p.add_argument('--ifc-workers', type=int, help='Override IFC geometry worker count')
    engine = p.add_mutually_exclusive_group()
    engine.add_argument('--native', type=Path, metavar='EXE', help='Use a specific native core')
    engine.add_argument('--occt', action='store_true', help='Use Open CASCADE')
    a = p.parse_args()
    if a.ifc_workers is not None and a.ifc_workers < 1:
        p.error('--ifc-workers must be positive')
    if a.native is not None and not a.native.is_file():
        p.error(f'native engine does not exist: {a.native}')
    native = a.native
    if native is None and not a.occt and NATIVE_EXE.is_file():
        native = NATIVE_EXE
    server = ThreadingHTTPServer(('127.0.0.1', a.port), Handler)
    server.native_engine = native.resolve() if native is not None else None
    server.ifc_available = find_spec('ifcopenshell') is not None
    server.occt_available = find_spec('OCP') is not None
    server.desktop_picker = os.name == 'nt' and find_spec('tkinter') is not None
    server.ifc_workers = a.ifc_workers
    if server.native_engine is None:
        import OCP
        from engine import load_stream
        server.ocp_version = getattr(OCP, '__version__', 'unknown')
        server.load_stream = load_stream
    print(f'Open http://127.0.0.1:{server.server_port} in a browser',flush=True)
    server.serve_forever()
if __name__=='__main__':main()
