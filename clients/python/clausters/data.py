"""Reading the server: the three subscriptions a script watches.

Most of what a script reads off the server it *asks* for and gets back at
once — a buffer's samples (`clausters.defs.Buffer.get_samples`), a query's
reply, a summary built from either (`clausters._native.peaks_cache`). What is
here is the other kind: what the server keeps sending, because the thing being
watched changes faster than anything could ask.

- `BusStream` — control buses (``/bus_stream``): the newest value of each bus,
  as often as asked for.
- `TapStream` — audio buses (``/bus_tapStream``): the newest window of samples
  of each bus, on that bus's own sample axis.
- `RecordingStream` — takes as they record (``/buffer_stream``): the overview
  of what was written, since the audio thread that fills a buffer is the one
  place that cannot send a message.

The GUI host reads all three paths itself — that is why a GuiDef naming a bus,
a tap or a take draws without a line of script. This module is the same paths
opened to the **script**, for what a program does with the data besides look at
it: a read-out, a decision, a summary it hands on, a test.

**Nothing here draws, and nothing here computes a drawing.** An oscilloscope's
display window and trigger, a decibel curve, a row of pixel columns: those
belong to whoever draws, and what draws is the GUI host. A script that wants to
see any of this names a view — `clausters.plot`, `clausters.scope`, or a
widget in a GuiDef — and the host reads the very same paths.

**Where the reports arrive.** This client's reply path is pulled, not pushed:
`clausters.defs.Server.request` reads the carrier and drops what it did not ask
for, so a subscription sent over the command carrier would have nobody
listening. Each stream here sends its subscription out of **its own**
`clausters.base.OscReceiver` socket — the shape the ``/node_end`` recycler
already uses — and the reports land on the responder thread, like every
`clausters.responders.OscFunc` callback. Keep a handler to storing and reading,
never a round trip. It also settles the server's *one subscription per client*
rule in this client's favour: each stream is its own client, so two of them, or
one beside a `stream_buses` call the script makes itself, replace nothing.
"""

import array
import math
import threading

from .base._oscinterface import OscReceiver
from .errors import CommandError, ReplyTimeout
from ._native import peaks_cache_empty, peaks_cache_write_buckets
from .base.bulk import blob_to_samples

__all__ = ["RECORDING_PERIOD_MS", "STREAM_PERIOD_MS", "BusStream",
           "RecordingStream", "TakeShape", "TapStream", "TapWindow"]

#: The subscription period a live view runs at, in milliseconds: ~30 fps,
#: the GUI host's own.
STREAM_PERIOD_MS = 33

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


def _bus_index(bus) -> int:
    """A `clausters.defs.Bus` handle or a bare index, as the index. Read off the
    object rather than by type, so a handle this module never heard of works."""
    index = getattr(bus, "index", None)
    return int(bus) if index is None else int(index)


