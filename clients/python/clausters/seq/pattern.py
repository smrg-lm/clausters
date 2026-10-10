"""Patterns: what a pattern is, and how one is written (port of ``sc3/seq/pattern.py``).

A **pattern** is a reusable, lazy description of a sequence of values -- the
definition of a generator. Iterating it starts a fresh **stream**, which yields
the values: ``for x in pseq([1, 2, 3])`` loops over 1, 2, 3, and a second loop
starts again from 1. A pattern holds no state of its own; whatever a walk needs
to remember (a position, a random walk's value, a seed) lives in that walk.

**Value patterns and event patterns** are the two kinds, and they differ in
what their stream yields. A value pattern yields values -- numbers, lists,
anything a key takes. An event pattern yields `Event`s, and it is the only kind
that plays: `EventPattern.play` runs it on a clock and `render_events` renders
it into an `clausters.seq.EventSequence`. `pbind` is the event pattern that
builds events out of value patterns, one per key. A list pattern (`pseq`,
`prand`, `pn`) whose every element is an event pattern is one too; which kind a
pattern is, is decided when it is built.

**A pattern is written as a generator function** under `pattern`, as a UGen is
a function returning a graph node. The function receives the **incoming value**
first -- ``inval``, the event a `pbind` is building when the pattern is one of
its keys, else ``None`` -- and then its own arguments; each ``yield`` gives a
value and receives the next incoming one::

    @pattern
    def pcount(inval, n):
        "0, 1, ... n - 1."
        for i in range(n):
            inval = yield i
        return inval

A sub-pattern is embedded with ``inval = yield from embed(item, inval)``, which
passes the incoming values through and hands back the last one.

**A pattern composes with the operators** a UGen does, by the same names:
``pseq([1, 2, 3]) + 60`` is a pattern of 61, 62, 63, and ``.midicps()``,
``.min(x)`` or ``% 12`` compose the same way. Each value is computed by the
`clausters.base.builtins` function of that name, so a pattern, a list and a
signal answer an operator with one function. An operator applies to each value
a stream yields; a function over a whole list (``normalize``) is not an
operator, and is applied to the list before it becomes a pattern. An event
pattern yields events, which have no arithmetic, and refuses an operator.

The patterns themselves are in `clausters.seq.patterns`, by family.
"""

import functools
import math
from typing import Any, Callable, Generator, overload

from ..base.absobject import AbstractObject
from ..base.stream import Stream, StopStream

INF = math.inf


# ---- streams over a pattern ----

class PatternStream(Stream):
    """A walk over a pattern: one generator, started on the first `next`.

    `next` takes the incoming value (``inval``) and returns the pattern's next
    value, or raises `clausters.base.stream.StopStream` at its end. `reset`
    starts the walk again.
    """

    def __init__(self, pattern):
        self.pattern = pattern
        self._gen = None
        self._done = False

    def next(self, inval=None):
        if self._done:
            raise StopStream
        try:
            if self._gen is None:
                self._gen = self.pattern.embed(inval)
                return next(self._gen)
            return self._gen.send(inval)
        except StopIteration:
            self._done = True
            raise StopStream from None

    def reset(self):
        self._gen = None
        self._done = False


class _ConstStream(Stream):
    """A bare value as a stream: the value, forever."""

    def __init__(self, value):
        self.value = value

    def next(self, inval=None):
        return self.value


def embed(value, inval=None):
    """Yields ``value``, or every value of it if it is a pattern; returns the
    last incoming value. Inside a pattern: ``inval = yield from embed(x, inval)``."""
    if isinstance(value, Pattern):
        return (yield from value.embed(inval))
    return (yield value)


def as_stream(value) -> Stream:
    """A stream over ``value``: a fresh walk of a pattern, the stream itself,
    or a bare value repeated forever -- how an argument that may be either is
    read one value at a time."""
    if isinstance(value, Pattern):
        return value.stream()
    if isinstance(value, Stream):
        return value
    return _ConstStream(value)


def as_pattern(value) -> "Pattern":
    """``value`` if it is a pattern, else a `pconst` of it."""
    if isinstance(value, Pattern):
        return value
    from .patterns.valuepatterns import pconst

    return pconst(value)


# ---- the pattern ----

def _held(value):
    """An argument as a pattern keeps it: a list, a dict or any other iterable
    is copied when the pattern is built, so editing the list afterwards does
    not change the pattern, and a one-shot iterable (a generator) is read
    once rather than consumed by the first walk."""
    if isinstance(value, (Pattern, Stream, str, bytes)) or callable(value):
        return value
    if type(value) is dict:
        return dict(value)
    if hasattr(value, "__iter__") and not isinstance(value, dict):
        return list(value)
    return value


def _refuse_events(*operands):
    for operand in operands:
        if isinstance(operand, EventPattern):
            raise TypeError(
                f"{operand!r} is an event pattern: it yields events, which have "
                "no arithmetic -- an operator composes a value pattern")


