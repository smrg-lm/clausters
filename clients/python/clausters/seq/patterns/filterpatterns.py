"""Patterns made of another pattern.

A filter pattern reads the pattern it filters and changes what comes out: how
often, how many, which, or what each value becomes. It is the kind of the
pattern it filters -- over an event pattern it is an event pattern, and plays
-- except the ones that make values of values (`pclump`, `pflatten`, `pdiff`,
`pwrap`, `pprorate`, `pavaroh`), and the ones that work on events' time
(`pdur`, `pdelta`), which are event patterns by what they do.
"""

import copy
import inspect

from ...base import builtins
from ...base.stream import StopStream
from ..event import Event, rest
from ..pattern import INF, EventPattern, as_stream, embed, pattern
from .valuepatterns import _counting


def _over_event(source=None, *args, **kwargs):
    return isinstance(source, EventPattern)


def _over_second(first=None, source=None, *args, **kwargs):
    return isinstance(source, EventPattern)


def _arity(func):
    try:
        return len(inspect.signature(func).parameters)
    except (TypeError, ValueError):
        return 1


def _call(func, value, inval):
    """``func(value)``, or ``func(value, inval)`` when it takes the incoming
    value too -- the two forms sclang's function filters accept."""
    return func(value, inval) if _arity(func) >= 2 else func(value)


@pattern(events=_over_event)
def pn(inval, source=None, repeats=INF, key=None):
    """``source`` from start to end, ``repeats`` times. With a ``key``, the
    event each repeat starts on is marked with it as ``True`` -- in place, so
    as a key of a `pbind` it marks the event being built, which a `pgate`
    later in the same `pbind` reads to take a new value -- and the event after
    the last as ``False``."""
    for _ in _counting(repeats):
        if key is not None:
            if not isinstance(inval, dict):
                inval = {}
            inval[key] = True
        inval = yield from embed(source, inval)
    if key is not None and isinstance(inval, dict):
        inval[key] = False
    return inval


@pattern(events=_over_event)
def pgate(inval, source=None, repeats=INF, key=None):
    """``source``'s values, a new one only when the incoming event's ``key`` is
    true and the last one held otherwise: a key of a `pbind` that changes when
    a `pn` with that ``key`` starts a repeat of the phrase."""
    for _ in _counting(repeats):
        stream = as_stream(source)
        held = None
        try:
            while True:
                if held is None or (isinstance(inval, dict) and inval.get(key)):
                    held = stream.next(inval)
                inval = yield copy.copy(held)
        except StopStream:
            pass
    return inval


@pattern(events=_over_second)
def pcollect(inval, func, source):
    """``func`` of each of ``source``'s values (and of the incoming value, when
    it takes two arguments)."""
    stream = as_stream(source)
    try:
        while True:
            inval = yield _call(func, stream.next(inval), inval)
    except StopStream:
        pass
    return inval


@pattern(events=_over_second)
def pselect(inval, func, source):
    """``source``'s values for which ``func`` is true."""
    stream = as_stream(source)
    try:
        while True:
            value = stream.next(inval)
            if _call(func, value, inval):
                inval = yield value
    except StopStream:
        pass
    return inval


@pattern(events=_over_second)
def preject(inval, func, source):
    """``source``'s values for which ``func`` is false."""
    stream = as_stream(source)
    try:
        while True:
            value = stream.next(inval)
            if not _call(func, value, inval):
                inval = yield value
    except StopStream:
        pass
    return inval


@pattern(events=_over_event)
def pdrop(inval, source, n):
    """``source`` without its first ``n`` values."""
    stream = as_stream(source)
    try:
        for _ in range(int(n)):
            stream.next(inval)
        while True:
            inval = yield stream.next(inval)
    except StopStream:
        pass
    return inval


@pattern(events=_over_event)
def plen(inval, source, n):
    """The first ``n`` values of ``source`` (sclang's ``Pfin``)."""
    stream = as_stream(source)
    try:
        for _ in range(int(n)):
            inval = yield stream.next(inval)
    except StopStream:
        pass
    return inval


@pattern(events=_over_event)
def pstutter(inval, source, n):
    """Each of ``source``'s values ``n`` times in a row; ``n`` may be a
    pattern, read once per value."""
    stream, counts = as_stream(source), as_stream(n)
    try:
        while True:
            value = stream.next(inval)
            for _ in range(abs(int(counts.next(inval)))):
                inval = yield copy.copy(value)
    except StopStream:
        pass
    return inval


@pattern(events=_over_event)
def platch(inval, source, trig=True):
    """``source``'s next value where ``trig`` is true and the last one held
    where it is false (sclang's ``Pclutch``): a latch a pattern of
    booleans opens."""
    stream, trigs = as_stream(source), as_stream(trig)
    held, have = None, False
    try:
        while True:
            if trigs.next(inval) or not have:
                held, have = stream.next(inval), True
                inval = yield held
            else:
                inval = yield copy.copy(held)
    except StopStream:
        pass
    return inval