class _Subscription:
    """What the three streams share: the socket a subscription is sent from,
    the ack it waits for, the listener list, and the two verbs that end it.

    Each stream is its **own** OSC client — it sends its subscription out of its
    own receiver and the server answers there — so the machinery is identical
    and only the command, its arguments and what a reply means differ.
    """

    #: the command this subscription is opened and cancelled with.
    _addr = ""

    def __init__(self, server, recv: "OscReceiver"):
        #: the `clausters.defs.Server` this stream reads.
        self.server = server
        self._recv = recv
        self._own_receiver = True
        self._listeners = []
        self._lock = threading.Lock()
        self._closed = False

    # ---- the shape a subclass fills in ----

    def _cancel_args(self) -> tuple:
        """The arguments that cancel this subscription: a period of zero, plus
        whatever else the command's own shape requires."""
        return (0,)

    def _on_reply(self, addr, args, when, src):
        raise NotImplementedError

    def _forget(self):
        """Drops what this stream accumulated (called under the lock)."""

    # ---- the shared machinery ----

    def _listen(self, handler):
        with self._lock:
            self._listeners.append(handler)

        def off():
            with self._lock:
                if handler in self._listeners:
                    self._listeners.remove(handler)
        return off

    def _fire(self, *args):
        """Calls every listener outside the lock, on the snapshot taken under
        it — a handler that unsubscribes must not deadlock, and one that runs
        long must not hold the next report up."""
        with self._lock:
            listeners = list(self._listeners)
        for handler in listeners:
            handler(*args)

    def _subscribe(self, args, timeout, own: bool):
        try:
            self._recv.add(self._on_reply)
            self._await_ack(args, timeout)
        except BaseException:
            self.free(close_receiver=own)
            raise
        self._own_receiver = own

    def _await_ack(self, args, timeout):
        """Sends this stream's command out of its own socket and waits for the
        server's ``/done``, which arrives there rather than on the command
        carrier — so the subscription and its replies belong to one client."""
        timeout = self.server.timeout if timeout is None else timeout
        done = threading.Event()
        failure = []

        def ack(raddr, rargs, when, src):
            if not rargs or str(rargs[0]) != self._addr:
                return
            if raddr == "/fail":
                failure.append(rargs)
                done.set()
            elif raddr == "/done":
                done.set()

        self._recv.add(ack)
        try:
            self._recv.send(self.server.target, self._addr, *args)
            if not done.wait(timeout):
                raise ReplyTimeout(f"no reply to {self._addr}")
            if failure:
                raise CommandError(f"{self._addr} failed: {failure[0]}")
        finally:
            self._recv.remove(ack)

    def stop(self, timeout: "float | None" = None):
        """Cancels the subscription on the server and stops decoding. What the
        stream already holds stays readable until `free`."""
        if self._closed:
            return
        self._closed = True
        with self._lock:
            self._listeners.clear()
        try:
            self._await_ack(self._cancel_args(), timeout)
        finally:
            self._recv.remove(self._on_reply)

    def free(self, close_receiver: "bool | None" = None):
        """Drops what the stream holds and its responder. The stream is
        unusable afterwards; a receiver it started is closed with it."""
        self._closed = True
        with self._lock:
            self._listeners.clear()
            self._forget()
        self._recv.remove(self._on_reply)
        if close_receiver is None:
            close_receiver = self._own_receiver
        if close_receiver:
            self._recv.close()


class BusStream(_Subscription):
    """A live view of a set of control buses, over ``/bus_stream``.

    ```python
    buses = BusStream.open(server, [level, cutoff])
    buses.on_snapshot(lambda values, s: print(values[0]))   # ~30 times a second
    ...
    buses.stop()
    ```

    The whole object is a **latest value** store, not a history: a snapshot
    replaces the previous one. A read-out that wants a rolling trace keeps its
    own history from `on_snapshot` — how long a trace is, is its decision.

    At most `clausters.defs.ServerInfo.max_stream_buses` per subscription (the
    server's ``--max-stream-buses``, clamped to what this stream's carrier
    delivers in one reply). Watch every bus in one stream rather than opening
    several: the ceiling is per subscription, and one reply is cheaper than
    two.
    """

    _addr = "/bus_stream"

    def __init__(self, server, buses, recv: "OscReceiver"):
        super().__init__(server, recv)
        #: the bus indices watched, in the order `values` holds them.
        self.buses = tuple(buses)
        #: the newest snapshot, one entry per bus, in `buses` order.
        self.values = array.array("f", bytes(4 * len(self.buses)))
        #: snapshots seen so far — a read-out can tell a repaint from a stall.
        self.snapshots = 0
        self._slot = {bus: i for i, bus in enumerate(self.buses)}

    @classmethod
    def open(cls, server, buses, *, period_ms: int = STREAM_PERIOD_MS,
             timeout: "float | None" = None,
             recv: "OscReceiver | None" = None) -> "BusStream":
        """Subscribes to ``buses`` and returns once the server has acked, with
        the first snapshot already applied where it arrived in time.

        Args:
            server: the `clausters.defs.Server` the buses live on.
            buses: `clausters.defs.Bus` handles or bare indices.
            period_ms: the snapshot cadence (10 ms floor at the server).
            timeout: how long to wait for the ack, the server handle's when
                omitted.
            recv: the receiver the subscription is sent from and the snapshots
                come back to; one is started for this stream when omitted.
        """
        own = recv is None
        recv = OscReceiver().start() if own else recv
        indices = [_bus_index(b) for b in buses]
        stream = cls(server, indices, recv)
        stream._subscribe((int(period_ms), *indices), timeout, own)
        return stream

    def value(self, bus) -> float:
        """The newest value of one bus, ``nan`` when it is not in this
        stream."""
        slot = self._slot.get(_bus_index(bus))
        return math.nan if slot is None else self.values[slot]

    def on_snapshot(self, handler):
        """Calls ``handler(values, stream)`` with each snapshot as it lands;
        returns the callable that unsubscribes it. ``values`` is this stream's
        own array, so read it rather than keeping it."""
        return self._listen(handler)

    def _forget(self):
        self._slot.clear()

    def _on_reply(self, addr, args, when, src):
        """One ``/bus_stream.reply bus value ...`` snapshot into `values`."""
        if addr != "/bus_stream.reply":
            return
        touched = False
        with self._lock:
            for i in range(0, len(args) - 1, 2):
                slot = self._slot.get(int(args[i]))
                if slot is None:
                    continue
                self.values[slot] = float(args[i + 1])
                touched = True
            if not touched:
                return
            self.snapshots += 1
        self._fire(self.values, self)


