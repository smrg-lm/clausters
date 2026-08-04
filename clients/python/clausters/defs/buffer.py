"""Buffers, with client-side index allocation.

The server's buffer pool is a finite boot-time resource (``--max-buffers``,
4096 by default), indices allocated by the client (like scsynth). A `Buffer`
holds an index and the server it lives on, and owns the ``/buffer_*`` commands
addressed to it: `Buffer.alloc` and `Buffer.read` create one, and `Buffer.gen`,
`Buffer.write`, `Buffer.query`, `Buffer.get_samples`, `Buffer.set_samples` and
`Buffer.free` drive it. The read and write pair is what an editor view needs:
samples come back client-side, are edited there, and go back into the buffer.

The allocator is a **registry** (the core's occupancy map): a freed slot is
always reusable, a double free is refused loudly, exhaustion raises instead of
wrapping. The `Server` sizes it from its `ServerOptions` (``max_buffers``).
"""

from array import array

from .. import _native
from ..errors import CommandError
from .info import BufferInfo, parse_buffer_list
from ._wire import resolve as _resolve

NUM_BUFFERS = 4096


class Buffer:
    """A block of samples the server holds, and a synth reads or writes.

    Where a `Bus` is a patch point that forgets everything each block, a
    buffer **stays**: it is memory on the server, addressed by a slot number,
    that outlives every node touching it. That makes it the one place a piece
    keeps something with random access — a sound file to play back or granulate,
    a wavetable an oscillator scans, a delay line, an impulse response, a
    recording being written while it plays.

    A buffer is not itself audible. Something has to read it: the ``play_buf``
    UGen for straight playback, ``buf_rd`` for reading at an index you compute
    (a phasor, a granulator's random offsets), ``vosc``/``shaper`` for
    wavetables, ``disk_in``/``disk_out`` to stream past what fits in memory. So
    a buffer usually reaches your ears the same way anything else does — through
    a `Synth` whose def names its ``bufnum``.

    **Allocation is asynchronous, and that is the thing to get right.** The
    server does the work on a worker thread and answers ``/done``, so
    `alloc` and `read` block for that reply by default; sequenced offline (NRT)
    they are scored at time 0 instead, so the renderer has the samples before
    time advances. Either way the buffer is ready when the call returns. Pass
    ``wait=False`` only when you are going to sequence the barrier yourself
    with `Server.sync` — and never block inside a routine.

    The shape (`frames`, `channels`, `sample_rate`) is cached, because unlike a
    node's state a buffer only changes shape when you change it. `info`
    refreshes it from the server.

    A sound file read in, played back, and its samples pulled to the client:

    ```python
    from clausters import Buffer, Server, Synth, SynthDef
    from clausters.defs import control, out, play_buf

    s = Server.boot()
    d = SynthDef("player",
                 out(0, play_buf(control("bufnum", 0.0), 0.0,
                                 control("rate", 1.0)) * 0.5))
    d.send(s)

    b = Buffer.read("sample.wav", server=s)      # blocks until /done
    print(b.frames, b.channels, b.sample_rate)   # the shape, already known

    n = Synth("player", {"bufnum": b.bufnum}, server=s)

    head = b.get_samples(0, 64)                  # the first 64 frames, client-side
    b.set_samples([v * 0.5 for v in head])       # edited, and written back
    n.free()
    b.free()                                     # the slot returns to the pool
    ```

    Attributes:
        bufnum: the slot in the server's pool. This is the number a def names,
            so it is what travels into a synth's ``bufnum`` control.
        frames: frames per channel, 0 while unknown.
        channels: channels per frame.
        sample_rate: the server's rate for these samples, 0.0 while unknown.
        server: the `Server` this buffer lives on; `None` falls back to the
            ambient one.
    """

    def __init__(self, bufnum: int, frames: int = 0, channels: int = 1,
                 sample_rate: float = 0.0, server=None):
        """Names an existing slot by number, for a buffer the server already
        holds — one another client allocated, or one `Server.query_buffers`
        reported. Sends nothing, and knows nothing about the shape until you
        ask (`info`). To get a **new** buffer, use `alloc` (empty) or `read`
        (from a sound file), which is what a piece normally wants.

        Args:
            bufnum: the slot number.
            frames: frames per channel, if already known.
            channels: channels per frame.
            sample_rate: the samples' rate, if already known.
            server: the `Server` it lives on; `None` takes the ambient one.
        """
        #: what the server holds under this slot, as last read from it — a
        #: buffer's shape only changes by a command of yours, so unlike a
        #: node's record this one can be kept. `info` refreshes it; `frames`,
        #: `channels` and `sample_rate` read it.
        self._info = BufferInfo(bufnum=bufnum, frames=frames, channels=channels,
                                sample_rate=sample_rate)
        #: the `Server` this buffer lives on (set by `alloc` / `read`), so its
        #: commands know where to go without being told.
        self.server = server

    @property
    def bufnum(self) -> int:
        """The slot this buffer occupies in the server's pool."""
        return self._info.bufnum

    @property
    def frames(self) -> int:
        """Frames per channel, 0 while unknown (see `info`)."""
        return self._info.frames

    @property
    def channels(self) -> int:
        return self._info.channels

    @property
    def sample_rate(self) -> float:
        """The server's rate for this buffer, 0.0 while unknown (see `info`)."""
        return self._info.sample_rate

    # ---- constructors ----

    @classmethod
    def alloc(cls, frames: int, channels: int = 1, *, wait: bool = True,
              timeout: "float | None" = None, server=None) -> "Buffer":
        """Allocates a zeroed buffer. In NRT it scores ``/buffer_alloc`` at time 0
        (so the renderer installs it before time advances); in RT ``wait=True``
        (default) blocks on ``/done``, ``wait=False`` is fire-and-forget."""
        srv = _resolve(server)
        bufnum = srv.buffers.alloc()
        buf = cls(bufnum, frames, channels, server=srv)
        if buf._scored() or not wait:
            srv.send_msg("/buffer_alloc", bufnum, frames, channels)
            return buf
        addr, args = srv.request("/buffer_alloc", bufnum, frames, channels,
                                 timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            srv.buffers.free(bufnum)
            raise CommandError(f"/buffer_alloc {bufnum} failed: {args}")
        return buf

    @classmethod
    def read(cls, path, *, file_start: int = 0, num_frames: int = 0,
             wait: bool = True, timeout: "float | None" = None, server=None) -> "Buffer":
        """Allocate a buffer and read a sound file into it (``/buffer_allocRead``): the
        shape and sample rate come from the file (``num_frames`` 0 = the whole
        file, from ``file_start``). Decoding is by content (WAV, FLAC, OGG, MP3,
        …). In NRT it scores at time 0; in RT ``wait=True`` blocks on ``/done``.

        The shape is the **file's**, so the client cannot know it in advance:
        waiting reads it back (one `info` round trip) and the returned buffer
        carries it. Not waiting — and NRT, which has no reply — leaves
        ``frames``/``channels`` at 0 until you call `info` yourself."""
        srv = _resolve(server)
        bufnum = srv.buffers.alloc()
        buf = cls(bufnum, server=srv)
        extra = [int(file_start), int(num_frames)] if (file_start or num_frames) else []
        if buf._scored() or not wait:
            srv.send_msg("/buffer_allocRead", bufnum, str(path), *extra)
            return buf
        addr, args = srv.request("/buffer_allocRead", bufnum, str(path), *extra,
                                 timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            srv.buffers.free(bufnum)
            raise CommandError(f"/buffer_allocRead {bufnum} {path!r} failed: {args}")
        buf.info(timeout=timeout)
        return buf

    # ---- the commands addressed to this buffer ----

    def gen(self, cmd: str, *args, wait: bool = True, timeout: "float | None" = None):
        """Fills this buffer through ``/buffer_gen`` (the wavetable/generator commands:
        ``"env"``, ``"sine1"``/``"sine2"``/``"sine3"``, ``"cheby"``, ``"copy"``,
        and ``"prepare_partconv" fft_size ir_bufnum`` — the partitioned-kernel
        preparation the `conv` UGen reads; size the target with
        `clausters.defs.partconv_frames`).
        Like `alloc`: NRT scores at time 0; RT ``wait=True`` blocks on
        ``/done``, ``wait=False`` is fire-and-forget."""
        srv = self._server()
        if self._scored() or not wait:
            srv.send_msg("/buffer_gen", self.bufnum, cmd, *args)
            return
        addr, rargs = srv.request("/buffer_gen", self.bufnum, cmd, *args,
                                  timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/buffer_gen {self.bufnum} {cmd} failed: {rargs}")

    def read_into(self, path, *, file_start: int = 0, num_frames: int = -1,
                  buf_start: int = 0, wait: bool = True, timeout: "float | None" = None):
        """Read a sound file into this buffer (``/buffer_read``), keeping its shape.
        NRT scores at time 0; RT ``wait=True`` blocks on ``/done``."""
        srv = self._server()
        args = [self.bufnum, str(path), int(file_start), int(num_frames),
                int(buf_start)]
        if self._scored() or not wait:
            srv.send_msg("/buffer_read", *args)
            return
        addr, rargs = srv.request("/buffer_read", *args, timeout=timeout,
                                  expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/buffer_read {self.bufnum} {path!r} failed: {rargs}")

    def write(self, path, *, sample_format: str = "int16", num_frames: int = -1,
              buf_start: int = 0, wait: bool = True, timeout: "float | None" = None):
        """Write this buffer to a WAV file (``/buffer_write``); ``sample_format`` is
        ``"int16"``, ``"int24"`` or ``"float"``. NRT scores at time 0; RT
        ``wait=True`` blocks on ``/done``."""
        srv = self._server()
        args = [self.bufnum, str(path), "wav", str(sample_format),
                int(num_frames), int(buf_start)]
        if self._scored() or not wait:
            srv.send_msg("/buffer_write", *args)
            return
        addr, rargs = srv.request("/buffer_write", *args, timeout=timeout,
                                  expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/buffer_write {self.bufnum} {path!r} failed: {rargs}")

    def zero(self, *, wait: bool = True, timeout: "float | None" = None):
        """Zero this buffer (``/buffer_zero``). NRT scores at time 0; RT
        ``wait=True`` blocks on ``/done``."""
        srv = self._server()
        if self._scored() or not wait:
            srv.send_msg("/buffer_zero", self.bufnum)
            return
        addr, rargs = srv.request("/buffer_zero", self.bufnum, timeout=timeout,
                                  expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/buffer_zero {self.bufnum} failed: {rargs}")

    def info(self, timeout: "float | None" = None) -> BufferInfo:
        """Ask the running server what it holds in this slot (``/buffer_query`` →
        ``/buffer_query.reply bufnum frames channels sampleRate``), keep the record on the
        handle and return it.

        Unlike a node's, a buffer's record is worth keeping: its shape changes
        only by a command of yours, so what this reads stays true until you
        change it. A slot with nothing in it (never allocated, or freed) comes
        back with ``exists`` false rather than raising. RT only (it needs a
        reply)."""
        _, args = self._server().request("/buffer_query", self.bufnum,
                                         timeout=timeout, expect=("/buffer_query.reply",))
        self._info = parse_buffer_list(args)[0]
        return self._info

    def get_samples(self, start: int = 0, count: int = -1, *,
                    chunk: "int | None" = None, timeout: "float | None" = None):
        """Fetch interleaved samples from this buffer (``/buffer_getRange`` →
        ``/buffer_getRange.reply``), in chunks, as a stdlib ``array('f')``. ``count`` -1 = to
        the end (the shape is queried first). RT only (it needs replies); for
        display the GUI host fetches buffers itself.

        ``chunk`` (samples per round-trip) defaults to the transport's bound:
        over a stream transport (TCP/WebSocket) it is sized from the frame
        ceiling the server advertises in ``/server_query`` — megabytes per
        reply — while over UDP each reply must fit a datagram, so it stays at
        1024. Pass an explicit ``chunk`` to override either."""
        srv = self._server()
        if chunk is None:
            chunk = srv._bulk_chunk(timeout)
        if count < 0:
            shape = self.info(timeout=timeout)
            total = shape.frames * shape.channels
            count = max(0, total - start)
        out = array("f")
        got = 0
        while got < count:
            n = min(chunk, count - got)
            _, args = srv.request("/buffer_getRange", self.bufnum, start + got, n,
                                  timeout=timeout, expect=("/buffer_getRange.reply",))
            # /buffer_getRange.reply: bufnum, start, count, value...
            out.extend(float(v) for v in args[3:3 + int(args[2])])
            got += n
        return out

    def set_samples(self, samples, start: int = 0, *, chunk: "int | None" = None,
                    wait: bool = True, timeout: "float | None" = None):
        """Write interleaved samples into this buffer (``/buffer_setRange``), in
        chunks — the write half of `get_samples`, and the step that closes an
        editor's read → edit → write cycle.

        ``samples`` is any sequence of numbers (a list, an ``array('f')``, what
        `get_samples` returned) laid down from flat index ``start``. Indices are
        flat across channels, so a stereo buffer is written interleaved
        ``L R L R ...``, exactly as it reads back.

        The buffer must already exist and keeps its shape: writing past its end
        raises rather than being clamped, since a short write would lose samples
        you believe you stored. The shape is read from the server's mirror, so a
        write immediately after `alloc` needs the alloc to have completed —
        which ``wait=True`` (the default) on that call already guarantees.

        ``chunk`` sizes each round trip and defaults to the transport's bound,
        exactly as in `get_samples`. NRT scores at time 0; RT ``wait=True``
        blocks on each chunk's ``/done``.
        """
        srv = self._server()
        values = [float(s) for s in samples]
        if not values:
            return
        if chunk is None:
            chunk = srv._bulk_chunk(timeout)
        scored = self._scored()
        for at in range(0, len(values), chunk):
            run = values[at:at + chunk]
            args = [self.bufnum, start + at, len(run), *run]
            if scored or not wait:
                srv.send_msg("/buffer_setRange", *args)
                continue
            addr, rargs = srv.request("/buffer_setRange", *args, timeout=timeout,
                                      expect=("/done", "/fail"))
            if addr == "/fail":
                raise CommandError(
                    f"/buffer_setRange {self.bufnum} at {start + at} failed: {rargs}")

    def set_sample(self, index: int, value: float, *, wait: bool = True,
                   timeout: "float | None" = None):
        """Write one sample by flat index (``/buffer_set``) — the single-sample
        counterpart of `get_samples`' range form, for a touch-up that does not
        deserve a run. NRT scores at time 0; RT ``wait=True`` blocks on
        ``/done``."""
        srv = self._server()
        args = [self.bufnum, int(index), float(value)]
        if self._scored() or not wait:
            srv.send_msg("/buffer_set", *args)
            return
        addr, rargs = srv.request("/buffer_set", *args, timeout=timeout,
                                  expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/buffer_set {self.bufnum} at {index} failed: {rargs}")

    def free(self):
        """Free this buffer on the server (``/buffer_free``) and return its slot to
        the pool."""
        srv = self._server()
        srv.send_msg("/buffer_free", self.bufnum)
        srv.buffers.free(self.bufnum)

    # ---- internals ----

    def _server(self):
        return _resolve(self.server)

    def _scored(self) -> bool:
        """True when the interface is an offline score: the message is written
        at time 0 and nothing is waited for."""
        return getattr(self._server().interface, "time_mode", "unix") == "score"

    def __repr__(self):
        return f"Buffer(bufnum={self.bufnum}, frames={self.frames}, channels={self.channels})"


class BufferAllocator:
    def __init__(self, size: int = NUM_BUFFERS):
        self.size = size
        self._registry = _native.Registry(0, size)

    def alloc(self) -> int:
        """A free buffer index; raises when the pool is exhausted."""
        bufnum = self._registry.alloc()
        if bufnum is None:
            raise RuntimeError("out of buffer slots")
        return bufnum

    def free(self, bufnum: int):
        """Returns ``bufnum`` to the pool. A double free (or an index this
        allocator never handed out) raises — a lost buffer slot is a client
        bug, never absorbed silently."""
        if self._registry.release(bufnum) != 0:
            raise RuntimeError(
                f"double free of buffer {bufnum}: not currently allocated")

    @property
    def in_use(self) -> int:
        """How many buffer slots are currently allocated."""
        return self._registry.in_use
