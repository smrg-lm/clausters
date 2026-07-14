"""The free-standing ``play`` — one verb for everything playable.

`play` is the interactive front door: it plays whatever you hand it against the
ambient context, so you never spell out a server or a clock for a quick take.
Like SuperCollider's ``play`` (and sc3's), it dispatches by kind:

- an `clausters.seq.event.Event` -> a note (immediate outside a clock, timetagged
  inside one);
- an event `clausters.seq.pattern.Pattern` (a ``Pbind``) -> an
  `clausters.seq.eventstream.EventStreamPlayer` on a clock;
- a `clausters.base.stream.Routine` / `clausters.base.stream.Stream` -> scheduled
  on a clock.

Everything resolves against the ambient environment (the running session, else
the default session `clausters.default_session`): ``server`` defaults to the
booted default server and ``clock`` to the running routine's clock or, outside
one, the default session's clock (created and started on first use). So after a
single ``Server.boot()``:

```python
from clausters import play
from clausters.seq.event import Event

play(Event(degree=0))                       # one note, now
play(Pbind(degree=Pseq([0, 2, 4]), dur=.5)) # a phrase, on the default clock
```

Each playable also carries the same ambient ``.play()``; the free function is
the uniform entry that picks the right one.
"""

from .base.main import main


def play(playable, *, server=None, clock=None, quant=None):
    """Play ``playable`` against the ambient context.

    Args:
        playable: an `clausters.seq.event.Event`, an event
            `clausters.seq.pattern.Pattern`, or a `clausters.base.stream.Routine`
            / `clausters.base.stream.Stream`.
        server: the destination server; ``None`` resolves the ambient one (the
            running session's, else the booted default — see
            `clausters.base.main.Main.resolve_server`).
        clock: the clock to schedule on (patterns and routines); ``None``
            resolves the running routine's clock, else the default session's
            (started on first use). Ignored by a bare event played immediately.
        quant: start quantization for a pattern/routine (see
            `clausters.base.clock.TempoClock.play`).

    Returns:
        Whatever the underlying play returns — the synth node id for an event,
        the `clausters.seq.eventstream.EventStreamPlayer` for a pattern, the
        routine for a routine.
    """
    from .seq.event import Event
    from .seq.pattern import Pattern
    from .base.stream import Stream, Routine

    if isinstance(playable, Event):
        return playable.play(main.resolve_server(server))
    if isinstance(playable, Pattern):
        return playable.play(clock, main.resolve_server(server), quant)
    if isinstance(playable, (Routine, Stream)):
        clock = clock or main.resolve_clock() or main.get_default_clock()
        return playable.play(clock, quant)
    raise TypeError(
        f"don't know how to play {type(playable).__name__}; expected an Event, "
        "an event Pattern (Pbind), or a Routine/Stream"
    )