class TapWindow:
    """One audio bus's newest window, on that bus's own sample axis:
    ``samples`` oldest first, and ``end_position`` the total samples ever
    recorded at the window's end.

    The position is what places consecutive windows on the bus's timeline —
    they overlap or gap by exactly its delta, never by a guess about the
    period."""

    __slots__ = ("samples", "end_position")

    def __init__(self, samples, end_position: int):
        self.samples = samples
        self.end_position = int(end_position)

    def __repr__(self):
        return (f"TapWindow({len(self.samples)} samples, "
                f"end_position={self.end_position})")


class TapStream(_Subscription):
    """A live view of a set of audio buses, over ``/bus_tapStream``.

    ```python
    taps = TapStream.open(server, [left], frames=2048)
    taps.on_data(lambda bus, w: print(max(w.samples)))
    ...
    taps.stop()      # and the server stops recording
    ```

    A control bus carries one value per block; an analysis needs the samples
    themselves, so the server **records** the buses it is asked for and sends
    the newest window of each. Opening the stream is what starts that recording
    and stopping it is what ends it — there is no separate routing step and no
    ring index anywhere.

    At most 8 buses per subscription. ``frames`` is clamped by the server to
    the carrier's bound and to half the ring, so a window may come back shorter
    than asked, and a bus whose recording has not filled one yet sends nothing
    at all.

    **What is not here is the trace.** Framing a display window and aligning it
    on a trigger so a periodic signal stands still is what an oscilloscope
    *draws*, and the drawing is the GUI host's — `clausters.scope`, or a
    ``scope`` widget in a GuiDef, which asks the server for the same tap. What
    a script does with a window here is measure it.
    """

    _addr = "/bus_tapStream"

    def __init__(self, server, buses, frames: int, recv: "OscReceiver"):
        super().__init__(server, recv)
        #: the audio buses watched.
        self.buses = tuple(buses)
        #: frames per window this stream asked for.
        self.frames = int(frames)
        self._windows = {}

    @classmethod
    def open(cls, server, buses, *, frames: int = 2048,
             period_ms: int = STREAM_PERIOD_MS,
             timeout: "float | None" = None,
             recv: "OscReceiver | None" = None) -> "TapStream":
        """Subscribes to ``buses`` and returns once the server has acked.

        Args:
            server: the `clausters.defs.Server` the buses live on.
            buses: `clausters.defs.Bus` handles or bare indices.
            frames: samples per window, clamped by the server.
            period_ms: the snapshot cadence (10 ms floor at the server).
            timeout: how long to wait for the ack, the server handle's when
                omitted.
            recv: the receiver the subscription is sent from and the windows
                come back to; one is started for this stream when omitted.
        """
        own = recv is None
        recv = OscReceiver().start() if own else recv
        indices = [_bus_index(b) for b in buses]
        stream = cls(server, indices, frames, recv)
        stream._subscribe((int(period_ms), int(frames), *indices), timeout, own)
        return stream

    def window(self, bus) -> "TapWindow | None":
        """One bus's newest window, or ``None`` before its first snapshot."""
        with self._lock:
            return self._windows.get(_bus_index(bus))

    def interleaved(self, first, count: int) -> "array.array":
        """The newest windows of ``count`` adjacent buses from ``first``,
        interleaved frame-major (``L R L R ...``) over the frames they share —
        the layout `clausters._native.correlation` and
        `clausters._native.lissajous` take, and what
        `clausters.render.channels` splits again.

        Empty until every one of those buses has a window. The windows may
        differ in length, and what is paired is the **freshest**: they are
        aligned on their newest sample, not on their start."""
        first = _bus_index(first)
        with self._lock:
            windows = [self._windows.get(first + i) for i in range(count)]
        if count <= 0 or any(w is None for w in windows):
            return array.array("f")
        frames = min(len(w.samples) for w in windows)
        out = array.array("f", bytes(4 * frames * count))
        for ch, w in enumerate(windows):
            out[ch::count] = array.array("f", w.samples[len(w.samples) - frames:])
        return out

    def on_data(self, handler):
        """Calls ``handler(bus, window)`` with each window as it lands (one call
        per tap per period); returns the callable that unsubscribes it."""
        return self._listen(handler)

    def _cancel_args(self) -> tuple:
        # The command takes a frame count before its buses, so cancelling it
        # takes one too -- zero, since no window is being asked for.
        return (0, 0)

    def _forget(self):
        self._windows.clear()

    def _on_reply(self, addr, args, when, src):
        """One ``/bus_tapStream.reply bus endPosition blob`` window."""
        if addr != "/bus_tapStream.reply" or len(args) < 3:
            return
        bus = int(args[0])
        blob = args[2]
        if bus not in self.buses or not isinstance(blob, (bytes, bytearray, memoryview)):
            return
        window = TapWindow(blob_to_samples(blob), int(args[1]))
        with self._lock:
            self._windows[bus] = window
        self._fire(bus, window)


