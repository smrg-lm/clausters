"""Patterns that read a list.

An item may itself be a pattern, which is **embedded**: all its values come out
in its place before the next item. A list pattern whose every item is an event
pattern is an event pattern, and plays (``pseq([pbind(...), pbind(...)])`` is a
phrase of phrases); otherwise it is a value pattern. The random ones draw from
the random context (see `clausters.seq.patterns.randompatterns`).
"""

from ...base import builtins, rand
from ...base.stream import StopStream
from ..pattern import INF, all_events, as_stream, embed, pattern
from .valuepatterns import _counting


def _over_events(items=(), *args, **kwargs):
    return all_events(items)


@pattern(events=_over_events)
def pseq(inval, items=(), repeats=1, offset=0):
    """The items in order, ``repeats`` times, starting at ``offset``."""
    n = len(items)
    for _ in _counting(repeats):
        for i in range(n):
            inval = yield from embed(items[(i + offset) % n], inval)
    return inval


@pattern(events=_over_events)
def pser(inval, items=(), length=1, offset=0):
    """The items in order, cycling from ``offset``, until exactly ``length``
    items have come out."""
    for i in _counting(length):
        inval = yield from embed(items[(i + offset) % len(items)], inval)
    return inval


@pattern(events=_over_events)
def prand(inval, items=(), length=INF):
    """A random item, ``length`` times."""
    for _ in _counting(length):
        inval = yield from embed(rand.choose(items), inval)
    return inval


@pattern(events=_over_events)
def pxrand(inval, items=(), length=INF):
    """A random item, ``length`` times, never the same one twice in a row."""
    n = len(items)
    index = rand.irand(n)
    for _ in _counting(length):
        index = (index + rand.irand(n - 1) + 1) % n
        inval = yield from embed(items[index], inval)
    return inval


@pattern(events=_over_events)
def pwrand(inval, items=(), weights=(), length=INF):
    """A random item, ``length`` times, chosen with the relative ``weights``
    (they need not sum to 1); ``weights`` may be a pattern of weight lists."""
    weight_lists = as_stream(weights)
    try:
        for _ in _counting(length):
            inval = yield from embed(
                rand.wchoose(items, weight_lists.next(inval)), inval)
    except StopStream:
        pass
    return inval


@pattern(events=_over_events)
def pshuffle(inval, items=(), repeats=1):
    """The items in one random order, the same order ``repeats`` times."""
    order = rand.scramble(items)
    for _ in _counting(repeats):
        for item in order:
            inval = yield from embed(item, inval)
    return inval


@pattern(events=_over_events)
def pslide(inval, items=(), repeats=1, length=3, step=1, start=0, wrap=True):
    """``repeats`` segments of ``length`` items, each starting ``step`` items
    after the last, from ``start``: a window sliding along the list. Past the
    end it wraps, or with ``wrap`` false it stops."""
    lengths, steps = as_stream(length), as_stream(step)
    n = len(items)
    pos = start
    try:
        for _ in _counting(repeats):
            for j in range(int(lengths.next(inval))):
                if wrap:
                    item = items[(pos + j) % n]
                elif 0 <= pos + j < n:
                    item = items[pos + j]
                else:
                    return inval
                inval = yield from embed(item, inval)
            pos += int(steps.next(inval))
    except StopStream:
        pass
    return inval


@pattern(events=_over_events)
def pwalk(inval, items=(), steps=None, directions=1, start=0):
    """A walk over the list: from ``start``, each item moves the position by
    the next of ``steps`` (by default one either way, at random), times the
    direction. At an end the next of ``directions`` sets the direction:
    ``1`` keeps it and wraps around, and ``pseq([1, -1], INF)`` bounces."""
    if steps is None:
        steps = prand([-1, 1])
    step_stream, direction_stream = as_stream(steps), as_stream(directions)
    n = len(items)
    index = start
    try:
        direction = direction_stream.next(inval)
        while True:
            step = step_stream.next(inval)
            inval = yield from embed(items[int(index)], inval)
            step *= direction
            if not 0 <= index + step < n:
                direction = direction_stream.next(inval)
                step = abs(step) * builtins.sign(direction)
            index = (index + step) % n
    except StopStream:
        pass
    return inval


