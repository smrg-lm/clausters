"""Reading the server: a take followed while it records.

Most of what a script reads off the server it *asks* for and gets back at
once — a buffer's samples (`clausters.defs.Buffer.get_samples`), a query's
reply, a summary built from either (`clausters._native.peaks_cache`). A
recording is the one thing that answers no question: a ``RecordBuf`` fills a
buffer block by block from the audio thread, which is the one place that must
never send a message, so what the writer publishes instead is how far it has
got — into the server's shared memory, where a peer that maps the segment
reads it directly and everybody else reads nothing.

``/buffer_stream`` is that reading for whoever cannot map: the server sends the
**overview** of the frames that appeared — ``min``, ``max`` and mean square per
bucket, the peak pyramid's own three statistics, at about a hundredth of the
audio's bandwidth. `RecordingStream` is the receiving end, one peak cache per
take, growing as the reports land: the same cache the samples would have built,
for a script that never sees them.

The primitive underneath stays where it is and is still the right call when
what you want is the *file* a picture maps: `clausters.gui.peaks_cache_stream_file`
grows a cache on disk that a ``waveform(cache=...)`` reads as it fills.
"""

import threading

from .base._oscinterface import OscReceiver
from .errors import CommandError, ReplyTimeout
from ._native import peaks_cache_empty, peaks_cache_write_buckets

__all__ = ["RECORDING_PERIOD_MS", "RecordingStream", "TakeShape"]

#: The default report cadence in milliseconds: 20 a second, finer than a take
#: grows.
RECORDING_PERIOD_MS = 50


class TakeShape:
    """A take being recorded, as `RecordingStream` needs to know it: the
    buffer's slot, its **full** length in frames per channel (not what has been
    written), and its channel count. A `clausters.defs.Buffer` already answers
    all three, so pass one of those where you have it."""

    __slots__ = ("bufnum", "frames", "channels")

    def __init__(self, bufnum: int, frames: int, channels: int = 1):
        self.bufnum = int(bufnum)
        self.frames = int(frames)
        self.channels = int(channels)

    def __repr__(self):
        return (f"TakeShape(bufnum={self.bufnum}, frames={self.frames}, "
                f"channels={self.channels})")


def _shape_of(take) -> TakeShape:
    """A `Buffer`, a `TakeShape` or a ``(bufnum, frames, channels)`` tuple, as
    the one shape this module reads."""
    if isinstance(take, TakeShape):
        return take
    if isinstance(take, tuple):
        return TakeShape(*take)
    return TakeShape(take.bufnum, take.frames, take.channels)


def _bufnum_of(take) -> int:
    return int(take) if isinstance(take, int) else int(_shape_of(take).bufnum)


