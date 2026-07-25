"""ctypes binding over the shared native core (`clausters-ffi`).

Loads ``libclausters_ffi`` (the C ABI over ``clausters-core``, built with
``cargo build -p clausters-ffi``) and exposes its builtins, seeded white noise
and clock/sample conversions to Python. Because the server's native UGens use
the very same ``clausters-core``, these results match the server by
construction for the operators it computes natively.

Boundary rule (project-wide, same as `clausters.ipc`): only flat
data crosses — Python floats/ints in, `array.array` ``'f'`` (or a plain
float for scalar calls) out. Nothing heavy is imported; a numpy user can wrap
the returned ``array`` without copying.

The library is loaded lazily on first use, so importing this module (and the
package) never fails just because the cdylib has not been built yet.
"""

import ctypes
import json
import os
import threading as _threading
from array import array
from enum import IntEnum

from . import _libpath

CORE_ABI_VERSION = 12

# cdylib file names across platforms (Linux / macOS / Windows).
_FFI_NAMES = ("libclausters_ffi.so", "libclausters_ffi.dylib", "clausters_ffi.dll")


class BinaryOp(IntEnum):
    """Binary operators; discriminants match ``clausters_core::builtins``."""

    ADD = 0
    SUB = 1
    MUL = 2
    DIV = 3
    MOD = 4
    POW = 5
    MIN = 6
    MAX = 7
    ATAN2 = 8
    GT = 9
    LT = 10
    GE = 11
    LE = 12
    EQ = 13
    NE = 14
    AND = 15
    OR = 16
    XOR = 17
    LSH = 18
    RSH = 19
    HYPOT = 20
    RING1 = 21
    RING2 = 22
    RING3 = 23
    RING4 = 24
    SUMSQR = 25
    DIFSQR = 26
    SQRSUM = 27
    SQRDIF = 28
    ABSDIF = 29
    THRESH = 30
    CLIP2 = 31
    EXCESS = 32
    ROUND = 33
    TRUNC = 34
    FOLD2 = 35
    WRAP2 = 36
    GCD = 37
    LCM = 38
    HYPOT_APX = 39


class UnaryOp(IntEnum):
    """Unary operators; discriminants match ``clausters_core::builtins``."""

    NEG = 0
    ABS = 1
    SIN = 2
    COS = 3
    TAN = 4
    ASIN = 5
    ACOS = 6
    ATAN = 7
    EXP = 8
    EXP10 = 9
    LOG = 10
    LOG10 = 11
    SQRT = 12
    FLOOR = 13
    CEIL = 14
    RINT = 15
    INTCAST = 16
    FLOATCAST = 17
    SQUARED = 18
    CUBED = 19
    RECIP = 20
    FRAC = 21
    SIGN = 22
    LOG2 = 23
    SINH = 24
    COSH = 25
    TANH = 26
    MIDICPS = 27
    CPSMIDI = 28
    MIDIRATIO = 29
    RATIOMIDI = 30
    DBAMP = 31
    AMPDB = 32
    OCTCPS = 33
    CPSOCT = 34
    DISTORT = 35
    SOFTCLIP = 36


class Window(IntEnum):
    """Smoothing-window types; values match ``clausters_core::window::Window``
    and the ``wintype`` an ``FFT``/``IFFT`` UGen carries."""

    RECTANGULAR = -1
    HANN = 0
    SINE = 1
    WELCH = 2
    HAMMING = 3
    BLACKMAN = 4


# ---- library loading (lazy, versioned) ----

_LIB = None
_HAS_NOTATION = False
_HAS_ENGRAVER = False


def _find_library() -> str:
    # Precedence (see _libpath): env override, the bundled wheel copy, then the
    # workspace target/ of a source checkout.
    candidates = [os.environ.get("CLAUSTERS_FFI_LIB")]
    candidates += _libpath.bundled_candidates(_FFI_NAMES)
    candidates += _libpath.workspace_candidates(_FFI_NAMES)
    for c in candidates:
        if c and os.path.exists(c):
            return c
    raise OSError(
        "libclausters_ffi not found: install the wheel (it bundles the "
        "library) or, in a source checkout, build it with "
        "`cargo build -p clausters-ffi` (add --release for the release dir) "
        "or point CLAUSTERS_FFI_LIB at it"
    )


