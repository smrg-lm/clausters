"""Time: the piece's beat↔second map, and the questions it answers.

A **beat is not a unit of time**. It is a logical coordinate, and what turns
one into a second is the tempo — which can change along the piece. So the two
things the word "tempo" covers are kept apart here:

- the **tempo function**, what a user writes: the tempo at a beat, and how it
  moves from there (a step, a ramp);
- the **time map** (`TempoMap`), what everything queries: the second a beat
  falls on. It is the integral of ``1 / tempo`` over the beat axis, and it is
  computed once, in the native core, so every client and the editor answer
  from one implementation.

The rule that follows, and the reason this module exists rather than a
``beats / tempo`` in each caller: **a length in beats is not a duration**. The
same four beats last different seconds depending on where they sit, so seconds
come from two *positions* (`TempoMap.span_secs`), never from a beat count and a
tempo.

A `clausters.base.TempoClock` holds a map and reads it to pace and to stamp;
this module is the other half — the same map read as a **question about the
piece**, with no clock running and nothing playing:

    >>> from clausters.base.time import TempoMap, secs_to_samples
    >>> tempo = TempoMap(1.0)              # one beat a second
    >>> tempo.ramp(8.0, 16.0, 1.0, 2.0)    # accelerate over bars 3-4
    >>> tempo.secs_at(16.0)                # when does bar 5 arrive?
    13.545...
    >>> tempo.span_secs(8.0, 16.0)         # how long is the accelerando?
    5.545...
    >>> tempo.span_beats(0.0, 30.0)        # what fits in the first 30 seconds?
    48.909...

**The shapes, and their closed forms.** Every shape is written over the
segment's *normalised position* ``u = (b - b0) / db``, which is what makes the
rest of this work. Writing ``T0`` and ``T1`` for the tempos at the two ends:

| shape | tempo ``T(u)`` |
| --- | --- |
| ``"step"`` | ``T0`` |
| ``"linear"`` | ``T0 + (T1 - T0)*u`` |
| ``"exponential"`` | ``T0 * (T1/T0)**u`` |
| a curvature ``c`` | ``A + B*exp(c*u)``, with ``B = -(T1-T0)/(1-exp(c))`` and ``A = T0 + (T1-T0)/(1-exp(c))`` |

A curvature of 0 **is** linear — the knob is continuous through its middle
rather than a shape apart — positive starts slow and negative starts fast.
That is `Env`'s own convention, and these are `Env`'s own shape numbers, so one
vocabulary spells a tempo curve and an amplitude curve. A shape is named by the
plain string a caller writes (``"lin"`` and ``"exp"`` are accepted too), and a
unit the same way: ``"beats"``, or ``"seconds"`` (``"secs"``). They are options,
not constants to import — an unknown spelling is refused rather than quietly
taken for beats.

**The seconds** are the integral of ``1/T`` over the beat axis. Per unit of
beat, ``K`` is that integral from ``u = 0`` to ``u = 1``:

| shape | ``K`` |
| --- | --- |
| ``"step"`` | ``1/T0`` |
| ``"linear"`` | ``log(T1/T0) / (T1 - T0)`` |
| ``"exponential"`` | ``(1/T0 - 1/T1) / log(T1/T0)`` |
| a curvature ``c`` | ``(1 - log((A + B*exp(c))/(A + B))/c) / A`` |

so a stretch ``db`` beats wide lasts ``db * K`` seconds. **This is where an
average of the two tempos goes wrong**: over eight beats from 1 to 2 beats a
second the true length is ``log(2)/0.125 = 5.545`` s and the average says
``8/1.5 = 5.333`` s — a fifth of a second, audible and, drawn, visible.

**The extent in seconds** follows from the same ``K``, and it is why the shapes
are written over ``u`` rather than over beats: ``K`` does not depend on how wide
the segment is, so asking for a change that lasts ``dt`` seconds is one
division — ``db = dt / K`` — exact for every shape and never a search. For a
straight ramp that makes ``db`` the logarithmic mean of the two tempos times the
seconds.

**The inverse** — the beat falling on a second, which a running clock reads on
every `TempoClock.beats` — is closed for ``"linear"``
(``u = T0*(exp(k*s) - 1)/k``, ``k = T1 - T0``) and for ``"exponential"``
(``u = -log(1 - s*T0*log(T1/T0))/log(T1/T0)``). A curvature mixes ``u`` and
``exp(c*u)`` and has **no** closed inverse, so the core solves it with a
safeguarded Newton iteration — one implementation, so every client inverts to
the same place. It is also why `Env`'s ``sin`` and ``wel`` are **not** tempo
shapes: they integrate in closed form but invert transcendentally, and
inverting is the operation a clock cannot pay for on every read.

The free conversions beside it are the rest of the time seam every client
shares — the beat grid (`bar`, `beat_in_bar`, `quant_delay`) and the sample
axis (`secs_to_samples`, `samples_to_secs`) — re-exported here so the whole of
"what time is it, in which unit" reads from one import.
"""

from .. import _native
from .._native import TempoMap

__all__ = [
    "TempoMap",
    "bar",
    "beat_in_bar",
    "quant_delay",
    "secs_to_samples",
    "samples_to_secs",
]


def bar(beats: float, quant: float) -> float:
    """The bar a beat position falls in, on a grid of ``quant`` beats per bar
    (0-based; ``quant <= 0`` → bar 0).

    A bar count is a reading of the *beat* axis, so it needs no map: bars are
    beats grouped, not seconds grouped.
    """
    return _native.bar(float(beats), float(quant))


def beat_in_bar(beats: float, quant: float) -> float:
    """The beat within its bar, on a grid of ``quant`` beats per bar (0-based).
    The other half of `bar`."""
    return _native.beat_in_bar(float(beats), float(quant))


def quant_delay(pos: float, quant: float) -> float:
    """Beats to wait from ``pos`` for the next ``quant`` boundary (a position
    already on one waits 0; ``quant <= 0`` → now).

    The shared quantization rule every client applies, and what `play`'s
    ``quant`` argument is computed with.
    """
    return _native.quant_delay(float(pos), float(quant))


def secs_to_samples(secs: float, sample_rate: float) -> int:
    """Seconds → a sample count at ``sample_rate``, rounded the way the server
    rounds. A length of audio crosses on this and never on a tempo: its seconds
    were fixed before any tempo was."""
    return _native.secs_to_samples(float(secs), float(sample_rate))


def samples_to_secs(samples: int, sample_rate: float) -> float:
    """A sample count → seconds at ``sample_rate`` — the inverse of
    `secs_to_samples`."""
    return _native.samples_to_secs(int(samples), float(sample_rate))
