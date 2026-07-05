"""TempoClock (port of ``sc3/base/clock.py``, native-backed).

The seam between the native core and the host language. The clock owns the
scheduling queue and the beat/second arithmetic — the latter delegated to
``clausters-core`` through `clausters._native`, so timing matches the
server's sample clock. The queue holds **routines** (and one-shot callables);
resuming a routine (the ``yield`` driver) stays in Python.

One clock, two drives:

- `run` / `start` — real time: a background thread sleeps between
  events using a **monotonic** pacing clock; the logical beat still advances
  only by the routines' ``yield``s, so inter-event timing is exact and the OSC
  timetags (stamped from a separate wall clock) carry that exactness.
- `render` — non-real time: drain the queue in beat order with no
  sleeping, advancing a logical clock; used to build a score.

The clock does **not** talk to the server: it only schedules and exposes the
current time (`beats`, `beats2secs`, `start_time`). Sending
events belongs to `clausters.defs.server.Server`, which owns the
destination/communication interface and reads the time from the clock of the
routine being resumed (the clock sets ``routine.clock`` and
``main.current_tt`` around each wake). Swapping that interface (RT/NRT/MIDI) is
the seam — and it lives on the Server, not here.
"""

import threading
import time

from .. import _native
from .stream import Stream, StopStream
from .timebase import MonotonicTimebase, SampleClockTimebase