def _configure(lib: ctypes.CDLL) -> ctypes.CDLL:
    f32p = ctypes.POINTER(ctypes.c_float)
    lib.clausters_core_abi_version.restype = ctypes.c_uint32
    got = lib.clausters_core_abi_version()
    if got != CORE_ABI_VERSION:
        raise OSError(
            f"libclausters_ffi speaks ABI v{got}, this binding v{CORE_ABI_VERSION}"
        )
    lib.clausters_core_unary.restype = ctypes.c_int32
    lib.clausters_core_unary.argtypes = [ctypes.c_uint32, f32p, ctypes.c_size_t, f32p, ctypes.c_size_t]
    lib.clausters_core_binary.restype = ctypes.c_int32
    lib.clausters_core_binary.argtypes = [
        ctypes.c_uint32, f32p, ctypes.c_size_t, f32p, ctypes.c_size_t, f32p, ctypes.c_size_t,
    ]
    lib.clausters_core_whitenoise.argtypes = [ctypes.c_uint64, f32p, ctypes.c_size_t]
    # Smoothing windows (ABI v4): the same shapes the server's FFT chain applies.
    lib.clausters_core_window.restype = None
    lib.clausters_core_window.argtypes = [ctypes.c_int32, f32p, ctypes.c_size_t]
    lib.clausters_core_beats_to_secs.restype = ctypes.c_double
    lib.clausters_core_beats_to_secs.argtypes = [ctypes.c_double] * 4
    lib.clausters_core_secs_to_beats.restype = ctypes.c_double
    lib.clausters_core_secs_to_beats.argtypes = [ctypes.c_double] * 4
    lib.clausters_core_secs_to_samples.restype = ctypes.c_int64
    lib.clausters_core_secs_to_samples.argtypes = [ctypes.c_double, ctypes.c_double]
    lib.clausters_core_samples_to_secs.restype = ctypes.c_double
    lib.clausters_core_samples_to_secs.argtypes = [ctypes.c_int64, ctypes.c_double]
    lib.clausters_core_unix_to_sample.restype = ctypes.c_int64
    lib.clausters_core_unix_to_sample.argtypes = [
        ctypes.c_double, ctypes.c_double, ctypes.c_int64, ctypes.c_double,
    ]
    # Seam-audit surface (ABI v5): quantization, NTP timetag packing, pitch
    # math, the seeded value stream, the beat queue and the clock-sync model —
    # the value/time logic every client shares instead of reimplementing.
    lib.clausters_core_quant_delay.restype = ctypes.c_double
    lib.clausters_core_quant_delay.argtypes = [ctypes.c_double, ctypes.c_double]
    # Ruler/axis scalars (ABI v9): the bar:beat read of the quant grid and the
    # perceptual frequency scales shared with the GUI spectrogram axis.
    for name in ("bar", "beat_in_bar"):
        fn = getattr(lib, f"clausters_core_{name}")
        fn.restype = ctypes.c_double
        fn.argtypes = [ctypes.c_double, ctypes.c_double]
    for name in ("hz_to_mel", "mel_to_hz", "hz_to_bark", "bark_to_hz"):
        fn = getattr(lib, f"clausters_core_{name}")
        fn.restype = ctypes.c_double
        fn.argtypes = [ctypes.c_double]
    lib.clausters_core_ntp_timetag.restype = ctypes.c_uint64
    lib.clausters_core_ntp_timetag.argtypes = [ctypes.c_double]
    lib.clausters_core_unix_to_ntp.restype = ctypes.c_uint64
    lib.clausters_core_unix_to_ntp.argtypes = [ctypes.c_double]
    lib.clausters_core_degree_to_midinote.restype = ctypes.c_double
    lib.clausters_core_degree_to_midinote.argtypes = [
        ctypes.c_double, ctypes.c_double, ctypes.c_double, f32p, ctypes.c_size_t,
    ]
    u64p = ctypes.POINTER(ctypes.c_uint64)
    lib.clausters_rng_seed.restype = ctypes.c_uint64
    lib.clausters_rng_seed.argtypes = [ctypes.c_uint64]
    lib.clausters_rng_next_f64.restype = ctypes.c_double
    lib.clausters_rng_next_f64.argtypes = [u64p]
    lib.clausters_rng_next_below.restype = ctypes.c_uint64
    lib.clausters_rng_next_below.argtypes = [u64p, ctypes.c_uint64]
    lib.clausters_rng_next_u64.restype = ctypes.c_uint64
    lib.clausters_rng_next_u64.argtypes = [u64p]
    f64p = ctypes.POINTER(ctypes.c_double)
    lib.clausters_sched_new.restype = ctypes.c_void_p
    lib.clausters_sched_free.restype = None
    lib.clausters_sched_free.argtypes = [ctypes.c_void_p]
    lib.clausters_sched_push.restype = None
    lib.clausters_sched_push.argtypes = [ctypes.c_void_p, ctypes.c_double, ctypes.c_uint64]
    lib.clausters_sched_peek_time.restype = ctypes.c_int32
    lib.clausters_sched_peek_time.argtypes = [ctypes.c_void_p, f64p]
    lib.clausters_sched_pop_due.restype = ctypes.c_int32
    lib.clausters_sched_pop_due.argtypes = [ctypes.c_void_p, ctypes.c_double, f64p, u64p]
    lib.clausters_sched_remove.restype = ctypes.c_size_t
    lib.clausters_sched_remove.argtypes = [ctypes.c_void_p, ctypes.c_uint64]
    lib.clausters_sched_len.restype = ctypes.c_size_t
    lib.clausters_sched_len.argtypes = [ctypes.c_void_p]
    lib.clausters_sched_clear.restype = None
    lib.clausters_sched_clear.argtypes = [ctypes.c_void_p]
    lib.clausters_clocksync_new.restype = ctypes.c_void_p
    lib.clausters_clocksync_new.argtypes = [ctypes.c_double, ctypes.c_size_t]
    lib.clausters_clocksync_free.restype = None
    lib.clausters_clocksync_free.argtypes = [ctypes.c_void_p]
    lib.clausters_clocksync_add_anchor.restype = None
    lib.clausters_clocksync_add_anchor.argtypes = [
        ctypes.c_void_p, ctypes.c_double, ctypes.c_int64, ctypes.c_double,
    ]
    lib.clausters_clocksync_sample_at.restype = ctypes.c_int64
    lib.clausters_clocksync_sample_at.argtypes = [ctypes.c_void_p, ctypes.c_double]
    lib.clausters_clocksync_local_time_of.restype = ctypes.c_double
    lib.clausters_clocksync_local_time_of.argtypes = [ctypes.c_void_p, ctypes.c_int64]
    for name in ("drift_ppm", "span", "rate", "slope", "intercept"):
        fn = getattr(lib, f"clausters_clocksync_{name}")
        fn.restype = ctypes.c_double
        fn.argtypes = [ctypes.c_void_p]
    # Peak-pyramid cache builder (ABI v3): the shared analysis the GUI host maps
    # to render a waveform without re-sending samples (the bulk path).
    u8p = ctypes.POINTER(ctypes.c_ubyte)
    lib.clausters_core_peaks_cache_size.restype = ctypes.c_size_t
    lib.clausters_core_peaks_cache_size.argtypes = [ctypes.c_size_t, ctypes.c_size_t]
    lib.clausters_core_peaks_build.restype = ctypes.c_size_t
    lib.clausters_core_peaks_build.argtypes = [f32p, ctypes.c_size_t, ctypes.c_size_t, u8p, ctypes.c_size_t]
    # Multichannel peak-pyramid cache (ABI v8): one cache resource holding every
    # channel of an interleaved buffer, the editor-grade waveform's format.
    lib.clausters_core_peaks_multi_cache_size.restype = ctypes.c_size_t
    lib.clausters_core_peaks_multi_cache_size.argtypes = [
        ctypes.c_size_t, ctypes.c_size_t, ctypes.c_size_t,
    ]
    lib.clausters_core_peaks_multi_build.restype = ctypes.c_size_t
    lib.clausters_core_peaks_multi_build.argtypes = [
        f32p, ctypes.c_size_t, ctypes.c_size_t, ctypes.c_size_t, u8p, ctypes.c_size_t,
    ]
    # Stereo-field measurements (ABI v7): the correlation and Lissajous/goniometer
    # geometry the GUI phasescope reads, shared so a headless capture matches it.
    lib.clausters_core_correlation.restype = ctypes.c_int32
    lib.clausters_core_correlation.argtypes = [f32p, f32p, ctypes.c_size_t, f32p]
    lib.clausters_core_lissajous.restype = ctypes.c_int32
    lib.clausters_core_lissajous.argtypes = [f32p, f32p, ctypes.c_size_t, f32p]
    # The patcher cord->bus pass (ABI v11): a directed patch JSON in, its
    # GraphDef wiring JSON out (size-query then fill, the peaks pattern).
    lib.clausters_core_patch_compile.restype = ctypes.c_size_t
    lib.clausters_core_patch_compile.argtypes = [u8p, ctypes.c_size_t, u8p, ctypes.c_size_t]
    # Finite-resource registry (ABI v10): the one id-allocator model — node
    # ids, buses, buffers — shared with the server's reserved ranges.
    lib.clausters_registry_new.restype = ctypes.c_void_p
    lib.clausters_registry_new.argtypes = [ctypes.c_int64, ctypes.c_uint64]
    lib.clausters_registry_free.restype = None
    lib.clausters_registry_free.argtypes = [ctypes.c_void_p]
    lib.clausters_registry_alloc.restype = ctypes.c_int64
    lib.clausters_registry_alloc.argtypes = [ctypes.c_void_p, ctypes.c_uint64]
    lib.clausters_registry_release.restype = ctypes.c_int32
    lib.clausters_registry_release.argtypes = [ctypes.c_void_p, ctypes.c_int64, ctypes.c_uint64]
    lib.clausters_registry_in_use.restype = ctypes.c_uint64
    lib.clausters_registry_in_use.argtypes = [ctypes.c_void_p]
    lib.clausters_registry_capacity.restype = ctypes.c_uint64
    lib.clausters_registry_capacity.argtypes = [ctypes.c_void_p]
    lib.clausters_registry_contains.restype = ctypes.c_int32
    lib.clausters_registry_contains.argtypes = [ctypes.c_void_p, ctypes.c_int64]
    lib.clausters_registry_clear.restype = None
    lib.clausters_registry_clear.argtypes = [ctypes.c_void_p]
    lib.clausters_registry_node_partition.restype = ctypes.c_int32
    lib.clausters_registry_node_partition.argtypes = [
        ctypes.c_uint64, ctypes.POINTER(ctypes.c_int64),
    ]
    lib.clausters_registry_graph_audio_reserved.restype = ctypes.c_uint64
    lib.clausters_registry_graph_audio_reserved.argtypes = []
    lib.clausters_registry_graph_control_reserved.restype = ctypes.c_uint64
    lib.clausters_registry_graph_control_reserved.argtypes = []
    # WebSocket client transport (ABI v2). A connection is an opaque handle;
    # bytes (with embedded NULs) cross via c_char_p + an explicit length, so OSC
    # packets are passed whole, not NUL-truncated.
    lib.clausters_ws_connect.restype = ctypes.c_void_p
    lib.clausters_ws_connect.argtypes = [ctypes.c_char_p, ctypes.c_uint16, ctypes.c_char_p]
    lib.clausters_ws_send.restype = ctypes.c_int32
    lib.clausters_ws_send.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]
    lib.clausters_ws_recv.restype = ctypes.c_ssize_t
    lib.clausters_ws_recv.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t, ctypes.c_uint32]
    lib.clausters_ws_close.restype = None
    lib.clausters_ws_close.argtypes = [ctypes.c_void_p]
    lib.clausters_ws_last_error.restype = ctypes.c_char_p
    lib.clausters_ws_last_error.argtypes = []
    _configure_notation(lib)
    return lib


