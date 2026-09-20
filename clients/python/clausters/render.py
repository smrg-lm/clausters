"""The free-standing ``render`` -- one verb for the change of state to sound.

`render` is the third ambient verb, next to `clausters.play` and
`clausters.plot`: it turns a **generator** thing (an algorithm that describes
sound) into a **generated** one (samples -- random-access audio). It dispatches
by kind:

- a binary **score** (``bytes``) -> the embedded offline renderer, exactly the
  historical ``clausters.render`` (see `clausters.ipc.render`);
- a **def** (`clausters.defs.SynthDef` / `clausters.defs.FaustDef` /
  `clausters.defs.GraphDef`) or a bare **expression** (a `clausters.defs.Ugen`
  graph, a Faust `clausters.defs.Signal` or `clausters.defs.Box`, coerced
  through `clausters.defs.asdef.as_def`) -> instanced offline for ``dur``
  seconds -- the audible sibling of ``plot(def)``;
- an arrangement **`Element`** -> with a ``destination``, delegates to
  `clausters.form.render` (the arrangement's own seam: RT or NRT by the
  destination); without one, an **offline bounce** -- an ephemeral NRT session
  plays it and renders the score;
- a `clausters.seq.timeline.Timeline` -> the same dual: played on
  ``destination`` on its own clock, or the offline bounce, whose seconds are
  the timeline's own tempo map;
- an `clausters.seq.pattern.EventPattern`, a `clausters.base.stream.Routine`
  / `clausters.base.stream.Stream` or a bare **generator** -> offline bounce
  only (they are forward-only; sounding them live is `clausters.play`'s job).
  An endless source needs ``until`` (the bounce would never drain);
- a **value pattern** (a `clausters.seq.pattern.Pattern` whose values are not
  events) -> the values it generates, as a list. An endless one needs
  ``count``.

**An offline bounce runs in an offline session**, and the tempo is the clock's:
``clock`` is the one it plays on, as it is for `clausters.play`. It must be a
clock of an offline session (`clausters.Session.nrt`), on its
`clausters.base.LogicalTimebase`, and the render is that session's; with no
``clock`` the render makes an offline session of its own and plays on its clock,
at tempo 1.0.

The one thing a render returns that is not a `RenderStats` is a value
pattern's list, since there is no audio in it.

Every offline path returns a `RenderStats`: the frame, channel and event
counts, per-channel peak and RMS, and the samples themselves (interleaved
float32 in a stdlib ``array('f')``) when the render kept them. Passing
``path`` sends the audio to a file instead -- written by the server's own
``--nrt`` renderer, so it never crosses into this process -- and leaves
``samples`` ``None``. That holds for every kind of render, a bare expression
included: there is no second writer on this side. Read one back with
`read_soundfile`.

A render with no ``seed`` draws a fresh one, so anything with a stochastic
UGen in it is a new take every call; ``stats.seed`` reports the one used, and
handing it back replays that take exactly.

```python
from clausters import render
from clausters.defs import sine

stats = render(sine(440.0) * 0.2, dur=2.0)
render(Pbind(degree=Pseq([0, 2, 4]), dur=0.5), path="phrase.wav")
render(Pseq([1, 2, 3], 2))                          # [1, 2, 3, 1, 2, 3]
render(multitrack, until=64.0, path="take.wav")    # an arrangement, bounced
```
"""

from array import array
from dataclasses import dataclass, field

from .base.main import main
from .defs.node import Group, Synth

__all__ = ["render", "bounce_def", "RenderStats", "read_soundfile",
           "render_to_file", "channels", "interleave", "MAX_BOUNCED_EVENTS"]


#: How many events -- or values -- a render takes before it decides its
#: source is endless, when no ``until`` (or ``count``) bounds it.
#:
#: A render holds what it generated in memory, so a million is already past any
#: real run and nowhere near a legitimate one -- which is what makes the cap
#: honest *here* and wrong inside `clausters.base.TempoClock.render`, where a
#: long offline render of a real score is exactly the thing that runs for a very
#: long time on purpose.
MAX_BOUNCED_EVENTS = 1_000_000


