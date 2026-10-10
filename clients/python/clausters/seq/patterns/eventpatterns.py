"""Event patterns: the patterns whose stream yields `clausters.seq.Event`s.

An event pattern is what plays (`clausters.seq.EventPattern.play`) and what
renders into an `clausters.seq.EventSequence`. Each value pattern given to one
is read once per event, and receives the event being built as its incoming
value, so a key can be computed from the keys before it (`pkey`).

**Articulation and inflection.** An articulation is how distinct notes relate
-- legato, detached, a tie, a slur; an inflection is a variation inside one
note -- its bend, its pressure, any control moving -- which is the note's own
expression (``Event.expression``). `pvoice` is the pattern of articulation:
its events are notes of one voice, and ``legato`` says how each goes on to
the next.
"""

import heapq

from ...base.stream import StopStream
from ..event import Event, rest
from ..pattern import INF, all_events, as_stream, embed, pattern
from .valuepatterns import _counting


def _bindings(args, patterns):
    return dict(*args, **patterns)


def _proto(inval):
    """The event a pattern builds on: a copy of the incoming one, else empty."""
    return dict(inval) if isinstance(inval, dict) else {}


@pattern(events=True)
def pbind(inval, *args, **patterns):
    """**Binds keys to value patterns**: an `Event` per step, each key's value
    the next of its pattern, ending when any key's pattern ends. A bare value is
    held for every event.

    Builds like a ``dict`` -- keyword arguments
    (``pbind(freq=pseq([440, 880]), dur=0.5)``), a mapping
    (``pbind({"freq": pseq([440, 880]), "dur": 0.5})``), or both merged, with
    keywords winning. The mapping form also admits keys that are not valid
    Python identifiers."""
    streams = {key: as_stream(value)
               for key, value in _bindings(args, patterns).items()}
    while True:
        event = _proto(inval)
        try:
            for key, stream in streams.items():
                event[key] = stream.next(event)
        except StopStream:
            return inval
        inval = yield Event(event)


@pattern
def pkey(inval, key, length=INF):
    """The value ``key`` has in the event being built -- a key of a `pbind`
    read by a later one: ``pbind(degree=..., octave=pkey("degree") // 7 + 4)``.
    It ends when the event lacks the key."""
    keys = as_stream(key)
    try:
        for _ in _counting(length):
            name = keys.next(inval)
            if not isinstance(inval, dict) or name not in inval:
                break
            inval = yield inval[name]
    except StopStream:
        pass
    return inval


@pattern(events=True)
def pchain(inval, *patterns):
    """**Event patterns chained**, right to left: each event of the last is the
    incoming event of the one before it, so a later pattern's keys are the
    defaults an earlier one overrides -- sclang's ``a <> b``. It ends when any
    of them does."""
    streams = [as_stream(p) for p in reversed(patterns)]
    try:
        while True:
            event = _proto(inval)
            for stream in streams:
                event = stream.next(event)
            inval = yield event if isinstance(event, Event) else Event(event)
    except StopStream:
        pass
    return inval


@pattern(events=True)
def pevent(inval, source, event=None):
    """``source`` with ``event`` as the incoming event of every step: the keys
    every event starts from."""
    stream = as_stream(source)
    proto = dict(event or {})
    try:
        while True:
            inval = yield stream.next(dict(proto))
    except StopStream:
        pass
    return inval


def _joins(note, following, artic):
    """Whether ``note`` goes on into ``following`` in a voice: never across a
    rest, always without ``artic``, and with it while the note sounds at least
    until the next starts (``legato >= 1``, sclang's ``sustain >= delta``)."""
    if following is None:
        return False
    if note.get("type", "note") != "note" or following.get("type", "note") != "note":
        return False
    return not artic or note.sustain() >= note.delta()


@pattern(events=True)
def pvoice(inval, instrument, *args, artic=False, voice=None, **patterns):
    """**A voice: notes that go on into one another** -- sclang's ``Pmono``
    and, with ``artic``, ``PmonoArtic``.

    It binds keys to patterns as `pbind` does, and every event is a note of
    ``instrument``. What it adds is the **articulation between consecutive
    notes**, which ``legato`` says: without ``artic`` every note goes on into
    the next, and with it a note joins the next while ``legato >= 1`` and
    the next one re-attacks when it is less. A rest always breaks the line.
    ``voice`` is written on every note (the notation key: which voice of the
    staff it is), so voices played together stay apart on a page.

    How a join sounds is the destination's, by one rule: on the server the
    voice holds **one node** and each joined note sets its controls (with a
    control's lag, a glide), and the note that ends a line releases it -- by
    its gate when the events say ``has_gate=True``, which an instrument with a
    gated envelope wants as it does under `pbind`, else by freeing the node,
    which cuts the sound; on MIDI, notes that overlap; rendered into an
    `clausters.seq.EventSequence`, a ``tie`` where a joined note keeps its
    pitch and a slur over each run that moves. What varies inside one note is
    an inflection, and is the note's own expression, not this."""
    streams = {key: as_stream(value)
               for key, value in _bindings(args, patterns).items()}

    def build(inval):
        event = _proto(inval)
        event["instrument"] = instrument
        if voice is not None:
            event["voice"] = voice
        for key, stream in streams.items():
            event[key] = stream.next(event)
        return Event(event)

    try:
        note = build(inval)
    except StopStream:
        return inval
    previous = None
    while True:
        # One note ahead, to know whether this one goes on into it.
        try:
            following = build(inval)
        except StopStream:
            following = None
        joined = _joins(note, following, artic)
        if previous is not None and previous.get("node") is not None:
            note["continues"] = previous["node"]
        if joined:
            note["held"] = True
        inval = yield note
        if following is None:
            return inval
        previous = note if joined else None
        note = following


@pattern(events=lambda items=(), *args, **kwargs: True)
def ppar(inval, items=(), repeats=1):
    """**Event patterns played together**: their events merged in time order
    into one stream, each event's ``delta`` set to the time to the next one of
    any of them, and a rest where one has ended and the others have not yet
    reached their next."""
    for _ in _counting(repeats):
        queue = [(0.0, i, as_stream(p)) for i, p in enumerate(items)]
        heapq.heapify(queue)
        order = len(queue)
        now = 0.0
        while queue:
            _, i, stream = heapq.heappop(queue)
            try:
                event = stream.next(inval)
            except StopStream:
                if queue and queue[0][0] > now:
                    inval = yield rest(queue[0][0] - now)
                    now = queue[0][0]
                continue
            if not isinstance(event, Event):
                event = Event(event)
            heapq.heappush(queue, (now + event.delta(), order, stream))
            order += 1
            following = queue[0][0]
            # In place: the event the source yielded is the one that plays,
            # so what the source keeps of it (a voice's node) is kept.
            event["delta"] = following - now
            inval = yield event
            now = following
    return inval