def _configure_notation(lib: ctypes.CDLL) -> None:
    """The notation surface (ABI v12), bound only when the library carries it.

    Both of its features are off by default in the crate, and the wheel turns
    them on, so a hand-built cdylib may legitimately lack these symbols. Missing
    ones leave `has_notation`/`has_engraver` false and `clausters.gui.notation`
    raises a readable error; nothing else in the binding is affected.
    """
    global _HAS_NOTATION, _HAS_ENGRAVER
    u8p = ctypes.POINTER(ctypes.c_ubyte)
    size = ctypes.c_size_t
    try:
        # The pure half: the SVG walk and the voice -> MEI encoder.
        lib.clausters_core_svg_to_display_list.restype = size
        lib.clausters_core_svg_to_display_list.argtypes = [u8p, size, u8p, size]
        lib.clausters_core_voice_to_mei.restype = size
        lib.clausters_core_voice_to_mei.argtypes = [u8p, size] * 4 + [u8p, size]
    except AttributeError:
        return
    _HAS_NOTATION = True
    try:
        # The engraver: an opaque score handle, every text return size-then-fill.
        lib.clausters_score_open.restype = ctypes.c_void_p
        lib.clausters_score_open.argtypes = [
            u8p, size, ctypes.c_int32, ctypes.c_int32, u8p, size,
        ]
        lib.clausters_score_free.restype = None
        lib.clausters_score_free.argtypes = [ctypes.c_void_p]
        lib.clausters_score_display_list.restype = size
        lib.clausters_score_display_list.argtypes = [
            ctypes.c_void_p, ctypes.c_int32, u8p, size,
        ]
        lib.clausters_score_mei.restype = size
        lib.clausters_score_mei.argtypes = [ctypes.c_void_p, u8p, size]
        lib.clausters_score_transpose.restype = ctypes.c_int32
        lib.clausters_score_transpose.argtypes = [
            ctypes.c_void_p, u8p, size, ctypes.c_int32,
        ]
        lib.clausters_score_edit.restype = ctypes.c_int32
        lib.clausters_score_edit.argtypes = [ctypes.c_void_p, u8p, size, u8p, size]
        for name in ("undo", "redo", "can_undo", "can_redo"):
            fn = getattr(lib, f"clausters_score_{name}")
            fn.restype = ctypes.c_int32
            fn.argtypes = [ctypes.c_void_p]
    except AttributeError:
        return
    _HAS_ENGRAVER = True


