"""Sequencing layer (port of ``sc3/seq``): events, patterns, stream-patterns.

This layer ships:

- `event` — `Event` (a note plays a synth and
  schedules its release at the exact logical beat).
- `pattern` — `Pattern` (the definition of a generator) and the value
  patterns (``Pseq``, ``Pser``, ``Prand``, ``Pwhite``, ``Pseries``, ``Pgeom``,
  ``Pfunc``, ``Pn``, ``Pconst``), plus `EventPattern`, what plays: `Pbind`, and
  a ``Pseq``/``Prand``/``Pn`` over event patterns only.
- `eventstream` — `EventStreamPlayer`.
- `timeline` — `Timeline` (a static, editable, random-access sequence) and
  its own transport (play/pause/stop/locate/loop) and its own tempo map, plus the `OscItem` /
  `MidiItem` raw-message items, plus `item_data` / `item_from_data`, the one
  description of what an item is as plain data.

A ``Pbind(...).play(clock, server)`` runs live (RT) or builds an NRT score for
``server.render()`` purely by which interface the Server holds — the seam.
"""

from .automation import Automation, add_automation_def
from .event import Event, rest
from .eventstream import EventStreamPlayer
from .timeline import (MidiItem, OscItem, Timeline, item_data,
                       item_from_data)
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
    "EventPattern",
    "Event",
    "rest",
    "Automation",
    "add_automation_def",
    "EventStreamPlayer",
    "Timeline",
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
