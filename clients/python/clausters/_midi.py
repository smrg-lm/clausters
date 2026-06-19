"""ctypes binding over the MIDI file core (`clausters-midi`).

Loads ``libclausters_midi`` (the C ABI over the SMF writer, built with
``cargo build -p clausters-midi``) and exposes :func:`write_smf`: turn a list of
``(tick, message_bytes)`` channel-voice events into Standard MIDI File bytes.

Boundary rule (same as :mod:`clausters._native`): only flat data crosses — ints
and byte buffers in, ``bytes`` out. The library is loaded lazily and version
checked on first use, so importing this module never fails just because the
cdylib has not been built yet.
"""

import ctypes
import os
from array import array

MIDI_ABI_VERSION = 1

_LIB = None


def _find_library() -> str:
    candidates = [os.environ.get("CLAUSTERS_MIDI_LIB")]
    here = os.path.dirname(os.path.abspath(__file__))
    # clients/python/clausters/_midi.py -> repo root is three levels up.
    root = os.path.dirname(os.path.dirname(os.path.dirname(here)))
    for profile in ("release", "debug"):
        for name in ("libclausters_midi.so", "libclausters_midi.dylib", "clausters_midi.dll"):
            candidates.append(os.path.join(root, "target", profile, name))
    for c in candidates:
        if c and os.path.exists(c):
            return c
    raise OSError(
        "libclausters_midi not found: build it with "
        "`cargo build -p clausters-midi` (add --release for the release dir) "
        "or point CLAUSTERS_MIDI_LIB at it"
    )


def _configure(lib: ctypes.CDLL) -> ctypes.CDLL:
    lib.clausters_midi_abi_version.restype = ctypes.c_uint32
    got = lib.clausters_midi_abi_version()
    if got != MIDI_ABI_VERSION:
        raise OSError(
            f"libclausters_midi speaks ABI v{got}, this binding v{MIDI_ABI_VERSION}"
        )
    u32p = ctypes.POINTER(ctypes.c_uint32)
    u8p = ctypes.POINTER(ctypes.c_uint8)
    lib.clausters_midi_write_smf.restype = u8p
    lib.clausters_midi_write_smf.argtypes = [
        u32p, u8p, ctypes.c_size_t, ctypes.c_uint16, ctypes.POINTER(ctypes.c_size_t),
    ]
    lib.clausters_midi_free.argtypes = [u8p, ctypes.c_size_t]
    return lib


def lib(path: str | None = None) -> ctypes.CDLL:
    """The loaded, version-checked cdylib (cached after the first call)."""
    global _LIB
    if _LIB is None or path is not None:
        _LIB = _configure(ctypes.CDLL(path or _find_library()))
    return _LIB


def abi_version() -> int:
    return lib().clausters_midi_abi_version()


def write_smf(events, ppq: int) -> bytes:
    """Standard MIDI File bytes from ``events`` (a list of ``(tick, message)``,
    ``message`` 2-3 raw channel-voice bytes) at ``ppq`` ticks per quarter note.
    """
    events = list(events)
    n = len(events)
    if n == 0:
        raise ValueError("write_smf needs at least one event")
    ticks = array("I", (int(t) & 0xFFFFFFFF for t, _ in events))
    msgs = bytearray(3 * n)
    for i, (_, message) in enumerate(events):
        b = bytes(message)[:3]
        msgs[3 * i : 3 * i + len(b)] = b

    u32p = ctypes.POINTER(ctypes.c_uint32)
    u8p = ctypes.POINTER(ctypes.c_uint8)
    ticks_ptr = ctypes.cast(ticks.buffer_info()[0], u32p)
    msgs_ptr = ctypes.cast((ctypes.c_uint8 * len(msgs)).from_buffer(msgs), u8p)
    out_len = ctypes.c_size_t(0)
    ptr = lib().clausters_midi_write_smf(ticks_ptr, msgs_ptr, n, int(ppq), ctypes.byref(out_len))
    if not ptr:
        raise RuntimeError("clausters_midi_write_smf returned null")
    try:
        return bytes(ctypes.cast(ptr, ctypes.POINTER(ctypes.c_uint8 * out_len.value)).contents)
    finally:
        lib().clausters_midi_free(ptr, out_len.value)
