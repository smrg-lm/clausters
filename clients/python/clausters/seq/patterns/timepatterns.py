"""Value patterns of time: what a pattern yields depends on the beat the walk
is at, the logical beat of the routine playing it.

They read the clock, so they have meaning only where a clock moves -- as keys
of an event pattern that plays, or in a routine. Walked by a plain loop the
beat does not move: `ptime` stays at 0, and `pstep` and `pseg` hold their
first value, endlessly.
"""

from ...base.moment import Moment
from ...base.stream import StopStream
from ...defs.ugens.env import _resolve_curve
from ... import _native
from ..pattern import INF, as_pattern, as_stream, embed, pattern
from .listpatterns import pseq
from .valuepatterns import _counting


def _beat():
    return Moment.current().beat


def _listed(value):
    """A list given where a pattern is expected is a `pseq` of it."""
    return pseq(value) if isinstance(value, (list, tuple)) else as_pattern(value)


@pattern
def ptime(inval, length=INF):
    """The beats since the walk started, at each step."""
    start = _beat()
    for _ in _counting(length):
        inval = yield _beat() - start
    return inval


@pattern
def pstep(inval, levels, durs=1, repeats=1):
    """A step function of time: each of ``levels`` held for its duration in
    ``durs`` (beats), whatever is asked of it in between -- a value that
    changes on a grid of its own, not the events'. ``levels`` and ``durs``
    may be lists or patterns."""
    end = _beat()
    for _ in _counting(repeats):
        level_stream = as_stream(_listed(levels))
        dur_stream = as_stream(_listed(durs))
        try:
            while True:
                level, dur = level_stream.next(inval), dur_stream.next(inval)
                end += dur
                while end > _beat():
                    inval = yield from embed(level, inval)
        except StopStream:
            pass
    return inval


@pattern
def pseg(inval, levels, durs=1, curves="lin", repeats=1):
    """A curve of time: from each of ``levels`` to the next over its duration
    in ``durs`` (beats), by the shape in ``curves`` (``"lin"``, ``"exp"``, a
    curvature ...) -- what an envelope draws, read at the beat it is asked.
    ``levels``, ``durs`` and ``curves`` may be lists or patterns."""
    for _ in _counting(repeats):
        level_stream = as_stream(_listed(levels))
        dur_stream = as_stream(_listed(durs))
        curve_stream = as_stream(_listed(curves))
        try:
            level = level_stream.next(inval)
            end = _beat()
            while True:
                start_level = level
                level = level_stream.next(inval)
                dur = dur_stream.next(inval)
                shape, curvature = _resolve_curve(curve_stream.next(inval))
                start, end = end, end + dur
                now = _beat()
                while end > now:
                    t = (now - start) / dur
                    inval = yield _native.shape_value(shape, curvature, start_level, level, t)
                    now = _beat()
        except StopStream:
            pass
    return inval
