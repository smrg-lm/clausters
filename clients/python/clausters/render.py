"""The free-standing ``render`` — one verb for the change of state to sound.

`render` is the third ambient verb, next to `clausters.play` and
`clausters.plot`: it turns a **generator** thing (an algorithm that describes
sound) into a **generated** one (samples — random-access audio). It dispatches
by kind:

- a binary **score** (``bytes``) -> the embedded offline renderer, exactly the
  historical ``clausters.render`` (see `clausters.ipc.render`);
- a **def** (`clausters.defs.SynthDef` / `clausters.defs.FaustDef` /
  `clausters.defs.GraphDef`) or a bare **expression** (a `clausters.defs.Ugen`
  graph, a Faust `clausters.defs.Signal` or `clausters.defs.Box`, coerced
  through `clausters.defs.asdef.as_def`) -> instanced offline for ``dur``
  seconds — the audible sibling of ``plot(def)``;
- an arrangement **`Element`** -> with a ``destination``, delegates to
  `clausters.form.render` (the arrangement's own seam: RT or NRT by the
  destination); without one, an **offline bounce** — an ephemeral NRT session
  plays it and renders the score;
- a `clausters.seq.timeline.Timeline` -> the same dual: a
  `clausters.seq.timeline.Playhead` on ``destination``, or the offline bounce;
- an event `clausters.seq.pattern.Pattern`, a `clausters.base.stream.Routine`
  / `clausters.base.stream.Stream` or a bare **generator** -> offline bounce
  only (they are forward-only; sounding them live is `clausters.play`'s job).
  An endless source needs ``until`` (the bounce would never drain).

Every offline path returns ``(samples, frames)`` — interleaved float32 in a
stdlib ``array('f')`` — and, when ``path`` is given, also writes the audio as
a float32 WAV there.

```python
from clausters import render
from clausters.defs import sin_osc

samples, frames = render(sin_osc(440.0) * 0.2, dur=2.0)
render(Pbind(degree=Pseq([0, 2, 4]), dur=0.5), path="phrase.wav")
render(my_piece, until=64.0, path="piece.wav")     # an arrangement, bounced
```
"""

import struct
import sys
from array import array

from .base.main import main

__all__ = ["render", "bounce_def"]


def render(obj, *, destination=None, clock=None, at: float = 0.0, quant=None,
           ports=None, dur: float = 1.0, controls=None, defs=(),
           until: float | None = None, tempo: float = 1.0,
           sample_rate: float = 48_000.0, channels: int = 2,
           workers: int = 0, path=None):
    """Render ``obj`` — offline to ``(samples, frames)``, or onto a live
    ``destination`` when it has one to sound on.

    Args:
        obj: what to render — a binary score (``bytes``), a def or bare
            expression, an arrangement `Element`, a `Timeline`, an event
            `Pattern`, a `Routine`/`Stream` or a generator.
        destination: a `Server` to sound on — only an `Element` or a
            `Timeline` accepts one (the delegating paths); the rest are
            offline by nature.
        clock: the clock for a live ``destination``; ``None`` resolves the
            ambient one.
        at: start beat on a live ``destination`` (see `clausters.form.render`).
        quant: start quantization on a live ``destination``.
        ports: ``{name: value}`` overrides for a logical `Group`'s surface
            (see `clausters.form.render`).
        dur: seconds a def or expression is held before it is freed — the
            rendered length. Ignored by the other kinds (their content sets
            the length).
        controls: ``{name: value}`` controls a def or expression is instanced
            with.
        defs: extra defs a def render needs first (a `GraphDef`'s member
            defs; the ephemeral offline session starts empty).
        until: stop the offline bounce at this beat — required for an endless
            source (an infinite pattern never drains on its own).
        tempo: the offline bounce's clock tempo, in beats per second (beats
            of ``obj`` map to ``beat / tempo`` seconds).
        sample_rate: offline render rate, in Hz.
        channels: interleaved output channel count of the offline render.
        workers: renderer worker threads for the ``bytes`` score path.
        path: also write the offline result there as a float32 WAV.

    Returns:
        ``(samples, frames)`` for every offline path — interleaved float32 in
        a stdlib ``array('f')``. The delegating paths return what
        `clausters.form.render` returns (a `Playhead`, or the instance group
        of a logical `Group`).
    """
    from .base.stream import Routine, Stream
    from .defs import Box, FaustDef, GraphDef, Signal, SynthDef, Ugen
    from .defs.asdef import as_def
    from .form.element import Element
    from .seq.pattern import Pattern
    from .seq.timeline import Playhead, Timeline

    if isinstance(obj, (bytes, bytearray)):
        from .ipc import render as render_score

        samples, frames = render_score(bytes(obj), sample_rate, channels, workers)
        return _deliver(samples, frames, channels, sample_rate, path)

    if isinstance(obj, (Ugen, Signal, Box, SynthDef, FaustDef, GraphDef)):
        samples = bounce_def(as_def(obj), dur, controls, defs, sample_rate,
                             channels)
        samples = samples if isinstance(samples, array) else array("f", samples)
        return _deliver(samples, len(samples) // channels, channels,
                        sample_rate, path)

    if isinstance(obj, Element):
        if destination is not None:
            from .form import render as render_element

            return render_element(obj, destination, clock, at=at, quant=quant,
                                  ports=ports)
        return _bounce(lambda session: _start_element(obj, session, at),
                       until, tempo, sample_rate, channels, path)

    if isinstance(obj, Timeline):
        if destination is not None:
            clock = clock or main.resolve_clock() or main.get_default_clock()
            playhead = Playhead(obj, clock, destination)
            playhead.play(at=at, quant=quant)
            return playhead
        return _bounce(
            lambda session: Playhead(obj, session.clock, session.server)
            .play(at=at),
            until, tempo, sample_rate, channels, path)

    playable = obj
    if not isinstance(playable, (Pattern, Routine, Stream)):
        import inspect

        if inspect.isgenerator(playable) or inspect.isgeneratorfunction(playable):
            from .play import _as_routine

            playable = _as_routine(playable)
        else:
            raise TypeError(
                f"don't know how to render {type(obj).__name__}; expected a "
                "score (bytes), a def or bare expression (Ugen/Signal/Box), "
                "an arrangement Element, a Timeline, an event Pattern, or a "
                "Routine/Stream/generator"
            )
    if destination is not None:
        raise ValueError(
            "a pattern or routine renders offline only (it is forward-only); "
            "to sound it live, use play()"
        )
    if isinstance(playable, Pattern):
        return _bounce(
            lambda session: playable.play(session.clock, session.server),
            until, tempo, sample_rate, channels, path)
    return _bounce(lambda session: playable.play(session.clock),
                   until, tempo, sample_rate, channels, path)


