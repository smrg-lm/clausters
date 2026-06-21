"""Sequencing layer (port of ``sc3/seq``): events, patterns, stream-patterns.

This layer ships:

- `event` — `Event` (a note plays a synth and
  schedules its release at the exact logical beat).
- `pattern` — `Pattern` and the value patterns
  (``Pseq``, ``Pser``, ``Prand``, ``Pwhite``, ``Pseries``, ``Pgeom``,
  ``Pfunc``, ``Pn``, ``Pconst``) plus `Pbind` (an event pattern).
- `eventstream` — `EventStreamPlayer`.

A ``Pbind(...).play(clock, server)`` runs live (RT) or builds an NRT score for
``server.render()`` purely by which interface the Server holds — the seam.
"""

from .event import Event, rest
from .eventstream import EventStreamPlayer
from .pattern import (
    INF,
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
    "Event",
    "rest",
    "EventStreamPlayer",
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