@pattern(events=_over_second)
def pwhile(inval, func, source):
    """``source`` again and again while ``func()`` is true (``func(inval)``
    when it takes the incoming value)."""
    while (func(inval) if _arity(func) >= 1 else func()):
        inval = yield from embed(source, inval)
    return inval


@pattern
def pwrap(inval, source, lo, hi):
    """``source``'s values wrapped into ``lo``..``hi``; the bounds may be
    patterns."""
    stream, los, his = as_stream(source), as_stream(lo), as_stream(hi)
    try:
        while True:
            lo_, hi_ = los.next(inval), his.next(inval)
            inval = yield builtins.wrap(stream.next(inval), lo_, hi_)
    except StopStream:
        pass
    return inval


@pattern
def pclump(inval, source, n):
    """``source``'s values gathered in lists of ``n`` (the last one shorter if
    the source ends inside it); ``n`` may be a pattern."""
    stream, sizes = as_stream(source), as_stream(n)
    group = []
    try:
        while True:
            group = []
            for _ in range(int(sizes.next(inval))):
                group.append(stream.next(inval))
            inval = yield group
    except StopStream:
        if group:
            inval = yield group
    return inval


def _flatten(value, levels):
    if levels <= 0 or not isinstance(value, (list, tuple)):
        return [value]
    return [x for v in value for x in _flatten(v, levels - 1)]


@pattern
def pflatten(inval, source, n=1):
    """``source``'s values with the lists among them opened ``n`` levels deep
    and their elements yielded one by one."""
    stream, levels = as_stream(source), as_stream(n)
    try:
        while True:
            depth = int(levels.next(inval))
            for item in _flatten(stream.next(inval), depth):
                inval = yield item
    except StopStream:
        pass
    return inval


@pattern
def pdiff(inval, source):
    """The difference between each of ``source``'s values and the one before
    it: the steps of a line."""
    stream = as_stream(source)
    try:
        previous = stream.next(inval)
        while True:
            value = stream.next(inval)
            inval = yield value - previous
            previous = value
    except StopStream:
        pass
    return inval


@pattern
def pprorate(inval, source, proportion=1):
    """Each of ``source``'s values divided by ``proportion``: ``c`` gives the
    two parts ``c * value`` and ``(1 - c) * value``, a list gives a part per
    element -- a duration split into a rhythm."""
    stream, proportions = as_stream(source), as_stream(proportion)
    try:
        while True:
            value = stream.next(inval)
            c = proportions.next(inval)
            parts = c if isinstance(c, (list, tuple)) else (c, 1 - c)
            for part in parts:
                inval = yield part * value
    except StopStream:
        pass
    return inval


@pattern
def pavaroh(inval, source, aroh, avaroh, steps=12):
    """``source``'s scale degrees as keys, read with the scale ``aroh`` while
    the line rises and ``avaroh`` while it falls -- the ascending and
    descending forms of a raga."""
    stream, step_stream = as_stream(source), as_stream(steps)
    last = 0
    try:
        while True:
            octave_steps = step_stream.next(inval)
            degree = stream.next(inval)
            scale = aroh if degree >= last else avaroh
            last = degree
            inval = yield builtins.degkey(degree, scale, octave_steps)
    except StopStream:
        pass
    return inval


@pattern(events=True)
def pdelta(inval, time, source):
    """A rest of ``time`` beats, then ``source``: an event pattern started
    late."""
    if time > 0:
        inval = yield rest(time)
    return (yield from embed(source, inval))


@pattern(events=True)
def pdur(inval, dur, source, tolerance=0.001, quant=None):
    """``source``'s events until ``dur`` beats have passed, the last one cut to
    end exactly there (sclang's ``Pfindur``). With ``quant``, a source that
    ends early is padded with a rest to the next multiple of it."""
    stream = as_stream(source)
    elapsed = 0.0
    try:
        while True:
            event = stream.next(inval)
            if not isinstance(event, Event):
                event = Event(event)
            delta = event.delta()
            if builtins.roundup(elapsed + delta, tolerance) >= dur:
                last = Event(event)
                last["delta"] = dur - elapsed
                return (yield last)
            elapsed += delta
            inval = yield event
    except StopStream:
        if quant is not None:
            pad = builtins.roundup(elapsed, quant) - elapsed
            if pad > 0:
                inval = yield rest(pad)
    return inval


@pattern(events=_over_event)
def ptrace(inval, source, prefix="", keys=None):
    """``source`` unchanged, each value printed as it passes, after
    ``prefix`` -- only ``keys`` of an event, when given."""
    stream = as_stream(source)
    try:
        while True:
            value = stream.next(inval)
            shown = ({k: value.get(k) for k in keys}
                     if keys and isinstance(value, dict) else value)
            print(f"{prefix}{shown}")
            inval = yield value
    except StopStream:
        pass
    return inval
