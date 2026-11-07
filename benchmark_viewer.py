"""Compare native first-frame times with alternating fresh processes."""

import argparse
import csv
import hashlib
import os
from pathlib import Path
import re
import statistics
import subprocess
import tempfile


def windows_resources(process):
    if os.name != "nt":
        return {}
    import ctypes as c
    from ctypes import wintypes as w

    class Memory(c.Structure):
        _fields_ = [("size", w.DWORD), ("faults", w.DWORD)] + [
            (name, c.c_size_t) for name in (
                "peak_ws", "ws", "peak_paged", "paged", "peak_nonpaged",
                "nonpaged", "pagefile", "peak_commit", "private_bytes")]

    memory = Memory(c.sizeof(Memory))
    get_memory = c.WinDLL("psapi").GetProcessMemoryInfo
    get_memory.argtypes = (w.HANDLE, c.POINTER(Memory), w.DWORD)
    get_memory.restype = w.BOOL
    values = {}
    if get_memory(int(process._handle), c.byref(memory), c.sizeof(memory)):
        values.update(peak_commit_mib=memory.peak_commit / 1048576,
                      peak_ws_mib=memory.peak_ws / 1048576)
    get_times = c.WinDLL("kernel32").GetProcessTimes
    get_times.argtypes = (w.HANDLE,) + (c.POINTER(w.FILETIME),) * 4
    get_times.restype = w.BOOL
    times = [w.FILETIME() for _ in range(4)]
    if get_times(int(process._handle), *(c.byref(value) for value in times)):
        values["cpu_ms"] = sum((value.dwHighDateTime << 32) + value.dwLowDateTime
                               for value in times[2:]) / 10000
    return values


def run(executable, model, args, threads=None, image=None):
    environment = os.environ.copy()
    environment["STEPFLASH_PROFILE"] = "1"
    if args.backend:
        environment["WGPU_BACKEND"] = args.backend
    if threads is not None:
        environment["VTX_FBX_THREADS"] = str(threads)
    command = [str(executable), "--check", str(model)]
    if image is not None:
        command.append(str(image))
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               text=True, env=environment,
                               creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
    try:
        output, errors = process.communicate(timeout=120)
    except subprocess.TimeoutExpired:
        process.kill()
        process.communicate()
        raise
    if process.returncode:
        raise RuntimeError(f"{executable.name}: {output}\n{errors}")
    values = dict(item.split("=", 1) for item in output.split() if "=" in item)
    if float(values["first_frame_ms"]) <= 0:
        raise RuntimeError("Viewer did not report a completed frame")
    values.update(windows_resources(process))
    for phase, key in (("file+parse", "parse_ms"), ("triangulation", "triangulation_ms"),
                       ("vertices+uv", "vertices_uv_ms")):
        match = re.search(re.escape(phase) + r"=([0-9.]+) ms", errors)
        if match:
            values[key] = float(match[1])
    return values


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    parser.add_argument("models", type=Path, nargs="+")
    parser.add_argument("--runs", type=int, default=12)
    parser.add_argument("--csv", type=Path, required=True)
    parser.add_argument("--backend", choices=("vulkan", "dx12", "metal", "gl"))
    parser.add_argument("--warm-cache", action="store_true")
    parser.add_argument("--verify-images", action="store_true", help="Require identical PNG hashes before timing")
    parser.add_argument("--before-fbx-threads", type=int)
    parser.add_argument("--after-fbx-threads", type=int)
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")
    versions = [("before", args.before.resolve(), args.before_fbx_threads),
                ("after", args.after.resolve(), args.after_fbx_threads)]
    rows = []
    buffer = bytearray(8 * 1024 * 1024)
    for model in map(Path.resolve, args.models):
        counts = None
        if args.verify_images:
            hashes = []
            with tempfile.TemporaryDirectory(prefix="vtx-frames-") as temporary:
                for name, executable, threads in versions:
                    image = Path(temporary) / f"{name}.png"
                    run(executable, model, args, threads, image)
                    with image.open("rb") as source:
                        hashes.append(hashlib.file_digest(source, "sha256").hexdigest())
            if hashes[0] != hashes[1]:
                raise RuntimeError(f"Rendered images differ: {model}")
            print(f"Image match: {model.name}; SHA-256 {hashes[0]}", flush=True)
        for trial in range(args.runs):
            for name, executable, threads in versions[::(-1 if trial % 2 else 1)]:
                if args.warm_cache:
                    with model.open("rb") as source:
                        while source.readinto(buffer):
                            pass
                values = run(executable, model, args, threads)
                current = tuple(values[key] for key in ("triangles", "points", "issues"))
                if counts is not None and current != counts:
                    raise RuntimeError(f"Geometry or issue counts differ: {model}")
                counts = current
                rows.append(dict(model=str(model), trial=trial + 1, version=name, **values))
            print(f"{model.name}: pair {trial + 1}/{args.runs}", flush=True)
        for name, _, _ in versions:
            samples = [float(row["first_frame_ms"]) for row in rows
                       if row["model"] == str(model) and row["version"] == name]
            print(f"{model.name}: {name} median {statistics.median(samples):.3f} ms", flush=True)
    columns = ["model", "trial", "version"] + sorted(set().union(*(row.keys() for row in rows))
                                                     - {"model", "trial", "version"})
    with args.csv.open("w", encoding="utf-8", newline="") as output:
        writer = csv.DictWriter(output, fieldnames=columns)
        writer.writeheader()
        writer.writerows(rows)


if __name__ == "__main__":
    main()