class Pattern(AbstractObject):
    """The definition of a generator: iterating it starts a fresh walk that
    yields its values. It has no ``play`` -- what plays is an `EventPattern`;
    `clausters.render` of a value pattern generates its values.

    Made by a function written under `pattern`; ``Pattern(gfunc, args,
    kwargs)`` is what that builds."""

    def __init__(self, gfunc, args=(), kwargs=None, name=None):
        self._gfunc = gfunc
        self._args = tuple(args)
        self._kwargs = dict(kwargs or {})
        self._name = name or getattr(gfunc, "__name__", type(self).__name__)

    def embed(self, inval=None):
        """The generator that walks this pattern, fed ``inval`` first."""
        return self._gfunc(inval, *self._args, **self._kwargs)

    def __iter__(self):
        return self.stream()

    def stream(self) -> PatternStream:
        """A fresh `PatternStream` over this pattern."""
        return PatternStream(self)

    def trace(self, prefix="", keys=None) -> "Pattern":
        """This pattern with each value printed as it passes (`ptrace`)."""
        from .patterns.filterpatterns import ptrace

        return ptrace(self, prefix, keys)

    def __repr__(self):
        parts = [repr(a) for a in self._args]
        parts += [f"{k}={v!r}" for k, v in self._kwargs.items()]
        return f"{self._name}({', '.join(parts)})"

    # ---- AbstractObject: an operator composes a pattern ----

    def _compose_unop(self, selector):
        _refuse_events(self)
        from .patterns.oppatterns import punop

        return punop(selector, self)

    def _compose_binop(self, selector, other):
        _refuse_events(self, other)
        from .patterns.oppatterns import pbinop

        return pbinop(selector, self, other)

    def _rcompose_binop(self, selector, other):
        _refuse_events(self, other)
        from .patterns.oppatterns import pbinop

        return pbinop(selector, other, self)

    def _compose_narop(self, selector, *args, **static):
        _refuse_events(self, *args)
        from .patterns.oppatterns import pnarop

        return pnarop(selector, self, *args, **static)


class EventPattern(Pattern):
    """A pattern whose values are events, and so a pattern that plays.

    `pbind` is one, and so is a list pattern (`pseq`, `prand`, `pn`) whose every
    element is one: which kind a pattern is, is decided when it is built.
    """

    def render_events(self, until: "float | None" = None):
        """**Render the pattern into the events it plays**: a
        `clausters.seq.EventSequence` in the pattern's beats, each event as it
        played -- its derived keys written in, at the beat it fell on. The
        render goes one way (see `clausters.seq.Timeline.render_events`).

        Args:
            until: stop at this beat. Required for an endless pattern, which
                is otherwise refused after
                `clausters.render.MAX_BOUNCED_EVENTS` events.
        """
        from .sequence import render_pattern

        return render_pattern(self, until)

    def play(self, clock=None, server=None, quant=None):
        """Play this event pattern on ``clock``, sending to ``server``.

        Both are optional and resolve against the ambient context (the running
        session, else the default session): ``server=None`` takes the booted
        default server, and ``clock=None`` takes the running routine's clock or,
        outside one, the default session's clock -- created and started on first
        use. So ``pbind(...).play()`` sounds with only a ``Server().boot()`` and no
        `Session`."""
        from ..base.main import main
        from .eventstream import EventStreamPlayer

        server = main.resolve_server(server)
        if clock is None:
            clock = main.play_clock()
        return EventStreamPlayer(self, server).play(clock, quant)


_Gfunc = Callable[..., Generator[Any, Any, Any]]
_Build = Callable[..., Pattern]


@overload
def pattern(gfunc: _Gfunc, *, events: "bool | Callable[..., bool]" = False) -> _Build: ...
@overload
def pattern(gfunc: None = None, *,
            events: "bool | Callable[..., bool]" = False) -> Callable[[_Gfunc], _Build]: ...


def pattern(gfunc=None, *, events=False):
    """**Make a pattern out of a generator function**: the decorated name
    becomes a function that builds the pattern, as ``sine`` builds a UGen.

    The generator receives the incoming value first (``inval``) and then the
    arguments the pattern was built with; it yields values and receives the
    next incoming value from each ``yield``, and returns the last one. Every
    walk calls it afresh, so a pattern is reusable: building one stores the
    function and its arguments (a list or dict argument is copied), and starts
    nothing.

    Args:
        events: what kind of pattern it builds -- ``False`` a value pattern,
            ``True`` an `EventPattern`, or a function of the arguments that
            decides when the pattern is built (a list pattern is an event
            pattern when every element is one).

    ::

        @pattern
        def pcount(inval, n):
            "0, 1, ... n - 1."
            for i in range(n):
                inval = yield i
            return inval

        list(pcount(3))   # [0, 1, 2]
    """
    def decorate(gfunc):
        @functools.wraps(gfunc)
        def build(*args, **kwargs):
            args = tuple(_held(a) for a in args)
            kwargs = {k: _held(v) for k, v in kwargs.items()}
            is_events = events(*args, **kwargs) if callable(events) else events
            cls = EventPattern if is_events else Pattern
            return cls(gfunc, args, kwargs)

        build.generator = gfunc
        return build

    return decorate if gfunc is None else decorate(gfunc)


def all_events(items) -> bool:
    """Whether every one of ``items`` is an event pattern (and there is one):
    the rule a list pattern is resolved by."""
    return bool(items) and all(isinstance(item, EventPattern) for item in items)
