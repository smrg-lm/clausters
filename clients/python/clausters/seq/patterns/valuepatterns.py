"""Value patterns computed from their arguments: a constant, a series.

Each is a value pattern: its stream yields values, ``length`` of them (endless
by default).
"""

from ..pattern import INF, pattern


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
def pseries(inval, start=0.0, step=1.0, length=INF):
    """The arithmetic series ``start, start + step, ...`` (``length`` values)."""
    value = start
    for _ in _counting(length):
        inval = yield value
        value += step
    return inval


@pattern
def pgeom(inval, start=1.0, grow=2.0, length=INF):
    """The geometric series ``start, start * grow, ...`` (``length`` values)."""
    value = start
    for _ in _counting(length):
        inval = yield value
        value *= grow
    return inval