def lib(path: str | None = None) -> ctypes.CDLL:
    """The loaded, version-checked cdylib (cached after the first call)."""
    global _LIB
    if _LIB is None or path is not None:
        _LIB = _configure(ctypes.CDLL(path or _find_library()))
    return _LIB


def abi_version() -> int:
    return lib().clausters_core_abi_version()


def has_notation() -> bool:
    """Whether the loaded library carries the notation layer's pure half (the
    SVG walk and the MEI encoder) -- the crate's `notation` feature."""
    lib()
    return _HAS_NOTATION


def has_engraver() -> bool:
    """Whether it also carries the engraver and the editable score -- the
    crate's `verovio` feature, which links libverovio."""
    lib()
    return _HAS_ENGRAVER


def size_then_fill(fn, *args) -> bytes:
    """Call a size-then-fill entry point the way the ABI expects: once with a
    null buffer to learn the byte count, once to fill it. ``args`` are the
    leading arguments; the ``out``/``out_cap`` pair is appended. Returns the
    bytes, empty when the call reports nothing (its own way of saying no)."""
    need = fn(*args, None, 0)
    if need == 0:
        return b""
    out = (ctypes.c_ubyte * need)()
    n = fn(*args, out, need)
    return bytes(out[:n])


def as_u8(data: bytes):
    """A ``bytes`` as the ``u8*`` pointer the ABI takes, null when empty. The
    cast keeps the copied buffer alive, so the pointer stands on its own."""
    if not data:
        return None
    buf = (ctypes.c_ubyte * len(data)).from_buffer_copy(data)
    return ctypes.cast(buf, ctypes.POINTER(ctypes.c_ubyte))


def compile_patch(patch: dict) -> dict:
    """Compile a **directed patch** into its GraphDef bus wiring via the shared
    `clausters_core::patch` pass — the one door every client's patcher uses, so a
    patch translates identically everywhere.

    ``patch`` is ``{"boxes": [...], "cords": [...]}``: each box a
    ``{"def": name, "ports": [{"name", "dir": "in"|"out", "rate":
    "audio"|"control"}, ...]}``, each cord a ``{"from_box", "from_port",
    "to_box", "to_port"}``. Returns ``{"buses": [{"name", "rate"}, ...],
    "members": [{"box_index", "def", "controls": [{"control", "bus"}, ...]}, ...]}``
    — one private bus per connected net (writers summing), named ``b0``, ``b1``, …
    A signal reaches hardware through a terminal def (a ``dac``), never a drawn bus.

    Raises `ValueError` on a malformed cord (reversed, rate-mismatched, out of
    range) or unserializable input.
    """
    data = json.dumps(patch).encode("utf-8")
    u8p = ctypes.POINTER(ctypes.c_ubyte)
    inp = (ctypes.c_ubyte * len(data)).from_buffer_copy(data) if data else (ctypes.c_ubyte * 0)()
    fn = lib().clausters_core_patch_compile
    need = fn(ctypes.cast(inp, u8p), len(data), None, 0)
    if need == 0:
        raise ValueError("patch is not valid JSON for the compiler")
    out = (ctypes.c_ubyte * need)()
    n = fn(ctypes.cast(inp, u8p), len(data), out, need)
    result = json.loads(bytes(out[:n]).decode("utf-8"))
    if "error" in result:
        raise ValueError(result["error"])
    return result


