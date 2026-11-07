"""Download the public benchmark corpus and verify its recorded checksums."""

import argparse
import hashlib
import json
from pathlib import Path
import urllib.request
import zipfile


def local_path(root, relative):
    target = (root / relative).resolve()
    if not target.is_relative_to(root.resolve()):
        raise ValueError(f"Path is outside the model directory: {relative}")
    return target


def matches(path, record):
    if not path.is_file() or path.stat().st_size != record["bytes"]:
        return False
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest() == record["sha256"]


def download(root, record):
    destination = local_path(root, record["path"])
    if matches(destination, record):
        print(f"Verified {record['path']}", flush=True)
        return
    destination.parent.mkdir(parents=True, exist_ok=True)
    partial = destination.with_name(destination.name + ".download")
    request = urllib.request.Request(
        record["url"], headers={"User-Agent": "VTX benchmark corpus downloader"}
    )
    print(f"Downloading {record['path']} ({record['bytes']:,} bytes)", flush=True)
    with urllib.request.urlopen(request, timeout=60) as source, partial.open("wb") as output:
        while chunk := source.read(4 * 1024 * 1024):
            output.write(chunk)
    if not matches(partial, record):
        raise ValueError(f"Downloaded size or checksum differs: {record['path']}")
    partial.replace(destination)


def extract(root, record):
    destination = local_path(root, record["extract_to"])
    with zipfile.ZipFile(local_path(root, record["path"])) as archive:
        for entry in archive.infolist():
            relative = entry.filename.replace("\\", "/")
            if ":" in relative:
                raise ValueError(f"Invalid archive path: {entry.filename}")
            local_path(destination, relative)
        archive.extractall(destination)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("models", nargs="*", help="Model IDs from the list shown with no arguments")
    parser.add_argument("--manifest", type=Path, default=Path(__file__).with_name("benchmark_models.json"))
    parser.add_argument("--all", action="store_true", help="Download every dataset in the manifest")
    parser.add_argument("--directory", type=Path, default=Path(__file__).parent / ".benchmark-models")
    args = parser.parse_args()
    datasets = {
        item["id"]: item
        for item in json.loads(args.manifest.read_text(encoding="utf-8"))["datasets"]
    }
    if args.all:
        args.models = list(datasets)
    if not args.models:
        for key, item in datasets.items():
            size = sum(file["bytes"] for file in item["files"]) / 1_000_000
            license_name = item["license"]["spdx"] or item["license"]["status"]
            print(f"{key}: {item['title']} | {size:.1f} MB | {license_name}")
        return
    unknown = set(args.models) - datasets.keys()
    if unknown:
        parser.error("Unknown model IDs: " + ", ".join(sorted(unknown)))
    root = args.directory.resolve()
    root.mkdir(parents=True, exist_ok=True)
    for key in dict.fromkeys(args.models):
        item = datasets[key]
        print(f"{item['title']} — {item['notes']}", flush=True)
        for file in item["files"]:
            download(root, file)
            if "extract_to" in file:
                extract(root, file)
        for model in item["models"]:
            if not matches(local_path(root, model["path"]), model):
                raise ValueError(f"Extracted model differs: {model['path']}")
            print(f"Ready {model['path']}", flush=True)


if __name__ == "__main__":
    main()
