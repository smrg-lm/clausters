"""ctypes binding over the MIDI file core (`clausters-midi`).

Loads ``libclausters_midi`` (the C ABI over the SMF writer and, with the
``live`` feature, the virtual MIDI ports) and exposes `write_smf`: turn a list
of ``(tick, message_bytes)`` channel-voice events into Standard MIDI File bytes.
The wheel bundles the library **built with ``live``**, so an installed package
plays and records MIDI with nothing else present; a source checkout falls back
to ``target/``, where a plain ``cargo build -p clausters-midi`` leaves a
library without the ports (rebuild it with ``--features live``).

Boundary rule (same as `clausters._native`): only flat data crosses -- ints
and byte buffers in, ``bytes`` out. The library is loaded lazily and version
checked on first use, so importing this module never fails just because the
cdylib has not been built yet.
"""

import ctypes
import json
import os
from array import array

from . import _libpath

MIDI_ABI_VERSION = 4

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
    lib.clausters_midi_write_smf_tempo.restype = u8p
    lib.clausters_midi_write_smf_tempo.argtypes = [
        u32p, u8p, ctypes.c_size_t, ctypes.c_uint16, u32p, u32p, ctypes.c_size_t,
        ctypes.POINTER(ctypes.c_size_t),
    ]
    lib.clausters_midi_read_smf.restype = u8p
    lib.clausters_midi_read_smf.argtypes = [u8p, ctypes.c_size_t, ctypes.POINTER(ctypes.c_size_t)]
    # MPE: the output side's channel assigner and the messages a zone and a
    # note's expression are written as.
    lib.clausters_mpe_assigner_new.restype = ctypes.c_void_p
    lib.clausters_mpe_assigner_new.argtypes = [ctypes.c_int32, ctypes.c_int32]
    lib.clausters_mpe_assigner_free.argtypes = [ctypes.c_void_p]
    lib.clausters_mpe_assigner_note_on.restype = ctypes.c_int32
    lib.clausters_mpe_assigner_note_on.argtypes = [ctypes.c_void_p, ctypes.c_uint8]
    lib.clausters_mpe_assigner_note_off.argtypes = [ctypes.c_void_p, ctypes.c_uint8, ctypes.c_uint8]
    lib.clausters_mpe_zone_messages.restype = ctypes.c_int32
    lib.clausters_mpe_zone_messages.argtypes = [ctypes.c_int32, ctypes.c_int32, u8p, ctypes.c_size_t]
    lib.clausters_mpe_expression_messages.restype = ctypes.c_int32
    lib.clausters_mpe_expression_messages.argtypes = [
        ctypes.c_uint8, ctypes.c_float, ctypes.c_float, ctypes.c_float, ctypes.c_float,
        ctypes.c_uint8, u8p, ctypes.c_size_t,
    ]
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


def _taken(ptr, out_len) -> bytes:
    """The bytes of a buffer the library allocated, freed once read."""
    if not ptr:
        raise RuntimeError("MIDI call returned null")
    try:
        return bytes(ctypes.cast(ptr, ctypes.POINTER(ctypes.c_uint8 * out_len.value)).contents)
    finally:
        lib().clausters_midi_free(ptr, out_len.value)


def write_smf_tempo(events, ppq: int, tempo) -> bytes:
    """Standard MIDI File bytes from timed channel-voice events and the file's
    tempo: ``(tick, microseconds per quarter note)`` marks."""
    events, tempo = list(events), list(tempo)
    ticks = array("I", (int(t) & 0xFFFFFFFF for t, _ in events))
    msgs = bytearray(3 * len(events))
    for i, (_, message) in enumerate(events):
        b = bytes(message)[:3]
        msgs[3 * i : 3 * i + len(b)] = b
    t_ticks = array("I", (int(t) for t, _ in tempo))
    t_micros = array("I", (int(m) for _, m in tempo))
    u32p = ctypes.POINTER(ctypes.c_uint32)
    u8p = ctypes.POINTER(ctypes.c_uint8)

    def ptr32(a):
        return ctypes.cast(a.buffer_info()[0], u32p) if len(a) else None

    msgs_ptr = (ctypes.cast((ctypes.c_uint8 * len(msgs)).from_buffer(msgs), u8p)
                if msgs else None)
    out_len = ctypes.c_size_t(0)
    ptr = lib().clausters_midi_write_smf_tempo(ptr32(ticks), msgs_ptr, len(events), int(ppq),
                                               ptr32(t_ticks), ptr32(t_micros), len(tempo),
                                               ctypes.byref(out_len))
    return _taken(ptr, out_len)


