"""The `demand` builders that mirror the wire, re-exported by `demand`.

Generated from the server's UGen registry and
`crates/clausters-server/src/dsp/registry/builders/demand.toml` by
`crates/clausters-server/tests/ugen_builders.rs`. Do not edit: change the
description and regenerate with
`CLAUSTERS_BLESS=1 cargo test -p clausters --test ugen_builders`.
"""

from ..graph import Ugen

__all__ = [
    "dseries",
    "dgeom",
    "dwhite",
    "diwhite",
    "dbrown",
    "dibrown",
    "dstutter",
    "dbufrd",
    "demand",
    "duty",
    "tduty",
]


def dseries(repeats=0.0, start=0.0, step=1.0) -> Ugen:
    """An arithmetic sequence: `start`, `start + step`, ... The step is read
    on every item, so it may itself be a stream.
    """
    return Ugen("Dseries", [repeats, start, step], rate="dr")


def dgeom(repeats=0.0, start=1.0, grow=2.0) -> Ugen:
    """A geometric sequence: `start`, `start * grow`, ..."""
    return Ugen("Dgeom", [repeats, start, grow], rate="dr")


def dwhite(repeats=0.0, lo=0.0, hi=1.0) -> Ugen:
    """Independent uniform draws on `[lo, hi]`."""
    return Ugen("Dwhite", [repeats, lo, hi], rate="dr")


def diwhite(repeats=0.0, lo=0.0, hi=1.0) -> Ugen:
    """`dwhite` over the integers in `[lo, hi]`, both ends included."""
    return Ugen("Diwhite", [repeats, lo, hi], rate="dr")


def dbrown(repeats=0.0, lo=0.0, hi=1.0, step=0.01) -> Ugen:
    """A random walk of at most `step` per item, **folded** into
    `[lo, hi]` -- it turns around at a bound rather than piling up against
    it.
    """
    return Ugen("Dbrown", [repeats, lo, hi, step], rate="dr")


def dibrown(repeats=0.0, lo=0.0, hi=1.0, step=1.0) -> Ugen:
    """`dbrown` over the integers."""
    return Ugen("Dibrown", [repeats, lo, hi, step], rate="dr")


def dstutter(repeats, value) -> Ugen:
    """Repeats each item of the `value` stream `repeats` times. The count
    is pulled per item, so it may vary.
    """
    return Ugen("Dstutter", [repeats, value], rate="dr")


def dbufrd(bufnum, phase, loop=1.0, channel=0.0) -> Ugen:
    """Reads the buffer frame the `phase` stream names -- a `dseries` phase
    walks it as a step sequence. Out of range it wraps when `loop` is set and
    clamps when it is not.
    """
    return Ugen("Dbufrd", [bufnum, phase, loop, channel], rate="dr")


def demand(trig, reset, source) -> Ugen:
    """Demand driver: pulls the next value from `source` on each rising edge
    of `trig` and holds it between triggers; a rising `reset` restarts the
    stream. Once the stream ends the last value is held.
    """
    return Ugen("Demand", [trig, reset, source])


def duty(dur, reset=0.0, level=1.0, done_action=0.0) -> Ugen:
    """Demand driver with a clock of its own: pulls one `level` every
    `dur` seconds and holds it.

    Both `dur` and `level` are pulled, which is what makes a sequencer of
    it -- a stream of durations against a stream of pitches, the two free to be
    different lengths. When either ends, `done_action` fires (see
    `DoneAction`).
    """
    return Ugen("Duty", [dur, reset, level, done_action])


def tduty(dur, reset=0.0, level=1.0, done_action=0.0, gap_first=0.0) -> Ugen:
    """`duty` emitting each level on its own sample and nothing in between -- a
    trigger stream whose amplitudes are the levels. With `gap_first` the
    first duration is spent before the first level, so the stream opens with a
    gap instead of a trigger.
    """
    return Ugen("TDuty", [dur, reset, level, done_action, gap_first])
