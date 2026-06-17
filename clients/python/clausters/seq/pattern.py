"""Patterns (port of ``sc3/seq/pattern.py`` + ``patterns/``).

A :class:`Pattern` is a reusable, lazy description of a value sequence; iterating
it yields the values (a fresh stream each time). Value patterns (``Pseq``,
``Pwhite``, …) feed :class:`Pbind`, which combines per-key value patterns into a
stream of :class:`~clausters.seq.event.Event` objects. An event pattern is
played on a clock with :meth:`Pattern.play` (see
:class:`~clausters.seq.eventstream.EventStreamPlayer`).

Patterns are plain Python generators under the hood, so nesting and composition
are natural; a sub-pattern used as a value is embedded (iterated) in place.
"""

import math
import random as _random

from .event import Event

INF = math.inf


def _embed(value):
    """Yields a value, or iterates it if it is itself a pattern."""
    if isinstance(value, Pattern):
        yield from iter(value)
    else:
        yield value


def as_pattern(value):
    return value if isinstance(value, Pattern) else Pconst(value)


class Pattern:
    def __iter__(self):
        raise NotImplementedError(f"{type(self).__name__}.__iter__")

    def stream(self):
        """A :class:`~clausters.base.stream.Stream` over this pattern."""
        from ..base.stream import FunctionStream

        it = iter(self)
        return FunctionStream(lambda _=None: next(it))

    def play(self, clock, server, quant=None):
        """Play this (event) pattern on ``clock``, sending to ``server``."""
        from .eventstream import EventStreamPlayer

        return EventStreamPlayer(self, server).play(clock, quant)


# ---- value patterns ----

class Pconst(Pattern):
    """A constant value, ``length`` times (infinite by default)."""

    def __init__(self, value, length=INF):
        self.value = value
        self.length = length

    def __iter__(self):
        i = 0
        while self.length is INF or i < self.length:
            yield self.value
            i += 1


class Pseq(Pattern):
    """The items in order, ``repeats`` times (sub-patterns are embedded)."""

    def __init__(self, items, repeats=1):
        self.items = list(items)
        self.repeats = repeats

    def __iter__(self):
        i = 0
        while self.repeats is INF or i < self.repeats:
            for item in self.items:
                yield from _embed(item)
            i += 1


class Pser(Pattern):
    """The items in order, yielding exactly ``length`` values (cycling)."""

    def __init__(self, items, length):
        self.items = list(items)
        self.length = length

    def __iter__(self):
        for i in range(int(self.length)):
            yield self.items[i % len(self.items)]


class Prand(Pattern):
    """Random items, ``length`` values."""

    def __init__(self, items, length=INF, seed=None):
        self.items = list(items)
        self.length = length
        self.seed = seed

    def __iter__(self):
        rng = _random.Random(self.seed)
        i = 0
        while self.length is INF or i < self.length:
            yield from _embed(rng.choice(self.items))
            i += 1


class Pwhite(Pattern):
    """Uniform random numbers in ``[lo, hi]``, ``length`` values."""

    def __init__(self, lo=0.0, hi=1.0, length=INF, seed=None):
        self.lo, self.hi, self.length, self.seed = lo, hi, length, seed

    def __iter__(self):
        rng = _random.Random(self.seed)
        i = 0
        while self.length is INF or i < self.length:
            yield rng.uniform(self.lo, self.hi)
            i += 1


class Pseries(Pattern):
    """Arithmetic series ``start, start+step, …`` (``length`` values)."""

    def __init__(self, start=0.0, step=1.0, length=INF):
        self.start, self.step, self.length = start, step, length

    def __iter__(self):
        value = self.start
        i = 0
        while self.length is INF or i < self.length:
            yield value
            value += self.step
            i += 1


class Pgeom(Pattern):
    """Geometric series ``start, start*grow, …`` (``length`` values)."""

    def __init__(self, start=1.0, grow=2.0, length=INF):
        self.start, self.grow, self.length = start, grow, length

    def __iter__(self):
        value = self.start
        i = 0
        while self.length is INF or i < self.length:
            yield value
            value *= self.grow
            i += 1


class Pfunc(Pattern):
    """Calls ``func()`` for each value (``length`` values)."""

    def __init__(self, func, length=INF):
        self.func = func
        self.length = length

    def __iter__(self):
        i = 0
        while self.length is INF or i < self.length:
            yield self.func()
            i += 1


class Pn(Pattern):
    """Repeats ``pattern`` ``n`` times."""

    def __init__(self, pattern, n=INF):
        self.pattern = pattern
        self.n = n

    def __iter__(self):
        i = 0
        while self.n is INF or i < self.n:
            yield from _embed(self.pattern)
            i += 1


# ---- event pattern ----

class Pbind(Pattern):
    """Binds keys to value patterns; yields an :class:`Event` per step, stopping
    when any key's stream stops. Constant values are held; sub-patterns advance
    one value per event."""

    def __init__(self, **patterns):
        self.patterns = patterns

    def __iter__(self):
        streams = {key: iter(as_pattern(value)) for key, value in self.patterns.items()}
        while True:
            event = {}
            for key, stream in streams.items():
                try:
                    event[key] = next(stream)
                except StopIteration:
                    return
            yield Event(event)
