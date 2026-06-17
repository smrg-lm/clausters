"""TempoClock (port of ``sc3/base/clock.py``, native-backed).

The seam between the native core and the host language. The clock owns the
scheduling queue and the beat/second arithmetic — the latter delegated to
``clausters-core`` through :mod:`clausters._native`, so timing matches the
server's sample clock. The queue holds **routines** (and one-shot callables);
resuming a routine (the ``yield`` driver) stays in Python.

One clock, two drives:

- :meth:`run` / :meth:`start` — real time: a background thread sleeps between
  events and resumes routines on the wall clock.
- :meth:`render` — non-real time: drain the queue in beat order with no
  sleeping, advancing a logical clock; used to build a score.

Either way, when a routine emits with :meth:`send_bundle` the clock hands the
event to its **interface** at the right time for that interface's ``time_mode``
(absolute wall clock for RT, seconds-from-start for NRT). Swapping the
interface is the whole trick.
"""

import heapq
import itertools
import threading
import time

from .. import _native
from .stream import Stream, StopStream


class TempoClock:
    def __init__(self, tempo: float = 1.0, target=None, interface=None):
        #: beats per second
        self.tempo = tempo
        self._base_beats = 0.0
        self._base_secs = 0.0
        self.target = target          # NetAddr (or None)
        self.interface = interface    # an Osc*Interface (or None)

        self._queue = []              # heap of (beat, seq, item)
        self._seq = itertools.count()
        self._cond = threading.Condition()
        self._mode = "stopped"        # 'rt' | 'nrt' | 'stopped'
        self._logical_beat = 0.0      # current beat while driving
        self._start_time = None       # wall-clock origin (RT)
        self._running = False
        self._thread = None

    # ---- beat/second math (native) ----

    def beats2secs(self, beats: float) -> float:
        return _native.beats_to_secs(self.tempo, self._base_beats, self._base_secs, beats)

    def secs2beats(self, secs: float) -> float:
        return _native.secs_to_beats(self.tempo, self._base_beats, self._base_secs, secs)

    def beats(self) -> float:
        """The current beat: logical while rendering, wall-clock-derived in RT."""
        if self._mode == "nrt" or self._start_time is None:
            return self._logical_beat
        return self.secs2beats(time.time() - self._start_time)

    def set_tempo(self, tempo: float):
        """Change tempo, pinning the current instant (no discontinuity)."""
        at = self.beats()
        self._base_beats = at
        self._base_secs = self.beats2secs(at)
        self.tempo = tempo

    # ---- scheduling ----

    def _push(self, beat: float, item):
        heapq.heappush(self._queue, (beat, next(self._seq), item))

    def sched(self, delay_beats: float, item):
        """Schedule ``item`` ``delay_beats`` from the current beat."""
        with self._cond:
            self._push(self.beats() + delay_beats, item)
            self._cond.notify()

    def sched_abs(self, beat: float, item):
        with self._cond:
            self._push(beat, item)
            self._cond.notify()

    def play(self, routine, quant=None):
        """Schedule a routine (or callable) to start now."""
        self.sched(0.0, routine)
        return routine

    def clear(self):
        with self._cond:
            self._queue.clear()

    # ---- event emission to the interface (the seam) ----

    def send_bundle(self, *messages, delay_beats: float = 0.0):
        """Emit a timetagged bundle of ``(addr, *args)`` messages at the current
        beat (+ optional lookahead). The interface decides destination/mode."""
        self._emit(self.beats() + delay_beats, messages)

    def send_msg(self, addr, *args):
        self._emit(self.beats(), ((addr, *args),))

    def _emit(self, beat: float, messages):
        if self.interface is None:
            raise RuntimeError("clock has no interface to emit to")
        when = self._when(beat)
        target = self.target.addr() if self.target is not None else None
        self.interface.send_bundle(target, when, *messages)

    def _when(self, beat: float) -> float:
        secs = self.beats2secs(beat)
        if getattr(self.interface, "time_mode", "unix") == "score":
            return secs  # seconds from render start
        base = self._start_time if self._start_time is not None else time.time()
        return base + secs  # absolute wall clock

    # ---- driving ----

    def _wake(self, item, beat):
        """Resume ``item`` at ``beat``; reschedule if it asks for more time."""
        from .main import main

        prev = main.current_tt
        main.current_tt = item
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
        ``until_beat``). Routines emit into the interface's score."""
        self._mode = "nrt"
        self._logical_beat = 0.0
        try:
            while self._queue:
                beat, _, item = self._queue[0]
                if until_beat is not None and beat > until_beat:
                    break
                heapq.heappop(self._queue)
                self._logical_beat = beat
                self._wake(item, beat)
        finally:
            self._mode = "stopped"
        return self

    def start(self):
        """Begin the real-time driver on a background thread."""
        if self._running:
            return self
        self._mode = "rt"
        self._running = True
        self._start_time = time.time()
        self._thread = threading.Thread(target=self._run_rt, name="TempoClock", daemon=True)
        self._thread.start()
        return self

    def stop(self):
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
                if not self._queue:
                    self._cond.wait(timeout=0.05)
                    continue
                beat, _, item = self._queue[0]
                wait = self.beats2secs(beat) - (time.time() - self._start_time)
                if wait > 0.0:
                    self._cond.wait(timeout=wait)
                    continue
                heapq.heappop(self._queue)
            # Outside the lock: emitting/sending must not block the queue.
            self._wake(item, beat)