class TempoClock:
    """A scheduler that keeps musical time in beats and resumes routines on it.

    A clock has a `tempo` (beats per second) and a queue of scheduled items --
    routines and one-shot callables. Two drives share that queue:

    - real time (`start` / `run`): a background thread sleeps between items,
      pacing against the `timebase`, and fires them live.
    - non-real time (`render`): the queue is drained in beat order with no
      sleeping, advancing a logical clock as fast as possible -- used to build
      a score offline.

    The defining property is that the **logical beat advances only by the
    routines' ``yield``s**, never by wall-clock drift: a routine that yields
    ``0.25`` is resumed exactly a quarter-beat later, whichever drive is running
    and whatever the OS scheduler does. That is what makes inter-event timing
    exact -- and, with a `SampleClockTimebase`, sample-accurate.

    The clock does not talk to the server. It only schedules and reports time
    (`beats`, `beats2secs`, `start_time`); a `Server` reads the clock of the
    routine it is resuming and emits from there. Choosing where events go (real
    time, offline, MIDI) is the Server's job, not the clock's.

    Args:
        tempo: beats per second.
        timebase: the pacing source -- the default monotonic clock, or a
            `SampleClockTimebase` to anchor pacing and scheduling to the
            server's own sample clock.
    """

    def __init__(self, tempo: float = 1.0, timebase=None):
        #: beats per second
        self.tempo = tempo
        self._base_beats = 0.0
        self._base_secs = 0.0

        #: pacing source — *only* used to decide how long to sleep between
        #: events. The default is the OS monotonic clock; pass a
        #: `SampleClockTimebase` to anchor to the
        #: server's sample clock. The Server reads this to choose how to stamp
        #: events (NTP timetag vs ``/sched`` absolute sample).
        self.timebase = timebase if timebase is not None else MonotonicTimebase()
        self._now = self.timebase

        #: the beat-ordered queue lives in the native core (`clausters-core`'s
        #: `Scheduler`); only beats and flat ids cross, and `_items` maps each
        #: id back to its routine (holding the strong reference while queued).
        self._queue = _native.Scheduler()
        self._items = {}              # id -> [item, pending_count]
        self._cond = threading.Condition()
        self._mode = "stopped"        # 'rt' | 'nrt' | 'stopped'
        self._logical_beat = 0.0      # current beat while driving (yield-exact)
        self._mono_start = None       # pacing origin (monotonic)
        self._unix_start = None       # wall-clock origin for OSC timetags
        self._running = False
        self._thread = None
        self._sample_clock = None     # the master-clock tracker, set by lock_to()
        self._transport = None        # joined shared beat grid, set by join_transport()

    # ---- beat/second math (native) ----

    def beats2secs(self, beats: float) -> float:
        """Convert a beat position to seconds under the current tempo (computed
        in the native core, so it matches the server's own arithmetic)."""
        return _native.beats_to_secs(self.tempo, self._base_beats, self._base_secs, beats)

    def secs2beats(self, secs: float) -> float:
        """Convert seconds to a beat position under the current tempo (native
        core, server-matching)."""
        return _native.secs_to_beats(self.tempo, self._base_beats, self._base_secs, secs)

    def beats(self) -> float:
        """The clock's current beat: the yield-driven logical beat while
        rendering or being woken, else the monotonic-paced elapsed beat in RT
        (used for scheduling relative to "now")."""
        if self._mode == "nrt" or self._mono_start is None:
            return self._logical_beat
        return self.secs2beats(self._now() - self._mono_start)

    @property
    def start_time(self):
        """Wall-clock origin (Unix seconds) while running in real time, else
        ``None``. The Server uses it to turn a logical beat into a wall-clock
        OSC timetag — the **wall** clock, kept separate from the monotonic
        pacing source so timetags stay valid Unix time."""
        return self._unix_start

    @property
    def pacing_origin(self):
        """The timebase value (seconds) captured at `start`. For a
        sample-clock timebase this is ``sample_origin / sample_rate``, which the
        Server turns into the absolute sample for ``/sched``."""
        return self._mono_start

    def set_tempo(self, tempo: float):
        """Change tempo, pinning the current instant (no discontinuity)."""
        at = self.beats()
        self._base_beats = at
        self._base_secs = self.beats2secs(at)
        self.tempo = tempo

    # ---- master-clock lock (sample timebase) ----

    def lock_to(self, server, warmup: bool = True, timeout: float = 2.0):
        """Lock this clock to a master ``server``'s sample clock, so events
        schedule on the server's own sample axis (drift-free) instead of a
        wall-clock OSC timetag.

        Opt-in: a plain clock paces against wall-clock OSC time, which works
        standalone, against another program, or across a network. `lock_to`
        switches it to the server's sample clock — over UDP it tracks the
        server's published `/clock` anchor on its own socket. The switch is
        **graceful**: an offline (score) server, or a master that does not
        answer, leaves the clock on wall-clock time, so a client with no
        Clausters server keeps working. Returns ``self``.

        **Blocking — call it before `start`/`run`, never from inside a
        routine** (it does `/clock` round trips). Release it with `unlock` or
        `close`.
        """
        # An offline (score) destination has no live clock to lock to.
        if getattr(getattr(server, "interface", None), "time_mode", "unix") == "score":
            return self
        sc = server.sample_clock(timeout=timeout)
        try:
            sc.anchor()           # one round trip: detect a reachable master
        except (TimeoutError, OSError, RuntimeError):
            sc.close()
            return self           # graceful: no master -> stay on wall clock
        if warmup:
            sc.warmup(n=4)        # firm up the model before scheduling
        sc.track()
        self._sample_clock = sc
        self.timebase = sc.timebase()
        self._now = self.timebase
        return self

    def unlock(self):
        """Undo a `lock_to`: close the tracker and return to wall-clock OSC
        time. Returns ``self``."""
        if self._sample_clock is not None:
            self._sample_clock.close()
            self._sample_clock = None
        self.timebase = MonotonicTimebase()
        self._now = self.timebase
        return self

    def close(self):
        """Stop the clock and release a `lock_to` tracker, if any."""
        self.stop()
        self.unlock()

    # ---- shared transport (phase alignment) ----

    def join_transport(self, server):
        """Adopt a master ``server``'s shared `/transport` beat grid as this
        clock's tempo and grid, so a `quant`-ed routine starts on the **same**
        beat as every other client joined to it.

        Reads the transport once; if the server has none defined, the clock
        keeps its own grid (no-op). A sample-locked clock (`lock_to`) aligns
        **sample-exactly**; a plain wall-clock clock aligns to beats through the
        server's OSC-time anchor (drift-bounded). Returns ``self``.

        **Blocking — call it before `start`/`run`, never from a routine.**
        """
        info = server.transport()
        if info is None:
            return self
        origin_sample, tempo = info
        self.tempo = tempo
        if isinstance(self.timebase, SampleClockTimebase):
            self._transport = ("sample", float(origin_sample), tempo)
        else:
            # Map the sample-defined origin to OSC time via the /clock anchor,
            # so a wall-clock client quantizes on the same grid (the offset is
            # the core's samples->seconds conversion, shared with the server).
            _, args = server.request("/clock", expect=("/clock.reply",))
            sample0, rate, osc0 = int(args[0]), float(args[1]), float(args[2])
            origin_osc = osc0 + _native.samples_to_secs(int(origin_sample) - sample0, rate)
            self._transport = ("wall", origin_osc, tempo)
        return self

    def leave_transport(self):
        """Stop following a joined transport; `quant` returns to the clock's own
        grid. Returns ``self``."""
        self._transport = None
        return self

    def _grid_beat(self) -> float:
        """Current position, in beats, on the grid `quant` snaps to: the shared
        transport grid when joined, else the clock's own elapsed beats."""
        if self._transport is None:
            return self.beats()
        kind, origin, tempo = self._transport
        if kind == "sample":
            now = self.timebase.current_sample()
            return (now - origin) * tempo / self.timebase.sample_rate
        return (time.time() - origin) * tempo

    def _quant_delay(self, quant) -> float:
        """Beats to wait so a routine starts on the next ``quant`` boundary of
        the grid (``None``/``0`` -> now; the snapping rule is the core's
        ``quant_delay``, shared by every client)."""
        if not quant:
            return 0.0
        return _native.quant_delay(self._grid_beat(), quant)

    # ---- scheduling ----

    def _push(self, beat: float, item):
        key = id(item)
        entry = self._items.get(key)
        if entry is None:
            self._items[key] = [item, 1]
        else:
            entry[1] += 1
        self._queue.push(beat, key)

    def _take(self, key):
        """The item for a popped ``key``, dropping the strong reference once no
        queued entry needs it."""
        entry = self._items[key]
        entry[1] -= 1
        if entry[1] == 0:
            del self._items[key]
        return entry[0]

    def sched(self, delay_beats: float, item):
        """Schedule ``item`` to run ``delay_beats`` from the current beat.

        ``item`` is a `Routine` (or any `Stream`), or a plain callable for a
        one-shot. When resumed, a routine is rescheduled by whatever delay it
        yields; a callable that returns a number is rescheduled by that number,
        and one returning ``None`` runs once. Safe to call from another thread
        or from inside a running routine.
        """
        with self._cond:
            self._push(self.beats() + delay_beats, item)
            self._cond.notify()

    def sched_abs(self, beat: float, item):
        """Schedule ``item`` at an absolute ``beat``, rather than relative to
        the current beat as `sched` does."""
        with self._cond:
            self._push(beat, item)
            self._cond.notify()

    def play(self, routine, quant=None):
        """Schedule a routine (or callable), snapping its start to a beat grid.

        Args:
            routine: a `Routine`, any `Stream`, or a one-shot callable.
            quant: start quantization -- the routine starts on the next beat
                that is a multiple of ``quant`` (e.g. ``4`` = the next bar in
                4/4). ``None`` or ``0`` starts immediately. The grid is the
                clock's own elapsed beats, or a shared one when the clock has
                joined a transport (`join_transport`); for multi-client
                alignment start the clock before playing the quantized routine.
        """
        self.sched(self._quant_delay(quant), routine)
        return routine

    def clear(self):
        """Drop every item currently in the schedule queue."""
        with self._cond:
            self._queue.clear()
            self._items.clear()

    def unsched(self, item):
        """Remove a specific scheduled ``item`` from the queue (by identity),
        leaving the rest in order. Used to cancel one routine — e.g. a
        `clausters.seq.timeline.Playhead` stopping or seeking — without clearing
        everything else `clear` would drop."""
        with self._cond:
            key = id(item)
            if key in self._items:
                removed = self._queue.remove(key)
                entry = self._items[key]
                entry[1] -= removed
                if entry[1] <= 0:
                    del self._items[key]
            self._cond.notify()

    # ---- driving ----

    def _wake(self, item, beat):
        """Resume ``item`` at ``beat``; reschedule if it asks for more time."""
        from .main import main

        prev = main.current_tt
        main.current_tt = item
        if isinstance(item, Stream):
            item.clock = self          # the running thread carries its clock (sc3)
            item._logical_beat = beat  # ...and its exact logical time (yield-driven)
        try:
            if isinstance(item, Stream):
                try:
                    delta = item.next(self)
                except StopStream:
                    return
            elif callable(item):
                delta = item()
            else:
                return
        finally:
            main.current_tt = prev
        if isinstance(delta, (int, float)):
            with self._cond:
                self._push(beat + float(delta), item)
                self._cond.notify()

    def render(self, until_beat: float | None = None):
        """NRT drive: process the queue in beat order without sleeping.

        Returns when the queue is empty (or the next event is past
        ``until_beat``). Whatever the routines emit (through a Server) lands in
        that Server's interface — here we only advance time and resume them."""
        self._mode = "nrt"
        self._logical_beat = 0.0
        try:
            while True:
                beat = self._queue.peek_time()
                if beat is None or (until_beat is not None and beat > until_beat):
                    break
                _, key = self._queue.pop_due(beat)
                self._logical_beat = beat
                self._wake(self._take(key), beat)
        finally:
            self._mode = "stopped"
        return self

    def start(self):
        """Begin the real-time driver on a background thread."""
        if self._running:
            return self
        self._mode = "rt"
        self._running = True
        self._mono_start = self._now()   # pacing origin (monotonic)
        self._unix_start = time.time()   # wall-clock origin (for timetags)
        self._thread = threading.Thread(target=self._run_rt, name="TempoClock", daemon=True)
        self._thread.start()
        return self

    def stop(self):
        """Stop the real-time driver and join its background thread; returns
        ``self``. Schedules built up by `run`/`start` end here."""
        with self._cond:
            self._running = False
            self._cond.notify_all()
        if self._thread is not None:
            self._thread.join(timeout=1.0)
            self._thread = None
        self._mode = "stopped"
        return self

    def run(self, seconds: float):
        """Convenience: run the RT driver for ``seconds`` then stop."""
        self.start()
        time.sleep(seconds)
        return self.stop()

    def _run_rt(self):
        while True:
            with self._cond:
                if not self._running:
                    break
                beat = self._queue.peek_time()
                if beat is None:
                    self._cond.wait(timeout=0.05)
                    continue
                wait = self.beats2secs(beat) - (self._now() - self._mono_start)
                if wait > 0.0:
                    self._cond.wait(timeout=wait)
                    continue
                _, key = self._queue.pop_due(beat)
                item = self._take(key)
            # Outside the lock: emitting/sending must not block the queue.
            self._wake(item, beat)
