"""Freeze the optional CAD reader. Run in an isolated Python 3.12 build venv.

This is an onedir build: Python/OCCT are bundled, and replaceable shared
libraries stay outside the executable. No system Python or CAD app is used.
Release redistribution requires the native-library license audit described in
native/cad-importer/README.md; this script does not publish a release.
"""
import argparse
from importlib import metadata
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--work', type=Path, required=True)
    args = parser.parse_args()
    args.work.mkdir(parents=True, exist_ok=True)
    command = [sys.executable, '-m', 'PyInstaller', '--noconfirm', '--clean', '--onedir',
               '--name', 'vtx-cad-import', '--distpath', str(args.output.resolve()),
               '--workpath', str((args.work / 'build').resolve()),
               '--specpath', str(args.work.resolve()),
               '--collect-all', 'OCP', '--collect-all', 'parasolid_kit',
               '--copy-metadata', 'parasolid-kit', '--copy-metadata', 'cadquery-ocp-novtk']
    # delvewheel/auditwheel sibling runtime directory is not a Python module.
    ocp = metadata.distribution('cadquery-ocp-novtk')
    for name in ['cadquery_ocp_novtk.libs', 'cadquery_ocp_novtk.dylibs']:
        libraries = Path(ocp.locate_file(name))
        if libraries.is_dir():
            command.extend(['--add-binary', f'{libraries}{__import__("os").pathsep}{name}'])
    command.append(str(ROOT / 'native/cad-importer/convert.py'))
    subprocess.run(command, check=True)
    bundle = args.output / 'vtx-cad-import'
    notices = bundle / 'licenses'
    notices.mkdir(exist_ok=True)
    # Retain Python package attribution. Native transitive library notices are
    # a separate required audit before public binary redistribution.
    for dist in metadata.distributions():
        name = dist.metadata['Name'] or 'unknown'
        for entry in dist.files or []:
            if any(part.lower().startswith(('license', 'licence', 'copying', 'notice')) for part in entry.parts):
                source = Path(dist.locate_file(entry))
                if source.is_file():
                    target = notices / name / entry
                    target.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(source, target)
    python_license = Path(sys.base_prefix) / 'LICENSE.txt'
    if python_license.is_file():
        shutil.copy2(python_license, notices / 'Python-LICENSE.txt')
    shutil.copy2(ROOT / 'native/cad-importer/README.md', notices / 'CAD_IMPORT.md')
    print(bundle)


if __name__ == '__main__':
    main()
