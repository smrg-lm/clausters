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

CORE_ABI_VERSION = 21

# cdylib file names across platforms (Linux / macOS / Windows).
_FFI_NAMES = ("libclausters_ffi.so", "libclausters_ffi.dylib", "clausters_ffi.dll")


class ShmShape(ctypes.Structure):
    """Where everything is in a mapped segment, as the shared core reports it.

    Mirrors ``clausters_core::shm::Shape`` — and mirroring *this* one struct is
    the whole point: every other number a reader needs is derived from it, so a
    binding carries one declaration instead of a transcription of the layout.
    """

    _fields_ = [
        ("control_buses", ctypes.c_uint64),
        ("audio_buses", ctypes.c_uint64),
        ("taps", ctypes.c_uint64),
        ("tap_frames", ctypes.c_uint64),
        ("buffer_rows", ctypes.c_uint64),
        ("controls_offset", ctypes.c_uint64),
        ("buses_offset", ctypes.c_uint64),
        ("taps_offset", ctypes.c_uint64),
        ("buffers_offset", ctypes.c_uint64),
        ("sample_rate_offset", ctypes.c_uint64),
        ("clock_offset", ctypes.c_uint64),
        ("transport_clock_offset", ctypes.c_uint64),
        ("transport_position_offset", ctypes.c_uint64),
        ("buffer_row_size", ctypes.c_uint64),
        ("tap_slot_size", ctypes.c_uint64),
        ("c2s_offset", ctypes.c_uint64),
        ("s2c_offset", ctypes.c_uint64),
        ("ring_capacity", ctypes.c_uint64),
        ("ring_prefix", ctypes.c_uint64),
        ("frame_header", ctypes.c_uint64),
    ]


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
    f64p_early = ctypes.POINTER(ctypes.c_double)
    lib.clausters_core_stats.argtypes = [
        ctypes.POINTER(ctypes.c_float), ctypes.c_size_t, ctypes.c_size_t,
        ctypes.c_size_t, ctypes.POINTER(ctypes.c_float),
    ]
    lib.clausters_core_stats.restype = ctypes.c_int
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
    # The shared-memory segment (ABI v21): a peer maps the file itself and asks
    # here for every offset, for the directory's seqlock and for the ring
    # framing — the numbers this binding used to transcribe.
    u64p = ctypes.POINTER(ctypes.c_uint64)
    u8p = ctypes.POINTER(ctypes.c_uint8)
    lib.clausters_core_shm_abi_version.restype = ctypes.c_uint32
    lib.clausters_core_shm_abi_version.argtypes = []
    lib.clausters_core_shm_shape.restype = ctypes.c_int32
    lib.clausters_core_shm_shape.argtypes = [
        ctypes.c_void_p, ctypes.c_size_t, ctypes.POINTER(ShmShape),
    ]
    lib.clausters_core_shm_segment_size.restype = ctypes.c_size_t
    lib.clausters_core_shm_segment_size.argtypes = [ctypes.c_size_t] * 4
    lib.clausters_core_shm_init.restype = ctypes.c_int32
    lib.clausters_core_shm_init.argtypes = [
        ctypes.c_void_p, ctypes.c_size_t, ctypes.c_size_t, ctypes.c_size_t,
        ctypes.c_size_t,
    ]
    lib.clausters_core_shm_buffer_info.restype = ctypes.c_int32
    lib.clausters_core_shm_buffer_info.argtypes = [
        ctypes.c_void_p, ctypes.c_size_t, ctypes.c_size_t,
        u64p, u64p, u64p, f64p_early,
    ]
    lib.clausters_core_shm_region_suffix.restype = ctypes.c_int32
    lib.clausters_core_shm_region_suffix.argtypes = [
        ctypes.c_size_t, ctypes.c_uint64, u8p, ctypes.c_size_t,
    ]
    lib.clausters_core_shm_push.restype = ctypes.c_int32
    lib.clausters_core_shm_push.argtypes = [
        ctypes.c_void_p, ctypes.c_size_t, ctypes.c_uint32, ctypes.c_uint32,
        u8p, ctypes.c_size_t,
    ]
    lib.clausters_core_shm_pop.restype = ctypes.c_int32
    lib.clausters_core_shm_pop.argtypes = [
        ctypes.c_void_p, ctypes.c_size_t, ctypes.c_uint32, u8p, ctypes.c_size_t,
        ctypes.POINTER(ctypes.c_uint32), ctypes.POINTER(ctypes.c_size_t),
    ]
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
    lib.clausters_core_peaks_multi_update.restype = ctypes.c_size_t
    lib.clausters_core_peaks_multi_update.argtypes = [
        ctypes.POINTER(ctypes.c_ubyte),
        ctypes.c_size_t,
        ctypes.POINTER(ctypes.c_float),
        ctypes.c_size_t,
        ctypes.c_size_t,
        ctypes.c_size_t,
    ]
    lib.clausters_core_peaks_multi_build.restype = ctypes.c_size_t
    lib.clausters_core_peaks_multi_build.argtypes = [
        f32p, ctypes.c_size_t, ctypes.c_size_t, ctypes.c_size_t, u8p, ctypes.c_size_t,
    ]
    lib.clausters_core_peaks_multi_empty.restype = ctypes.c_size_t
    lib.clausters_core_peaks_multi_empty.argtypes = [
        ctypes.c_size_t, ctypes.c_size_t, ctypes.c_size_t, u8p, ctypes.c_size_t,
    ]
    # The receiving half of /buffer_stream (ABI v21): buckets the writer
    # measured, folded into a cache at an offset -- for whoever hears about a
    # recording rather than mapping the memory it fills.
    lib.clausters_core_peaks_multi_write_buckets.restype = ctypes.c_size_t
    lib.clausters_core_peaks_multi_write_buckets.argtypes = [
        ctypes.POINTER(ctypes.c_ubyte),
        ctypes.c_size_t,
        ctypes.c_size_t,
        ctypes.c_size_t,
        ctypes.POINTER(ctypes.c_float),
        ctypes.c_size_t,
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
    # The document (ABI v18): a **handle**. The tree stays in Rust and only the
    # intent and the outcome cross, so an edit costs the edit -- the by-value
    # binding this replaces was linear in the whole composition (205 ms for one
    # placement on 10240 events). One implementation of what an edit *means*,
    # bound rather than re-derived.
    lib.clausters_document_open.restype = ctypes.c_void_p
    lib.clausters_document_open.argtypes = [u8p, ctypes.c_size_t]
    lib.clausters_document_free.restype = None
    lib.clausters_document_free.argtypes = [ctypes.c_void_p]
    lib.clausters_document_version.restype = ctypes.c_uint64
    lib.clausters_document_version.argtypes = [ctypes.c_void_p]
    lib.clausters_document_snapshot.restype = ctypes.c_size_t
    lib.clausters_document_snapshot.argtypes = [ctypes.c_void_p, u8p, ctypes.c_size_t]
    lib.clausters_document_apply.restype = ctypes.c_size_t
    lib.clausters_document_apply.argtypes = [
        ctypes.c_void_p, u8p, ctypes.c_size_t, u8p, ctypes.c_size_t,
        ctypes.c_double, u8p, ctypes.c_size_t,
    ]
    lib.clausters_document_resolve.restype = ctypes.c_size_t
    lib.clausters_document_resolve.argtypes = [
        ctypes.c_void_p, u8p, ctypes.c_size_t, ctypes.c_double,
        ctypes.c_int32, u8p, ctypes.c_size_t,
    ]
    # The undo log (ABI v16): a handle too, for its own reason -- a bulk inverse
    # leaves the log on purpose, so a by-value log would carry every spilled
    # span on every call, the cost spilling exists to avoid.
    lib.clausters_log_new.restype = ctypes.c_void_p
    lib.clausters_log_new.argtypes = [ctypes.c_size_t, ctypes.c_size_t]
    lib.clausters_log_free.restype = None
    lib.clausters_log_free.argtypes = [ctypes.c_void_p]
    lib.clausters_log_apply.restype = ctypes.c_size_t
    lib.clausters_log_apply.argtypes = [
        ctypes.c_void_p, ctypes.c_void_p, u8p, ctypes.c_size_t,
        u8p, ctypes.c_size_t, ctypes.c_double, u8p, ctypes.c_size_t,
        u8p, ctypes.c_size_t,
    ]
    lib.clausters_log_record.restype = ctypes.c_int32
    lib.clausters_log_record.argtypes = [
        ctypes.c_void_p, u8p, ctypes.c_size_t, u8p, ctypes.c_size_t,
        u8p, ctypes.c_size_t, ctypes.c_int32,
    ]
    for _log_fn in ("clausters_log_undo", "clausters_log_redo"):
        getattr(lib, _log_fn).restype = ctypes.c_size_t
        getattr(lib, _log_fn).argtypes = [
            ctypes.c_void_p, ctypes.c_void_p, u8p, ctypes.c_size_t,
        ]
    for _log_fn in ("clausters_log_can_undo", "clausters_log_can_redo"):
        getattr(lib, _log_fn).restype = ctypes.c_int32
        getattr(lib, _log_fn).argtypes = [ctypes.c_void_p]
    for _log_fn in ("clausters_log_undo_label", "clausters_log_redo_label"):
        getattr(lib, _log_fn).restype = ctypes.c_size_t
        getattr(lib, _log_fn).argtypes = [ctypes.c_void_p, u8p, ctypes.c_size_t]
    lib.clausters_log_len.restype = ctypes.c_size_t
    lib.clausters_log_len.argtypes = [ctypes.c_void_p]
    lib.clausters_log_clear.restype = None
    lib.clausters_log_clear.argtypes = [ctypes.c_void_p]
    # The component bundle (ABI v13): what an instance needs allocated, one
    # instance resolved, and the writers' pre-flight — the same three the
    # browser gets over wasm, on the same JSON-in/JSON-out shape.
    for _bundle_fn in (
        "clausters_core_bundle_requirements",
        "clausters_core_bundle_resolve",
        "clausters_core_bundle_validate",
    ):
        getattr(lib, _bundle_fn).restype = ctypes.c_size_t
        getattr(lib, _bundle_fn).argtypes = [u8p, ctypes.c_size_t, u8p, ctypes.c_size_t]
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
        lib.clausters_score_transpose_to.restype = ctypes.c_int32
        lib.clausters_score_transpose_to.argtypes = [
            ctypes.c_void_p, u8p, size, ctypes.c_int32, ctypes.c_int32,
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
    return ctypes.string_at(out, n)


def as_u8(data: bytes):
    """A ``bytes`` as the ``u8*`` pointer the ABI takes, null when empty. The
    cast keeps the copied buffer alive, so the pointer stands on its own."""
    if not data:
        return None
    buf = (ctypes.c_ubyte * len(data)).from_buffer_copy(data)
    return ctypes.cast(buf, ctypes.POINTER(ctypes.c_ubyte))


def _json_call(fn, payload: dict, what: str) -> dict:
    """One JSON-in/JSON-out core call: size query, then fill (the peaks
    pattern). Raises `ValueError` on unreadable input or on the error object
    the core answers with."""
    data = json.dumps(payload).encode("utf-8")
    u8p = ctypes.POINTER(ctypes.c_ubyte)
    inp = (ctypes.c_ubyte * len(data)).from_buffer_copy(data) if data else (ctypes.c_ubyte * 0)()
    need = fn(ctypes.cast(inp, u8p), len(data), None, 0)
    if need == 0:
        raise ValueError(f"{what}: the core could not read the request")
    out = (ctypes.c_ubyte * need)()
    n = fn(ctypes.cast(inp, u8p), len(data), out, need)
    result = json.loads(ctypes.string_at(out, n))
    if "error" in result:
        raise ValueError(result["error"])
    return result


def bundle_requirements(manifest: dict, template: dict | None = None) -> dict:
    """What one instance of a bundle needs allocated: ``{"widgets", "nodes",
    "buses", "buffers"}``, through the shared `clausters_core::bundle` pass.

    Pass ``template`` for a bundle written before the manifest carried a widget
    count — its id block is then measured from the ids the tree actually uses.
    """
    payload: dict = {"manifest": manifest}
    if template is not None:
        payload["template"] = template
    return _json_call(
        lib().clausters_core_bundle_requirements, payload, "bundle requirements"
    )


def bundle_resolve(
    manifest: dict,
    template: dict,
    allocation: dict,
    attributes: dict | None = None,
    preset: dict | None = None,
) -> dict:
    """One mounted instance: the template's widget ids offset by
    ``allocation["widget_base"]``, its ``@symbol`` and ``$param`` holes filled,
    its ``boot`` list lifted out — ``{"def_id", "tree", "boot", "params"}``.

    The caller allocates (``allocation`` is ``{"widget_base", "nodes", "buses",
    "buffers"}``), which is what keeps the pass pure. Raises `ValueError` on an
    unknown symbol, a missing parameter, a type mismatch or a value out of its
    declared range.
    """
    return _json_call(
        lib().clausters_core_bundle_resolve,
        {
            "manifest": manifest,
            "template": template,
            "allocation": allocation,
            "params": {"attributes": attributes or {}, "preset": preset or {}},
        },
        "bundle resolve",
    )


def bundle_validate(manifest: dict, template: dict, defs: list | None = None) -> None:
    """The writers' pre-flight: the mount dry-run over the declared defaults,
    plus the no-holes check on every def payload — so a bundle that would fail
    to mount fails to be written. Raises `ValueError` with the reason."""
    _json_call(
        lib().clausters_core_bundle_validate,
        {"manifest": manifest, "template": template, "defs": defs or []},
        "bundle validate",
    )


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
    result = json.loads(ctypes.string_at(out, n))
    if "error" in result:
        raise ValueError(result["error"])
    return result


# ---- the document ----


def _bytes(payload) -> tuple:
    """A JSON payload as the ``(pointer, length)`` pair the C ABI takes."""
    data = json.dumps(payload).encode("utf-8") if payload is not None else b""
    u8p = ctypes.POINTER(ctypes.c_ubyte)
    buf = (ctypes.c_ubyte * len(data)).from_buffer_copy(data) if data else (ctypes.c_ubyte * 0)()
    return ctypes.cast(buf, u8p), len(data)


def _why_refused(document) -> str:
    """Why the crate would not open ``document``, in the one case worth naming.

    The C ABI's ``clausters_document_open`` answers with a null handle and no
    message — it has no channel for one, unlike the wasm face, which raises the
    crate's own string. So the reason a caller sees is generic, and the reason
    that is *worth* seeing is the one a client can produce by accident: a node
    id on two different nodes, which the crate refuses because an intent
    addresses a node by its id. Looked for only once the crate has already
    refused, so nothing is validated twice on the ordinary path.
    """
    generic = "not a valid document for the crate"
    if not isinstance(document, dict):
        return generic
    seen: dict = {}
    stack = [document.get("root")]
    while stack:
        node = stack.pop()
        if not isinstance(node, dict):
            continue
        node_id = node.get("id")
        if node_id is not None:
            first = seen.setdefault(node_id, node)
            if first is not node and first != node:
                return (
                    f"{generic}: node id {node_id} names two different nodes, "
                    f"a {first.get('kind')} and a {node.get('kind')} — ids are "
                    "unique within a document"
                )
        for member in node.get("members") or ():
            if isinstance(member, dict):
                stack.append(member.get("node"))
        stack.append(node.get("rendered"))
    return generic


class Document:
    """One composition, held by the shared crate — the **only** implementation
    of what an edit means.

    A client does not apply an edit and then report it: it hands an intent over
    and takes back what happened. That is what keeps three clients from meaning
    three different things by the same edit.

    **The tree stays in Rust.** It used to cross on every call, and the cost was
    linear in the whole composition rather than in the edit: 205 ms for one
    placement on a 10240-event piece, the same for a stroke touching fifty
    samples. This is not an accessor handle — there is no call per field of the
    tree, just the same three verbs — and `snapshot` is how the JSON leaves,
    asked for rather than paid per edit.

    Args:
        document: a document as `clausters.form.to_document` writes it, or
            ``None`` for an empty composition.

    Usage::

        with Document(to_document(song)) as doc:
            outcome = doc.apply({"intent": "place", "node": 4, "offset": 2.0})
            song = from_document(doc.snapshot())

    Raises:
        ValueError: if the document will not parse.
    """

    def __init__(self, document: "dict | None" = None):
        ptr, length = _bytes(document) if document is not None else (None, 0)
        self._handle = lib().clausters_document_open(ptr, length)
        if not self._handle:
            raise ValueError(_why_refused(document))

    def __enter__(self) -> "Document":
        return self

    def __exit__(self, *exc) -> bool:
        self.close()
        return False

    def close(self) -> None:
        """Release the document. Idempotent; also runs on garbage collection."""
        handle, self._handle = getattr(self, "_handle", None), None
        if handle:
            lib().clausters_document_free(handle)

    def __del__(self):
        self.close()

    @property
    def version(self) -> int:
        """The monotonic version, bumped by every applied edit. Never zero.

        Read without serializing anything, which is what lets a caller name the
        state an edit was made against on every gesture.
        """
        return int(lib().clausters_document_version(self._handle))

    def snapshot(self) -> dict:
        """The whole tree as JSON — to save it, or to rebuild the client's own
        objects from it.

        The one call still the size of the composition. It is a pure read, so
        the crate serializes once per call rather than twice.
        """
        fn = lib().clausters_document_snapshot
        need = fn(self._handle, None, 0)
        if need == 0:
            raise ValueError("the document could not be serialized")
        out = (ctypes.c_ubyte * need)()
        n = fn(self._handle, out, need)
        return json.loads(ctypes.string_at(out, n))

    def apply(self, intent: dict, *, against=None, quant: float = 0.0) -> dict:
        """Apply one edit.

        Args:
            intent: the edit — ``{"intent": "place"|"configure"|"setmembers"|
                "writesamples", "node": id, …}``. Absolute: it states the
                *resulting* value, never an increment.
            against: the state the edit was made against — ``{"version": N}``,
                or ``None`` for unstated, which applies unchecked. An edit made
                against a superseded version comes back refused and marked
                ``stale``, with the value that now holds.
            quant: the musical grid a placement snaps to, in beats. ``0`` snaps
                nothing.

        Returns:
            ``{"effective", "applied", "reason", "stale"}``. There is no success
            flag to branch on: ``effective`` is the edit describing the document
            as it now stands, so applied, transformed and refused are one shape
            and a refusal is the previous value.

        Raises:
            ValueError: if the intent will not parse.
        """
        int_ptr, int_len = _bytes(intent)
        ag_ptr, ag_len = _bytes(against) if against is not None else (None, 0)
        fn = lib().clausters_document_apply
        args = (self._handle, int_ptr, int_len, ag_ptr, ag_len, float(quant))
        need = fn(*args, None, 0)
        if need == 0:
            raise ValueError("the intent is not valid JSON for the crate")
        out = (ctypes.c_ubyte * need)()
        n = fn(*args, out, need)
        return json.loads(ctypes.string_at(out, n))

    def resolve(self, selection: dict, *, frames_per_beat: float,
                in_beats: bool = False) -> list:
        """Resolve a selection to the spans of samples underneath it.

        Args:
            selection: ``{"start", "len", …}`` — see the crate's ``Selection``.
            frames_per_beat: the bridge between the arrangement's beats and the
                samples's frames. Supplied rather than derived: tempo is the
                caller's, the arithmetic is the crate's.
            in_beats: whether the selection's numbers are beats rather than
                frames on the shared axis.

        Returns:
            ``[{"node", "source", "generation", "range", "at"}, …]`` in tree
            order, with the placement's base, the element's trim and the clamp
            at both ends already applied. Empty when nothing samples was
            underneath — a group and a generator are in the way of a selection,
            not under it.
        """
        sel_ptr, sel_len = _bytes(selection)
        fn = lib().clausters_document_resolve
        args = (self._handle, sel_ptr, sel_len, float(frames_per_beat),
                int(bool(in_beats)))
        need = fn(*args, None, 0)
        if need == 0:
            raise ValueError("the selection is not valid JSON for the crate")
        out = (ctypes.c_ubyte * need)()
        n = fn(*args, out, need)
        return json.loads(ctypes.string_at(out, n))


def document_apply(document: dict, intent: dict, *, against=None, quant: float = 0.0) -> dict:
    """Apply one edit to a document given and returned **by value**.

    The convenience form, built out of `Document` — open, apply, snapshot, free
    — for a script that has a document in hand and wants the edited one back.
    It costs a serialization of the whole composition either way, which is why
    it is a wrapper here rather than the binding: an editor applying a gesture
    per drag holds a `Document` instead and pays nothing per edit.

    Returns:
        ``{"document": …, "outcome": {…}}`` — the shape this call has always
        had, so a caller written against it does not change.
    """
    with Document(document) as doc:
        outcome = doc.apply(intent, against=against, quant=quant)
        return {"document": doc.snapshot(), "outcome": outcome}


def document_resolve(document: dict, selection: dict, *, frames_per_beat: float,
                     in_beats: bool = False) -> list:
    """Resolve a selection against a document given **by value** — the
    convenience form of `Document.resolve`, for a caller that has one in hand.
    """
    with Document(document) as doc:
        return doc.resolve(selection, frames_per_beat=frames_per_beat,
                           in_beats=in_beats)


class Log:
    """The undo history of one document, living in the shared crate.

    Undo belongs with the document and not with a view: a view's log sees only
    the gestures *it* made, so a script editing the arrangement, a second editor
    or a re-render leaves it describing a document that has moved on — and undo
    then writes a state nobody was ever in. This is a handle onto the crate's
    log, so there is one history however many surfaces edit.

    It is a handle for its own reason, beyond the one `Document` has: the spill
    store. A bulk inverse *leaves* the log on purpose, so passing one by value
    would carry every spilled span on every call — which is the cost spilling
    exists to avoid.

    Args:
        budget: how many entries to keep before the oldest falls off. ``None``
            takes the crate's default.
        spill_above: how many ``f32`` values a sample payload must reach before
            it leaves the log for the spill store. ``None`` takes the default.

    Usage::

        log = Log()
        with Document(to_document(song)) as doc:
            log.apply(doc, {"intent": "place", "node": 3, "offset": 4.0},
                      label="move the clip")
            log.undo(doc)                    # exactly where it was

    Free with `close` (``__del__`` is the backstop), or use it as a context
    manager.
    """

    def __init__(self, *, budget: "int | None" = None,
                 spill_above: "int | None" = None):
        self._lib = lib()
        self._handle = self._lib.clausters_log_new(
            0 if budget is None else int(budget),
            0 if spill_above is None else int(spill_above),
        )

    def apply(self, document: "Document", intent: dict, *, against=None,
              quant: float = 0.0, label: str = "edit") -> dict:
        """Apply an edit to ``document`` **and record it**, in one call.

        One call rather than two because the inverse has to be read out of the
        document *before* the edit lands: a surface that let you apply first and
        record second would let you record the wrong thing. Nothing is recorded
        unless the document actually changed, so a refusal — stale or otherwise
        — leaves no entry, and neither does a resend.

        Arguments are `Document.apply`'s, plus the document and ``label``: what
        an undo menu calls this. Returns the same outcome object; the document
        changed behind its handle.
        """
        int_ptr, int_len = _bytes(intent)
        ag_ptr, ag_len = _bytes(against) if against is not None else (None, 0)
        lb_ptr, lb_len = _text(label)
        return self._sized(
            self._lib.clausters_log_apply,
            (document._handle, int_ptr, int_len, ag_ptr, ag_len,
             float(quant), lb_ptr, lb_len),
            "the intent is not valid JSON for the crate",
        )

    def record(self, forward: dict, backward: dict, *, label: str = "edit",
               coalesce: bool = False):
        """Record an entry the document cannot supply the inverse for.

        The destructive case: a write's overwritten samples are not in the tree,
        so the caller reads the span it is about to write and hands the pair
        over. This **applies nothing** — the write has already happened; what is
        recorded is how to put it back.

        Args:
            forward: a step — ``{"edit": <intent>}``, or
                ``{"recompute": <params>}`` for a deterministic operation the
                owner re-runs rather than replays. The second is what makes a
                redo of a million-sample operation cost a few bytes.
            backward: the inverse, an ordinary intent.
            label: what an undo menu calls this.
            coalesce: merge into the entry before it when both touch the same
                node the same way — a run of small adjustments becoming one
                undo. You decide, because only you know where the hand stopped.
        """
        f_ptr, f_len = _bytes(forward)
        b_ptr, b_len = _bytes(backward)
        lb_ptr, lb_len = _text(label)
        code = self._lib.clausters_log_record(
            self._handle, f_ptr, f_len, b_ptr, b_len, lb_ptr, lb_len,
            int(bool(coalesce)))
        if code != 0:
            raise ValueError("the step or its inverse is not valid JSON for the crate")

    def undo(self, document: "Document") -> "dict | None":
        """Undo the last transaction, applying its inverses to ``document``.

        Returns ``{"undone": [<intent>, …]}``, or ``None`` when there was
        nothing to undo; the document changed behind its handle. It applies
        rather than handing the inverses back, because the cursor moves with it:
        two steps could half-happen, and a log that disagrees with its document
        is worse than no log.
        """
        return self._step(self._lib.clausters_log_undo, document)

    def redo(self, document: "Document") -> "dict | None":
        """Redo what was last undone, applying what it can.

        Returns ``{"remaining": [<step>, …]}``, or ``None`` when there was
        nothing to redo. The ordinary edits at the front are already applied to
        the document; ``remaining`` holds the steps from the first one the crate
        **cannot perform** onward — a deterministic operation kept as its
        parameters, which you re-run, because the crate holds no algorithms. It
        stops at the first rather than skipping it, so a later edit is never
        applied over a state the operation before it was meant to produce.
        """
        return self._step(self._lib.clausters_log_redo, document)

    @property
    def can_undo(self) -> bool:
        """Whether there is anything to undo."""
        return bool(self._lib.clausters_log_can_undo(self._handle))

    @property
    def can_redo(self) -> bool:
        """Whether there is anything to redo."""
        return bool(self._lib.clausters_log_can_redo(self._handle))

    @property
    def undo_label(self) -> "str | None":
        """What an undo would be called, for a menu item."""
        return self._label(self._lib.clausters_log_undo_label)

    @property
    def redo_label(self) -> "str | None":
        """What a redo would be called."""
        return self._label(self._lib.clausters_log_redo_label)

    def clear(self):
        """Forget everything, releasing what was spilled — what closing a
        document or loading another one leaves behind."""
        self._lib.clausters_log_clear(self._handle)

    def close(self):
        """Free the handle (idempotent)."""
        if getattr(self, "_handle", None):
            self._lib.clausters_log_free(self._handle)
            self._handle = None

    def __len__(self) -> int:
        return self._lib.clausters_log_len(self._handle)

    def __enter__(self) -> "Log":
        return self

    def __exit__(self, *_):
        self.close()

    def __del__(self):
        try:
            self.close()
        except Exception:  # interpreter teardown: the library may be gone
            pass

    # ---- the size-then-fill dance ----
    #
    # Every call here mutates, so the crate's rule is that the mutation happens
    # only when the bytes are written: a sizing pass (a null buffer) is free of
    # consequence. That is what makes calling twice correct rather than an edit
    # applied twice.

    def _sized(self, fn, args, error: str) -> dict:
        need = fn(self._handle, *args, None, 0)
        if need == 0:
            raise ValueError(error)
        out = (ctypes.c_ubyte * need)()
        n = fn(self._handle, *args, out, need)
        return json.loads(ctypes.string_at(out, n))

    def _step(self, fn, document: "Document") -> "dict | None":
        result = self._sized(
            fn, (document._handle,), "the document handle is not usable")
        # `{}` is "there was nothing to do", which the crate keeps distinct from
        # a parse failure (0 bytes) and from a step that changed nothing.
        return result or None

    def _label(self, fn) -> "str | None":
        need = fn(self._handle, None, 0)
        if need == 0:
            return None
        out = (ctypes.c_ubyte * need)()
        n = fn(self._handle, out, need)
        return ctypes.string_at(out, n).decode("utf-8")


def _text(value: str) -> tuple:
    """A string as the ``(pointer, length)`` pair the C ABI takes."""
    data = str(value).encode("utf-8")
    u8p = ctypes.POINTER(ctypes.c_ubyte)
    buf = (ctypes.c_ubyte * len(data)).from_buffer_copy(data) if data else (ctypes.c_ubyte * 0)()
    return ctypes.cast(buf, u8p), len(data)


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
    reply thread releases on ``/node_end``. ``capacity=None`` builds the
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
        foreign ``/node_end`` ids."""
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



def peaks_cache_update(cache: bytes, samples, start: int, frames: int) -> bytes:
    """Rewrites the part of a peak cache a **frame span** touches, returning the
    new bytes — what keeps an editor's overview true after an edit without
    re-summarizing the whole take.

    ``samples`` is the **whole** buffer as it now stands (interleaved), not the
    span: a bucket at either edge of it holds untouched samples too. The result
    is identical to rebuilding the cache from the edited samples, so an
    updated overview and a fresh one cannot drift apart over a session.

    Raises `ValueError` when the buffer is not the one the cache describes — an
    edit that changed the *length* is a rebuild (`peaks_cache`), not an update.
    """
    a, _ = _as_array(samples)
    buf = (ctypes.c_ubyte * len(cache)).from_buffer_copy(cache)
    written = lib().clausters_core_peaks_multi_update(
        buf, len(cache), _ptr(a), len(a), start, frames)
    if written != len(cache):
        raise ValueError(
            "clausters_core_peaks_multi_update refused: the samples are not the "
            "buffer this cache describes (a length change is a rebuild)")
    return bytes(buf)

def peaks_cache_empty(frames: int, channels: int = 1, base_bucket: int = 256) -> bytes:
    """The peak cache of a take that has been **allocated and not recorded
    into**: `frames` frames of `channels` channels, every bucket a measured
    zero, ready to be filled by `peaks_cache_write_buckets` as the reports
    arrive.

    It is `peaks_cache` with no samples to read, which is the point — building
    one from a buffer of silence would allocate the take (230 MB for ten
    minutes of stereo) to summarize what nobody wrote."""
    size = lib().clausters_core_peaks_multi_cache_size(frames, channels, base_bucket)
    if size == 0:
        raise ValueError(
            "clausters_core_peaks_multi_cache_size returned 0 (base_bucket must be > 0)")
    out = (ctypes.c_ubyte * size)()
    written = lib().clausters_core_peaks_multi_empty(
        frames, channels, base_bucket, out, size)
    if written != size:
        raise ValueError(f"clausters_core_peaks_multi_empty wrote {written} of {size} bytes")
    return bytes(out)


def peaks_cache_write_buckets(cache: bytes, start_frame: int, bucket: int, stats) -> bytes:
    """Folds a run of **already-summarized buckets** into a peak cache,
    returning the new bytes — the receiving half of ``/buffer_stream``, which
    sends the overview of samples as it is written instead of the samples.

    ``stats`` is the reply's blob read as floats, **bucket-major and
    channel-minor**: for each bucket of ``bucket`` frames in order, for each
    channel, ``min``, ``max`` and mean square — the pyramid's own three
    statistics in its own energy form, so nothing is converted here.
    ``start_frame`` is where the report begins on the buffer's own sample axis.

    Nothing is measured: the writer measured, and this puts the buckets where
    they belong and rebuilds the levels above them. So a client that never sees
    the samples ends up with the cache the samples would have built.

    Raises `ValueError` when the report is on another grid than the cache — a
    different bucket size, a start off a bucket boundary, a run past the end,
    or a length that is not whole buckets across every channel. A refused
    report changes nothing.
    """
    a, _ = _as_array(stats)
    buf = (ctypes.c_ubyte * len(cache)).from_buffer_copy(cache)
    written = lib().clausters_core_peaks_multi_write_buckets(
        buf, len(cache), start_frame, bucket, _ptr(a), len(a))
    if written != len(cache):
        raise ValueError(
            "clausters_core_peaks_multi_write_buckets refused: the report is not "
            "on this cache's grid (bucket size, start alignment, or length)")
    return bytes(buf)


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
            return ctypes.string_at(self._buf, n)
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


# ---- the shared-memory segment ----------------------------------------------
#
# A peer maps the file with `mmap` — that part is Python's — and asks the core
# for everything else: where each region is, the directory's seqlock, the ring
# framing, the name a region file carries. What used to live here instead was a
# transcription of the layout, which is how this binding came to declare 1024
# control buses against a server that had had 16 384 for months: wrong, unused,
# and caught by nothing.


def shm_shape(address: int, length: int) -> "ShmShape | None":
    """Every count and byte offset of the segment mapped at `address`, or
    ``None`` when that memory is not a segment of this layout version."""
    out = ShmShape()
    rc = lib().clausters_core_shm_shape(ctypes.c_void_p(address), length, ctypes.byref(out))
    return out if rc == 0 else None


def shm_segment_size(control_buses: int, taps: int, tap_frames: int, buffers: int) -> int:
    """How big a segment carrying these counts is — what a peer sizes a file to
    before creating one."""
    return lib().clausters_core_shm_segment_size(control_buses, taps, tap_frames, buffers)


def shm_init(address: int, length: int, control_buses: int, taps: int,
             tap_frames: int) -> bool:
    """Writes a fresh header over the mapping at `address`, making it a segment.

    For a peer that **creates** one rather than attaching: in the editor's
    arrangement the process that owns the samples owns the segment, and every
    server attaches to it. Nothing else may have attached yet.
    """
    return lib().clausters_core_shm_init(
        ctypes.c_void_p(address), length, control_buses, taps, tap_frames) == 0


def shm_buffer_info(address: int, length: int,
                    bufnum: int) -> "tuple[int, int, int, float] | None":
    """What the directory says buffer `bufnum` is — ``(generation, frames,
    channels, sample_rate)`` — read under the generation twice, so a row caught
    mid-write is re-read rather than believed. ``None`` when the slot is empty.
    """
    generation = ctypes.c_uint64()
    frames = ctypes.c_uint64()
    channels = ctypes.c_uint64()
    rate = ctypes.c_double()
    rc = lib().clausters_core_shm_buffer_info(
        ctypes.c_void_p(address), length, bufnum,
        ctypes.byref(generation), ctypes.byref(frames),
        ctypes.byref(channels), ctypes.byref(rate))
    if rc != 0:
        return None
    return generation.value, frames.value, channels.value, rate.value


def shm_region_suffix(bufnum: int, generation: int) -> str:
    """The suffix a buffer's region file carries beside the segment's own path.
    Three processes name that file, so the name is built in one place."""
    buf = (ctypes.c_uint8 * 64)()
    n = lib().clausters_core_shm_region_suffix(bufnum, generation, buf, len(buf))
    if n < 0:
        raise ValueError(f"region suffix for buffer {bufnum} did not fit")
    return bytes(buf[:n]).decode()


def shm_push(address: int, length: int, role: int, peer: int, packet: bytes) -> bool:
    """Pushes one OSC packet into the segment's outbound ring, tagged for
    `peer`. ``False`` means the ring is momentarily full — backpressure, and the
    caller may retry, since nothing was dropped."""
    data = (ctypes.c_uint8 * len(packet)).from_buffer_copy(packet)
    rc = lib().clausters_core_shm_push(
        ctypes.c_void_p(address), length, role, peer, data, len(packet))
    return rc == 0


def shm_pop(address: int, length: int, role: int,
            cap: int = 65536) -> "tuple[int, bytes] | None":
    """The next frame of the segment's inbound ring as ``(peer, packet)``, or
    ``None`` when it is empty (or held garbage, which resyncs it)."""
    buf = (ctypes.c_uint8 * cap)()
    peer = ctypes.c_uint32()
    got = ctypes.c_size_t()
    rc = lib().clausters_core_shm_pop(
        ctypes.c_void_p(address), length, role, buf, cap,
        ctypes.byref(peer), ctypes.byref(got))
    if rc != 0:
        return None
    return peer.value, bytes(buf[:got.value])
