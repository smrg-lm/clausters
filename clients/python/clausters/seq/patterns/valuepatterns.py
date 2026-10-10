"""Value patterns computed from their arguments: a constant, a series.

Each is a value pattern: its stream yields values, ``length`` of them (endless
by default). An argument may itself be a pattern, read one value per step
(``pseries(0, pseq([1, 2]))`` steps by 1, then 2, then ends) -- the
arguments sclang reads as streams.
"""

from ...base.stream import StopStream
from ..pattern import INF, as_stream, embed, pattern


def _counting(length):
    i = 0
    while length is INF or i < length:
        yield i
        i += 1


@pattern
def pconst(inval, value, length=INF):
    """``value``, ``length`` times (endless by default)."""
    for _ in _counting(length):
        inval = yield value
    return inval


@pattern
def pvalue(inval, value):
    """``value`` once -- every value of it, if it is a pattern: one item in
    a pattern's place."""
    return (yield from embed(value, inval))


@pattern
def pseries(inval, start=0.0, step=1.0, length=INF):
    """The arithmetic series ``start, start + step, ...`` (``length`` values);
    ``step`` may be a pattern."""
    steps = as_stream(step)
    value = start
    try:
        for _ in _counting(length):
            increment = steps.next(inval)
            inval = yield value
            value += increment
    except StopStream:
        pass
    return inval


@pattern
def pgeom(inval, start=1.0, grow=2.0, length=INF):
    """The geometric series ``start, start * grow, ...`` (``length`` values);
    ``grow`` may be a pattern."""
    grows = as_stream(grow)
    value = start
    try:
        for _ in _counting(length):
            factor = grows.next(inval)
            inval = yield value
            value *= factor
    except StopStream:
        pass
    return inval
