"""ctypes binding over the MIDI file core (`clausters-midi`).

Loads ``libclausters_midi`` (the C ABI over the SMF writer and, with the
``live`` feature, the virtual MIDI ports) and exposes `write_smf`: turn a list
of ``(tick, message_bytes)`` channel-voice events into Standard MIDI File bytes.
The wheel bundles the library **built with ``live``**, so an installed package
plays and records MIDI with nothing else present; a source checkout falls back
to ``target/``, where a plain ``cargo build -p clausters-midi`` leaves a
library without the ports (rebuild it with ``--features live``).

Boundary rule (same as `clausters._native`): only flat data crosses — ints
and byte buffers in, ``bytes`` out. The library is loaded lazily and version
checked on first use, so importing this module never fails just because the
cdylib has not been built yet.
"""

import ctypes
import os
from array import array

from . import _libpath

MIDI_ABI_VERSION = 2

# cdylib file names across platforms (Linux / macOS / Windows).
_MIDI_NAMES = ("libclausters_midi.so", "libclausters_midi.dylib", "clausters_midi.dll")

_LIB = None


def _find_library() -> str:
    # Precedence (see _libpath): env override, the bundled wheel copy, then the
    # workspace target/ of a source checkout.
    candidates = [os.environ.get("CLAUSTERS_MIDI_LIB")]
    candidates += _libpath.bundled_candidates(_MIDI_NAMES)
    candidates += _libpath.workspace_candidates(_MIDI_NAMES)
    for c in candidates:
        if c and os.path.exists(c):
            return c
    raise OSError(
        "libclausters_midi not found: install the wheel (it bundles the "
        "library) or, in a source checkout, build it with "
        "`cargo build -p clausters-midi --features live` (add --release for the "
        "release dir) or point CLAUSTERS_MIDI_LIB at it"
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
    writer_argtypes = [u32p, u8p, ctypes.c_size_t, ctypes.c_uint16, ctypes.POINTER(ctypes.c_size_t)]
    for name in ("clausters_midi_write_smf", "clausters_midi_write_clip"):
        fn = getattr(lib, name)
        fn.restype = u8p
        fn.argtypes = writer_argtypes
    lib.clausters_midi_free.argtypes = [u8p, ctypes.c_size_t]
    # Live I/O (only present if the cdylib was built with `--features live`).
    if hasattr(lib, "clausters_midi_output_open"):
        lib.clausters_midi_output_open.restype = ctypes.c_void_p
        lib.clausters_midi_output_open.argtypes = [u8p, ctypes.c_size_t]
        lib.clausters_midi_output_send.restype = ctypes.c_int32
        lib.clausters_midi_output_send.argtypes = [ctypes.c_void_p, u8p, ctypes.c_size_t]
        lib.clausters_midi_output_close.argtypes = [ctypes.c_void_p]
        lib.clausters_midi_input_open.restype = ctypes.c_void_p
        lib.clausters_midi_input_open.argtypes = [u8p, ctypes.c_size_t]
        lib.clausters_midi_input_poll.restype = ctypes.c_int32
        lib.clausters_midi_input_poll.argtypes = [
            ctypes.c_void_p, u8p, ctypes.c_size_t, ctypes.POINTER(ctypes.c_size_t)
        ]
        lib.clausters_midi_input_close.argtypes = [ctypes.c_void_p]
    return lib


def lib(path: str | None = None) -> ctypes.CDLL:
    """The loaded, version-checked cdylib (cached after the first call)."""
    global _LIB
    if _LIB is None or path is not None:
        _LIB = _configure(ctypes.CDLL(path or _find_library()))
    return _LIB


def abi_version() -> int:
    return lib().clausters_midi_abi_version()


def _write(writer, events, ppq: int) -> bytes:
    """Marshal ``events`` (``(tick, message)``, 2-3 raw channel-voice bytes) and
    call ``writer`` (an SMF or clip C function), returning the file bytes."""
    events = list(events)
    n = len(events)
    if n == 0:
        raise ValueError("need at least one event")
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
    ptr = writer(ticks_ptr, msgs_ptr, n, int(ppq), ctypes.byref(out_len))
    if not ptr:
        raise RuntimeError("MIDI writer returned null")
    try:
        return bytes(ctypes.cast(ptr, ctypes.POINTER(ctypes.c_uint8 * out_len.value)).contents)
    finally:
        lib().clausters_midi_free(ptr, out_len.value)


def write_smf(events, ppq: int) -> bytes:
    """Standard MIDI File (`.mid`) bytes from timed channel-voice events."""
    return _write(lib().clausters_midi_write_smf, events, ppq)


def write_clip(events, ppq: int) -> bytes:
    """MIDI 2.0 Clip File (SMF2CLIP) bytes — note velocities at 16-bit
    resolution — from timed channel-voice events."""
    return _write(lib().clausters_midi_write_clip, events, ppq)


# ---- live output (needs the cdylib built with `--features live`) ----


def _require_live():
    if not hasattr(lib(), "clausters_midi_output_open"):
        raise OSError(
            "libclausters_midi was built without the `live` feature: no virtual "
            "MIDI ports. Rebuild it with `cargo build -p clausters-midi "
            "--features live` (the wheel's bundled copy always has it -- "
            "`scripts/refresh-bin.sh` restages it)"
        )


def output_open(name: str = "clausters"):
    """Open a virtual MIDI output port; returns an opaque handle."""
    _require_live()
    nb = name.encode("utf-8")
    buf = (ctypes.c_uint8 * len(nb)).from_buffer_copy(nb)
    ptr = ctypes.cast(buf, ctypes.POINTER(ctypes.c_uint8))
    handle = lib().clausters_midi_output_open(ptr, len(nb))
    if not handle:
        raise OSError(f"could not open MIDI output port {name!r}")
    return handle


def output_send(handle, message) -> None:
    """Send raw MIDI bytes out the port now."""
    b = bytes(message)
    buf = (ctypes.c_uint8 * len(b)).from_buffer_copy(b)
    ptr = ctypes.cast(buf, ctypes.POINTER(ctypes.c_uint8))
    if lib().clausters_midi_output_send(handle, ptr, len(b)) != 0:
        raise RuntimeError("MIDI output send failed")


def output_close(handle) -> None:
    lib().clausters_midi_output_close(handle)


# ---- live input (needs the cdylib built with `--features live`) ----


def input_open(name: str = "clausters-in"):
    """Open a virtual MIDI input port other apps route into; returns an opaque
    handle. Drain it with `input_poll`."""
    _require_live()
    nb = name.encode("utf-8")
    buf = (ctypes.c_uint8 * len(nb)).from_buffer_copy(nb)
    ptr = ctypes.cast(buf, ctypes.POINTER(ctypes.c_uint8))
    handle = lib().clausters_midi_input_open(ptr, len(nb))
    if not handle:
        raise OSError(f"could not open MIDI input port {name!r}")
    return handle


def input_poll(handle) -> bytes | None:
    """Dequeue the next pending input message as ``bytes``, or ``None`` when the
    queue is empty. Poll in a loop to drain everything received since last
    time."""
    buf = (ctypes.c_uint8 * 256)()
    ptr = ctypes.cast(buf, ctypes.POINTER(ctypes.c_uint8))
    out_len = ctypes.c_size_t(0)
    rc = lib().clausters_midi_input_poll(handle, ptr, 256, ctypes.byref(out_len))
    if rc == 1:
        return bytes(buf[: out_len.value])
    if rc == 0:
        return None
    raise RuntimeError(f"MIDI input poll failed ({rc})")


def input_close(handle) -> None:
    lib().clausters_midi_input_close(handle)
