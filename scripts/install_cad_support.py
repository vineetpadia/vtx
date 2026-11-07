#!/usr/bin/env python3
"""Install optional local VTX drawing support; Python 3.10+.

DXF: python scripts/install_cad_support.py
DWG (Linux/macOS, requires C compiler, make, pkg-config): add --build-libredwg
Alternatively install dwg2dxf separately and put it on PATH / set VTX_DWG2DXF.
"""
import argparse
import hashlib
from pathlib import Path
import os
import subprocess
import sys
import tarfile
import urllib.request
import venv

LIBREDWG_URL = "https://github.com/LibreDWG/libredwg/releases/download/0.14/libredwg-0.14.tar.xz"
LIBREDWG_SHA256 = "62ebb73b984f865960f20ed26619ea5f8789d5e3fd088fa40a2598384da81275"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prefix", type=Path, default=Path.home() / ".local/share/vtx")
    parser.add_argument("--build-libredwg", action="store_true")
    args = parser.parse_args()
    prefix = args.prefix.resolve()
    environment = prefix / "cad-venv"
    python = environment / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
    if not python.is_file():
        venv.EnvBuilder(with_pip=True).create(environment)
    subprocess.run([str(python), "-m", "pip", "install", "ezdxf==1.4.3", "Pillow==12.3.0"], check=True)
    if args.build_libredwg:
        if os.name == "nt":
            parser.error("On Windows install a LibreDWG dwg2dxf binary and set VTX_DWG2DXF; this source build uses make.")
        build = prefix / "cad-build"
        build.mkdir(parents=True, exist_ok=True)
        archive = build / "libredwg-0.14.tar.xz"
        urllib.request.urlretrieve(LIBREDWG_URL, archive)
        if hashlib.sha256(archive.read_bytes()).hexdigest() != LIBREDWG_SHA256:
            raise RuntimeError("LibreDWG archive checksum does not match pinned release")
        with tarfile.open(archive) as source:
            # Python 3.10 compatibility: reject absolute paths, traversal, and links.
            for member in source.getmembers():
                if member.issym() or member.islnk() or not (member.isfile() or member.isdir()) or not (build / member.name).resolve().is_relative_to(build.resolve()):
                    raise RuntimeError(f"Unexpected archive member: {member.name}")
            source.extractall(build)
        source_dir = build / "libredwg-0.14"
        for command in [
            ["./configure", f"--prefix={prefix / 'cad-tools'}", "--disable-bindings", "--disable-shared"],
            ["make", f"-j{min(os.cpu_count() or 2, 4)}"], ["make", "install"],
        ]:
            subprocess.run(command, cwd=source_dir, check=True)
    print(f"CAD Python: {python}\nFor custom installs set VTX_CAD_PYTHON to this path.")
    if args.build_libredwg:
        print(f"DWG converter: {prefix / 'cad-tools/bin/dwg2dxf'}\nLibreDWG is GPL-3.0-or-later; installed as a separate executable.")


if __name__ == "__main__":
    main()
