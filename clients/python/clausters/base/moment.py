"""When something is happening: a clock and an exact beat on it.

The one place that answers "what time is it *for this event*". A running
routine carries the beat the clock stamped on it (yield-accumulated, not
wall-clock now), so everything emitted from one wake shares a single instant
and inter-event timing stays exact. Outside any routine there is no clock, and
physical time stands in for logical time rather than opening a second code
path.

`Moment` only reads a clock -- it maps a beat onto seconds and onto a Unix
instant. What a destination *does* with that instant is the destination's:
`clausters.base.destination`.
"""

import time
from typing import NamedTuple

from .main import main


class Moment(NamedTuple):
    """A clock and an exact beat on it.

    ``clock`` is ``None`` outside any routine; beats then read as seconds
    (tempo 1.0), which is what lets a bare ``Event().play()`` use the same
    machinery as one inside a routine.
    """

    clock: "object | None"
    beat: float

    @classmethod
    def current(cls, clock=None) -> "Moment":
        """The ambient moment.

        Inside a routine, the exact beat the clock stamped on it. That beat
        belongs to *its* clock, so an explicit ``clock`` that is not the one
        the routine plays on is asked for its own ``beats()`` instead. With no
        clock in either place, the clockless moment.
        """
        tt = main.current_tt
        on = clock if clock is not None else getattr(tt, "clock", None)
        if on is None:
            return cls(None, 0.0)
        if getattr(tt, "clock", None) is on:
            return cls(on, tt._logical_beat)
        return cls(on, on.beats())

    def at(self, delta_beats: float = 0.0) -> "Moment":
        """This moment moved ``delta_beats`` later on the same clock."""
        return Moment(self.clock, self.beat + delta_beats)

    def secs(self) -> float:
        """Seconds on the clock's own axis (measured from its beat zero), or
        the beat itself when there is no clock (tempo 1.0)."""
        if self.clock is None:
            return self.beat
        return self.clock.beats2secs(self.beat)

    def instant(self) -> float:
        """Unix seconds -- what an OSC timetag is made of.

        With no clock, or before the clock's first `start` placed its
        wall-clock origin, this is now plus whatever the moment carries.
        """
        start = getattr(self.clock, "start_time", None)
        if start is None:
            return time.time() + self.secs()
        return start + self.secs()
