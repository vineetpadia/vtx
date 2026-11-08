"""Package a built VTX desktop app. Requires Python 3.11+ and Cargo metadata."""

import argparse
import hashlib
import json
from pathlib import Path
import plistlib
import shutil
import struct
import subprocess
import tarfile
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]
PLATFORMS = ("windows-x64", "linux-x64", "macos-arm64", "macos-x64")


def demo_model(destination):
    """Write an original, self-contained colored cube for a quick render check."""
    faces = [
        [(1, -1, -1), (1, 1, -1), (1, 1, 1), (1, -1, 1)],
        [(-1, -1, 1), (-1, 1, 1), (-1, 1, -1), (-1, -1, -1)],
        [(-1, 1, -1), (-1, 1, 1), (1, 1, 1), (1, 1, -1)],
        [(-1, -1, 1), (-1, -1, -1), (1, -1, -1), (1, -1, 1)],
        [(1, -1, 1), (1, 1, 1), (-1, 1, 1), (-1, -1, 1)],
        [(-1, -1, -1), (-1, 1, -1), (1, 1, -1), (1, -1, -1)],
    ]
    colors = [(0.95, 0.15, 0.1), (0.1, 0.4, 0.95), (0.15, 0.8, 0.25),
              (0.9, 0.65, 0.1), (0.6, 0.15, 0.85), (0.05, 0.8, 0.85)]
    data, views, accessors, primitives = bytearray(), [], [], []
    for face_id, points in enumerate(faces):
        offset = len(data)
        data.extend(struct.pack("<12f6H", *(axis for point in points for axis in point),
                                0, 1, 2, 0, 2, 3))
        views.extend([dict(buffer=0, byteOffset=offset, byteLength=48, target=34962),
                      dict(buffer=0, byteOffset=offset + 48, byteLength=12, target=34963)])
        accessors.extend([
            dict(bufferView=face_id * 2, componentType=5126, count=4, type="VEC3",
                 min=[min(point[axis] for point in points) for axis in range(3)],
                 max=[max(point[axis] for point in points) for axis in range(3)]),
            dict(bufferView=face_id * 2 + 1, componentType=5123, count=6, type="SCALAR"),
        ])
        primitives.append(dict(attributes={"POSITION": face_id * 2},
                               indices=face_id * 2 + 1, material=face_id))
    document = dict(asset=dict(version="2.0", generator="VTX demo"), scene=0,
                    scenes=[dict(nodes=[0])], nodes=[dict(mesh=0)],
                    meshes=[dict(primitives=primitives)],
                    materials=[dict(pbrMetallicRoughness=dict(baseColorFactor=[*color, 1],
                                    metallicFactor=0, roughnessFactor=0.6)) for color in colors],
                    buffers=[dict(byteLength=len(data))], bufferViews=views, accessors=accessors)
    metadata = json.dumps(document, separators=(",", ":")).encode("utf-8")
    metadata += b" " * (-len(metadata) % 4)
    destination.write_bytes(struct.pack("<III", 0x46546C67, 2, 28 + len(metadata) + len(data))
                            + struct.pack("<II", len(metadata), 0x4E4F534A) + metadata
                            + struct.pack("<II", len(data), 0x004E4942) + data)


