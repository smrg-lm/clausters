"""Patterns whose values a function computes when the walk asks for them.

A function given to one is called as late as possible -- when the value is
needed -- and with the incoming value when it takes an argument (the event a
`pbind` is building, when the pattern is one of its keys).
"""

import inspect

from ...base.stream import StopStream
from ..pattern import INF, Pattern, as_stream, embed, pattern
from .valuepatterns import _counting


def _takes_inval(func):
    try:
        return len(inspect.signature(func).parameters) >= 1
    except (TypeError, ValueError):
        return False


def _call(func, inval):
    return func(inval) if _takes_inval(func) else func()


@pattern
def pfunc(inval, func, length=INF):
    """``func()`` for each value, ``length`` times: a value computed at the
    moment it is needed (``func(inval)`` when it takes the incoming value)."""
    for _ in _counting(length):
        inval = yield _call(func, inval)
    return inval


@pattern
def pfuncn(inval, func, repeats=1):
    """``func``'s value ``repeats`` times, each embedded -- a function that
    returns a pattern plays it (sclang's ``Pfuncn``, once by default)."""
    for _ in _counting(repeats):
        inval = yield from embed(_call(func, inval), inval)
    return inval


@pattern
def prout(inval, func):
    """A generator function as a pattern: its ``yield``s are the values, and
    each ``yield`` receives the next incoming value. ``func`` takes the first
    incoming value when it has a parameter -- a routine's body, walked as a
    pattern."""
    gen = func(inval) if _takes_inval(func) else func()
    try:
        value = next(gen)
        while True:
            inval = yield value
            value = gen.send(inval)
    except StopIteration as stop:
        return stop.value if stop.value is not None else inval


@pattern
def plazy(inval, func):
    """The pattern ``func`` returns, built when the walk starts and embedded:
    a pattern decided at the moment it is played."""
    return (yield from embed(_call(func, inval), inval))


@pattern
def pif(inval, condition, iftrue, iffalse=None):
    """The next value of ``iftrue`` where ``condition`` is true, of ``iffalse``
    where it is false; it ends when the condition does or the side it asks
    does."""
    tests = as_stream(condition)
    yes, no = as_stream(iftrue), as_stream(iffalse)
    try:
        while True:
            side = yes if tests.next(inval) else no
            inval = yield side.next(inval)
    except StopStream:
        pass
    return inval


@pattern
def pproduct(inval, func, patterns):
    """Every combination of the values of ``patterns``, the last one changing
    fastest -- nested loops, one per pattern -- each as ``func`` of the list of
    values (the list itself when ``func`` is ``None``)."""
    patterns = list(patterns)

    def level(i, values, inval):
        stream = as_stream(patterns[i])
        try:
            while True:
                values[i] = stream.next(inval)
                if i < len(patterns) - 1:
                    inval = yield from level(i + 1, values, inval)
                else:
                    inval = yield (list(values) if func is None else func(list(values)))
        except StopStream:
            pass
        return inval

    if not patterns:
        return inval
    return (yield from level(0, [None] * len(patterns), inval))
