"""Sequencing layer (port of ``sc3/seq``): events, patterns, stream-patterns.

This layer ships:

- `event` -- `Event` (a note plays a synth and
  schedules its release at the exact logical beat).
- `pattern` -- `Pattern` (the definition of a generator) and the value
  patterns (``Pseq``, ``Pser``, ``Prand``, ``Pwhite``, ``Pseries``, ``Pgeom``,
  ``Pfunc``, ``Pn``, ``Pconst``), plus `EventPattern`, what plays: `Pbind`, and
  a ``Pseq``/``Prand``/``Pn`` over event patterns only.
- `eventstream` -- `EventStreamPlayer`.
- `timeline` -- `Timeline` (a static, editable, random-access sequence) and
  its own transport (play/pause/stop/locate/loop) and its own tempo map, plus `OscItem` /
  `MidiItem`, which make events of a raw message, plus `item_data` / `item_from_data`, the one
  description of what an item is as plain data.
- `sequence` -- `EventSequence`: events as concrete data, each with an id, in
  beats with their tempo map -- what a notes editor edits and what a timeline
  renders into. A handle to the document's own structure.
- `curves` -- `CurveEmitter`: what sends the curves of the events a server
  plays, a stretch at a time.

A ``Pbind(...).play(clock, server)`` runs live (RT) or builds an NRT score for
``server.render()`` purely by which interface the Server holds -- the seam.
"""

from .curves import CurveEmitter
from .event import Event, rest
from .eventstream import EventStreamPlayer
from .timeline import (MidiItem, OscItem, Timeline, item_data,
                       item_from_data)
from .sequence import EventSequence, SeqAutomation, SeqEvent, SeqEvents
from .pattern import (
    INF,
    EventPattern,
    Pattern,
    Pbind,
    Pconst,
    Pfunc,
    Pgeom,
    Pn,
    Prand,
    Pseq,
    Pser,
    Pseries,
    Pwhite,
)

__all__ = [
    "CurveEmitter",
    "EventPattern",
    "Event",
    "rest",
    "EventStreamPlayer",
    "Timeline",
    "EventSequence",
    "SeqEvent",
    "SeqEvents",
    "SeqAutomation",
    "OscItem",
    "MidiItem",
    "item_data",
    "item_from_data",
    "Pattern",
    "Pbind",
    "Pconst",
    "Pfunc",
    "Pgeom",
    "Pn",
    "Prand",
    "Pseq",
    "Pser",
    "Pseries",
    "Pwhite",
    "INF",
]
