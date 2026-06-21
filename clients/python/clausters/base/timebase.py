"""Selectable pacing timebase for the clock.

A `TempoClock`'s logical beat advances only by the routines' ``yield``s;
the *timebase* is the monotonic-ish source the clock paces its sleeps against
(and, in real time, anchors its OSC timetags to). Two choices:

- `MonotonicTimebase` (default) — the OS monotonic clock. Events are sent
  as NTP-timetagged bundles; simple, drift between the client and server clocks
  is small but real.
- `SampleClockTimebase` — seconds derived from the **server's sample
  counter** (``sample() / sample_rate``). The client paces against the server's
  own clock, and the Server emits via ``/sched <absolute_sample>`` instead of a
  wall-clock timetag, so there is no inter-clock drift and timing is exact at
  the sample. ``sample`` is any callable returning the current sample count
  (e.g. ``Clausters.clock`` or ``ShmClient.clock``).

A timebase is callable (``tb()`` == ``tb.now()``) so it also works as the plain
``timebase`` callable the clock accepts.
"""

import time


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
        """The absolute sample for a time in *this timebase's* seconds."""
        return round(seconds * self.sample_rate)