@dataclass(frozen=True)
class RenderStats:
    """What a render did -- the one thing every render returns.

    ``samples`` holds the audio when the render kept it in memory, and is
    ``None`` when a ``path`` sent it to a file instead: **the path chooses
    where the output goes, not whether there is one**, so the stats come back
    either way. Read a written file with `read_soundfile`.

    ``peak`` and ``rms`` are **per channel**, in channel order, measured by
    the renderer as it streamed -- not by a second pass here, which for a
    file-bound render would mean reading the file back.

    ``seed`` is the one this render's stochastic UGens started from. Unless
    you asked for a seed you got a fresh one, so **this is how you get a take
    back**: pass it as ``seed=`` and the render repeats sample for sample.
    """

    frames: int
    channels: int
    sample_rate: float
    events: int
    peak: tuple[float, ...] = ()
    rms: tuple[float, ...] = ()
    path: str | None = None
    seed: int = 0
    samples: array | None = field(default=None, repr=False)

    @property
    def duration(self) -> float:
        """Length in seconds."""
        return self.frames / self.sample_rate if self.sample_rate else 0.0

    def channel(self, index: int) -> array:
        """Channel `index` on its own, deinterleaved (see `channels`)."""
        if self.samples is None:
            raise ValueError(
                "this render went to a file, so it has no samples here; read "
                "them back with read_soundfile(stats.path)"
            )
        return self.samples[index::self.channels]


def channels(samples, count: int) -> tuple[array, ...]:
    """Split interleaved `samples` into `count` per-channel arrays.

    Interleaved is the currency everywhere in Clausters -- it is the server's
    own buffer layout (`/buffer_getRange` indexes ``frame * channels + channel``, and
    `/buffer_export` writes the same order), so audio *going to* the server needs
    no conversion. Deinterleaving is for analysis on this side, and it is
    cheap: extended slicing on an `array` is a C-level strided copy, a few
    percent of what the render itself costs. `interleave` is the inverse.
    """
    return tuple(samples[i::count] for i in range(count))


def interleave(*chans) -> array:
    """Weave per-channel arrays back into one interleaved `array('f')` -- the
    inverse of `channels`, and the layout the server wants."""
    if not chans:
        return array("f")
    n = len(chans[0])
    if any(len(c) != n for c in chans):
        raise ValueError("every channel must have the same length")
    out = array("f", bytes(4 * n * len(chans)))
    for i, c in enumerate(chans):
        out[i::len(chans)] = array("f", c)
    return out


def read_soundfile(path, start: int = 0, frames: int = -1) -> RenderStats:
    """Read an audio file through **the server's own decoder**.

    WAV, FLAC, OGG/Vorbis, MP3, MP4/AAC, ALAC, AIFF and the rest -- whatever
    the file holds, the samples come back interleaved `float32`, scaled to
    ``[-1, 1]``, at the file's own sample rate (nothing here resamples). This
    is the same decoder ``/buffer_allocRead`` uses, which is why the client needs
    no audio library of its own: the stdlib `wave` module cannot even read
    the float32 WAV that `render` writes.

    Reads `frames` frames from `start`; ``frames = -1`` means to the end.
    Returns a `RenderStats` with ``samples`` filled and ``events`` 0.
    """
    from . import ipc

    samples, n, chans, rate = ipc.read_soundfile(path, start, frames)
    peak, rms = _measure(samples, chans)
    return RenderStats(frames=n, channels=chans, sample_rate=rate, events=0,
                       peak=peak, rms=rms, path=str(path), samples=samples)


def _measure(samples, chans: int):
    """Per-channel peak and RMS, through the core so the numbers match the
    server's own (`clausters_core_stats`)."""
    from . import ipc

    return ipc.channel_stats(samples, chans)


