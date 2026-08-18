"""Clausters Python bindings: the standard library, plus the shared core.

Two ways in, both speaking ordinary OSC bytes (build them however you like;
``examples/json_client.py`` has stdlib helpers):

- `Clausters` — the **embedded** server: loads the cdylib (build it
  with ``cargo build --release --features embed,realtime``) and runs the
  whole server in-process. Commands are function calls, no network anywhere.
- `ShmClient` — attaches to a **separate** server started with
  ``clausters --shm <path>``: commands/replies travel through a
  shared-memory ring, and the *data plane* (sample clock, control buses) is
  read and written directly in mapped memory.

Plus `render` — the synchronous "scientific" call: hand it a binary
score, get the interleaved float32 samples back, no server running at all.

Boundary rule (project-wide): only flat data crosses — ``bytes`` in,
``array('f')``/floats/ints out. A numpy user can wrap the results without
copying (``numpy.frombuffer``), but nothing here imports anything heavy.

The one exception to "standard library only" is the segment's **layout**:
`ShmClient` maps the file with `mmap` and asks the shared core
(`clausters-ffi`, through `clausters._native`) where everything in it is, for
the directory's seqlock and for the ring's framing. Those used to be
transcribed here, which is how this binding came to declare 1024 control buses
against a server that had had 16 384 for months — wrong, unused, and caught by
nothing. A layout mirrored by hand is a layout that drifts, so it is asked for
rather than restated.

Caveats of the shm path: Python has no atomics, so what it reads through the
mapping relies on x86-TSO-style ordering of aligned accesses — fine on the
supported platforms, documented in docs/ipc.md.
"""

import ctypes
import mmap
import os
import struct
import time
from array import array

from . import _libpath, _native
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

ABI_VERSION = 10

#: The stride between successive stochastic-UGen seeds within one render —
#: ``SEED_STRIDE`` in ``clausters_core::rng``. A client needs it to reproduce a
#: server-side noise stream; it is **not** a starting seed (a render with no
#: seed draws a fresh one, and reports it).
SEED_STRIDE = 0x9E37_79B9_7F4A_7C15

# embed cdylib file names across platforms (Linux / macOS / Windows).
_EMBED_NAMES = ("libclausters.so", "libclausters.dylib", "clausters.dll")

# ---- segment layout ---------------------------------------------------------
#
# There is none here, and that is the point. `clausters_core::shm` is the one
# definition of the segment's layout; this client maps the file and asks it for
# every offset and count (`_native.shm_shape`), for the buffer directory's
# seqlock and for the ring's framing. What used to sit here was a copy of the
# arithmetic, kept honest by a version number — which cannot check a layout,
# and did not.

_MAGIC = 0x5541_4C43  # "CLAU", read only to say "this is not a segment" nicely

#: The peer tag a client sends under when it does not pick one -- the single
#: client a segment used to have (``ipc::DEFAULT_PEER``).
DEFAULT_PEER = 0

#: Which end of the ring pair this side holds, as the core's C door takes it:
#: a client writes commands and drains replies.
_ROLE_SERVER = 0
_ROLE_CLIENT = 1


class MappedBuffer:
    """One pool buffer's samples, mapped — the material itself, not a copy.

    ``samples`` is a writable ``memoryview`` of ``f32`` in the buffer's own
    interleaved order, so reading is a memory read and writing is what the
    engine plays next block. Per-sample only: a reader crossing a writer sees
    some old samples and some new, never half of one, which is the rule the
    whole buffer model states (`docs/ipc.md`).

    Close it when done — or use it as a context manager. The mapping stays
    valid even after the buffer is freed (the region is unlinked, not deleted),
    which is what makes freeing a take safe while somebody is drawing it; what
    tells you it is history is the directory, through `ShmClient.buffer_info`.
    """

    __slots__ = ("_mm", "generation", "frames", "channels", "sample_rate", "samples")

    def __init__(self, mm, generation: int, frames: int, channels: int, rate: float):
        self._mm = mm
        #: Odd, and the number the region file is named with.
        self.generation = generation
        self.frames = frames
        self.channels = channels
        self.sample_rate = rate
        #: The samples, interleaved, writable.
        self.samples = memoryview(mm).cast("f")

    def __enter__(self) -> "MappedBuffer":
        return self

    def __exit__(self, *exc) -> bool:
        self.close()
        return False

    def close(self):
        """Unmaps. Idempotent."""
        if self._mm is not None:
            self.samples.release()
            self._mm.close()
            self._mm = None

    def __repr__(self) -> str:
        return (f"MappedBuffer(generation={self.generation}, frames={self.frames}, "
                f"channels={self.channels}, sample_rate={self.sample_rate})")