# ---- flat-data helpers ----


def _as_array(x) -> tuple[array, bool]:
    """A float `array` and whether the input was a bare scalar."""
    if isinstance(x, (int, float)):
        return array("f", (float(x),)), True
    return array("f", (float(v) for v in x)), False


def _ptr(a: array):
    addr, _ = a.buffer_info()
    return ctypes.cast(addr, ctypes.POINTER(ctypes.c_float))


def unary(op: int, x):
    """Applies unary `op` to a scalar or sequence; returns a float for a scalar
    input, else an `array('f')`."""
    a, scalar = _as_array(x)
    n = len(a)
    out = array("f", bytes(4 * n))
    rc = lib().clausters_core_unary(int(op), _ptr(a), len(a), _ptr(out), n)
    if rc != 0:
        raise ValueError(f"clausters_core_unary failed ({rc})")
    return out[0] if scalar else out


def binary(op: int, a, b):
    """Applies binary `op` with the core's broadcast rule (a length-1 operand
    is a constant). Returns a float when both inputs are scalars, else an
    `array('f')` of the broadcast length."""
    av, a_scalar = _as_array(a)
    bv, b_scalar = _as_array(b)
    n = max(len(av), len(bv))
    if len(av) not in (1, n) or len(bv) not in (1, n):
        raise ValueError(f"length mismatch: {len(av)} vs {len(bv)}")
    out = array("f", bytes(4 * n))
    rc = lib().clausters_core_binary(
        int(op), _ptr(av), len(av), _ptr(bv), len(bv), _ptr(out), n
    )
    if rc != 0:
        raise ValueError(f"clausters_core_binary failed ({rc})")
    return out[0] if (a_scalar and b_scalar) else out


def white_noise(seed: int, n: int) -> array:
    """`n` white-noise samples from `seed`, identical to the server's
    `WhiteNoise` UGen seeded the same way."""
    out = array("f", bytes(4 * n))
    lib().clausters_core_whitenoise(ctypes.c_uint64(seed), _ptr(out), n)
    return out


def window(wintype: int, n: int) -> array:
    """`n` samples of smoothing window `wintype` (a `Window` value), **identical**
    to the window the server's `FFT`/`IFFT` applies — so a client that pre-windows
    audio agrees with the server bit for bit. Periodic (DFT-even)."""
    out = array("f", bytes(4 * n))
    lib().clausters_core_window(int(wintype), _ptr(out), n)
    return out


def beats_to_secs(tempo: float, base_beats: float, base_seconds: float, beats: float) -> float:
    return lib().clausters_core_beats_to_secs(tempo, base_beats, base_seconds, beats)


def secs_to_beats(tempo: float, base_beats: float, base_seconds: float, secs: float) -> float:
    return lib().clausters_core_secs_to_beats(tempo, base_beats, base_seconds, secs)


def secs_to_samples(secs: float, sample_rate: float) -> int:
    return lib().clausters_core_secs_to_samples(secs, sample_rate)


def samples_to_secs(samples: int, sample_rate: float) -> float:
    return lib().clausters_core_samples_to_secs(samples, sample_rate)


def unix_to_sample(unix_secs: float, anchor_unix: float, anchor_sample: int, sample_rate: float) -> int:
    return lib().clausters_core_unix_to_sample(unix_secs, anchor_unix, anchor_sample, sample_rate)


def quant_delay(pos: float, quant: float) -> float:
    """Beats to wait so a routine starts on the next ``quant`` boundary of a
    grid currently at ``pos`` beats (``quant <= 0`` -> now) — the shared
    quantization rule every client applies."""
    return lib().clausters_core_quant_delay(float(pos), float(quant))


def bar(beats: float, quant: float) -> float:
    """The bar index ``beats`` falls in on a grid of ``quant`` beats per bar
    (0-based; ``quant <= 0`` -> 0, no bar grid) — the display complement of
    `quant_delay` for reading a position off the grid."""
    return lib().clausters_core_bar(float(beats), float(quant))


def beat_in_bar(beats: float, quant: float) -> float:
    """The beat within its bar for ``beats`` on a grid of ``quant`` beats per
    bar (0-based, in ``[0, quant)``; ``quant <= 0`` returns ``beats``)."""
    return lib().clausters_core_beat_in_bar(float(beats), float(quant))


def hz_to_mel(hz: float) -> float:
    """Hertz -> mel (O'Shaughnessy), the perceptual frequency scale the GUI
    spectrogram axis shares."""
    return lib().clausters_core_hz_to_mel(float(hz))


def mel_to_hz(mel: float) -> float:
    """Mel -> hertz, the exact inverse of `hz_to_mel`."""
    return lib().clausters_core_mel_to_hz(float(mel))


def hz_to_bark(hz: float) -> float:
    """Hertz -> bark (Traunmüller closed form; -0.53 at 0 Hz, the axis
    floor a display normalizes against)."""
    return lib().clausters_core_hz_to_bark(float(hz))


