"""Selectable pacing timebase for the clock.

A `TempoClock`'s logical beat advances only by the routines' ``yield``s;
the *timebase* is the monotonic-ish source the clock paces its sleeps against
(and, in real time, anchors its OSC timetags to). Two choices:

- `MonotonicTimebase` (default) -- the OS monotonic clock. Events are sent
  as NTP-timetagged bundles; simple, drift between the client and server clocks
  is small but real.
- `SampleClockTimebase` -- seconds derived from the **server's sample
  counter** (``sample() / sample_rate``). The client paces against the server's
  own clock, and the Server emits via ``/sched_at <absolute_sample>`` instead of a
  wall-clock timetag, so there is no inter-clock drift and timing is exact at
  the sample. ``sample`` is any callable returning the current sample count
  (e.g. ``Clausters.clock`` or ``ShmClient.clock``).

A timebase is callable (``tb()`` == ``tb.now()``) so it also works as the plain
``timebase`` callable the clock accepts.
"""

import time

from .. import _native


class Timebase:
    kind = "abstract"

    def now(self) -> float:
        raise NotImplementedError(f"{type(self).__name__}.now")

    def __call__(self) -> float:
        return self.now()


class MonotonicTimebase(Timebase):
    kind = "monotonic"

    def now(self) -> float:
        return time.monotonic()


class ManualTimebase(Timebase):
    """Time driven by hand: it advances only when `advance` is called.

    What a test paces with, so the very code path a real clock runs advances
    deterministically and instantly instead of waiting on a wall clock. It is
    also the honest timebase for anything stepped by something that is not
    time -- a frame, a user's gesture, a render tick.
    """

    kind = "manual"

    def __init__(self, start: float = 0.0):
        self._seconds = float(start)

    def now(self) -> float:
        return self._seconds

    def advance(self, secs: float) -> None:
        """Moves time forward by ``secs`` (never backwards)."""
        self._seconds += max(float(secs), 0.0)


class LogicalTimebase(Timebase):
    """The system clock of a non-real-time run: seconds that advance only when
    whatever is due next is woken.

    Offline there is no physical time to wait on, so this stands in for it --
    a clock at tempo 1 -- and **every** `clausters.base.TempoClock` of the run
    shares it: each has its origin on these seconds, exactly as a live clock
    has its origin on the monotonic clock, so a script runs the same live and
    offline. What a script sets up before rendering happens at second 0; what a
    routine starts at second 4 starts at second 4.

    It keeps the clocks that share it, in the order they joined, so a render
    wakes whatever is due next across all of them (`TempoClock.render`).
    """

    kind = "logical"

    def __init__(self, start: float = 0.0):
        self._seconds = float(start)
        #: the clocks on this time, in the order they joined; ties between two
        #: clocks due on the same second wake in this order.
        self.clocks = []
        #: the clock being woken right now, whose routine's beat is exact.
        self.waking = None

    def now(self) -> float:
        return self._seconds

    def advance_to(self, seconds: float) -> None:
        """Moves time to ``seconds`` (never backwards)."""
        self._seconds = max(self._seconds, float(seconds))

    def join(self, clock) -> None:
        """Adds ``clock`` to the clocks this time drives (once)."""
        if not any(c is clock for c in self.clocks):
            self.clocks.append(clock)


class SampleClockTimebase(Timebase):
    """Seconds from the server's sample clock: ``now = sample() / sample_rate``."""

    kind = "sample"

    def __init__(self, sample, sample_rate: float):
        #: callable returning the server's current sample counter (u64)
        self.sample = sample
        self.sample_rate = float(sample_rate)

    def now(self) -> float:
        return self.sample() / self.sample_rate

    def current_sample(self) -> int:
        return int(self.sample())

    def sample_at(self, seconds: float) -> int:
        """The absolute sample for a time in *this timebase's* seconds
        (the core's seconds->samples conversion, ties to even)."""
        return _native.secs_to_samples(seconds, self.sample_rate)
