"""Clausters Python bindings (M14). Standard library only, by design.

Two ways in, both speaking ordinary OSC bytes (build them however you like;
``examples/json_client.py`` has stdlib helpers):

- :class:`Clausters` — the **embedded** server: loads the cdylib (build it
  with ``cargo build --release --features embed,realtime``) and runs the
  whole server in-process. Commands are function calls, no network anywhere.
- :class:`ShmClient` — attaches to a **separate** server started with
  ``clausters --shm <path>``: commands/replies travel through a
  shared-memory ring, and the *data plane* (sample clock, control buses) is
  read and written directly in mapped memory.

Plus :func:`render` — the synchronous "scientific" call: hand it a binary
score, get the interleaved float32 samples back, no server running at all.

Boundary rule (project-wide): only flat data crosses — ``bytes`` in,
``array('f')``/floats/ints out. A numpy user can wrap the results without
copying (``numpy.frombuffer``), but nothing here imports anything heavy.

Caveats of the pure-Python shm path: Python has no atomics, so cursor reads
and writes rely on x86-TSO-style ordering of aligned 32-bit accesses — fine
on the supported platforms, documented in docs/ipc.md.
"""

import ctypes
import mmap
import os
import struct
import time
from array import array

from . import _libpath
from .errors import (
    AbiMismatchError,
    CommandRingFull,
    LibraryFeatureError,
    LibraryNotFoundError,
    RenderError,
    ReplyTimeout,
    SegmentError,
    ServerError,
)

ABI_VERSION = 2

# embed cdylib file names across platforms (Linux / macOS / Windows).
_EMBED_NAMES = ("libclausters.so", "libclausters.dylib", "clausters.dll")

# ---- segment layout (must match src/server/ipc.rs; pinned by tests) ----
# Fixed prefix: the header, then the c2s and s2c rings. The control-bus array
# is a trailing, dynamically-sized region after the rings (its length lives in
# the header), so `--control-buses` changes the segment size but not these
# ring offsets. The whole file is mmap'd, so any control count is supported.

_MAGIC = 0x5541_4C43  # "CLAU"
_HEADER_SIZE = 64
_OFF_MAGIC = 0
_OFF_VERSION = 4
_OFF_SAMPLE_RATE = 8  # f64 bits
_OFF_CLOCK = 16  # u64
_OFF_CONTROL_BUSES = 28  # u32: number of slots in the trailing control region
_RING_CAPACITY = 64 * 1024
_RING_HEADER = 64  # head u32, tail u32, padding
_OFF_C2S = _HEADER_SIZE  # 64; rings come right after the header
_OFF_S2C = _OFF_C2S + _RING_HEADER + _RING_CAPACITY  # 65664
_OFF_CONTROLS = _OFF_S2C + _RING_HEADER + _RING_CAPACITY  # 131264 (trailing)
_DEFAULT_CONTROL_BUSES = 1024
# Segment size for the default control-bus count (the actual size is the file's
# length; the server sizes it from `--control-buses`).
SEGMENT_SIZE = _OFF_CONTROLS + 4 * _DEFAULT_CONTROL_BUSES  # 135360


class _Ring:
    """One SPSC byte ring inside the mapped segment (length-prefixed
    packets padded to 4). ``produce``/``consume`` depend on which side of
    the pair we are."""

    def __init__(self, mm: mmap.mmap, base: int):
        self.mm, self.base = mm, base

    def _cursor(self, off: int) -> int:
        return struct.unpack_from("<I", self.mm, self.base + off)[0]

    def _set_cursor(self, off: int, value: int):
        struct.pack_into("<I", self.mm, self.base + off, value & 0xFFFFFFFF)

    def push(self, packet: bytes) -> bool:
        head, tail = self._cursor(0), self._cursor(4)
        padded = (len(packet) + 3) // 4 * 4
        total = 4 + padded
        if not packet or total > _RING_CAPACITY:
            return False
        if _RING_CAPACITY - ((head - tail) & 0xFFFFFFFF) < total:
            return False  # backpressure: retry later
        self._write(head, struct.pack("<I", len(packet)))
        self._write(head + 4, packet)
        self._set_cursor(0, head + total)  # publish last
        return True

    def pop(self) -> bytes | None:
        head, tail = self._cursor(0), self._cursor(4)
        used = (head - tail) & 0xFFFFFFFF
        if used == 0:
            return None
        (length,) = struct.unpack("<I", self._read(tail, 4))
        total = 4 + (length + 3) // 4 * 4
        if length == 0 or total > used:
            self._set_cursor(4, head)  # resync: drop garbage
            return None
        packet = self._read(tail + 4, length)
        self._set_cursor(4, tail + total)
        return packet

    def _write(self, at: int, data: bytes):
        start = at % _RING_CAPACITY
        first = min(len(data), _RING_CAPACITY - start)
        base = self.base + _RING_HEADER
        self.mm[base + start : base + start + first] = data[:first]
        self.mm[base : base + len(data) - first] = data[first:]

    def _read(self, at: int, n: int) -> bytes:
        start = at % _RING_CAPACITY
        first = min(n, _RING_CAPACITY - start)
        base = self.base + _RING_HEADER
        out = self.mm[base + start : base + start + first]
        return out + self.mm[base : base + n - first]