class ShmClient:
    """Client of a ``clausters --shm <path>`` server (same machine)."""

    def __init__(self, path: str):
        self._file = open(path, "r+b")
        # Map the whole file: the server sized it from its own --control-buses,
        # --taps and --tap-frames, and the buffer directory is what remains of
        # that length.
        self.mm = mmap.mmap(self._file.fileno(), 0)
        # The address the core reads through. `from_buffer` keeps the mapping
        # pinned for as long as this object holds it, which is what makes
        # handing the pointer across the boundary safe.
        self._cell = (ctypes.c_char * len(self.mm)).from_buffer(self.mm)
        self._addr = ctypes.addressof(self._cell)
        magic = struct.unpack_from("<I", self.mm, 0)[0]
        if magic != _MAGIC:
            self._release()
            raise SegmentError(f"{path} is not a clausters segment")
        #: Where everything in this segment is, as the shared core reports it —
        #: the one place the layout is known.
        self.shape = _native.shm_shape(self._addr, len(self.mm))
        if self.shape is None:
            version = struct.unpack_from("<I", self.mm, 4)[0]
            self._release()
            raise SegmentError(
                f"segment ABI v{version}, this client speaks v{ABI_VERSION}"
                if version != ABI_VERSION else f"{path} is not a valid segment")
        #: control-bus count the server created the segment with.
        self.control_buses = self.shape.control_buses
        #: audio-tap ring count and per-tap capacity in samples. The rings
        #: themselves are read by the GUI host; capture them from Python over
        #: ``/bus_tapStream`` (``Server.stream_taps``) instead.
        self.taps = self.shape.taps
        self.tap_frames = self.shape.tap_frames
        #: audio-bus count of the bus region, the length of both the bus -> tap
        #: directory and the level table.
        self.audio_buses = self.shape.audio_buses
        #: How many buffers this segment's directory can describe.
        self.buffers = self.shape.buffer_rows
        self._path = path

    def _release(self):
        """Drops the pinning view and then the mapping, in that order — a
        `mmap` refuses to close while an exported buffer is outstanding, and
        the view is what exports it. Nothing may hold a *local* reference to it
        either, which is why this drops the attribute and never names it."""
        self._cell = None
        if getattr(self, "mm", None) is not None:
            self.mm.close()
            self.mm = None
        if getattr(self, "_file", None) is not None:
            self._file.close()
            self._file = None

    # -- data plane: no commands, just shared memory --

    @property
    def clock(self) -> int:
        """The engine's sample counter, mirrored every block (64 samples)."""
        return struct.unpack_from("<Q", self.mm, self.shape.clock_offset)[0]

    @property
    def transport_clock(self) -> int:
        """Samples elapsed **under the transport**, held while it is stopped.

        The counterpart of `clock`, which never stops. A view drawing where the
        piece is reads this one; anything pacing on the device reads `clock`.
        The two only differ while a transport with a governed group is
        stopped.
        """
        return struct.unpack_from("<Q", self.mm, self.shape.transport_clock_offset)[0]

    @property
    def transport_position(self) -> int:
        """Where the transport is **in the piece**, in samples of the material.

        Not a clock. `transport_clock` counts what has elapsed and only goes
        forward; this says where the piece is, so it jumps to wherever
        `/transport_locate` puts it and wraps at the end of a loop. A playhead
        reads this one.
        """
        return struct.unpack_from("<Q", self.mm, self.shape.transport_position_offset)[0]

    @property
    def sample_rate(self) -> float:
        return struct.unpack_from("<d", self.mm, self.shape.sample_rate_offset)[0]

    def ctl_get(self, index: int) -> float:
        if not 0 <= index < self.control_buses:
            raise IndexError(f"control bus {index} out of range 0..{self.control_buses}")
        return struct.unpack_from("<f", self.mm, self.shape.controls_offset + 4 * index)[0]

    def ctl_set(self, index: int, value: float):
        """Writes the very atomic the engine's InCtl reads next block."""
        if not 0 <= index < self.control_buses:
            raise IndexError(f"control bus {index} out of range 0..{self.control_buses}")
        struct.pack_into("<f", self.mm, self.shape.controls_offset + 4 * index, value)

    def level(self, bus: int) -> float:
        """Audio bus `bus`'s level: the peak magnitude of the engine's last
        block. One number per block, published for every audio bus — what a
        meter reads, and why metering a bus costs no tap ring.
        """
        if not 0 <= bus < self.audio_buses:
            raise IndexError(f"audio bus {bus} out of range 0..{self.audio_buses}")
        return struct.unpack_from(
            "<f", self.mm, self.shape.buses_offset + 4 * (self.audio_buses + bus))[0]

    def tap_of_bus(self, bus: int) -> "int | None":
        """Which tap ring is recording audio bus `bus`, or ``None``. The ring
        index is the segment's own bookkeeping — a reader names the bus.
        """
        if not 0 <= bus < self.audio_buses:
            raise IndexError(f"audio bus {bus} out of range 0..{self.audio_buses}")
        tap = struct.unpack_from("<i", self.mm, self.shape.buses_offset + 4 * bus)[0]
        return tap if tap >= 0 else None

    def buffer_info(self, bufnum: int) -> "tuple[int, int, int, float] | None":
        """What the directory says buffer `bufnum` is: ``(generation, frames,
        channels, sample_rate)``, or ``None`` when the slot is empty.

        The **generation** is the number that does three jobs: it is odd while
        the buffer is live and even when the slot is empty, it names the region
        file, and it is the seqlock the read is taken under — a row caught
        mid-write is re-read rather than believed. The retry is the shared
        core's, not a second implementation of it here.
        """
        return _native.shm_buffer_info(self._addr, len(self.mm), bufnum)

    def map_buffer(self, bufnum: int) -> "MappedBuffer | None":
        """Maps buffer `bufnum`'s **samples**, or ``None`` when there is no such
        buffer (or its region cannot be opened, which is what a buffer freed
        between the two steps looks like).

        This is the data plane's other half: the samples are not messages. What
        comes back is the server's own memory — writing a sample is what the
        engine reads on the next block, exactly as a control-bus write is — so
        `clausters.defs.Buffer.get_samples` is a *fetch* only for a client that
        cannot map the segment.

        **What may be written here is material, not computation**: samples a
        caller already holds (a drawn stroke, a pasted block). Every operation
        over samples — a gain, a fade, a reverse, a render — stays a command,
        because one place performs audio processing and it is the server.
        """
        info = self.buffer_info(bufnum)
        if info is None:
            return None
        generation, frames, channels, rate = info
        path = self._path + _native.shm_region_suffix(bufnum, generation)
        try:
            handle = open(path, "r+b")
        except OSError:
            return None
        with handle:
            data = mmap.mmap(handle.fileno(), frames * channels * 4)
        # A row that moved while the file was being opened describes a buffer
        # this mapping is not.
        if (self.buffer_info(bufnum) or (0,))[0] != generation:
            data.close()
            return None
        return MappedBuffer(data, generation, frames, channels, rate)

    # -- command plane: OSC packets through the ring --

    def send(self, packet: bytes, peer: int = DEFAULT_PEER) -> bool:
        """Pushes one OSC packet, authored by `peer`. The tag is the caller's to
        assign — the server only has to tell its clients apart, not name them —
        and a client that never picks one is the single client a segment used to
        have (`DEFAULT_PEER`)."""
        return _native.shm_push(self._addr, len(self.mm), _ROLE_CLIENT, peer, packet)

    def poll(self, peer: int = DEFAULT_PEER) -> bytes | None:
        """The next reply addressed to `peer`, or ``None``.

        Popping advances the one shared cursor, so a frame for a *different*
        peer is dropped here rather than left for its owner: a process holding
        several clients over one segment reads with `poll_any` and routes,
        which is what the browser page does. With one client -- what a Python
        process has -- there is nothing to route and this is the whole story.
        """
        frame = self.poll_any()
        if frame is None:
            return None
        to, packet = frame
        return packet if to == peer else None

    def poll_any(self) -> "tuple[int, bytes] | None":
        """The next reply as ``(peer, packet)``, whoever it is for — the door a
        process holding several clients over one segment routes with."""
        return _native.shm_pop(self._addr, len(self.mm), _ROLE_CLIENT)

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
        """Unmaps the segment. Idempotent."""
        self._release()


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
    `LibraryFeatureError` that names the symbol and the Cargo feature to
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
        # ... workers, seed (NULL = draw one), out_frames, out_events, out_seed
        ctypes.c_char_p, ctypes.c_size_t, ctypes.c_double, ctypes.c_uint32,
        ctypes.c_uint32, ctypes.POINTER(ctypes.c_uint64),
        ctypes.POINTER(ctypes.c_uint64), ctypes.POINTER(ctypes.c_uint64),
        ctypes.POINTER(ctypes.c_uint64), ctypes.c_char_p, ctypes.c_size_t,
    ]
    _require(lib, "clausters_free_samples", "embed").argtypes = [
        ctypes.POINTER(ctypes.c_float), ctypes.c_uint64]
    read_fn = _require(lib, "clausters_read_soundfile", "embed")
    read_fn.restype = ctypes.POINTER(ctypes.c_float)
    read_fn.argtypes = [
        ctypes.c_char_p, ctypes.c_uint64, ctypes.c_int64,
        ctypes.POINTER(ctypes.c_uint64), ctypes.POINTER(ctypes.c_uint32),
        ctypes.POINTER(ctypes.c_double), ctypes.c_char_p, ctypes.c_size_t,
    ]

    # `embed,realtime` surface: the live embedded server. Optional at load
    # time so `render()` works with an `embed`-only build; if it is absent the
    # error is deferred to `Clausters()`, where it is actually needed.
    lib._clausters_live_error = _bind_live(lib)
    return lib


