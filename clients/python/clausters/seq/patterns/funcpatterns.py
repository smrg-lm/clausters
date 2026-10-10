"""Patterns whose values a function computes when the walk asks for them."""

from ..pattern import INF, pattern
from .valuepatterns import _counting


@pattern
def pfunc(inval, func, length=INF):
    """``func()`` for each value, ``length`` times: a value computed at the
    moment it is needed."""
    for _ in _counting(length):
        inval = yield func()
    return inval