class ShmClient:
    """Client of a ``clausters --shm <path>`` server (same machine)."""

    def __init__(self, path: str):
        self._file = open(path, "r+b")
        # Map the whole file: its length is the server's --control-buses count.
        self.mm = mmap.mmap(self._file.fileno(), 0)
        magic, version = struct.unpack_from("<II", self.mm, 0)
        if magic != _MAGIC:
            raise SegmentError(f"{path} is not a clausters segment")
        if version != ABI_VERSION:
            raise SegmentError(f"segment ABI v{version}, this client speaks v{ABI_VERSION}")
        #: control-bus count the server created the segment with.
        self.control_buses = struct.unpack_from("<I", self.mm, _OFF_CONTROL_BUSES)[0]
        self._c2s = _Ring(self.mm, _OFF_C2S)  # we produce here
        self._s2c = _Ring(self.mm, _OFF_S2C)  # we consume here

    # -- data plane: no commands, just shared memory --

    @property
    def clock(self) -> int:
        """The engine's sample counter, mirrored every block (64 samples)."""
        return struct.unpack_from("<Q", self.mm, _OFF_CLOCK)[0]

    @property
    def sample_rate(self) -> float:
        return struct.unpack_from("<d", self.mm, _OFF_SAMPLE_RATE)[0]

    def ctl_get(self, index: int) -> float:
        if not 0 <= index < self.control_buses:
            raise IndexError(f"control bus {index} out of range 0..{self.control_buses}")
        return struct.unpack_from("<f", self.mm, _OFF_CONTROLS + 4 * index)[0]

    def ctl_set(self, index: int, value: float):
        """Writes the very atomic the engine's InCtl reads next block."""
        if not 0 <= index < self.control_buses:
            raise IndexError(f"control bus {index} out of range 0..{self.control_buses}")
        struct.pack_into("<f", self.mm, _OFF_CONTROLS + 4 * index, value)

    # -- command plane: OSC packets through the ring --

    def send(self, packet: bytes) -> bool:
        return self._c2s.push(packet)

    def poll(self) -> bytes | None:
        return self._s2c.pop()

    def request(self, packet: bytes, timeout: float = 2.0) -> bytes:
        """The synchronous facade: send, then block (the *client* blocks,
        never the server) until a reply arrives."""
        if not self.send(packet):
            raise CommandRingFull("command ring full")
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            reply = self.poll()
            if reply is not None:
                return reply
            time.sleep(0.001)
        raise ReplyTimeout("no reply through the ring")

    def close(self):
        self.mm.close()
        self._file.close()


# ---- the embed cdylib (feature `embed`) ----


def _find_library() -> str:
    # Precedence (see _libpath): env override, the bundled wheel copy, then the
    # workspace target/ of a source checkout.
    candidates = [os.environ.get("CLAUSTERS_LIB")]
    candidates += _libpath.bundled_candidates(_EMBED_NAMES)
    candidates += _libpath.workspace_candidates(_EMBED_NAMES)
    for c in candidates:
        if c and os.path.exists(c):
            return c
    raise LibraryNotFoundError(
        "libclausters not found: install the wheel (it bundles the library) "
        "or, in a source checkout, build it with "
        "`cargo build --release --features embed,realtime` "
        "or point CLAUSTERS_LIB at it"
    )


def _require(lib: ctypes.CDLL, name: str, feature: str):
    """Fetch an FFI symbol, turning a missing one into a concrete
    :class:`LibraryFeatureError` that names the symbol and the Cargo feature to
    rebuild with — instead of the bare ``AttributeError``/``undefined symbol``
    ctypes raises when the library was built without that feature."""
    try:
        return getattr(lib, name)
    except AttributeError:
        raise LibraryFeatureError(
            f"{os.path.basename(getattr(lib, '_name', None) or 'libclausters')}: "
            f"symbol {name!r} is missing -- the library was built without the "
            f"`{feature}` feature. Rebuild it with "
            f"`cargo build --release --features {feature}` (or point "
            f"CLAUSTERS_LIB at a library that has it).",
            symbol=name, feature=feature,
        ) from None


