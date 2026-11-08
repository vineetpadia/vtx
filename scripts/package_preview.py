"""Package the separately installed Windows Explorer preview. Python 3.11+."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import tempfile
import zipfile

from package_release import ROOT, notices


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--handler', type=Path, required=True)
    parser.add_argument('--metadata', type=Path, required=True)
    parser.add_argument('--version', required=True)
    parser.add_argument('--output', type=Path, default=ROOT / 'dist')
    args = parser.parse_args()
    version = args.version.removeprefix('preview-v').removeprefix('v')
    if not version or any(c not in '0123456789.' for c in version):
        parser.error('Version must be numeric, for example 0.2.1')
    for path in [args.binary, args.handler, args.metadata]:
        if not path.is_file():
            parser.error(f'Missing input: {path}')
    args.output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='vtx-preview-package-') as temporary:
        stage = Path(temporary)
        build = stage / 'build'
        build.mkdir()
        shutil.copy2(args.binary, build / 'VTX.exe')
        shutil.copy2(args.handler, build / 'VTXPreviewHD.dll')
        shutil.copy2(ROOT / 'assets/vtx.ico', build / 'VTX.ico')
        notices(build / 'licenses', args.metadata)
        source = ROOT / 'native/windows-preview'
        for name in ['install.ps1', 'README.md', 'LICENSE.libdeflate']:
            shutil.copy2(source / name, stage / name)
        for action, extra in [('Install', ''), ('Uninstall', ' -Uninstall')]:
            (stage / f'{action}.cmd').write_text(
                '@echo off\r\nsetlocal\r\n'
                'powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass '
                '-File "%~dp0install.ps1"' + extra + '\r\n'
                'if errorlevel 1 (echo VTX preview setup failed. See the message above.)\r\n'
                'pause\r\n', encoding='ascii', newline='')
        (stage / 'START_HERE.txt').write_text(
            f'VTX Explorer Preview {version}\n\n'
            'Extract the entire ZIP, then double-click Install.cmd. No admin access is needed.\n'
            'In Explorer, enable the Preview pane with Alt+P and select a supported file.\n'
            'Middle-drag rotates, Ctrl+middle-drag pans, scroll zooms, and F fits.\n'
            'DXF drawings pan in 2D.\n\n'
            'Supported extensions: SLDPRT, SLDASM, STEP, STP, P21, STL, OBJ, PLY,\n'
            'GLB, GLTF, FBX, 3MF, IFC, IFCZIP, DXF.\n'
            'Parasolid X_T/X_B, IGES, Fusion F3D and SolidWorks SLDDRW are not included.\n\n'
            'Double-click defaults and main-app Open-with commands are preserved.\n'
            'Run Uninstall.cmd to restore the previous preview handlers.\n'
            'This separate preview package is updated by installing a newer preview package;\n'
            'main-app automatic updates do not replace it. Windows 10+ x64 and a\n'
            'Vulkan or Direct3D 12 GPU are required. This build is unsigned.\n\n'
            'Source, limitations and licenses: README.md and build/licenses.\n'
            'https://github.com/vineetpadia/vtx\n', encoding='utf-8')
        files = {str(p.relative_to(stage)).replace('\\', '/'): hashlib.sha256(p.read_bytes()).hexdigest()
                 for p in sorted(stage.rglob('*')) if p.is_file()}
        (stage / 'manifest.json').write_text(json.dumps(dict(version=version, files=files), indent=2))
        archive = args.output / 'vtx-explorer-preview-windows-x64.zip'
        with zipfile.ZipFile(archive, 'w', compression=zipfile.ZIP_DEFLATED,
                             strict_timestamps=False) as output:
            for p in sorted(stage.rglob('*')):
                if p.is_file():
                    output.write(p, p.relative_to(stage))
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        archive.with_name(archive.name + '.sha256').write_text(
            f'{digest}  {archive.name}\n', encoding='ascii')
        print(archive)


if __name__ == '__main__':
    main()