def bark_to_hz(bark: float) -> float:
    """Bark -> hertz, the analytic inverse of `hz_to_bark`."""
    return lib().clausters_core_bark_to_hz(float(bark))


def ntp_timetag(ntp_secs: float) -> int:
    """Raw NTP-scale seconds (any epoch) packed into the 64 timetag bits
    (``seconds << 32 | fractional``, fraction **rounded**) — the one packing
    rule shared with the core, so identical instants give identical bytes."""
    return lib().clausters_core_ntp_timetag(float(ntp_secs))


def unix_to_ntp(unix_secs: float) -> int:
    """A Unix timestamp packed into the 64 NTP timetag bits (adds the
    1900->1970 offset, then packs like `ntp_timetag`)."""
    return lib().clausters_core_unix_to_ntp(float(unix_secs))


def degree_to_midinote(degree: float, octave: float, root: float, scale) -> float:
    """Scale-degree -> MIDI note number in the ``octave``/``root`` pitch space,
    wrapping degrees past the scale length with octave carry (floored division,
    sclang semantics) — computed in the shared core so every client's `Event`
    resolves pitch identically."""
    a, _ = _as_array(scale)
    return lib().clausters_core_degree_to_midinote(
        float(degree), float(octave), float(root), _ptr(a), len(a)
    )


# ---- seeded value stream (the sequencing layer's RNG) ----


def rng_seed(seed: int) -> int:
    """The initial state word for ``seed`` (splitmix64-mixed, never zero) —
    the same seeding as the server's ``WhiteNoise``. Hold the returned state
    and pass it to `rng_next_f64` / `rng_next_below`."""
    return lib().clausters_rng_seed(ctypes.c_uint64(seed).value)


class RngStream:
    """A resumable seeded value stream over the core generator: uniform
    ``f64`` in [0, 1) and bounded integers. The state is one ``u64`` word (flat
    data), so the same seed replays the same values in every client language —
    what the random patterns and the context RNG run on.

    Draws are serialized by a lock (ctypes releases the GIL during the call, so
    a stream shared across threads — e.g. the ``main.rng`` fallback — must not
    interleave state updates). `spawn` derives a child stream deterministically,
    the sclang-style inheritance a routine's generator is built from."""

    def __init__(self, seed: int):
        self._state = ctypes.c_uint64(lib().clausters_rng_seed(ctypes.c_uint64(seed).value))
        self._lock = _threading.Lock()

    def next_f64(self) -> float:
        """Uniform in [0, 1) with 53-bit resolution."""
        with self._lock:
            return lib().clausters_rng_next_f64(ctypes.byref(self._state))

    def uniform(self, lo: float, hi: float) -> float:
        """Uniform in [lo, hi) (degenerate to ``lo`` when ``hi <= lo``)."""
        return lo + self.next_f64() * max(hi - lo, 0.0)

    def next_below(self, n: int) -> int:
        """Uniform integer in [0, n) (0 when ``n`` is 0)."""
        with self._lock:
            return lib().clausters_rng_next_below(ctypes.byref(self._state), n)

    def next_u64(self) -> int:
        """The full-width random word (advances the stream one step)."""
        with self._lock:
            return lib().clausters_rng_next_u64(ctypes.byref(self._state))

    def choice(self, items):
        """A uniformly chosen element of ``items``."""
        return items[self.next_below(len(items))]

    def spawn(self) -> "RngStream":
        """A child stream seeded from this one's next word — deterministic
        derivation, so seeding a root context reproduces every stream created
        under it, in creation order."""
        return RngStream(self.next_u64())


# ---- beat-ordered scheduler queue ----


class Scheduler:
    """The core's beat-ordered event queue (min time first, insertion order for
    ties) behind a `TempoClock`. Only flat data crosses: beats and ``u64`` ids —
    the clock maps ids back to its routines. Free with `close` (``__del__`` is
    the backstop)."""

    def __init__(self):
        self._lib = lib()
        self._handle = self._lib.clausters_sched_new()

    def push(self, time: float, id_: int):
        self._lib.clausters_sched_push(self._handle, float(time), id_)

    def peek_time(self):
        """The earliest queued beat, or ``None`` when empty."""
        t = ctypes.c_double()
        if self._lib.clausters_sched_peek_time(self._handle, ctypes.byref(t)) != 0:
            return None
        return t.value

    def pop_due(self, now: float):
        """The earliest ``(time, id)`` with ``time <= now``, or ``None``."""
        t, i = ctypes.c_double(), ctypes.c_uint64()
        rc = self._lib.clausters_sched_pop_due(
            self._handle, float(now), ctypes.byref(t), ctypes.byref(i)
        )
        if rc != 0:
            return None
        return t.value, i.value

    def remove(self, id_: int) -> int:
        """Removes every queued entry with ``id_``; returns how many."""
        return self._lib.clausters_sched_remove(self._handle, id_)

    def __len__(self):
        return self._lib.clausters_sched_len(self._handle)

    def clear(self):
        self._lib.clausters_sched_clear(self._handle)

    def close(self):
        handle = getattr(self, "_handle", None)
        if handle:
            self._lib.clausters_sched_free(handle)
            self._handle = None

    def __del__(self):
        try:
            self.close()
        except Exception:
            pass


# ---- finite-resource registry ----


