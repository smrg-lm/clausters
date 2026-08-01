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
- a bare **expression** (a `clausters.defs.Ugen` graph, a
  `clausters.defs.ChannelList` of them, a Faust `clausters.defs.Signal` or
  `clausters.defs.Box`) or a **def**
  (`clausters.defs.SynthDef` / `clausters.defs.FaustDef` /
  `clausters.defs.GraphDef`) -> sent and instanced on the server; an
  expression is first wrapped in an ephemeral def
  (`clausters.defs.asdef.as_def` adds the ``out`` when it lacks one), so
  ``play(sine(440))`` just sounds and ``play(sine(440).dup())`` sounds in
  stereo. Returns the node handle — it plays until you free it;
- a `clausters.seq.timeline.Timeline` -> a `clausters.seq.timeline.Playhead`
  over the ambient clock and server;
- a `clausters.defs.Buffer` -> sounded through the stock playbuf instrument
  (a buffer sounds through an instrument; here the verb provides the default
  one — ``rate``/``amp`` controls, freed when the take ends);
- an `clausters.seq.automation.Automation` -> prepared if needed and
  triggered on the ambient server — the interactive "apply this curve to
  that node's control, now" (outside a clock its beats read as seconds);
- anything else following the **timeline-item protocol**
  (``play(destination)`` — an `OscEvent`, a `MidiEvent`, ...) -> dispatched
  to it with the ambient server.

An arrangement `Element` is **not** playable — its change of state to sound
is `clausters.form.render`.

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
from .defs.node import Group, Synth


def play(playable, *, server=None, clock=None, quant=None, controls=None):
    """Play ``playable`` against the ambient context.

    Args:
        playable: an `clausters.seq.event.Event` or a plain dict of event
            keys; an event `clausters.seq.pattern.Pattern`; a
            `clausters.base.stream.Routine` / `clausters.base.stream.Stream`
            or a bare generator (object or function); a bare expression
            (`clausters.defs.Ugen`, `clausters.defs.ChannelList`,
            `clausters.defs.Signal`, `clausters.defs.Box`) or a def
            (`clausters.defs.SynthDef` /
            `clausters.defs.FaustDef` / `clausters.defs.GraphDef`); a
            `clausters.seq.timeline.Timeline`; a `clausters.defs.Buffer`
            (sounded through the stock playbuf instrument); an
            `clausters.seq.automation.Automation` (prepared and triggered);
            or anything with a ``play(destination)`` (the timeline-item
            protocol).
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
            or expression is instanced with; for a `Buffer`, the stock
            instrument's (``rate``, a musical ratio, and ``amp``). Ignored by
            the other kinds.

    Returns:
        Something that knows how to end what just started: the completed
        event for an event or dict (``.free()`` / ``.release()``), the
        `clausters.seq.eventstream.EventStreamPlayer` for a pattern
        (``.stop()``), the routine for a routine, the node handle — a
        `clausters.defs.Synth` or instance `clausters.defs.Group` — for a
        def, expression or buffer (``.free()``), the
        `clausters.seq.timeline.Playhead` for a timeline (``.stop()``), and
        the `clausters.seq.automation.Automation` itself (``.stop()``).
    """
    from .seq.automation import Automation
    from .seq.event import Event
    from .seq.pattern import Pattern
    from .seq.timeline import Playhead, Timeline
    from .base.stream import Stream, Routine
    from .defs import Buffer, Expr, FaustDef, GraphDef, SynthDef
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
    if isinstance(playable, (Expr, SynthDef, FaustDef, GraphDef)):
        return _play_def(as_def(playable), main.resolve_server(server), controls)
    if isinstance(playable, Timeline):
        clock = clock or main.resolve_clock() or main.get_default_clock()
        playhead = Playhead(playable, clock, main.resolve_server(server))
        playhead.play(quant=quant)
        return playhead
    if isinstance(playable, Buffer):
        return _play_buffer(playable, main.resolve_server(server), controls)
    if isinstance(playable, Automation):
        resolved = main.resolve_server(server)
        if playable.buf is None or playable.bus is None:
            # Interactive trigger: we are off the clock thread, so preparing
            # (allocating and filling the control buffer) may block here.
            playable.prepare(resolved)
        playable.play(resolved)
        return playable
    from .form.element import Element

    if isinstance(playable, Element):
        # An Element carries a timeline-item play() (the hook flattening
        # uses), but the verb keeps the state split: rendering is its door.
        raise TypeError(
            "an arrangement Element is rendered, not played — see "
            "clausters.render / clausters.form.render"
        )
    if callable(getattr(playable, "play", None)):
        # The timeline-item protocol (`OscEvent`, `MidiEvent`, and anything
        # else a Playhead could play): play(destination).
        return playable.play(main.resolve_server(server))
    raise TypeError(
        f"don't know how to play {type(playable).__name__}; expected an Event "
        "or event dict, an event Pattern (Pbind), a Routine/Stream or "
        "generator, a def or bare expression (Ugen/ChannelList/Signal/Box), "
        "a Timeline, "
        "a Buffer, an Automation, or anything with play(destination). An "
        "arrangement Element is rendered, not played — see "
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
    `GraphDef`, ``/synth_new`` otherwise. Returns the node handle."""
    from .defs import GraphDef

    d.send(server)
    if isinstance(d, GraphDef):
        return Group.graph(d.name, controls, server=server)
    return Synth.new(d.name, controls, server=server)


def _play_buffer(buffer, server, controls):
    """A buffer sounds through an instrument (see docs/decisions.md); here
    the verb provides the stock one — one `play_buf` lane per channel, with
    ``rate`` and ``amp`` controls — and frees it when the take ends (the
    buffer's frames over its rate). Returns the `Synth`."""
    from .defs import (
        SynthDef, buf_sample_rate, control, out, play_buf, sample_rate,
    )

    if not buffer.frames and getattr(server.interface, "time_mode",
                                     "unix") != "score":
        buffer.info()      # fills frames/channels/sample_rate
    if not buffer.frames:
        raise ValueError(
            "cannot play a buffer of unknown length; fill the handle's "
            "frames (RT queries the server; NRT needs them up front)")
    channels = max(1, buffer.channels)
    buf = control("buf", 0.0)
    # `rate` is a musical ratio: the def rescales it by the buffer's own
    # sample rate (like sclang's BufRateScale), so a take plays at pitch on
    # any engine rate.
    rate = control("rate", 1.0) * buf_sample_rate(buf) / sample_rate()
    amp = control("amp", 1.0)
    sdef = SynthDef(f"_playbuf{channels}",
                    *[out(float(ch), play_buf(buf, float(ch), rate) * amp)
                      for ch in range(channels)])
    sdef.send(server)
    controls = {"buf": float(buffer.bufnum), **(controls or {})}
    node = Synth.new(sdef.name, controls, server=server)
    file_sr = buffer.sample_rate or 48_000.0
    dur = buffer.frames / file_sr / float(controls.get("rate", 1.0))
    server.send_bundle_after(dur, ("/node_free", node.id))
    return node
