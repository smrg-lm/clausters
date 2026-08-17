"""Clausters Python bindings. Standard library only, by design.

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

ABI_VERSION = 9

#: The stride between successive stochastic-UGen seeds within one render —
#: ``SEED_STRIDE`` in ``clausters_core::rng``. A client needs it to reproduce a
#: server-side noise stream; it is **not** a starting seed (a render with no
#: seed draws a fresh one, and reports it).
SEED_STRIDE = 0x9E37_79B9_7F4A_7C15

# embed cdylib file names across platforms (Linux / macOS / Windows).
_EMBED_NAMES = ("libclausters.so", "libclausters.dylib", "clausters.dll")

# ---- segment layout (must match src/server/ipc.rs; pinned by tests) ----
# Fixed prefix: the header, then the c2s and s2c rings. The control-bus array
# is a trailing, dynamically-sized region after the rings (its length lives in
# the header), so `--control-buses` changes the segment size but not these
# ring offsets. ABI v3 adds a trailing **audio-tap region** after the control
# buses (`--taps` slots of a 64-byte cursor line + a `--tap-frames` sample
# ring, the region 64-byte aligned); this client reads the counts from the
# header but does not map-read the rings — headless tap capture goes over
# `/bus_tapStream` (see `Server.stream_taps`), the recorded G18 decision. ABI v4
# inserts the **audio-bus region** between the control buses and the taps: two
# words per audio bus, the bus -> tap directory then the per-bus block level, so
# a reader names a bus rather than a ring. The whole file is mmap'd, so any
# control/tap count is supported.

_MAGIC = 0x5541_4C43  # "CLAU"
_HEADER_SIZE = 64
_OFF_MAGIC = 0
_OFF_VERSION = 4
_OFF_SAMPLE_RATE = 8  # f64 bits
_OFF_CLOCK = 16  # u64
_OFF_CONTROL_BUSES = 28  # u32: number of slots in the trailing control region
_OFF_TAPS = 32  # u32: audio-tap ring count (ABI v3)
_OFF_TAP_FRAMES = 36  # u32: per-tap ring capacity in samples (ABI v3)
_OFF_AUDIO_BUSES = 40  # u32: audio-bus count of the bus region (ABI v4)
# u64: the transport clock (ABI v6) -- samples elapsed *under the transport*,
# held while it is stopped, where `_OFF_CLOCK` never stops. It sits in what was
# reserved header space, so v6 moved no other offset and did not change the
# segment size.
_OFF_TRANSPORT_CLOCK = 48
# u64: the transport **position** (ABI v8) -- where the transport is in the
# piece, in samples of the material. Not the clock above: that one is elapsed
# time and is monotonic, this one jumps wherever `/transport_locate` puts it
# and wraps at a loop's end. It spends the last of the reserved header space,
# so v8 moved no offset either; the next field added here will. ABI v9 appends
# the **buffer directory** as the segment's tail -- one row per pool buffer,
# saying what shape it is and which generation it is, so a local peer can map a
# buffer's samples by name (`server::ipc::BufferRow`). It is a trailing region,
# so again no offset here moved; what changed is the segment's total size, and
# the row count is what remains of the mapped length rather than a header field
# -- the header has no reserved space left.
_OFF_TRANSPORT_POSITION = 56
_RING_CAPACITY = 64 * 1024
_RING_HEADER = 64  # head u32, tail u32, padding
# Each frame inside a ring: the payload length and the peer tag, both u32 LE
# (ABI v7). The tag says who authored the packet on the way in and who the
# reply is for on the way out, which is what lets one segment carry several
# independent clients. The payload is padded to 4, so a frame stays 4-aligned.
_FRAME_HEADER = 8
#: The peer tag a client sends under when it does not pick one -- the single
#: client a segment used to have (``ipc::DEFAULT_PEER``).
DEFAULT_PEER = 0
_OFF_C2S = _HEADER_SIZE  # 64; rings come right after the header
_OFF_S2C = _OFF_C2S + _RING_HEADER + _RING_CAPACITY  # 65664
_OFF_CONTROLS = _OFF_S2C + _RING_HEADER + _RING_CAPACITY  # 131264 (trailing)
#: The server's own default (`dsp::NUM_CONTROL_BUSES`). It said 1024 until
#: 2026-08-17 -- a drift nothing caught, because a client maps the *file* and
#: only `SEGMENT_SIZE` below is derived from this. Pinned by a test now, since
#: the number is documentation and documentation that is wrong is worse than
#: none.
_DEFAULT_CONTROL_BUSES = 16384
_DEFAULT_TAPS = 8
_DEFAULT_TAP_FRAMES = 16384
_TAP_ALIGN = 64  # each tap slot: a 64-byte cursor line + the sample ring
_NUM_AUDIO_BUSES = 128  # the bus region is always sized for the engine's cap


def _bus_region_offset(control_buses: int) -> int:
    """Byte offset of the audio-bus region: right after the control slots."""
    return _OFF_CONTROLS + 4 * control_buses


def _buffer_region_offset(control_buses: int, taps: int, tap_frames: int) -> int:
    """Byte offset of the **buffer directory** (ABI v9): the tap region's end.

    The directory is the segment's tail, so how many rows it has is what is
    left of the mapped length rather than a field in the header — the header
    has no reserved space, and a count there would move the rings.
    """
    return _tap_region_offset(control_buses) + taps * (_TAP_ALIGN + 4 * tap_frames)


def _tap_region_offset(control_buses: int, audio_buses: int = _NUM_AUDIO_BUSES) -> int:
    buses_end = _bus_region_offset(control_buses) + 8 * audio_buses
    return (buses_end + _TAP_ALIGN - 1) // _TAP_ALIGN * _TAP_ALIGN


#: Bytes per **buffer-directory** row (ABI v9): the generation, the frames, the
#: channels and the sample rate — `server::ipc::BufferRow`.
_BUFFER_ROW = 24
#: Default directory rows: the server's default buffer count.
_DEFAULT_BUFFERS = 4096

# Segment size for the default control-bus, tap and buffer counts (the actual
# size is the file's length; the server sizes it from `--control-buses`/
# `--taps`/`--tap-frames`, and the directory is what remains of the length).
# Mirrors `src/server/ipc.rs::SEGMENT_SIZE`.
SEGMENT_SIZE = (
    _tap_region_offset(_DEFAULT_CONTROL_BUSES)
    + _DEFAULT_TAPS * (_TAP_ALIGN + 4 * _DEFAULT_TAP_FRAMES)
    + _DEFAULT_BUFFERS * _BUFFER_ROW)


class _Ring:
    """One SPSC byte ring inside the mapped segment (each frame a length and a
    peer tag, then the payload padded to 4). ``produce``/``consume`` depend on
    which side of the pair we are."""

    def __init__(self, mm: mmap.mmap, base: int):
        self.mm, self.base = mm, base

    def _cursor(self, off: int) -> int:
        return struct.unpack_from("<I", self.mm, self.base + off)[0]

    def _set_cursor(self, off: int, value: int):
        struct.pack_into("<I", self.mm, self.base + off, value & 0xFFFFFFFF)

    def push(self, packet: bytes, peer: int = DEFAULT_PEER) -> bool:
        head, tail = self._cursor(0), self._cursor(4)
        padded = (len(packet) + 3) // 4 * 4
        total = _FRAME_HEADER + padded
        if not packet or total > _RING_CAPACITY:
            return False
        if _RING_CAPACITY - ((head - tail) & 0xFFFFFFFF) < total:
            return False  # backpressure: retry later
        self._write(head, struct.pack("<II", len(packet), peer))
        self._write(head + _FRAME_HEADER, packet)
        self._set_cursor(0, head + total)  # publish last
        return True

    def pop(self) -> "tuple[int, bytes] | None":
        """The next frame as ``(peer, packet)``, or ``None`` when the ring is
        empty. ``peer`` is who authored it (inbound) or who it is for
        (outbound)."""
        head, tail = self._cursor(0), self._cursor(4)
        used = (head - tail) & 0xFFFFFFFF
        if used == 0:
            return None
        length, peer = struct.unpack("<II", self._read(tail, _FRAME_HEADER))
        total = _FRAME_HEADER + (length + 3) // 4 * 4
        if length == 0 or total > used:
            self._set_cursor(4, head)  # resync: drop garbage
            return None
        packet = self._read(tail + _FRAME_HEADER, length)
        self._set_cursor(4, tail + total)
        return peer, packet

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
        # Map the whole file: its length is the server's --control-buses count.
        self.mm = mmap.mmap(self._file.fileno(), 0)
        magic, version = struct.unpack_from("<II", self.mm, 0)
        if magic != _MAGIC:
            raise SegmentError(f"{path} is not a clausters segment")
        if version != ABI_VERSION:
            raise SegmentError(f"segment ABI v{version}, this client speaks v{ABI_VERSION}")
        #: control-bus count the server created the segment with.
        self.control_buses = struct.unpack_from("<I", self.mm, _OFF_CONTROL_BUSES)[0]
        #: audio-tap ring count and per-tap capacity in samples (ABI v3). The
        #: rings themselves are read by the GUI host; capture them from Python
        #: over ``/bus_tapStream`` (``Server.stream_taps``) instead.
        self.taps = struct.unpack_from("<I", self.mm, _OFF_TAPS)[0]
        self.tap_frames = struct.unpack_from("<I", self.mm, _OFF_TAP_FRAMES)[0]
        #: audio-bus count of the bus region (ABI v4), the length of both the
        #: bus -> tap directory and the level table.
        self.audio_buses = struct.unpack_from("<I", self.mm, _OFF_AUDIO_BUSES)[0]
        self._buses_at = _bus_region_offset(self.control_buses)
        #: Byte offset and row count of the buffer directory (ABI v9): where
        #: this server's material is, buffer by buffer.
        self._buffers_at = _buffer_region_offset(
            self.control_buses, self.taps, self.tap_frames)
        self.buffers = max(0, (len(self.mm) - self._buffers_at) // _BUFFER_ROW)
        self._path = path
        self._c2s = _Ring(self.mm, _OFF_C2S)  # we produce here
        self._s2c = _Ring(self.mm, _OFF_S2C)  # we consume here

    # -- data plane: no commands, just shared memory --

    @property
    def clock(self) -> int:
        """The engine's sample counter, mirrored every block (64 samples)."""
        return struct.unpack_from("<Q", self.mm, _OFF_CLOCK)[0]

    @property
    def transport_clock(self) -> int:
        """Samples elapsed **under the transport**, held while it is stopped.

        The counterpart of `clock`, which never stops. A view drawing where the
        piece is reads this one; anything pacing on the device reads `clock`.
        The two only differ while a transport with a governed group is
        stopped.
        """
        return struct.unpack_from("<Q", self.mm, _OFF_TRANSPORT_CLOCK)[0]

    @property
    def transport_position(self) -> int:
        """Where the transport is **in the piece**, in samples of the material.

        Not a clock. `transport_clock` counts what has elapsed and only goes
        forward; this says where the piece is, so it jumps to wherever
        `/transport_locate` puts it and wraps at the end of a loop. A playhead
        reads this one.
        """
        return struct.unpack_from("<Q", self.mm, _OFF_TRANSPORT_POSITION)[0]

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

    def level(self, bus: int) -> float:
        """Audio bus `bus`'s level: the peak magnitude of the engine's last
        block. One number per block, published for every audio bus — what a
        meter reads, and why metering a bus costs no tap ring.
        """
        if not 0 <= bus < self.audio_buses:
            raise IndexError(f"audio bus {bus} out of range 0..{self.audio_buses}")
        return struct.unpack_from("<f", self.mm, self._buses_at + 4 * self.audio_buses + 4 * bus)[0]

    def tap_of_bus(self, bus: int) -> "int | None":
        """Which tap ring is recording audio bus `bus`, or ``None``. The ring
        index is the segment's own bookkeeping — a reader names the bus.
        """
        if not 0 <= bus < self.audio_buses:
            raise IndexError(f"audio bus {bus} out of range 0..{self.audio_buses}")
        tap = struct.unpack_from("<i", self.mm, self._buses_at + 4 * bus)[0]
        return tap if tap >= 0 else None

    def buffer_info(self, bufnum: int) -> "tuple[int, int, int, float] | None":
        """What the directory says buffer `bufnum` is: ``(generation, frames,
        channels, sample_rate)``, or ``None`` when the slot is empty.

        The **generation** is the number that does three jobs: it is odd while
        the buffer is live and even when the slot is empty, it names the region
        file, and it is the seqlock this read is taken under — a row caught
        mid-write is re-read rather than believed.
        """
        if not 0 <= bufnum < self.buffers:
            return None
        at = self._buffers_at + _BUFFER_ROW * bufnum
        for _ in range(8):
            before = struct.unpack_from("<Q", self.mm, at)[0]
            if before % 2 == 0:
                return None
            frames, channels = struct.unpack_from("<II", self.mm, at + 8)
            rate = struct.unpack_from("<d", self.mm, at + 16)[0]
            if struct.unpack_from("<Q", self.mm, at)[0] == before:
                return before, frames, channels, rate
        return None

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
        path = f"{self._path}.buf{bufnum}.{generation}"
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
        return self._c2s.push(packet, peer)

    def poll(self, peer: int = DEFAULT_PEER) -> bytes | None:
        """The next reply addressed to `peer`, or ``None``.

        Popping advances the one shared cursor, so a frame for a *different*
        peer is dropped here rather than left for its owner: a process holding
        several clients over one segment reads with `poll_any` and routes,
        which is what the browser page does. With one client -- what a Python
        process has -- there is nothing to route and this is the whole story.
        """
        frame = self._s2c.pop()
        if frame is None:
            return None
        to, packet = frame
        return packet if to == peer else None

    def poll_any(self) -> "tuple[int, bytes] | None":
        """The next reply as ``(peer, packet)``, whoever it is for — the door a
        process holding several clients over one segment routes with."""
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