def _bind_live(lib: ctypes.CDLL) -> LibraryFeatureError | None:
    """Bind the live-server symbols. Returns the `LibraryFeatureError`
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
           workers: int = 0, lib_path: str | None = None,
           seed: int | None = None) -> tuple[array, int, int, int]:
    """Synchronous offline render: binary score in, ``(samples, frames,
    events, seed)`` out — the samples interleaved float32 in a stdlib
    ``array('f')``. The whole call blocks the caller and nothing else; there is
    no server involved.

    ``seed`` starts the render's stochastic UGens. ``None`` (the default) draws
    a fresh one, so the same score is a new take every time; the seed actually
    used is the fourth element of the result, and passing it back replays that
    take exactly.
    """
    lib = _load(lib_path)
    frames = ctypes.c_uint64(0)
    events = ctypes.c_uint64(0)
    used = ctypes.c_uint64(0)
    err = ctypes.create_string_buffer(512)
    want = None if seed is None else ctypes.byref(ctypes.c_uint64(seed))
    ptr = lib.clausters_render(score, len(score), sample_rate, channels,
                               workers, want, ctypes.byref(frames),
                               ctypes.byref(events), ctypes.byref(used),
                               err, len(err))
    if not ptr:
        raise RenderError(err.value.decode() or "render failed")
    total = frames.value * channels
    samples = array("f", ctypes.cast(ptr, ctypes.POINTER(ctypes.c_float * total)).contents)
    lib.clausters_free_samples(ptr, total)
    return samples, frames.value, events.value, used.value


def read_soundfile(path, start: int = 0, frames: int = -1,
                   lib_path: str | None = None) -> tuple[array, int, int, float]:
    """Decode an audio file through the server's decoder: ``(samples, frames,
    channels, sample_rate)``, the samples interleaved float32 in an
    ``array('f')``.

    WAV goes through hound, everything else (FLAC, OGG/Vorbis, MP3, MP4/AAC,
    ALAC, AIFF, ...) through symphonia; integer files are scaled to
    ``[-1, 1]``. The rate is the file's own — nothing resamples.
    """
    lib = _load(lib_path)
    n = ctypes.c_uint64(0)
    chans = ctypes.c_uint32(0)
    rate = ctypes.c_double(0.0)
    err = ctypes.create_string_buffer(512)
    ptr = lib.clausters_read_soundfile(
        str(path).encode(), start, frames, ctypes.byref(n), ctypes.byref(chans),
        ctypes.byref(rate), err, len(err))
    if not ptr:
        raise RenderError(err.value.decode() or f"could not read {path}")
    total = n.value * chans.value
    samples = array("f", ctypes.cast(ptr, ctypes.POINTER(ctypes.c_float * total)).contents)
    lib.clausters_free_samples(ptr, total)
    return samples, n.value, chans.value, rate.value


def channel_stats(samples, channels: int) -> tuple[tuple[float, ...], tuple[float, ...]]:
    """Per-channel ``(peak, rms)`` of an interleaved buffer, measured by the
    shared core (`clausters_core_stats`) so the numbers match the server's.

    Returns two tuples in channel order. Falls back to a Python pass if the
    core library is not loadable, so a stats read never fails a render.
    """
    if channels <= 0 or not samples:
        return (), ()
    try:
        from . import _native

        lib = _native.lib()
        buf = (ctypes.c_float * len(samples)).from_buffer_copy(samples)
        out = (ctypes.c_float * 2)()
        peak, rms = [], []
        for c in range(channels):
            lib.clausters_core_stats(buf, len(samples), channels, c, out)
            peak.append(float(out[0]))
            rms.append(float(out[1]))
        return tuple(peak), tuple(rms)
    except Exception:
        peak, rms = [], []
        for c in range(channels):
            ch = samples[c::channels]
            peak.append(max((abs(x) for x in ch), default=0.0))
            rms.append((sum(x * x for x in ch) / len(ch)) ** 0.5 if ch else 0.0)
        return tuple(peak), tuple(rms)


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