@pattern
def place(inval, items=(), repeats=1, offset=0):
    """The items in order, ``repeats`` times, where an item that is a list
    gives one of its elements per pass: ``place([1, [2, 3]], 4)`` is
    ``1 2 1 3 1 2 1 3`` -- lists laced into a line."""
    n = len(items)
    for j in _counting(repeats):
        for i in range(n):
            item = items[(i + offset) % n]
            if isinstance(item, (list, tuple)):
                item = item[j % len(item)]
            inval = yield from embed(item, inval)
    return inval


@pattern(events=_over_events)
def placep(inval, items=(), repeats=1, offset=0):
    """The items' values interlaced: one value of each pattern in turn,
    ``repeats`` rounds; an item that has ended is skipped, and the pattern
    ends when every one has."""
    streams = [as_stream(item) for item in items]
    n = len(streams)
    ended = 0
    index = repeat = 0
    while repeat < repeats and ended < n:
        try:
            value = streams[(offset + index) % n].next(inval)
            ended = 0
            inval = yield from embed(value, inval)
        except StopStream:
            ended += 1
        index += 1
        if index == n:
            index = 0
            repeat += 1
    return inval


@pattern
def ptuple(inval, items=(), repeats=1):
    """A list per step of the next value of each item, until any ends;
    ``repeats`` times -- the items played side by side as one value."""
    for _ in _counting(repeats):
        streams = [as_stream(item) for item in items]
        try:
            while True:
                inval = yield [s.next(inval) for s in streams]
        except StopStream:
            pass
    return inval


@pattern(events=_over_events)
def pswitch(inval, items=(), which=0):
    """The item each value of ``which`` indexes, embedded whole (wrapping
    past the end)."""
    indices = as_stream(which)
    n = len(items)
    try:
        while True:
            inval = yield from embed(items[int(indices.next(inval)) % n], inval)
    except StopStream:
        pass
    return inval


@pattern
def pswitch1(inval, items=(), which=0):
    """One value of the item each value of ``which`` indexes: the items are
    walked side by side, and ``which`` says which one speaks."""
    streams = [as_stream(item) for item in items]
    indices = as_stream(which)
    n = len(streams)
    try:
        while True:
            inval = yield streams[int(indices.next(inval)) % n].next(inval)
    except StopStream:
        pass
    return inval


@pattern
def pfsm(inval, states=(), repeats=1):
    """A finite state machine: ``states`` is the list of entry states, then
    for each state its item and the states it may go to next,
    ``[entry, item0, next0, item1, next1, ...]``; each step goes to one of the
    next states at random and embeds its item. A state whose item is ``None``
    ends a pass."""
    last = (len(states) - 1) // 2 - 1
    for _ in _counting(repeats):
        index = 0
        while True:
            index = builtins.clip(rand.choose(states[index]), 0, last)
            index = int(index) * 2 + 2
            item = states[index - 1]
            if item is None:
                break
            inval = yield from embed(item, inval)
    return inval


@pattern
def pdfsm(inval, states=(), start_state=0, repeats=1):
    """A deterministic finite state machine: ``states[0]`` is a pattern of
    signals, and each state after it maps a signal (or ``"default"``) to the
    next state and what to embed, ``{signal: (next_state, item)}``. It ends
    when the signals do or a next state is ``None`` or past the last."""
    count = len(states) - 1
    for _ in _counting(repeats):
        current = start_state
        signals = as_stream(states[0])
        try:
            while True:
                signal = signals.next(inval)
                state = states[current + 1]
                current, item = state[signal] if signal in state else state["default"]
                if current is None or current >= count:
                    break
                inval = yield from embed(item, inval)
        except StopStream:
            pass
    return inval