class Registry:
    """The core's finite-resource id registry over ``[base, base + capacity)``.

    Node ids, buses and buffers are finite server resources fixed at boot; a
    registry is the occupancy map of one such space. Every `release` makes the
    ids allocatable again, exhaustion is an explicit ``None`` (never a wrapped
    counter), and a bad release reports instead of corrupting the map. The
    handle is internally locked, so the clock thread can allocate while the
    reply thread releases on ``/n_end``. ``capacity=None`` builds the
    **unbounded** registry (NRT/score rendering: allocation never fails).
    Free with `close` (``__del__`` is the backstop)."""

    #: `release` refusal codes (mirror the C surface).
    OUT_OF_RANGE = -1
    NOT_ALLOCATED = -2

    def __init__(self, base: int, capacity: "int | None"):
        if capacity is not None and capacity <= 0:
            raise ValueError(f"registry capacity must be positive, got {capacity}")
        self._lib = lib()
        self._handle = self._lib.clausters_registry_new(
            int(base), 0 if capacity is None else int(capacity))

    def alloc(self, width: int = 1) -> "int | None":
        """First id of a run of ``width`` contiguous ids, or ``None`` when the
        space is exhausted."""
        first = self._lib.clausters_registry_alloc(self._handle, max(1, int(width)))
        return None if first == -1 else first

    def release(self, first: int, width: int = 1) -> int:
        """Returns a run to the pool. ``0`` on success, `OUT_OF_RANGE` or
        `NOT_ALLOCATED` on refusal (nothing released)."""
        return self._lib.clausters_registry_release(
            self._handle, int(first), max(1, int(width)))

    def contains(self, id_: int) -> bool:
        """Whether ``id_`` falls in this registry's space — the filter for
        foreign ``/n_end`` ids."""
        return bool(self._lib.clausters_registry_contains(self._handle, int(id_)))

    @property
    def in_use(self) -> int:
        return self._lib.clausters_registry_in_use(self._handle)

    @property
    def capacity(self) -> "int | None":
        """The size of the id space; ``None`` when unbounded."""
        cap = self._lib.clausters_registry_capacity(self._handle)
        return None if cap == 0 else cap

    def clear(self):
        """Releases everything back to the pool (a client reset)."""
        self._lib.clausters_registry_clear(self._handle)

    def close(self):
        handle = getattr(self, "_handle", None)
        if handle:
            self._lib.clausters_registry_free(handle)
            self._handle = None

    def __del__(self):
        try:
            self.close()
        except Exception:
            pass


def node_id_partition(max_nodes: int) -> dict:
    """The boot-derived partition of the node-id space for a node table of
    ``max_nodes`` slots — the same formula the server applies, so the two
    agree by construction. Returns the keys ``client_base``,
    ``client_capacity``, ``auto_base``, ``auto_capacity``, ``midi_base``,
    ``midi_capacity``."""
    out = (ctypes.c_int64 * 6)()
    if lib().clausters_registry_node_partition(max(1, int(max_nodes)), out) != 0:
        raise RuntimeError("node_id_partition failed")
    keys = ("client_base", "client_capacity", "auto_base",
            "auto_capacity", "midi_base", "midi_capacity")
    return dict(zip(keys, out))


def graph_bus_reserved() -> tuple[int, int]:
    """The ``(audio, control)`` bus widths GraphDef instances reserve at the
    top of each bus space (before clamping to a smaller configured count)."""
    l = lib()
    return (l.clausters_registry_graph_audio_reserved(),
            l.clausters_registry_graph_control_reserved())


# ---- sample-clock tracking model ----


class ClockSyncModel:
    """The core's least-squares sample-clock model (``sample = a + b*t`` over a
    sliding anchor window). The transport that produces anchors stays in the
    host language; the fit lives in the core so every client predicts the same
    sample from the same anchors. Free with `close`."""

    def __init__(self, nominal_rate: float = 48_000.0, window: int = 64):
        self._lib = lib()
        self._handle = self._lib.clausters_clocksync_new(float(nominal_rate), int(window))

    def add_anchor(self, t_local: float, sample: int, rate: float = 0.0):
        """Adds an anchor pair and refits; a positive ``rate`` updates the
        nominal rate (0 keeps it)."""
        self._lib.clausters_clocksync_add_anchor(
            self._handle, float(t_local), int(sample), float(rate)
        )

    def sample_at(self, t_local: float) -> int:
        return self._lib.clausters_clocksync_sample_at(self._handle, float(t_local))

    def local_time_of(self, sample: int) -> float:
        return self._lib.clausters_clocksync_local_time_of(self._handle, int(sample))

    def drift_ppm(self) -> float:
        return self._lib.clausters_clocksync_drift_ppm(self._handle)

    def span(self) -> float:
        return self._lib.clausters_clocksync_span(self._handle)

    @property
    def rate(self) -> float:
        return self._lib.clausters_clocksync_rate(self._handle)

    @property
    def a(self) -> float:
        """Fitted intercept (samples at local time 0)."""
        return self._lib.clausters_clocksync_intercept(self._handle)

    @property
    def b(self) -> float:
        """Fitted slope (samples per local second)."""
        return self._lib.clausters_clocksync_slope(self._handle)

    def close(self):
        handle = getattr(self, "_handle", None)
        if handle:
            self._lib.clausters_clocksync_free(handle)
            self._handle = None

    def __del__(self):
        try:
            self.close()
        except Exception:
            pass


