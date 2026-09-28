"""Event sequences: events as concrete data, held by the document.

An `EventSequence` is what a notes editor edits and what a timeline renders
into. Where a `clausters.seq.Timeline` holds **playables** -- events, patterns,
routines, other timelines, each code that runs when it plays -- a sequence
holds **events**, each with an identity of its own, in beats, with the tempo
map that times them. The step from one to the other is a render, and it goes
one way: a timeline's generators, its nesting and its tempo curve become the
events they produced, and nothing rebuilds the timeline from them.

The sequence itself lives on the Rust side (the document's ``EventSequence``);
this object is a handle to it, so every client edits the same structure and a
notes editor opened on it edits it in place, with no copy to write back.
"""

import json

from .. import _native
from .event import Event


class EventSequence:
    """A sequence of events in beats, each with an id.

    Built from ``(beat, event)`` pairs, like a `clausters.seq.Timeline`; an
    event is an `Event` or anything a dict of its keys. Iterating yields
    ``(beat, Event)`` pairs in beat order; `entries` adds each one's id, which is
    what the edits name an event by.

    Args:
        events: ``(beat, event)`` pairs.
        tempo_map: the `clausters.base.TempoMap` that times the beats, or
            ``None``.
    """

    def __init__(self, events=(), *, tempo_map=None):
        data = {"events": [{"at": float(beat), "data": _keys(event)}
                           for beat, event in events]}
        if tempo_map is not None:
            data["tempo_map"] = json.loads(tempo_map.dump())
        self._seq = _native.SequenceHandle(data)

    @classmethod
    def from_data(cls, data) -> "EventSequence":
        """The sequence `data` wrote (or a bare list of ``{"at", "data"}``)."""
        sequence = cls.__new__(cls)
        sequence._seq = _native.SequenceHandle(data)
        return sequence

    def data(self) -> dict:
        """The sequence as plain data: its events with their ids, its tempo map
        and its lanes -- what a session stores and `from_data` reads."""
        return self._seq.call("state")

    # ---- reading ----

    def __len__(self) -> int:
        return int(self._seq.call("len")["len"])

    def __iter__(self):
        for _id, beat, event in self.entries():
            yield beat, event

    def entries(self) -> list:
        """Every event as ``(id, beat, Event)``, in beat order."""
        return [(e["id"], float(e["at"]), Event(e.get("data") or {}))
                for e in self.data().get("events", [])]

    def get(self, id: int) -> tuple:
        """The event with this id as ``(beat, Event)``. `KeyError` when there is
        none."""
        event = self._seq.call("event", id=int(id))
        if event is None:
            raise KeyError(id)
        return float(event["at"]), Event(event.get("data") or {})

    def duration(self) -> float:
        """Where the last event stops sounding, in beats."""
        return float(self._seq.call("duration")["duration"])

    @property
    def tempo_map(self):
        """The `clausters.base.TempoMap` that times the beats, or ``None``.
        Setting one is an edit."""
        written = self.data().get("tempo_map")
        return None if written is None else _native.TempoMap.load(json.dumps(written))

    @tempo_map.setter
    def tempo_map(self, value):
        written = None if value is None else json.loads(value.dump())
        self.apply({"intent": "tempo", "tempo_map": written})

    # ---- editing ----

    def apply(self, intent: dict) -> dict:
        """Apply one edit in the sequence's vocabulary (``add``, ``remove``,
        ``move``, ``set``, ``keys``, ``setevents``, ``tempo``, ``restore``) and
        answer ``{"applied", "current"}`` -- ``current`` the edit that puts it
        back, read before this one landed -- with ``"id"`` for an add.
        `ValueError` when refused."""
        return self._seq.call("apply", intent=intent)

    def add(self, beat: float, event) -> int:
        """Add an event at ``beat``; its new id."""
        answer = self.apply({"intent": "add",
                             "event": {"at": float(beat), "data": _keys(event)}})
        return int(answer["id"])

    def remove(self, id: int) -> None:
        """Remove the event with this id."""
        self.apply({"intent": "remove", "id": int(id)})

    def move(self, id: int, beat: float) -> None:
        """Move the event with this id to ``beat``."""
        self.apply({"intent": "move", "id": int(id), "at": float(beat)})

    def set(self, id: int, key: str, value) -> None:
        """Write one key of an event, with its family's coherence: a moved
        ``midinote`` moves the ``freq`` and the ``degree`` the event holds."""
        self.apply({"intent": "set", "id": int(id), "key": key, "value": value})

    def __repr__(self):
        return f"EventSequence({len(self)} events)"


def _keys(event) -> dict:
    """An event's keys as the document stores them."""
    if isinstance(event, Event):
        return event.keys_data()
    return Event(event).keys_data() if isinstance(event, dict) else dict(event)
