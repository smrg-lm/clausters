"""Buffers, with client-side index allocation.

The server's buffer pool is a finite boot-time resource (``--max-buffers``,
4096 by default), indices allocated by the client (like scsynth). A `Buffer`
holds an index and the server it lives on, and owns the ``/b_*`` commands
addressed to it: `Buffer.alloc` and `Buffer.read` create one, and `Buffer.gen`,
`Buffer.write`, `Buffer.query`, `Buffer.get_samples` and `Buffer.free` drive it.

The allocator is a **registry** (the core's occupancy map): a freed slot is
always reusable, a double free is refused loudly, exhaustion raises instead of
wrapping. The `Server` sizes it from its `ServerOptions` (``max_buffers``).
"""

from array import array

from .. import _native
from ..errors import CommandError
from ._wire import resolve as _resolve

NUM_BUFFERS = 4096


class Buffer:
    def __init__(self, bufnum: int, frames: int = 0, channels: int = 1,
                 sample_rate: float = 0.0, server=None):
        self.bufnum = bufnum
        self.frames = frames
        self.channels = channels
        self.sample_rate = sample_rate
        #: the `Server` this buffer lives on (set by `alloc` / `read`), so its
        #: commands know where to go without being told.
        self.server = server

    # ---- constructors ----

    @classmethod
    def alloc(cls, frames: int, channels: int = 1, *, wait: bool = True,
              timeout: float = 5.0, server=None) -> "Buffer":
        """Allocates a zeroed buffer. In NRT it scores ``/b_alloc`` at time 0
        (so the renderer installs it before time advances); in RT ``wait=True``
        (default) blocks on ``/done``, ``wait=False`` is fire-and-forget."""
        srv = _resolve(server)
        bufnum = srv.buffers.alloc()
        buf = cls(bufnum, frames, channels, server=srv)
        if buf._scored() or not wait:
            srv.send_msg("/b_alloc", bufnum, frames, channels)
            return buf
        addr, args = srv.request("/b_alloc", bufnum, frames, channels,
                                 timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            srv.buffers.free(bufnum)
            raise CommandError(f"/b_alloc {bufnum} failed: {args}")
        return buf

    @classmethod
    def read(cls, path, *, file_start: int = 0, num_frames: int = 0,
             wait: bool = True, timeout: float = 5.0, server=None) -> "Buffer":
        """Allocate a buffer and read a sound file into it (``/b_allocRead``): the
        shape and sample rate come from the file (``num_frames`` 0 = the whole
        file, from ``file_start``). Decoding is by content (WAV, FLAC, OGG, MP3,
        …). In NRT it scores at time 0; in RT ``wait=True`` blocks on ``/done``.
        The returned buffer's ``frames``/``channels`` are unknown client-side
        until `query`."""
        srv = _resolve(server)
        bufnum = srv.buffers.alloc()
        buf = cls(bufnum, server=srv)
        extra = [int(file_start), int(num_frames)] if (file_start or num_frames) else []
        if buf._scored() or not wait:
            srv.send_msg("/b_allocRead", bufnum, str(path), *extra)
            return buf
        addr, args = srv.request("/b_allocRead", bufnum, str(path), *extra,
                                 timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            srv.buffers.free(bufnum)
            raise CommandError(f"/b_allocRead {bufnum} {path!r} failed: {args}")
        return buf

    # ---- the commands addressed to this buffer ----

    def gen(self, cmd: str, *args, wait: bool = True, timeout: float = 5.0):
        """Fills this buffer through ``/b_gen`` (the wavetable/generator commands:
        ``"env"``, ``"sine1"``/``"sine2"``/``"sine3"``, ``"cheby"``, ``"copy"``,
        and ``"prepare_partconv" fft_size ir_bufnum`` — the partitioned-kernel
        preparation the `conv` UGen reads; size the target with
        `clausters.defs.partconv_frames`).
        Like `alloc`: NRT scores at time 0; RT ``wait=True`` blocks on
        ``/done``, ``wait=False`` is fire-and-forget."""
        srv = self._server()
        if self._scored() or not wait:
            srv.send_msg("/b_gen", self.bufnum, cmd, *args)
            return
        addr, rargs = srv.request("/b_gen", self.bufnum, cmd, *args,
                                  timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/b_gen {self.bufnum} {cmd} failed: {rargs}")

    def read_into(self, path, *, file_start: int = 0, num_frames: int = -1,
                  buf_start: int = 0, wait: bool = True, timeout: float = 5.0):
        """Read a sound file into this buffer (``/b_read``), keeping its shape.
        NRT scores at time 0; RT ``wait=True`` blocks on ``/done``."""
        srv = self._server()
        args = [self.bufnum, str(path), int(file_start), int(num_frames),
                int(buf_start)]
        if self._scored() or not wait:
            srv.send_msg("/b_read", *args)
            return
        addr, rargs = srv.request("/b_read", *args, timeout=timeout,
                                  expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/b_read {self.bufnum} {path!r} failed: {rargs}")

    def write(self, path, *, sample_format: str = "int16", num_frames: int = -1,
              buf_start: int = 0, wait: bool = True, timeout: float = 5.0):
        """Write this buffer to a WAV file (``/b_write``); ``sample_format`` is
        ``"int16"``, ``"int24"`` or ``"float"``. NRT scores at time 0; RT
        ``wait=True`` blocks on ``/done``."""
        srv = self._server()
        args = [self.bufnum, str(path), "wav", str(sample_format),
                int(num_frames), int(buf_start)]
        if self._scored() or not wait:
            srv.send_msg("/b_write", *args)
            return
        addr, rargs = srv.request("/b_write", *args, timeout=timeout,
                                  expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/b_write {self.bufnum} {path!r} failed: {rargs}")

    def zero(self, *, wait: bool = True, timeout: float = 5.0):
        """Zero this buffer (``/b_zero``). NRT scores at time 0; RT
        ``wait=True`` blocks on ``/done``."""
        srv = self._server()
        if self._scored() or not wait:
            srv.send_msg("/b_zero", self.bufnum)
            return
        addr, rargs = srv.request("/b_zero", self.bufnum, timeout=timeout,
                                  expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/b_zero {self.bufnum} failed: {rargs}")

    def query(self, timeout: float = 5.0) -> "Buffer":
        """Ask the running server for this buffer's shape (``/b_query`` →
        ``/b_info bufnum frames channels sampleRate``) and fill it into the
        handle, which is returned. RT only (it needs a reply)."""
        _, args = self._server().request("/b_query", self.bufnum,
                                         timeout=timeout, expect=("/b_info",))
        # /b_info: bufnum, frames, channels, sampleRate
        self.frames, self.channels = int(args[1]), int(args[2])
        self.sample_rate = float(args[3])
        return self

    def get_samples(self, start: int = 0, count: int = -1, *,
                    chunk: "int | None" = None, timeout: float = 5.0):
        """Fetch interleaved samples from this buffer (``/b_getn`` →
        ``/b_setn``), in chunks, as a stdlib ``array('f')``. ``count`` -1 = to
        the end (the shape is queried first). RT only (it needs replies); for
        display the GUI host fetches buffers itself.

        ``chunk`` (samples per round-trip) defaults to the transport's bound:
        over a stream transport (TCP/WebSocket) it is sized from the frame
        ceiling the server advertises in ``/server_info`` — megabytes per
        reply — while over UDP each reply must fit a datagram, so it stays at
        1024. Pass an explicit ``chunk`` to override either."""
        srv = self._server()
        if chunk is None:
            chunk = srv._bulk_chunk(timeout)
        if count < 0:
            shape = self.query(timeout=timeout)
            total = shape.frames * shape.channels
            count = max(0, total - start)
        out = array("f")
        got = 0
        while got < count:
            n = min(chunk, count - got)
            _, args = srv.request("/b_getn", self.bufnum, start + got, n,
                                  timeout=timeout, expect=("/b_setn",))
            # /b_setn: bufnum, start, count, value...
            out.extend(float(v) for v in args[3:3 + int(args[2])])
            got += n
        return out

    def free(self):
        """Free this buffer on the server (``/b_free``) and return its slot to
        the pool."""
        srv = self._server()
        srv.send_msg("/b_free", self.bufnum)
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