def render(obj, *, destination=None, clock=None, at: float = 0.0, quant=None,
           ports=None, dur: float = 1.0, controls=None, defs=(),
           until: float | None = None, count: int | None = None,
           sample_rate: float = 48_000.0, channels: int = 2,
           workers: int = 0, path=None, seed: int | None = None):
    """Render ``obj`` -- offline to a `RenderStats`, or onto a live
    ``destination`` when it has one to sound on.

    Args:
        obj: what to render -- a binary score (``bytes``), a def or bare
            expression, an arrangement `Element`, a `Timeline`, an
            `EventPattern`, a `Routine`/`Stream`, a generator, or a value
            pattern.
        destination: a `Server` to sound on -- only an `Element` or a
            `Timeline` accepts one (the delegating paths); the rest are
            offline by nature.
        clock: the clock it plays on, as for `clausters.play`. Offline it is a
            clock of an offline session, whose render this is; ``None`` makes
            an offline session of its own. On a live ``destination``, ``None``
            resolves the ambient one.
        at: start beat on a live ``destination`` (see `clausters.form.render`).
        quant: start quantization on a live ``destination``.
        ports: ``{name: value}`` overrides for a logical `Group`'s surface
            (see `clausters.form.render`).
        dur: seconds a def or expression is held before it is freed -- the
            rendered length. Ignored by the other kinds (their content sets
            the length).
        controls: ``{name: value}`` controls a def or expression is instanced
            with.
        defs: extra defs the render needs first -- a `GraphDef`'s member defs,
            or the instrument a bounced pattern, timeline or routine names.
            Every offline path starts from an **empty** ephemeral session, so
            whatever it names has to ride along.
        until: stop the offline bounce at this beat of ``clock`` -- required
            for an endless source (an infinite pattern never drains on its
            own; an event pattern with no bound raises after
            `MAX_BOUNCED_EVENTS` events).
        count: how many values a value pattern generates -- required for an
            endless one, which otherwise raises after `MAX_BOUNCED_EVENTS`.
        sample_rate: offline render rate, in Hz.
        channels: interleaved output channel count of the offline render --
            the outputs the offline server has, not a property of what is
            being rendered. A bare expression wider than that is writing onto
            internal buses that reach no file, and raises.
        workers: renderer worker threads for the ``bytes`` score path.
        path: send the audio to this file instead of returning it; the
            server writes it (see `render_to_file`).
        seed: starting seed for the render's stochastic UGens. ``None`` draws
            a fresh one, so anything with noise in it renders a new take every
            call; ``stats.seed`` reports the one used, and handing it back
            here replays that take exactly.

    Returns:
        A `RenderStats` for every offline path but a value pattern's, which
        is the list of its values. A timeline on a ``destination``
        returns the timeline; an `Element` on one returns what
        `clausters.form.render` returns (the timeline it flattened to, or the
        instance group of a logical `Group`).
    """
    from .base.stream import Routine, Stream
    from .defs import Expr, FaustDef, GraphDef, SynthDef
    from .defs.asdef import as_def
    from .form.element import Element
    from .seq.pattern import EventPattern, Pattern
    from .seq.timeline import Timeline

    if isinstance(obj, (bytes, bytearray)):
        return render_score(bytes(obj), sample_rate, channels, workers, path,
                            seed)

    if isinstance(obj, (Expr, SynthDef, FaustDef, GraphDef)):
        _check_expr_width(obj, channels)
        return bounce_def(as_def(obj), dur, controls, defs, sample_rate,
                          channels, seed, path)

    if isinstance(obj, Element):
        if destination is not None:
            from .form import render as render_element

            return render_element(obj, destination, clock, at=at, quant=quant,
                                  ports=ports)
        return _bounce(lambda session, on: _start_element(obj, session, on, at),
                       clock, until, sample_rate, channels, path, seed, defs)

    if isinstance(obj, Timeline):
        if destination is not None:
            return obj.play(at=at, quant=quant, destination=destination)
        return _bounce(
            lambda session, on: obj.play(at=at, destination=session.server),
            clock, until, sample_rate, channels, path, seed, defs)

    if isinstance(obj, Pattern) and not isinstance(obj, EventPattern):
        if destination is not None:
            raise ValueError(
                "a value pattern has nothing to sound: its render is the values "
                "it generates"
            )
        return _values(obj, count)

    playable = obj
    if not isinstance(playable, (EventPattern, Routine, Stream)):
        import inspect

        if inspect.isgenerator(playable) or inspect.isgeneratorfunction(playable):
            from .play import _as_routine

            playable = _as_routine(playable)
        else:
            raise TypeError(
                f"don't know how to render {type(obj).__name__}; expected a "
                "score (bytes), a def or bare expression "
                "(Ugen/ChannelList/Signal/Box), "
                "an arrangement Element, a Timeline, a pattern, or a "
                "Routine/Stream/generator"
            )
    if destination is not None:
        raise ValueError(
            "a pattern or routine renders offline only (it is forward-only); "
            "to sound it live, use play()"
        )
    if isinstance(playable, EventPattern):
        return _bounce(
            lambda session, on: playable.play(on, session.server),
            clock, until, sample_rate, channels, path, seed, defs,
            guard="event pattern")
    return _bounce(lambda session, on: playable.play(on),
                   clock, until, sample_rate, channels, path, seed, defs)


