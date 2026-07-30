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
from clausters.defs import sine

samples, frames = render(sine(440.0) * 0.2, dur=2.0)
render(Pbind(degree=Pseq([0, 2, 4]), dur=0.5), path="phrase.wav")
render(my_piece, until=64.0, path="piece.wav")     # an arrangement, bounced
```
"""

import struct
import sys
from array import array
from dataclasses import dataclass, field

from .base.main import main

__all__ = ["render", "bounce_def", "RenderStats", "read_soundfile",
           "render_to_file", "channels", "interleave"]


@dataclass(frozen=True)
class RenderStats:
    """What a render did — the one thing every render returns.

    ``samples`` holds the audio when the render kept it in memory, and is
    ``None`` when a ``path`` sent it to a file instead: **the path chooses
    where the output goes, not whether there is one**, so the stats come back
    either way. Read a written file with `read_soundfile`.

    ``peak`` and ``rms`` are **per channel**, in channel order, measured by
    the renderer as it streamed — not by a second pass here, which for a
    file-bound render would mean reading the file back.
    """

    frames: int
    channels: int
    sample_rate: float
    events: int
    peak: tuple[float, ...] = ()
    rms: tuple[float, ...] = ()
    path: str | None = None
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

    Interleaved is the currency everywhere in Clausters — it is the server's
    own buffer layout (`/b_getn` indexes ``frame * channels + channel``, and
    `/b_export` writes the same order), so audio *going to* the server needs
    no conversion. Deinterleaving is for analysis on this side, and it is
    cheap: extended slicing on an `array` is a C-level strided copy, a few
    percent of what the render itself costs. `interleave` is the inverse.
    """
    return tuple(samples[i::count] for i in range(count))


def interleave(*chans) -> array:
    """Weave per-channel arrays back into one interleaved `array('f')` — the
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

    WAV, FLAC, OGG/Vorbis, MP3, MP4/AAC, ALAC, AIFF and the rest — whatever
    the file holds, the samples come back interleaved `float32`, scaled to
    ``[-1, 1]``, at the file's own sample rate (nothing here resamples). This
    is the same decoder ``/b_allocRead`` uses, which is why the client needs
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
        return render_score(bytes(obj), sample_rate, channels, workers, path)

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
    return session.render(sample_rate=sample_rate, channels=channels).samples


# ---- the offline bounce ----

def _bounce(start, until, tempo, sample_rate, channels, path):
    """An ephemeral NRT session: ``start(session)`` schedules the source on
    its clock and server, the drained score renders to samples."""
    from .session import Session

    session = Session.nrt(tempo=tempo)
    with session._active():
        start(session)
    return session.render(sample_rate=sample_rate, channels=channels,
                          until=until, path=path)


def _start_element(element, session, at):
    from .form import render as render_element

    render_element(element, session.server, session.clock, at=at)


def render_score(score: bytes, sample_rate: float = 48_000.0, channels: int = 2,
                 workers: int = 0, path=None, seed: int | None = None,
                 sample_format: str = "float") -> RenderStats:
    """Render a binary score. Without `path` the samples come back in the
    stats; with it the server writes the file (see `render_to_file`)."""
    from . import ipc

    seed = ipc.SEED_STRIDE if seed is None else seed
    if path is not None:
        return render_to_file(score, path, sample_rate, channels, workers,
                              seed, sample_format)
    samples, frames, events = ipc.render(score, sample_rate=sample_rate,
                                         channels=channels, workers=workers,
                                         seed=seed)
    peak, rms = ipc.channel_stats(samples, channels)
    return RenderStats(frames=frames, channels=channels,
                       sample_rate=sample_rate, events=events, peak=peak,
                       rms=rms, samples=samples)


def _deliver(samples, frames, channels, sample_rate, path):
    """Wrap in-memory samples as stats, writing them out first when asked.

    Only the def/expression branch lands here: a `dur`-second bounce is short
    by construction and already in memory, so writing it from this side costs
    nothing. Score, pattern and routine renders take the server's writer.
    """
    if path is not None:
        _write_wav(path, samples, channels, sample_rate)
    from . import ipc

    peak, rms = ipc.channel_stats(samples, channels)
    return RenderStats(frames=frames, channels=channels,
                       sample_rate=sample_rate, events=0, peak=peak, rms=rms,
                       path=None if path is None else str(path),
                       samples=samples)


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


def render_to_file(score: bytes, path, sample_rate: float, channels: int,
                    workers: int, seed: int, sample_format: str):
    """Hand a binary score to the ``clausters --nrt`` renderer, which writes
    the file itself — the one place in the client that turns a score into a
    soundfile.

    The score goes out through a temporary ``.osc``, because ``--nrt`` takes
    two paths; ``--stats`` brings the render's numbers back as one JSON line,
    so the caller learns the frames, events, peak and RMS without opening the
    file the server just wrote.
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
                "--format", sample_format, "--seed", str(int(seed)),
                "--workers", str(int(workers)), "--stats"]
        done = subprocess.run(argv, capture_output=True, text=True)
        if done.returncode != 0:
            raise RenderError((done.stderr or done.stdout).strip() or "render failed")
        info = json.loads(done.stdout.strip().splitlines()[-1])
    finally:
        os.unlink(score_path)
    return RenderStats(frames=info["frames"], channels=info["channels"],
                       sample_rate=info["sampleRate"], events=info["events"],
                       peak=tuple(info["peak"]), rms=tuple(info["rms"]),
                       path=str(path))
