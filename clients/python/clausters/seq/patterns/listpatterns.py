"""Patterns that read a list.

An item may itself be a pattern, which is **embedded**: all its values come out
in its place before the next item. A list pattern whose every item is an event
pattern is an event pattern, and plays (``pseq([pbind(...), pbind(...)])`` is a
phrase of phrases); otherwise it is a value pattern.
"""

from ...base import rand
from ..pattern import INF, all_events, embed, pattern
from .valuepatterns import _counting


def _over_events(items=(), *args, **kwargs):
    return all_events(items)


@pattern(events=_over_events)
def pseq(inval, items=(), repeats=1):
    """The items in order, ``repeats`` times."""
    for _ in _counting(repeats):
        for item in items:
            inval = yield from embed(item, inval)
    return inval


@pattern
def pser(inval, items, length):
    """The items in order, cycling, until exactly ``length`` values have come
    out (an item is not embedded: each counts as one value)."""
    for i in range(int(length)):
        inval = yield items[i % len(items)]
    return inval


@pattern(events=_over_events)
def prand(inval, items=(), length=INF):
    """A random item, ``length`` times, drawn from the random context (see
    `clausters.seq.patterns.randompatterns`)."""
    for _ in _counting(length):
        inval = yield from embed(rand.choose(items), inval)
    return inval