def _values(pattern, count):
    """The values a value pattern generates: ``count`` of them, or all of them
    for a finite one -- refused past `MAX_BOUNCED_EVENTS` with no ``count``."""
    values = []
    limit = MAX_BOUNCED_EVENTS if count is None else int(count)
    for value in pattern:
        if len(values) == limit:
            if count is None:
                raise RuntimeError(
                    f"render: the {type(pattern).__name__} did not end after "
                    f"{MAX_BOUNCED_EVENTS} values -- pass count= to take a number "
                    f"of them"
                )
            break
        values.append(value)
    return values


def _check_expr_width(obj, channels):
    """Refuses a bare expression laid past the render's outputs.

    ``channels`` is the offline server's output count -- how many channels the
    render *has* -- not a property of the graph, so it is not derived from one
    (`clausters.defs.expr_channels` explains the split between the verbs).
    An expression `clausters.defs.as_def` lays on more buses than that writes
    the surplus onto internal buses, which reach no file: silently half a take.
    Only the buses the coercion itself assigned are checked -- an explicit
    ``out(8, sig)`` is the caller's own routing.
    """
    from .defs.asdef import expr_channels

    width = expr_channels(obj)
    if width is not None and width > channels:
        raise ValueError(
            f"this expression writes {width} channels but the render has "
            f"{channels} output channels, so channels {channels}..{width - 1} "
            f"would land on internal buses and reach no file; pass "
            f"channels={width} (or mix() the expression down)"
        )


def bounce_def(obj, dur, controls, defs, sample_rate, channels, seed=None,
               path=None):
    """Renders a def offline: an ephemeral NRT session, the ``defs`` it needs
    plus the def itself sent at score time 0, one instance with ``controls``,
    freed at ``dur`` seconds. Returns the whole `RenderStats` -- `plot` draws
    its samples, `render` returns it as is -- because the take's ``seed`` is
    part of what happened, and a def with a noise UGen in it has a different
    one every call unless ``seed`` says otherwise.

    ``path`` goes to the session, which is to say to the server's own writer:
    this branch has no writer of its own, so a def bounce lands in a file the
    same way every other render does, and its ``samples`` come back ``None``
    like every other render's."""
    from .defs.graphdef import GraphDef
    from .session import Session

    session = Session.nrt()  # its clock at tempo 1.0: beats == seconds
    server = session.server
    for d in defs:
        d.send(server)
    obj.send(server)
    if isinstance(obj, GraphDef):
        node = Group.graph(obj.name, controls, server=server)
    else:
        node = Synth(obj.name, controls, server=server)
    server.send_bundle_after(float(dur), ("/node_free", node.id))
    return session.render(sample_rate=sample_rate, channels=channels, seed=seed,
                          path=path)


# ---- the offline bounce ----

