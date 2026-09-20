"""Patterns (port of ``sc3/seq/pattern.py`` + ``patterns/``).

A `Pattern` is a reusable, lazy description of a value sequence -- the
definition of a generator: iterating it yields the values (a fresh stream each
time), and it does not play. Value patterns (``Pseq``, ``Pwhite``, ...) feed
`Pbind`, which combines per-key value patterns into a stream of `Event`
objects. An `EventPattern` -- `Pbind`, and a list pattern whose every element
is an event pattern -- is what plays, on a clock, with `EventPattern.play` (see
`EventStreamPlayer`).

Patterns are plain Python generators under the hood, so nesting and composition
are natural; a sub-pattern used as a value is embedded (iterated) in place.
"""

import math

from ..base import rand
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


def _made(cls, base, event_cls, events):
    """An instance of ``event_cls`` when ``base`` itself is being built over
    event patterns only, else of ``cls``: how a list pattern resolves whether
    it is an `EventPattern` when it is made."""
    return object.__new__(event_cls if cls is base and events else cls)


def _all_events(items) -> bool:
    return bool(items) and all(isinstance(item, EventPattern) for item in items)


class Pattern:
    """The definition of a generator: iterating it yields its values. It has no
    `play` -- what plays is an `EventPattern`. `clausters.render` of a value
    pattern generates its values."""

    def __iter__(self):
        raise NotImplementedError(f"{type(self).__name__}.__iter__")

    def stream(self):
        """A `Stream` over this pattern."""
        from ..base.stream import FunctionStream

        it = iter(self)
        return FunctionStream(lambda _=None: next(it))


class EventPattern(Pattern):
    """A pattern whose values are events, and so a pattern that plays.

    `Pbind` is one, and so is a list pattern (`Pseq`, `Prand`, `Pn`) whose
    every element is one: it resolves its class when it is built.
    """

    def play(self, clock=None, server=None, quant=None):
        """Play this event pattern on ``clock``, sending to ``server``.

        Both are optional and resolve against the ambient context (the running
        session, else the default session): ``server=None`` takes the booted
        default server, and ``clock=None`` takes the running routine's clock or,
        outside one, the default session's clock -- created and started on first
        use. So ``Pbind(...).play()`` sounds with only a ``Server().boot()`` and no
        `Session`."""
        from ..base.main import main
        from .eventstream import EventStreamPlayer

        server = main.resolve_server(server)
        if clock is None:
            clock = main.resolve_clock() or main.get_default_clock()
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
    """The items in order, ``repeats`` times (sub-patterns are embedded). Over
    event patterns only, it is an `EventPattern`."""

    def __new__(cls, items=(), repeats=1):
        items = list(items)
        self = _made(cls, Pseq, _EventPseq, _all_events(items))
        self.items = items
        return self

    def __init__(self, items=(), repeats=1):
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
    """Random items, ``length`` values, drawn from the **random context** (the
    running routine's generator, or the root outside one -- see
    `clausters.base.rand`): ``main.seed(n)`` reproduces the choices along with
    everything else in the script. There is no per-pattern seed -- independent
    seeds would break whole-script consistency. Over event patterns only, it is
    an `EventPattern`."""

    def __new__(cls, items=(), length=INF):
        items = list(items)
        self = _made(cls, Prand, _EventPrand, _all_events(items))
        self.items = items
        return self

    def __init__(self, items=(), length=INF):
        self.length = length

    def __iter__(self):
        i = 0
        while self.length is INF or i < self.length:
            yield from _embed(rand.choice(self.items))
            i += 1


class Pwhite(Pattern):
    """Uniform random numbers in ``[lo, hi)``, ``length`` values, drawn from
    the **random context** (the running routine's generator, or the root
    outside one -- see `clausters.base.rand`): ``main.seed(n)`` reproduces the
    sequence along with everything else in the script. There is no per-pattern
    seed -- independent seeds would break whole-script consistency."""

    def __init__(self, lo=0.0, hi=1.0, length=INF):
        self.lo, self.hi, self.length = lo, hi, length

    def __iter__(self):
        i = 0
        while self.length is INF or i < self.length:
            yield rand.uniform(self.lo, self.hi)
            i += 1


class Pseries(Pattern):
    """Arithmetic series ``start, start+step, ...`` (``length`` values)."""

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
    """Geometric series ``start, start*grow, ...`` (``length`` values)."""

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
    """Repeats ``pattern`` ``n`` times. Over an event pattern, it is an
    `EventPattern`."""

    def __new__(cls, pattern=None, n=INF):
        return _made(cls, Pn, _EventPn, isinstance(pattern, EventPattern))

    def __init__(self, pattern=None, n=INF):
        self.pattern = pattern
        self.n = n

    def __iter__(self):
        i = 0
        while self.n is INF or i < self.n:
            yield from _embed(self.pattern)
            i += 1


# ---- event pattern ----

class Pbind(EventPattern):
    """Binds keys to value patterns; yields an `Event` per step, stopping
    when any key's stream stops. Constant values are held; sub-patterns advance
    one value per event.

    Constructs exactly like a ``dict`` -- keyword arguments
    (``Pbind(freq=Pseq([440, 880]), dur=0.5)``), a mapping
    (``Pbind({"freq": Pseq([440, 880]), "dur": 0.5})``), or both merged, with
    keywords winning. The dict form also admits keys that are not valid Python
    identifiers."""

    def __init__(self, *args, **patterns):
        self.patterns = dict(*args, **patterns)

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


# ---- list patterns over events ----
#
# What a list pattern is made as when every element is an event pattern: the
# same pattern, and playable. `Pseq([Pbind(...), Pbind(...)])` is one.

class _EventPseq(Pseq, EventPattern):
    pass


class _EventPrand(Prand, EventPattern):
    pass


class _EventPn(Pn, EventPattern):
    pass
