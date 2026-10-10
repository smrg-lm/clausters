"""Patterns made of another pattern.

A filter pattern is the kind of the pattern it filters: over an event pattern
it is an event pattern, and plays.
"""

from ..pattern import INF, EventPattern, embed, pattern
from .valuepatterns import _counting


def _over_event(source=None, *args, **kwargs):
    return isinstance(source, EventPattern)


@pattern(events=_over_event)
def pn(inval, source=None, repeats=INF):
    """``source`` from start to end, ``repeats`` times."""
    for _ in _counting(repeats):
        inval = yield from embed(source, inval)
    return inval
