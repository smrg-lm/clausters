"""The free-standing ``play`` — one verb for everything playable.

`play` is the interactive front door: it plays whatever you hand it against the
ambient context, so you never spell out a server or a clock for a quick take.
Like SuperCollider's ``play`` (and sc3's), it dispatches by kind:

- an `clausters.seq.event.Event` — or a plain **dict** of event keys — -> a
  note (immediate outside a clock, timetagged inside one);
- an event `clausters.seq.pattern.Pattern` (a ``Pbind``) -> an
  `clausters.seq.eventstream.EventStreamPlayer` on a clock;
- a `clausters.base.stream.Routine` / `clausters.base.stream.Stream` — or a
  bare **generator** (object or function) -> scheduled on a clock;
- a bare **expression** (a `clausters.defs.Ugen` graph, a Faust
  `clausters.defs.Signal` or `clausters.defs.Box`) or a **def**
  (`clausters.defs.SynthDef` / `clausters.defs.FaustDef` /
  `clausters.defs.GraphDef`) -> sent and instanced on the server; an
  expression is first wrapped in an ephemeral def
  (`clausters.defs.asdef.as_def` adds the ``out`` when it lacks one), so
  ``play(sin_osc(440))`` just sounds. Returns the node handle — it plays
  until you free it;
- a `clausters.seq.timeline.Timeline` -> a `clausters.seq.timeline.Playhead`
  over the ambient clock and server. An arrangement `Element` is **not**
  playable — its change of state to sound is `clausters.form.render`.

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

import inspect

from .base.main import main


def play(playable, *, server=None, clock=None, quant=None, controls=None):
    """Play ``playable`` against the ambient context.

    Args:
        playable: an `clausters.seq.event.Event` or a plain dict of event
            keys; an event `clausters.seq.pattern.Pattern`; a
            `clausters.base.stream.Routine` / `clausters.base.stream.Stream`
            or a bare generator (object or function); a bare expression
            (`clausters.defs.Ugen`, `clausters.defs.Signal`,
            `clausters.defs.Box`) or a def (`clausters.defs.SynthDef` /
            `clausters.defs.FaustDef` / `clausters.defs.GraphDef`); or a
            `clausters.seq.timeline.Timeline`.
        server: the destination server; ``None`` resolves the ambient one (the
            running session's, else the booted default — see
            `clausters.base.main.Main.resolve_server`).
        clock: the clock to schedule on (patterns, routines and timelines);
            ``None`` resolves the running routine's clock, else the default
            session's (started on first use). Ignored by a bare event played
            immediately and by a def or expression.
        quant: start quantization for a pattern/routine/timeline (see
            `clausters.base.clock.TempoClock.play`).
        controls: ``{name: value}`` controls (ports, for a `GraphDef`) a def
            or expression is instanced with. Ignored by the other kinds.

    Returns:
        Whatever the underlying play returns — the synth node id for an event,
        the `clausters.seq.eventstream.EventStreamPlayer` for a pattern, the
        routine for a routine, the node handle (a `clausters.defs.Synth` or
        instance `clausters.defs.Group`) for a def or expression, the
        `clausters.seq.timeline.Playhead` for a timeline.
    """
    from .seq.event import Event
    from .seq.pattern import Pattern
    from .seq.timeline import Playhead, Timeline
    from .base.stream import Stream, Routine
    from .defs import Box, FaustDef, GraphDef, Signal, SynthDef, Ugen
    from .defs.asdef import as_def

    if isinstance(playable, Event):
        return playable.play(main.resolve_server(server))
    if isinstance(playable, dict):
        return Event(playable).play(main.resolve_server(server))
    if isinstance(playable, (Routine, Stream)):
        clock = clock or main.resolve_clock() or main.get_default_clock()
        return playable.play(clock, quant)
    if inspect.isgenerator(playable) or inspect.isgeneratorfunction(playable):
        routine = _as_routine(playable)
        clock = clock or main.resolve_clock() or main.get_default_clock()
        return routine.play(clock, quant)
    if isinstance(playable, Pattern):
        return playable.play(clock, main.resolve_server(server), quant)
    if isinstance(playable, (Ugen, Signal, Box, SynthDef, FaustDef, GraphDef)):
        return _play_def(as_def(playable), main.resolve_server(server), controls)
    if isinstance(playable, Timeline):
        clock = clock or main.resolve_clock() or main.get_default_clock()
        playhead = Playhead(playable, clock, main.resolve_server(server))
        playhead.play(quant=quant)
        return playhead
    raise TypeError(
        f"don't know how to play {type(playable).__name__}; expected an Event "
        "or event dict, an event Pattern (Pbind), a Routine/Stream or "
        "generator, a def or bare expression (Ugen/Signal/Box), or a "
        "Timeline. An arrangement Element is rendered, not played — see "
        "clausters.form.render."
    )


def _as_routine(playable):
    """A `Routine` over a generator: a genfunc is wrapped directly; an
    already-created generator object is played through once (a `reset`
    cannot restart it — pass the function to keep it re-runnable)."""
    from .base.stream import Routine

    if inspect.isgeneratorfunction(playable):
        return Routine(playable)
    return Routine(lambda _=None: playable)


def _play_def(d, server, controls):
    """Sends ``d`` (any family) and instances it: ``/graph_new`` for a
    `GraphDef`, ``/s_new`` otherwise. Returns the node handle."""
    from .defs import GraphDef

    server.add_def(d)
    if isinstance(d, GraphDef):
        return server.graph(d.name, controls)
    return server.synth(d.name, controls)
