"""Demand rate: streams that have a next value, not samples.

A demand UGen is pulled rather than run — `demand` (or `duty`) asks for the next
value on a trigger and the stream advances only then — and nesting them is how a
sequence is built.
"""

from .graph import ChannelList, Ugen
from .pan import _sources

# ---- demand rate (dr) ----


# A demand UGen is a *stream*: it has no samples, only a next value, and it
# yields one each time a driver asks. Its inputs may be streams too — that is
# what makes a sequence of phrases, rather than of numbers, expressible — so
# every builder below accepts another `d*` anywhere it accepts a number.
#
# ``repeats`` is how many the stream yields before it ends: **0 means
# endlessly**. sclang writes ``inf`` there, which a def cannot carry (the wire
# rejects a non-finite constant, and JSON has no spelling for one), so the count
# of none is the endless one. For a list source it counts *passes over the
# list*; for a random pick it counts *items* — scsynth's own asymmetry, and the
# useful reading of each.


def _values(values):
    """The value list of a demand source, as a list or a `ChannelList`."""
    items = list(values.items if isinstance(values, ChannelList) else values)
    if not items:
        raise ValueError("a demand source needs at least one value")
    return items


def dseq(values, repeats=0.0) -> Ugen:
    """A demand sequence: yields ``values`` in order, ``repeats`` times
    (``0`` endlessly), then ends.

    A value may be another demand stream, and then it is *drained* rather than
    taken once — ``dseq([dseries(3, 0, 1), 100])`` is four items — and restarted
    when the sequence comes round to it again."""
    return Ugen("Dseq", [repeats, *_values(values)], rate="dr")


def drand(values, repeats=0.0) -> Ugen:
    """``repeats`` items drawn at random from ``values``, each pick independent
    of the last. Unlike `dseq`, the count is of items, not passes."""
    return Ugen("Drand", [repeats, *_values(values)], rate="dr")


def dxrand(values, repeats=0.0) -> Ugen:
    """`drand` that never picks the value it just used — the same list without
    immediate repetition."""
    return Ugen("Dxrand", [repeats, *_values(values)], rate="dr")


def dshuf(values, repeats=0.0) -> Ugen:
    """``values`` shuffled **once** and then replayed in that order,
    ``repeats`` times. The shuffle is redrawn on a reset, not on each pass —
    that is what separates it from `drand`."""
    return Ugen("Dshuf", [repeats, *_values(values)], rate="dr")


def dseries(repeats=0.0, start=0.0, step=1.0) -> Ugen:
    """An arithmetic sequence: ``start``, ``start + step``, … The step is read
    on every item, so it may itself be a stream."""
    return Ugen("Dseries", [repeats, start, step], rate="dr")


def dgeom(repeats=0.0, start=1.0, grow=2.0) -> Ugen:
    """A geometric sequence: ``start``, ``start * grow``, …"""
    return Ugen("Dgeom", [repeats, start, grow], rate="dr")


def dwhite(repeats=0.0, lo=0.0, hi=1.0) -> Ugen:
    """Independent uniform draws on ``[lo, hi]``."""
    return Ugen("Dwhite", [repeats, lo, hi], rate="dr")


def diwhite(repeats=0.0, lo=0.0, hi=1.0) -> Ugen:
    """`dwhite` over the integers in ``[lo, hi]``, both ends included."""
    return Ugen("Diwhite", [repeats, lo, hi], rate="dr")


def dbrown(repeats=0.0, lo=0.0, hi=1.0, step=0.01) -> Ugen:
    """A random walk of at most ``step`` per item, **folded** into
    ``[lo, hi]`` — it turns around at a bound rather than piling up against
    it."""
    return Ugen("Dbrown", [repeats, lo, hi, step], rate="dr")


def dibrown(repeats=0.0, lo=0.0, hi=1.0, step=1.0) -> Ugen:
    """`dbrown` over the integers."""
    return Ugen("Dibrown", [repeats, lo, hi, step], rate="dr")


def dstutter(repeats, value) -> Ugen:
    """Repeats each item of the ``value`` stream ``repeats`` times. The count
    is pulled per item, so it may vary."""
    return Ugen("Dstutter", [repeats, value], rate="dr")


def dswitch1(which, *sources) -> Ugen:
    """Takes **one** item from the stream ``which`` picks, then picks again.

    Unlike `dseq`, an unselected stream is not advanced and the selected one is
    not drained — the ``1`` is the count. The index wraps into range. Accepts
    the sources as arguments or as one list."""
    return Ugen("Dswitch1", [which, *_sources(sources)], rate="dr")


def dbufrd(bufnum, phase, loop=1.0, channel=0.0) -> Ugen:
    """Reads the buffer frame the ``phase`` stream names — a `dseries` phase
    walks it as a step sequence. Out of range it wraps when ``loop`` is set and
    clamps when it is not."""
    return Ugen("Dbufrd", [bufnum, phase, loop, channel], rate="dr")


def demand(trig, reset, source) -> Ugen:
    """Demand driver: pulls the next value from ``source`` on each rising edge
    of ``trig`` and holds it between triggers; a rising ``reset`` restarts the
    stream. Once the stream ends the last value is held."""
    return Ugen("Demand", [trig, reset, source])


def duty(dur, reset=0.0, level=1.0, done_action=0) -> Ugen:
    """Demand driver with a clock of its own: pulls one ``level`` every
    ``dur`` seconds and holds it.

    Both ``dur`` and ``level`` are pulled, which is what makes a sequencer of
    it — a stream of durations against a stream of pitches, the two free to be
    different lengths. When either ends, ``done_action`` fires (see
    `DoneAction`)."""
    return Ugen("Duty", [dur, reset, level, done_action])


def tduty(dur, reset=0.0, level=1.0, done_action=0, gap_first=0.0) -> Ugen:
    """`duty` emitting each level on its own sample and nothing in between — a
    trigger stream whose amplitudes are the levels. With ``gap_first`` the
    first duration is spent before the first level, so the stream opens with a
    gap instead of a trigger."""
    return Ugen("TDuty", [dur, reset, level, done_action, gap_first])