class RecordingStream(_Subscription):
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

    The primitive underneath stays where it is and is still the right call when
    what you want is the **file** a picture maps:
    `clausters.gui.peaks_cache_stream_file` grows a cache on disk that a
    ``waveform(cache=...)`` reads as it fills. This class is for a script that
    wants the summary itself.
    """

    _addr = "/buffer_stream"

    def __init__(self, server, bucket: int, recv: "OscReceiver"):
        super().__init__(server, recv)
        #: the buckets each report is measured over, and the caches' own.
        self.bucket = int(bucket)
        #: reports applied so far — a view can tell a repaint from a stall.
        self.reports = 0
        self._takes = {}

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
        for take in takes:
            shape = _shape_of(take)
            # Empty rather than built over silence: a ten-minute stereo take
            # would be 230 MB of zeros to summarize what nobody wrote.
            stream._takes[shape.bufnum] = {
                "cache": peaks_cache_empty(shape.frames,
                                           max(1, shape.channels),
                                           base_bucket),
                "channels": max(1, shape.channels),
                "written": 0,
            }
        stream._subscribe(
            (int(period_ms), int(base_bucket), *stream._takes), timeout, own)
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
        return self._listen(handler)

    def _cancel_args(self) -> tuple:
        # The command takes the bucket size before its buffers, so cancelling
        # it takes one too -- the grid this stream's caches are on.
        return (0, self.bucket)

    def _forget(self):
        self._takes.clear()

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
        self._fire(bufnum, self)