def _load(path: str | None = None) -> ctypes.CDLL:
    lib = ctypes.CDLL(path or _find_library())

    # `embed`-only surface: ABI check + offline render. Required.
    abi = _require(lib, "clausters_abi_version", "embed")
    abi.restype = ctypes.c_uint32
    got = abi()
    if got != ABI_VERSION:
        raise AbiMismatchError(
            f"libclausters speaks ABI v{got}, this binding speaks v{ABI_VERSION}",
            got=got, expected=ABI_VERSION,
        )
    render_fn = _require(lib, "clausters_render", "embed")
    render_fn.restype = ctypes.POINTER(ctypes.c_float)
    render_fn.argtypes = [
        ctypes.c_char_p, ctypes.c_size_t, ctypes.c_double, ctypes.c_uint32,
        ctypes.c_uint32, ctypes.POINTER(ctypes.c_uint64), ctypes.c_char_p,
        ctypes.c_size_t,
    ]
    _require(lib, "clausters_free_samples", "embed").argtypes = [
        ctypes.POINTER(ctypes.c_float), ctypes.c_uint64]

    # `embed,realtime` surface: the live embedded server. Optional at load
    # time so `render()` works with an `embed`-only build; if it is absent the
    # error is deferred to `Clausters()`, where it is actually needed.
    lib._clausters_live_error = _bind_live(lib)
    return lib


def _bind_live(lib: ctypes.CDLL) -> LibraryFeatureError | None:
    """Bind the live-server symbols. Returns the :class:`LibraryFeatureError`
    to raise if they are missing (built without `realtime`), else ``None``."""
    try:
        lib.clausters_open.restype = ctypes.c_void_p
    except AttributeError:
        return LibraryFeatureError(
            f"{os.path.basename(getattr(lib, '_name', None) or 'libclausters')}: "
            "the embedded live server needs the `realtime` feature. Rebuild it "
            "with `cargo build --release --features embed,realtime`.",
            symbol="clausters_open", feature="embed,realtime",
        )
    lib.clausters_open.argtypes = [ctypes.c_uint32, ctypes.c_char_p, ctypes.c_size_t]
    lib.clausters_send.restype = ctypes.c_int32
    lib.clausters_send.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]
    lib.clausters_poll.restype = ctypes.c_int64
    lib.clausters_poll.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]
    lib.clausters_clock.restype = ctypes.c_uint64
    lib.clausters_clock.argtypes = [ctypes.c_void_p]
    lib.clausters_sample_rate.restype = ctypes.c_double
    lib.clausters_sample_rate.argtypes = [ctypes.c_void_p]
    lib.clausters_ctl_set.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_float]
    lib.clausters_ctl_get.restype = ctypes.c_float
    lib.clausters_ctl_get.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
    lib.clausters_close.argtypes = [ctypes.c_void_p]
    return None


def render(score: bytes, sample_rate: float = 48000.0, channels: int = 2,
           workers: int = 0, lib_path: str | None = None) -> tuple[array, int]:
    """Synchronous offline render: binary score in, ``(samples, frames)``
    out — interleaved float32 in a stdlib ``array('f')``. The whole call
    blocks the caller and nothing else; there is no server involved."""
    lib = _load(lib_path)
    frames = ctypes.c_uint64(0)
    err = ctypes.create_string_buffer(512)
    ptr = lib.clausters_render(score, len(score), sample_rate, channels,
                               workers, ctypes.byref(frames), err, len(err))
    if not ptr:
        raise RenderError(err.value.decode() or "render failed")
    total = frames.value * channels
    samples = array("f", ctypes.cast(ptr, ctypes.POINTER(ctypes.c_float * total)).contents)
    lib.clausters_free_samples(ptr, total)
    return samples, frames.value


class Clausters:
    """The embedded live server: audio device + engine in this process."""

    def __init__(self, workers: int = 0, lib_path: str | None = None):
        self._lib = _load(lib_path)
        if self._lib._clausters_live_error is not None:
            raise self._lib._clausters_live_error
        err = ctypes.create_string_buffer(512)
        self._h = self._lib.clausters_open(workers, err, len(err))
        if not self._h:
            raise ServerError(err.value.decode() or "clausters_open failed")
        self._buf = ctypes.create_string_buffer(64 * 1024)

    def send(self, packet: bytes):
        if self._lib.clausters_send(self._h, packet, len(packet)) != 0:
            raise CommandRingFull("command ring full")

    def poll(self) -> bytes | None:
        n = self._lib.clausters_poll(self._h, self._buf, len(self._buf))
        return self._buf.raw[:n] if n > 0 else None

    def request(self, packet: bytes, timeout: float = 2.0) -> bytes:
        """Synchronous facade: blocks this thread until the reply arrives."""
        self.send(packet)
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            reply = self.poll()
            if reply is not None:
                return reply
            time.sleep(0.001)
        raise ReplyTimeout("no reply from the embedded server")

    @property
    def clock(self) -> int:
        """Block-accurate sample counter, straight from shared memory."""
        return self._lib.clausters_clock(self._h)

    @property
    def sample_rate(self) -> float:
        return self._lib.clausters_sample_rate(self._h)

    def ctl_set(self, index: int, value: float):
        self._lib.clausters_ctl_set(self._h, index, value)

    def ctl_get(self, index: int) -> float:
        return self._lib.clausters_ctl_get(self._h, index)

    def close(self):
        if self._h:
            self._lib.clausters_close(self._h)
            self._h = None

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()