def _bounce(start, clock, until, sample_rate, channels, path, seed, defs=(),
            guard=None):
    """An offline session: ``start(session, clock)`` schedules the source on
    the clock it plays on and the session's server, and the drained score
    renders to samples.

    The session is ``clock``'s, which has to be an offline one; with no
    ``clock`` it is a new one, which starts **empty** -- it is not the one the
    caller has been working in -- so a pattern naming an instrument of its own
    has to bring it along in ``defs``, exactly as a def bounce does.

    ``guard`` names what an unbounded render is refused for after
    `MAX_BOUNCED_EVENTS` events.
    """
    from .base.timebase import LogicalTimebase
    from .session import Session

    if clock is None:
        session = Session.nrt()
        clock = session.clock
    else:
        session = clock.session
        if not isinstance(clock.timebase, LogicalTimebase) or session is None:
            raise ValueError(
                f"render plays on a clock of an offline session, and this clock "
                f"is on a {type(clock.timebase).__name__}"
                f"{'' if session is not None else ' in no session'}: a clock's "
                f"timebase is fixed when it is made. Pass a clock made in "
                f"Session.nrt(), or none for a session of the render's own"
            )
    for d in defs:
        d.send(session.server)
    steps = MAX_BOUNCED_EVENTS if guard is not None and until is None else None
    with session._active():
        start(session, clock)
        try:
            clock.render(until, max_steps=steps)
        except RuntimeError as exc:
            if steps is None:
                raise
            raise RuntimeError(
                f"render: the {guard} did not end after {MAX_BOUNCED_EVENTS} "
                f"events -- pass until= to bound it"
            ) from exc
    return session.server.render(sample_rate=sample_rate, channels=channels,
                                 path=path, seed=seed)


def _start_element(element, session, clock, at):
    from .form import render as render_element

    render_element(element, session.server, clock, at=at)


def render_score(score: bytes, sample_rate: float = 48_000.0, channels: int = 2,
                 workers: int = 0, path=None, seed: int | None = None,
                 sample_format: str = "float") -> RenderStats:
    """Render a binary score. Without `path` the samples come back in the
    stats; with it the server writes the file (see `render_to_file`).

    ``seed`` ``None`` renders a fresh take and reports the seed it drew in
    ``stats.seed``.
    """
    from . import ipc

    if path is not None:
        return render_to_file(score, path, sample_rate, channels, workers,
                              seed, sample_format)
    samples, frames, events, used = ipc.render(score, sample_rate=sample_rate,
                                               channels=channels,
                                               workers=workers, seed=seed)
    peak, rms = ipc.channel_stats(samples, channels)
    return RenderStats(frames=frames, channels=channels,
                       sample_rate=sample_rate, events=events, peak=peak,
                       rms=rms, seed=used, samples=samples)


def render_to_file(score: bytes, path, sample_rate: float, channels: int,
                    workers: int, seed: int | None, sample_format: str):
    """Hand a binary score to the ``clausters --nrt`` renderer, which writes
    the file itself -- the one place in the client that turns a score into a
    soundfile.

    The score goes out through a temporary ``.osc``, because ``--nrt`` takes
    two paths; ``--stats`` brings the render's numbers back as one JSON line,
    so the caller learns the frames, events, peak, RMS and seed without
    opening the file the server just wrote.

    ``seed`` ``None`` leaves ``--seed`` off, so the renderer draws one -- a
    bounce is a performance like any other. It comes back in ``stats.seed``.
    """
    import json
    import os
    import subprocess
    import tempfile

    from . import _cli
    from .errors import RenderError

    fd, score_path = tempfile.mkstemp(prefix="clausters-score-", suffix=".osc")
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(score)
        argv = [_cli.server_path(), "--nrt", score_path, str(path),
                "--rate", repr(float(sample_rate)), "--channels", str(int(channels)),
                "--format", sample_format,
                "--workers", str(int(workers)), "--stats"]
        if seed is not None:
            argv += ["--seed", str(int(seed))]
        done = subprocess.run(argv, capture_output=True, text=True)
        if done.returncode != 0:
            raise RenderError((done.stderr or done.stdout).strip() or "render failed")
        info = json.loads(done.stdout.strip().splitlines()[-1])
    finally:
        os.unlink(score_path)
    return RenderStats(frames=info["frames"], channels=info["channels"],
                       sample_rate=info["sampleRate"], events=info["events"],
                       peak=tuple(info["peak"]), rms=tuple(info["rms"]),
                       seed=info["seed"], path=str(path))