def bounce_def(obj, dur, controls, defs, sample_rate, channels):
    """Renders a def offline: an ephemeral NRT session, the ``defs`` it needs
    plus the def itself sent at score time 0, one instance with ``controls``,
    freed at ``dur`` seconds. Returns the interleaved samples (`plot` draws
    them; `render` delivers them)."""
    from .defs.graphdef import GraphDef
    from .session import Session

    session = Session.nrt(tempo=1.0)  # beats == seconds
    server = session.server
    for d in defs:
        server.add_def(d)
    server.add_def(obj)
    if isinstance(obj, GraphDef):
        node = server.graph(obj.name, controls)
    else:
        node = server.synth(obj.name, controls)
    server.send_bundle_after(float(dur), ("/n_free", node.id))
    samples, _frames = session.render(sample_rate=sample_rate, channels=channels)
    return samples


# ---- the offline bounce ----

def _bounce(start, until, tempo, sample_rate, channels, path):
    """An ephemeral NRT session: ``start(session)`` schedules the source on
    its clock and server, the drained score renders to samples."""
    from .session import Session

    session = Session.nrt(tempo=tempo)
    with session._active():
        start(session)
    samples, frames = session.render(sample_rate=sample_rate,
                                     channels=channels, until=until)
    return _deliver(samples, frames, channels, sample_rate, path)


def _start_element(element, session, at):
    from .form import render as render_element

    render_element(element, session.server, session.clock, at=at)


def _deliver(samples, frames, channels, sample_rate, path):
    if path is not None:
        _write_wav(path, samples, channels, sample_rate)
    return samples, frames


def _write_wav(path, samples, channels, sample_rate):
    """Writes interleaved float32 samples as a WAV (IEEE-float format, with
    the fact chunk non-PCM WAV requires)."""
    buf = samples if isinstance(samples, array) else array("f", samples)
    if sys.byteorder == "big":
        buf = array("f", buf)
        buf.byteswap()
    data = buf.tobytes()
    frames = len(data) // (4 * max(1, channels))
    byte_rate = int(sample_rate) * channels * 4
    header = (
        b"RIFF" + struct.pack("<I", 4 + 24 + 12 + 8 + len(data)) + b"WAVE"
        + b"fmt " + struct.pack("<IHHIIHH", 16, 3, channels,
                                int(sample_rate), byte_rate, channels * 4, 32)
        + b"fact" + struct.pack("<II", 4, frames)
        + b"data" + struct.pack("<I", len(data))
    )
    with open(path, "wb") as f:
        f.write(header)
        f.write(data)