def notices(destination, metadata):
    """Ship attribution and the available license texts of compiled dependencies."""
    destination.mkdir(parents=True)
    for name in ("LICENSE-MIT", "LICENSE-APACHE"):
        shutil.copy2(ROOT / "native" / name, destination / name)
    shutil.copy2(ROOT / "vendor/sldprt/LICENSE", destination / "sldprt-export-LICENSE")
    shutil.copy2(ROOT / "vendor/sldprt/NOTICE.md", destination / "sldprt-export-NOTICE.md")
    shutil.copy2(ROOT / "native/meshopt_decode/LICENSE", destination / "meshoptimizer-LICENSE")
    entries = ["VTX third-party software notices", "",
               "VTX's STEP core derives from Formlabs Foxtrot (MIT OR Apache-2.0).",
               "SolidWorks reader layouts derive from XRTC5/sldprt-export (MIT).", ""]
    entries.extend(("meshoptimizer 1.3 by Arseny Kapoulkine (MIT).",
                    "Source: https://github.com/zeux/meshoptimizer/tree/v1.3", ""))
    data = json.loads(metadata.read_text(encoding="utf-8"))
    for package in sorted(data["packages"], key=lambda item: (item["name"], item["version"])):
        if package.get("source") is None:
            continue
        label = f"{package['name']}-{package['version']}"
        entries.extend((label, f"License: {package.get('license') or 'see license files'}",
                        f"Source: {package.get('repository') or package['source']}"))
        directory = Path(package["manifest_path"]).parent
        candidates = [path for path in directory.iterdir()
                      if path.name.lower().startswith(("license", "licence", "copying", "notice"))]
        if package.get("license_file"):
            candidates.append(directory / package["license_file"])
        if package["name"] == "epaint_default_fonts":
            candidates.extend((directory / "fonts").rglob("*.txt"))
            candidates.append(directory / "README.md")
        for candidate in sorted(set(candidates)):
            files = [candidate] if candidate.is_file() else candidate.rglob("*")
            for source in files:
                if source.is_file():
                    relative = source.relative_to(directory)
                    target = destination / label / relative
                    target.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(source, target)
        entries.append("")
    (destination / "THIRD_PARTY_NOTICES.txt").write_text("\n".join(entries), encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=PLATFORMS, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--metadata", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--runtime", type=Path, action="append", default=[])
    parser.add_argument("--vpk", type=Path, help="Windows Velopack CLI: also build installer/update packages")
    parser.add_argument("--output", type=Path, default=ROOT / "dist")
    args = parser.parse_args()
    if not args.binary.is_file():
        parser.error(f"Missing executable: {args.binary}")
    version = args.version.removeprefix("v")
    if not version or any(character not in "0123456789." for character in version):
        parser.error("Version must be a numeric dotted version, such as v0.1.0")
    args.output.mkdir(parents=True, exist_ok=True)
    name = f"vtx-{args.platform}"
    with tempfile.TemporaryDirectory(prefix="vtx-package-") as temporary:
        stage = Path(temporary)
        package = stage / name
        package.mkdir()
        demo_model(package / "demo.glb")
        shutil.copy2(ROOT / "docs/TESTING.txt", package / "TESTING.txt")
        if args.platform.startswith("macos"):
            app = package / "VTX.app"
            contents = app / "Contents"
            executable = contents / "MacOS/VTX"
            resources = contents / "Resources"
            executable.parent.mkdir(parents=True)
            resources.mkdir()
            with (contents / "Info.plist").open("wb") as output:
                plistlib.dump({
                    "CFBundleName": "VTX", "CFBundleDisplayName": "VTX",
                    "CFBundleExecutable": "VTX", "CFBundleIdentifier": "com.vineetpadia.vtx",
                    "CFBundlePackageType": "APPL", "CFBundleShortVersionString": version,
                    "CFBundleVersion": version, "LSMinimumSystemVersion": "12.0",
                    "NSHighResolutionCapable": True,
                    "LSApplicationCategoryType": "public.app-category.graphics-design",
                }, output)
            license_directory = resources / "licenses"
        else:
            executable = package / ("VTX.exe" if args.platform == "windows-x64" else "VTX")
            license_directory = package / "licenses"
        shutil.copy2(args.binary, executable)
        executable.chmod(0o755)
        for runtime in args.runtime:
            if not runtime.is_file():
                parser.error(f"Missing runtime: {runtime}")
            shutil.copy2(runtime, executable.parent / runtime.name)
        notices(license_directory, args.metadata)
        (package / "START_HERE.txt").write_text(
            "VTX " + version + "\n\n"
            "Windows: extract this folder and double-click VTX.exe.\n"
            "Mac: drag VTX.app to Applications and open it.\n"
            "Linux: extract this folder and open VTX (an executable).\n\n"
            "Drop a model into the window, or press Ctrl+O (Command+O on Mac).\n"
            "Middle-drag to rotate, Ctrl+middle-drag to pan, scroll to zoom.\n"
            "Click edges to measure; Ctrl-click a second edge for gap and angle.\n"
            "The Windows Setup edition downloads updates in the background\n"
            "and installs them when the last VTX window closes.\n"
            "Keep OBJ/MTL textures and glTF companion files beside their model.\n\n"
            "Try the included demo.glb colored cube. See TESTING.txt for offline\n"
            "installation help, requirements, known limits, and feedback steps.\n\n"
            "Preview builds are unsigned. Installation help:\n"
            "https://github.com/vineetpadia/vtx/blob/main/docs/INSTALL.md\n\n"
            "Source and third-party notices:\n"
            "https://github.com/vineetpadia/vtx\n"
            "The licenses folder contains dependency attribution and license texts.\n",
            encoding="utf-8",
        )
        if args.platform.startswith("macos"):
            subprocess.run(["codesign", "--force", "--deep", "--sign", "-", str(app)], check=True)
            subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
        if args.vpk:
            if args.platform != "windows-x64":
                parser.error("--vpk applies to Windows x64 only")
            subprocess.run([str(args.vpk.resolve()), "--yes", "--skip-updates", "pack",
                            "--packId", "VTXDesktop", "--packVersion", version,
                            "--packTitle", "VTX", "--packAuthors", "Vineet Padia", "--runtime", "win-x64",
                            "--packDir", str(package), "--mainExe", "VTX.exe",
                            "--icon", str(ROOT / "assets/vtx.ico"),
                            "--shortcuts", "StartMenuRoot", "--delta", "None",
                            "--releaseNotes", str(ROOT / "docs/RELEASE_NOTES.md"),
                            "--outputDir", str(args.output.resolve())], check=True)
            setups = list(args.output.glob("*-Setup.exe"))
            if len(setups) != 1:
                raise RuntimeError("Expected exactly one VTX setup executable")
            setup_name = setups[0].name
            setups[0].replace(args.output / "VTX-Setup.exe")
            asset_index = args.output / "assets.win.json"
            if asset_index.is_file():
                assets = json.loads(asset_index.read_text(encoding="utf-8-sig"))
                for asset in assets:
                    if asset.get("RelativeFileName") == setup_name:
                        asset["RelativeFileName"] = "VTX-Setup.exe"
                asset_index.write_text(json.dumps(assets), encoding="utf-8")
        if args.platform == "linux-x64":
            archive = args.output / f"{name}.tar.gz"
            with tarfile.open(archive, "w:gz") as output:
                output.add(package, arcname=name)
        else:
            archive = args.output / f"{name}.zip"
            with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED,
                                 strict_timestamps=False) as output:
                for path in sorted(package.rglob("*")):
                    if path.is_file():
                        # Keep VTX.app at the root so Finder can open or drag it directly.
                        output.write(path, path.relative_to(package))
        with archive.open("rb") as source:
            digest = hashlib.file_digest(source, "sha256").hexdigest()
        archive.with_name(archive.name + ".sha256").write_text(
            f"{digest}  {archive.name}\n", encoding="ascii")
        print(archive)
        if args.vpk:
            for artifact in sorted(args.output.iterdir()):
                if artifact.is_file() and not artifact.name.endswith(".sha256"):
                    with artifact.open("rb") as source:
                        digest = hashlib.file_digest(source, "sha256").hexdigest()
                    artifact.with_name(artifact.name + ".sha256").write_text(f"{digest}  {artifact.name}\n",encoding="ascii")


if __name__ == "__main__":
    main()
