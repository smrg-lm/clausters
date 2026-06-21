"""EventStreamPlayer (port of ``sc3/seq/eventstream.py``).

Plays an **event pattern** (a `Pbind`) on a clock:
it is a routine that, for each event, plays it against the server (emitting at
the routine's exact logical beat) and yields the event's ``delta`` to advance.
Because the Server owns the interface, the *same* player runs live (RT) or
accumulates an NRT score just by which interface the Server has — the seam.
"""

from ..base.stream import Routine
from .event import Event


class EventStreamPlayer:
    def __init__(self, pattern, server):
        self.pattern = pattern
        self.server = server
        self.routine = None

    def play(self, clock, quant=None):
        events = iter(self.pattern)
        server = self.server

        def player():
            for event in events:
                if not isinstance(event, Event):
                    event = Event(event)
                event.play(server)        # emits at the current logical beat
                yield event.delta()       # advance time by dur * stretch

        self.routine = Routine(player)
        clock.play(self.routine, quant)
        return self

    def stop(self):
        if self.routine is not None:
            self.routine.state = "done"
        return self
