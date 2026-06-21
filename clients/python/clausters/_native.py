"""ctypes binding over the shared native core (`clausters-ffi`).

Loads ``libclausters_ffi`` (the C ABI over ``clausters-core``, built with
``cargo build -p clausters-ffi``) and exposes its builtins, seeded white noise
and clock/sample conversions to Python. Because the server's native UGens use
the very same ``clausters-core``, these results match the server by
construction for the operators it computes natively (C0).

Boundary rule (project-wide, same as :mod:`clausters.transport`): only flat
data crosses — Python floats/ints in, :class:`array.array` ``'f'`` (or a plain
float for scalar calls) out. Nothing heavy is imported; a numpy user can wrap
the returned ``array`` without copying.

The library is loaded lazily on first use, so importing this module (and the
package) never fails just because the cdylib has not been built yet.
"""

import ctypes
import os
from array import array
from enum import IntEnum

from . import _libpath

CORE_ABI_VERSION = 1

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


# ---- library loading (lazy, versioned) ----

_LIB = None


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
    return lib


def lib(path: str | None = None) -> ctypes.CDLL:
    """The loaded, version-checked cdylib (cached after the first call)."""
    global _LIB
    if _LIB is None or path is not None:
        _LIB = _configure(ctypes.CDLL(path or _find_library()))
    return _LIB


def abi_version() -> int:
    return lib().clausters_core_abi_version()


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