def read_smf(data: bytes) -> dict:
    """A Standard MIDI File as plain data: ``{"ppq", "events": [[tick,
    [bytes]]], "tempo": [[tick, micros]]}``. `ValueError` for bytes that are
    not one."""
    data = bytes(data)
    u8p = ctypes.POINTER(ctypes.c_uint8)
    buf = (ctypes.c_uint8 * len(data)).from_buffer_copy(data) if data else None
    out_len = ctypes.c_size_t(0)
    ptr = lib().clausters_midi_read_smf(ctypes.cast(buf, u8p) if buf else None, len(data),
                                        ctypes.byref(out_len))
    answer = json.loads(_taken(ptr, out_len))
    if "error" in answer:
        raise ValueError(answer["error"])
    return answer


def write_clip(events, ppq: int) -> bytes:
    """MIDI 2.0 Clip File (SMF2CLIP) bytes -- note velocities at 16-bit
    resolution -- from timed channel-voice events."""
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


# ---- MPE ----


def _messages(fill) -> list:
    """Three-byte messages a library call writes into a buffer."""
    buf = (ctypes.c_uint8 * 64)()
    n = fill(ctypes.cast(buf, ctypes.POINTER(ctypes.c_uint8)), 64)
    if n < 0:
        raise RuntimeError("MPE message buffer too small")
    return [bytes(buf[i : i + 3]) for i in range(0, n, 3)]


def zone_messages(members: int, upper: bool = False) -> list:
    """The messages that declare an MPE zone of ``members`` (the lower zone,
    master channel 0, unless ``upper``): RPN 6 on its master, then the null
    RPN."""
    return _messages(lambda out, cap: lib().clausters_mpe_zone_messages(
        int(upper), int(members), out, cap))


def expression_messages(channel: int, bend: float, bend_range: float,
                        pressure: float | None, timbre: float | None,
                        timbre_cc: int = 74) -> list:
    """A note's starting expression on its member ``channel``, sent before its
    note-on: the bend in semitones through ``bend_range``, the pressure and the
    timbre (0..1, ``None`` for one the note does not state, which goes back to
    its rest)."""
    return _messages(lambda out, cap: lib().clausters_mpe_expression_messages(
        int(channel), float(bend), float(bend_range),
        -1.0 if pressure is None else float(pressure),
        -1.0 if timbre is None else float(timbre), int(timbre_cc), out, cap))


class MpeAssigner:
    """Which member channel an outgoing note goes on: round robin over a
    zone's members, preferring a channel with nothing sounding, reusing the
    one held longest when all are busy (the library's rule)."""

    def __init__(self, members: int, upper: bool = False):
        self._handle = lib().clausters_mpe_assigner_new(int(upper), int(members))

    def note_on(self, key: int) -> int | None:
        channel = lib().clausters_mpe_assigner_note_on(self._handle, int(key) & 0x7F)
        return None if channel < 0 else channel

    def note_off(self, channel: int, key: int) -> None:
        lib().clausters_mpe_assigner_note_off(self._handle, int(channel), int(key) & 0x7F)

    def __del__(self):
        handle, self._handle = getattr(self, "_handle", None), None
        if handle:
            lib().clausters_mpe_assigner_free(handle)