def peaks_cache(samples, base_bucket: int = 256, channels: int = 1) -> bytes:
    """The min/max peak-pyramid cache for `samples`, built by the shared
    native core so it is **byte-identical** to one the GUI host (or the server)
    builds. These bytes are the GUI's mmap-able waveform overview: write them to
    a file a ``waveform(cache=...)`` maps, so a multi-megabyte buffer never rides
    OSC. `base_bucket` is the level-0 bucket size (default 256).

    With ``channels > 1`` the samples are interleaved frames and the result is
    the **multichannel** cache — one resource holding a pyramid per channel, the
    format the editor-grade waveform draws as stacked lanes."""
    a, _ = _as_array(samples)
    n = len(a)
    if channels > 1:
        frames = n // channels
        size = lib().clausters_core_peaks_multi_cache_size(frames, channels, base_bucket)
        if size == 0:
            raise ValueError(
                "clausters_core_peaks_multi_cache_size returned 0 (base_bucket must be > 0)")
        out = (ctypes.c_ubyte * size)()
        written = lib().clausters_core_peaks_multi_build(
            _ptr(a), n, channels, base_bucket, out, size)
        if written != size:
            raise ValueError(f"clausters_core_peaks_multi_build wrote {written} of {size} bytes")
        return bytes(out)
    size = lib().clausters_core_peaks_cache_size(n, base_bucket)
    if size == 0:
        raise ValueError("clausters_core_peaks_cache_size returned 0 (base_bucket must be > 0)")
    out = (ctypes.c_ubyte * size)()
    written = lib().clausters_core_peaks_build(_ptr(a), n, base_bucket, out, size)
    if written != size:
        raise ValueError(f"clausters_core_peaks_build wrote {written} of {size} bytes")
    return bytes(out)


def correlation(left, right) -> float | None:
    """The stereo **correlation** (Pearson's r) of two equal-length channels,
    in ``[-1, 1]``: ``+1`` mono/in-phase, ``0`` decorrelated, ``-1`` anti-phase
    (the mix cancels in mono) — the same measurement the GUI phasescope shows,
    computed by the shared native core so a headless analysis reads the identical
    number. Returns ``None`` when it is undefined: the inputs are empty or a
    channel is constant (silence or pure DC)."""
    a, _ = _as_array(left)
    b, _ = _as_array(right)
    if len(a) != len(b):
        raise ValueError(f"channels differ in length: {len(a)} vs {len(b)}")
    out = array("f", (0.0,))
    rc = lib().clausters_core_correlation(_ptr(a), _ptr(b), len(a), _ptr(out))
    return None if rc != 0 else out[0]


def lissajous(left, right) -> list[tuple[float, float]]:
    """The **Lissajous / goniometer** coordinates of stereo pairs ``(left,
    right)``: each pair maps to ``(x, y)`` where ``x`` is the side component
    ``(L - R)/√2`` and ``y`` the mid ``(L + R)/√2`` — the 45°-rotated stereo
    plane a goniometer draws (mono reads vertical, anti-phase horizontal). The
    geometry lives once in the shared core; useful for plotting or driving a
    stereo image in electroacoustic work, not only for the GUI phasescope.
    Returns a list of ``(x, y)`` tuples, one per input pair."""
    a, _ = _as_array(left)
    b, _ = _as_array(right)
    if len(a) != len(b):
        raise ValueError(f"channels differ in length: {len(a)} vs {len(b)}")
    n = len(a)
    out = array("f", bytes(4 * 2 * n))
    rc = lib().clausters_core_lissajous(_ptr(a), _ptr(b), n, _ptr(out))
    if rc != 0:
        raise ValueError(f"clausters_core_lissajous failed ({rc})")
    return [(out[2 * i], out[2 * i + 1]) for i in range(n)]


# ---- WebSocket client transport ----


def _ws_error(handle_lib) -> str:
    p = handle_lib.clausters_ws_last_error()
    return p.decode(errors="replace") if p else ""


class WsClient:
    """A WebSocket client connection backed by the native core (clausters-ffi,
    ``tungstenite``) — the **same** WebSocket implementation the server's
    ``--ws`` listener uses, reached by ctypes like the shm/embed handles. OSC
    packets cross as whole binary messages; the handshake and framing live in
    Rust, not here, so there is no second implementation to maintain.

    `recv` returns the bytes of one packet, or ``None`` on timeout. The handle is
    freed by `close` (and by ``__del__`` as a backstop)."""

    def __init__(self, host: str = "127.0.0.1", port: int = 57120, path: str = "/"):
        self._lib = lib()
        handle = self._lib.clausters_ws_connect(host.encode(), port, path.encode())
        if not handle:
            raise ConnectionError(_ws_error(self._lib) or "WebSocket connect failed")
        self._handle = handle
        self._buf = ctypes.create_string_buffer(65536)

    def send(self, data: bytes) -> None:
        rc = self._lib.clausters_ws_send(self._handle, data, len(data))
        if rc != 0:
            raise ConnectionError(_ws_error(self._lib) or f"WebSocket send failed ({rc})")

    def recv(self, timeout: float) -> bytes | None:
        ms = max(1, int(timeout * 1000))
        n = self._lib.clausters_ws_recv(self._handle, self._buf, len(self._buf), ms)
        if n > 0:
            return bytes(self._buf.raw[:n])
        if n == -2:
            raise ConnectionError(_ws_error(self._lib) or "WebSocket closed")
        return None  # 0 = timeout (or -3 oversize, impossible at 64 KiB for OSC)

    def close(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle:
            self._lib.clausters_ws_close(handle)
            self._handle = None

    def __del__(self):
        try:
            self.close()
        except Exception:
            pass
