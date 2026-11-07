"""Package a built VTX desktop app. Requires Python 3.11+ and Cargo metadata."""

import argparse
import hashlib
import json
from pathlib import Path
import plistlib
import shutil
import subprocess
import tarfile
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]
PLATFORMS = ("windows-x64", "linux-x64", "macos-arm64", "macos-x64")


def notices(destination, metadata):
    """Ship attribution and the available license texts of compiled dependencies."""
    destination.mkdir(parents=True)
    for name in ("LICENSE-MIT", "LICENSE-APACHE"):
        shutil.copy2(ROOT / "native" / name, destination / name)
    shutil.copy2(ROOT / "vendor/sldprt/LICENSE", destination / "sldprt-export-LICENSE")
    shutil.copy2(ROOT / "vendor/sldprt/NOTICE.md", destination / "sldprt-export-NOTICE.md")
    entries = ["VTX third-party software notices", "",
               "VTX's STEP core derives from Formlabs Foxtrot (MIT OR Apache-2.0).",
               "SolidWorks reader layouts derive from XRTC5/sldprt-export (MIT).", ""]
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
            "Drag to orbit, scroll to zoom, R to reset the camera.\n"
            "Keep OBJ/MTL textures and glTF companion files beside their model.\n\n"
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


if __name__ == "__main__":
    main()