class RecordingStream:
    """Follows takes as they record, over ``/buffer_stream``.

    ```python
    take = Buffer.alloc(10 * 48000, 1, server=server)
    stream = RecordingStream.open(server, [take])
    stream.on_report(lambda bufnum, s: print(s.written(bufnum)))
    Synth("record_something", {"buf": take.bufnum}, server=server)
    ```

    Each take gets a cache **allocated at its full length** and empty: a take's
    picture is the whole of the box it will fill, so the axis does not move
    while it fills. Reports write the buckets that were measured and nothing
    else, so what has not been recorded reads as the silence the buffer is —
    read only up to `written` to tell the two apart, which is what the GUI
    host's ``fills`` prop does for the picture.

    **Only the overview arrives.** Zoomed in past the base bucket there is no
    copy of the samples here and the wire carried none, so to edit or play what
    was recorded, read it back with `clausters.defs.Buffer.get_samples` once the
    take is finished.

    Reports land on the responder thread, like every `clausters.responders.OscFunc`
    callback: keep a handler to storing and reading, never a round trip. The
    subscription is sent from this stream's own `clausters.base.OscReceiver`
    socket — the same shape the node-lifecycle listener uses — so the reports
    reach the handlers whatever transport the command path uses, and a
    `stream_buffers` call the script makes itself over the server's own carrier
    is a *different* client and does not replace this one.
    """

    def __init__(self, server, bucket: int, recv: "OscReceiver"):
        #: the `clausters.defs.Server` the takes live on.
        self.server = server
        #: the buckets each report is measured over, and the caches' own.
        self.bucket = int(bucket)
        #: reports applied so far — a view can tell a repaint from a stall.
        self.reports = 0
        self._recv = recv
        self._takes = {}
        self._listeners = []
        self._lock = threading.Lock()
        self._closed = False

    @classmethod
    def open(cls, server, takes, *, period_ms: int = RECORDING_PERIOD_MS,
             base_bucket: int = 256, timeout: "float | None" = None,
             recv: "OscReceiver | None" = None) -> "RecordingStream":
        """Subscribes to ``takes`` and returns once the server has acked. A
        subscription watches what happens **next**: samples already recorded is
        a read (`clausters.defs.Buffer.get_samples`), not a stream.

        Args:
            server: the `clausters.defs.Server` the takes live on.
            takes: the takes to follow — `clausters.defs.Buffer` handles,
                `TakeShape`\\ s, or ``(bufnum, frames, channels)`` tuples.
            period_ms: the report cadence (10 ms floor at the server).
            base_bucket: the frames one bucket summarizes; the caches are built
                on this grid and the subscription asks for it, so the two agree
                by construction.
            timeout: how long to wait for the ack, the server handle's when
                omitted.
            recv: the receiver the subscription is sent from and the reports
                come back to; one is started for this stream when omitted.
        """
        own = recv is None
        recv = OscReceiver().start() if own else recv
        stream = cls(server, base_bucket, recv)
        try:
            for take in takes:
                shape = _shape_of(take)
                # Empty rather than built over silence: a ten-minute stereo
                # take would be 230 MB of zeros to summarize what nobody wrote.
                stream._takes[shape.bufnum] = {
                    "cache": peaks_cache_empty(shape.frames,
                                               max(1, shape.channels),
                                               base_bucket),
                    "channels": max(1, shape.channels),
                    "written": 0,
                }
            recv.add(stream._on_reply)
            stream._await_ack("/buffer_stream", period_ms,
                              list(stream._takes), timeout)
        except BaseException:
            stream.free(close_receiver=own)
            raise
        stream._own_receiver = own
        return stream

    def peaks(self, take) -> "bytes | None":
        """The peak cache of one take, or ``None`` when it is not in this
        stream. The same bytes `clausters._native.peaks_cache` builds from
        samples, so it goes wherever one of those goes — written to a file a
        ``waveform(cache=...)`` maps, or compared against one."""
        with self._lock:
            entry = self._takes.get(_bufnum_of(take))
            return None if entry is None else entry["cache"]

    def written(self, take) -> int:
        """How far one take has been reported, in frames — the end of the last
        whole bucket the writer had filled. Past it the cache is the silence the
        buffer was allocated as, so this is where a trace should stop."""
        with self._lock:
            entry = self._takes.get(_bufnum_of(take))
            return 0 if entry is None else entry["written"]

    def on_report(self, handler):
        """Calls ``handler(bufnum, stream)`` with each take that grew, as its
        report lands; returns the callable that unsubscribes it."""
        with self._lock:
            self._listeners.append(handler)

        def off():
            with self._lock:
                if handler in self._listeners:
                    self._listeners.remove(handler)
        return off

    def stop(self, timeout: "float | None" = None):
        """Cancels the subscription on the server and stops decoding. The caches
        stay readable — a finished take is still a picture — until `free`."""
        if self._closed:
            return
        self._closed = True
        with self._lock:
            self._listeners.clear()
        try:
            self._await_ack("/buffer_stream", 0, [], timeout)
        finally:
            self._recv.remove(self._on_reply)

    def free(self, close_receiver: "bool | None" = None):
        """Drops the caches and the responder. The stream is unusable
        afterwards; a receiver this stream started is closed with it."""
        self._closed = True
        with self._lock:
            self._listeners.clear()
            self._takes.clear()
        self._recv.remove(self._on_reply)
        own = getattr(self, "_own_receiver", True)
        if close_receiver is None:
            close_receiver = own
        if close_receiver:
            self._recv.close()

    # ---- the wire ----

    def _await_ack(self, addr, period_ms, bufnums, timeout):
        """Sends ``addr`` out of this stream's own socket and waits for the
        server's ``/done``, which arrives there rather than on the command
        carrier — so the subscription and its reports belong to one client."""
        timeout = self.server.timeout if timeout is None else timeout
        done = threading.Event()
        failure = []

        def ack(raddr, args, when, src):
            if not args or str(args[0]) != addr:
                return
            if raddr == "/fail":
                failure.append(args)
                done.set()
            elif raddr == "/done":
                done.set()

        self._recv.add(ack)
        try:
            self._recv.send(self.server.target, addr,
                            int(period_ms), int(self.bucket), *bufnums)
            if not done.wait(timeout):
                raise ReplyTimeout(f"no reply to {addr}")
            if failure:
                raise CommandError(f"{addr} failed: {failure[0]}")
        finally:
            self._recv.remove(ack)

    def _on_reply(self, addr, args, when, src):
        """One ``/buffer_stream.reply bufnum startFrame bucket blob`` into a
        cache. Nothing is measured here: the writer measured, and the core's
        ``write_buckets`` puts the buckets where they belong."""
        if addr != "/buffer_stream.reply" or len(args) < 4:
            return
        bufnum, start_frame, bucket, blob = (int(args[0]), int(args[1]),
                                             int(args[2]), args[3])
        if not isinstance(blob, (bytes, bytearray, memoryview)):
            return
        with self._lock:
            entry = self._takes.get(bufnum)
            if entry is None:
                return
            stats = memoryview(bytes(blob)).cast("f")
            try:
                entry["cache"] = peaks_cache_write_buckets(
                    entry["cache"], start_frame, bucket, stats)
            except ValueError:
                # A report on another grid than the cache changes nothing --
                # the core refused it, and so does the count.
                return
            buckets = len(stats) // (entry["channels"] * 3)
            entry["written"] = start_frame + buckets * bucket
            self.reports += 1
            listeners = list(self._listeners)
        for handler in listeners:
            handler(bufnum, self)
