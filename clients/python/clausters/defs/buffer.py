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
from ..base.ids import share_of
from ..base.bulk import blob_to_samples, samples_to_blob
from ..errors import CommandError, CommandRingFull
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

    s = Server().boot()
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

    @classmethod
    def read_channels(cls, path, channels, *, file_start: int = 0,
                      num_frames: int = 0, wait: bool = True,
                      timeout: "float | None" = None, server=None) -> "Buffer":
        """Load **selected channels** of a soundfile into a fresh buffer
        (``/buffer_allocReadChannel``) — how one channel of a stereo file lands
        in a mono buffer, which `read` cannot do (it takes the file whole).

        ``channels`` is the list of channel indices to keep, in the order given:
        ``[1]`` is the right channel alone, ``[1, 0]`` swaps a pair, ``[0, 0]``
        makes a mono file two-channel. A channel the file does not have raises
        rather than reading as silence. The buffer's shape comes from the file
        *and* the selection."""
        srv = _resolve(server)
        bufnum = srv.buffers.alloc()
        buf = cls(bufnum, server=srv)
        args = (bufnum, str(path), int(file_start), int(num_frames),
                *(int(c) for c in channels))
        if buf._scored() or not wait:
            srv.send_msg("/buffer_allocReadChannel", *args)
            return buf
        addr, rargs = srv.request("/buffer_allocReadChannel", *args, timeout=timeout,
                                  expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/buffer_allocReadChannel {bufnum} failed: {rargs}")
        buf.info(timeout)
        return buf

    def read_channels_into(self, path, channels, *, file_start: int = 0,
                           num_frames: int = -1, buf_start: int = 0,
                           wait: bool = True, timeout: "float | None" = None):
        """Read selected channels of a soundfile into **this** buffer
        (``/buffer_readChannel``), keeping its shape — so the selection must
        have as many channels as the buffer does. `read_channels` is the form
        that allocates for you."""
        args = (self.bufnum, str(path), int(file_start), int(num_frames),
                int(buf_start), *(int(c) for c in channels))
        srv = self._server()
        if self._scored() or not wait:
            srv.send_msg("/buffer_readChannel", *args)
            return
        addr, rargs = srv.request("/buffer_readChannel", *args, timeout=timeout,
                                  expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/buffer_readChannel {self.bufnum} failed: {rargs}")

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

    def attach(self, *, wait: bool = True, timeout: "float | None" = None):
        """Map this buffer out of the shared segment (``/buffer_attach``).

        Only meaningful against a server that **attached** to a segment
        somebody else owns — the RT server of an editor's arrangement, which
        holds the devices and plays what the on-demand session owns. It
        maps every buffer the owner had published when it started, so this is
        for one published since: after it, that server's engine plays the very
        cells the owner writes, and nothing about the samples travelled.

        Raises `CommandError` when this server owns the samples, has no shared
        segment, or finds no live buffer under this number.
        """
        srv = self._server()
        if self._scored() or not wait:
            srv.send_msg("/buffer_attach", self.bufnum)
            return
        addr, rargs = srv.request("/buffer_attach", self.bufnum, timeout=timeout,
                                  expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/buffer_attach {self.bufnum} failed: {rargs}")

    def touch(self, channel: int, start: int, frames: int):
        """Announce that a span of this buffer was written (``/buffer_touch``).

        For a local peer that edited the samples **in place** — through the
        shared segment, where a write reaches no wire at all. The span, not the
        samples: the server broadcasts ``/buffer_touched bufnum channel start
        frames`` to every client registered with ``/server_notify`` except the
        one that wrote, and whoever holds a picture of this take re-reads that
        span with `get_range`.

        There is no reply: it is a notification, not a command.
        """
        self._server().send_msg("/buffer_touch", self.bufnum, int(channel),
                                int(start), int(frames))

    def fill(self, *runs, wait: bool = True, timeout: "float | None" = None):
        """Write runs of one repeated value (``/buffer_fill``), each a
        ``(start, count, value)`` triple.

        Indices are **flat and interleaved**, like `set_samples` and unlike the
        editing verbs (`gain`, `reverse`), whose spans are frames — this is the
        writing family's member, not an editor's verb. Several runs ride in one
        message, and a run past the end raises rather than being clamped."""
        flat = []
        for run in runs:
            start, count, value = run
            flat += [int(start), int(count), float(value)]
        self._edit("/buffer_fill", tuple(flat), wait, timeout)

    def _edit(self, addr: str, args: tuple, wait: bool, timeout):
        """The shared body of the destructive edits: fire, or block on ``/done``.

        They are async like every other write, and they **compose in flight** —
        the server chains a batch of edits on one buffer, so several
        ``wait=False`` edits in a row each build on the last rather than each on
        the contents you started with."""
        srv = self._server()
        if self._scored() or not wait:
            srv.send_msg(addr, self.bufnum, *args)
            return
        raddr, rargs = srv.request(addr, self.bufnum, *args, timeout=timeout,
                                   expect=("/done", "/fail"))
        if raddr == "/fail":
            raise CommandError(f"{addr} {self.bufnum} failed: {rargs}")

    def gain(self, factor: float, start: int = 0, frames: int = -1, *,
             to: "float | None" = None, shape: int = 1, curve: float = 0.0,
             wait: bool = True, timeout: "float | None" = None):
        """Scale a span of this buffer (``/buffer_gain``) — the destructive edit
        an editor applies to a selection.

        ``start`` and ``frames`` are **frames**, not flat sample indices: a
        selection is a stretch of time across every channel, and every channel
        of a frame is scaled alike, so a fade can never tilt a stereo image.
        ``frames`` of -1 runs to the end.

        One value is a constant gain; give ``to`` for a fade, which sweeps
        ``factor`` to ``to`` along ``shape`` — the same envelope shape numbers
        `clausters.defs.Env` and the breakpoint editor speak, ``curve`` read
        only by the custom-curvature shape (5). So a fade in is
        ``gain(0.0, to=1.0)``, a fade out ``gain(1.0, to=0.0)``, and silence is
        `silence`, which lands on exact zeros where a fade only tends to one.
        """
        to = factor if to is None else to
        self._edit("/buffer_gain", (start, frames, float(factor), float(to),
                                    int(shape), float(curve)), wait, timeout)

    def fade(self, start: int = 0, frames: int = -1, *, out: bool = False,
             shape: int = 1, curve: float = 0.0, wait: bool = True,
             timeout: "float | None" = None):
        """A fade in over a span, or out with ``out=True`` — `gain`'s two
        common cases, spelled the way they are asked for."""
        self.gain(1.0 if out else 0.0, start, frames, to=0.0 if out else 1.0,
                  shape=shape, curve=curve, wait=wait, timeout=timeout)

    def silence(self, start: int = 0, frames: int = -1, *, wait: bool = True,
                timeout: "float | None" = None):
        """Silence a span, on exact zeros (``/buffer_gain`` with both ends at
        0). `zero` is the same thing over the whole buffer."""
        self.gain(0.0, start, frames, to=0.0, wait=wait, timeout=timeout)

    def reverse(self, start: int = 0, frames: int = -1, *, wait: bool = True,
                timeout: "float | None" = None):
        """Reverse a span of this buffer in place (``/buffer_reverse``).

        Frames are reversed, not samples: a stereo pair stays a stereo pair.
        ``start`` and ``frames`` are frames, ``frames=-1`` to the end."""
        self._edit("/buffer_reverse", (start, frames), wait, timeout)

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
            # /buffer_getRange.reply: bufnum, start, blob -- the samples arrive
            # as bytes and are unpacked in one C call, never per sample.
            run = blob_to_samples(args[2])
            out.extend(run)
            if len(run) < n:
                break                      # past the end: the server has no more
            got += n
        return out

    def peaks(self, bucket: int = 256, start: int = 0, frames: int = -1, *,
              timeout: "float | None" = None):
        """Fetch this buffer's **overview** (``/buffer_peaks`` →
        ``/buffer_peaks.reply``), as ``(start_frame, bucket, stats)``.

        The summary of a buffer that is standing still, and the sibling of the
        stream a recording pushes: the same blob either way — bucket-major and
        channel-minor, ``min``, ``max`` and mean square per bucket, in one flat
        ``array('f')`` — so it folds into a pyramid through the same door
        (`clausters.gui.peaks_cache_stream_file`, the core's ``write_buckets``)
        with nothing converted.

        It is what lets a picture of a long take exist without the take: about
        a hundredth of the samples' bandwidth, enough to draw the whole of it,
        and the spans under a zoom read back with `get_samples` as they are
        needed.

        Args:
            bucket: frames per bucket. Use the one the pyramid it is folded
                into was built at (256 unless it says otherwise), so the two
                grids agree by construction.
            start: the first frame, rounded **down** to a whole bucket for the
                same reason.
            frames: how many, -1 to the end. Long spans come back in several
                requests: the reply's own length says how much arrived, and
                this walks from where it ended until the span is covered or the
                server has no more.

        RT only (it needs replies)."""
        srv = self._server()
        # The channel count is what turns a blob's length back into buckets, so
        # it is asked for when the handle does not already carry it -- exactly
        # as `get_samples` asks for the shape it needs.
        if frames < 0 or not self.channels:
            shape = self.info(timeout=timeout)
            if frames < 0:
                frames = max(0, shape.frames - start)
        channels = max(1, self.channels)
        first = (start // bucket) * bucket
        end = start + frames
        out = array("f")
        at = first
        while at < end:
            _, args = srv.request("/buffer_peaks", self.bufnum, int(bucket), int(at),
                                  int(end - at), timeout=timeout,
                                  expect=("/buffer_peaks.reply",))
            run = blob_to_samples(args[3])
            if not run:
                break              # no whole bucket left: the span is covered
            out.extend(run)
            at += (len(run) // (channels * 3)) * bucket
        return first, bucket, out

    def set_samples(self, samples, start: int = 0, *, chunk: "int | None" = None,
                    wait: bool = True, timeout: "float | None" = None):
        """Write interleaved samples into this buffer (``/buffer_setRange``), in
        chunks — the write half of `get_samples`, and the step that closes an
        editor's read → edit → write cycle.

        ``samples`` is any sequence of numbers (a list, an ``array('f')``, what
        `get_samples` returned) laid down from flat index ``start``. Indices are
        flat across channels, so a stereo buffer is written interleaved
        ``L R L R ...``, exactly as it reads back. The samples cross as one
        little-endian ``f32`` blob per chunk rather than as float arguments —
        the protocol's rule for bulk data, and what makes writing a
        multi-megabyte edit a byte copy instead of a per-sample encode.

        The buffer must already exist and keeps its shape: writing past its end
        raises rather than being clamped, since a short write would lose samples
        you believe you stored. The shape is read from the server's mirror, so a
        write immediately after `alloc` needs the alloc to have completed —
        which ``wait=True`` (the default) on that call already guarantees.

        ``chunk`` sizes each message and defaults to the transport's bound,
        exactly as in `get_samples`; the chunks are sent as a batch and closed
        with **one** ``/server_sync`` barrier, so a long write costs one round
        trip rather than one per chunk (over a shared-memory carrier a batch
        long enough to fill the command ring barriers early and continues, which
        is backpressure rather than a failure). NRT scores at time 0; RT
        ``wait=True`` blocks on that barrier, and a chunk that fails raises
        from it.
        """
        self._set_runs("/buffer_setRange", samples, start, chunk, wait, timeout, ())

    def _set_runs(self, addr: str, samples, start: int, chunk, wait, timeout,
                  head: tuple):
        """The chunked blob write both `set_samples` and `set_channel_samples`
        send, differing only in the address and in what stands before the run
        (nothing, or the channel).

        The positions are in the address' own unit -- flat samples for
        ``/buffer_setRange``, frames of one channel for
        ``/buffer_setRangeChannel`` -- and the chunking is the same arithmetic
        either way, since a chunk advances the start by the values it carried.
        """
        srv = self._server()
        values = samples if isinstance(samples, array) else array("f", samples)
        if not len(values):
            return
        if chunk is None:
            chunk = srv._bulk_chunk(timeout)
        scored = self._scored()
        for at in range(0, len(values), chunk):
            # One C-speed pack per chunk: the samples cross as bytes, so nothing
            # here or in the OSC encoder touches them one at a time.
            args = (self.bufnum, *head, start + at,
                    samples_to_blob(values[at:at + chunk]))
            try:
                srv.send_msg(addr, *args)
            except CommandRingFull:
                # The shared-memory carrier's ring is a fixed size, and a batch
                # this long can outrun the server draining it. Backpressure is
                # not an error there -- nothing was sent, and the barrier both
                # waits for the queue and empties the ring, so the retry fits.
                srv._barrier(timeout)
                srv.send_msg(addr, *args)
        # One barrier for the whole batch rather than a /done per chunk: the
        # queue completes them in order anyway, so waiting per chunk would cost
        # a round trip per chunk -- time proportional to the edit's *length*
        # instead of its size.
        if wait and not scored:
            srv._barrier(timeout)

    def set_channel_samples(self, channel: int, samples, start: int = 0, *,
                            chunk: "int | None" = None, wait: bool = True,
                            timeout: "float | None" = None):
        """Write consecutive frames of **one channel** (``/buffer_setRangeChannel``).

        The channel form of `set_samples`, and the one an editor needs: storage
        is interleaved, so a channel's frames are ``channels`` apart and no flat
        start and length name one. Here ``start`` and the run are frames *of
        that channel*, so drawing over the left channel of a stereo take is one
        message and leaves the right one untouched.

        Everything else is `set_samples`: the samples cross as little-endian
        ``f32`` blobs, chunked and closed with one barrier, and a run past the
        end raises rather than being clamped — reported in frames, the unit you
        wrote in. A channel the buffer does not have raises too.
        """
        self._set_runs("/buffer_setRangeChannel", samples, start, chunk, wait,
                       timeout, (int(channel),))

    def set_channel_sample(self, channel: int, frame: int, value: float, *,
                           wait: bool = True, timeout: "float | None" = None):
        """Write one frame of one channel (``/buffer_setChannel``) — the
        single-sample counterpart of `set_channel_samples`, addressed by frame
        rather than by flat index."""
        srv = self._server()
        args = [self.bufnum, int(channel), int(frame), float(value)]
        if self._scored() or not wait:
            srv.send_msg("/buffer_setChannel", *args)
            return
        addr, rargs = srv.request("/buffer_setChannel", *args, timeout=timeout,
                                  expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/buffer_setChannel {self.bufnum} failed: {rargs}")

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
    def __init__(self, size: int = NUM_BUFFERS, share=None):
        self.size = size
        self._registry = _native.Registry(*share_of(0, size, share))

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
