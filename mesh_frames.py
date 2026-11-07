"""Shared binary mesh framing and worker memory probes."""

import json
import struct
import sys


FRAME_BYTES = 192 * 1024


def peak_memory():
    if sys.platform == "win32":
        import ctypes
        from ctypes import wintypes

        class Counters(ctypes.Structure):
            _fields_ = [("size", wintypes.DWORD), ("faults", wintypes.DWORD),
                        ("memory", ctypes.c_size_t * 8)]

        kernel = ctypes.WinDLL("kernel32")
        kernel.GetCurrentProcess.restype = wintypes.HANDLE
        query = kernel.K32GetProcessMemoryInfo
        query.argtypes = (wintypes.HANDLE, ctypes.POINTER(Counters), wintypes.DWORD)
        counters = Counters()
        counters.size = ctypes.sizeof(counters)
        if query(kernel.GetCurrentProcess(), ctypes.byref(counters), counters.size):
            return counters.memory[0]
        return None
    if sys.platform in ("linux", "darwin"):
        import resource
        scale = 1024 if sys.platform == "linux" else 1
        return resource.getrusage(resource.RUSAGE_SELF).ru_maxrss * scale
    return None


def write_frame(output, kind, payload=b"", **fields):
    header = json.dumps({"type": kind, "payload_bytes": len(payload), **fields},
                        separators=(",", ":")).encode()
    output.write(struct.pack("<I", len(header)))
    output.write(header)
    output.write(payload)


