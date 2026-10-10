"""Event patterns: the patterns whose stream yields `clausters.seq.Event`s.

An event pattern is what plays (`clausters.seq.EventPattern.play`) and what
renders into an `clausters.seq.EventSequence`. Each value pattern given to one
is read once per event, and receives the event being built as its incoming
value.
"""

from ...base.stream import StopStream
from ..event import Event
from ..pattern import as_stream, pattern


def _bindings(args, patterns):
    return dict(*args, **patterns)


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
        event = dict(inval) if isinstance(inval, dict) else {}
        try:
            for key, stream in streams.items():
                event[key] = stream.next(event)
        except StopStream:
            return inval
        inval = yield Event(event)
