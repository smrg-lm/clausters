"""Base layer (port of ``sc3/base``).

C2 ships the base layer:

- :mod:`~clausters.base.builtins` — numeric ops on scalars/lists, dispatched to
  the native core (f32, server-equivalent).
- :mod:`~clausters.base.absobject` — :class:`AbstractObject`, the operator
  overloading base (the graph subclass lands in C3).
- :mod:`~clausters.base.stream` — :class:`Stream`/:class:`Routine` (the
  ``yield`` coroutine layer).
- :mod:`~clausters.base.clock` — :class:`TempoClock` (native-backed, RT + NRT
  drives).
- :mod:`~clausters.base.main` — the global context (:data:`main`).
- :mod:`~clausters.base.netaddr` — :class:`NetAddr`.
- :mod:`~clausters.base._oscinterface` / :mod:`~clausters.base._midiinterface`
  — the RT/NRT (and stubbed MIDI/TCP) destination interfaces.
- :mod:`~clausters.base._osclib` — minimal OSC wire encoding.
"""

from .absobject import AbstractObject
from .clock import TempoClock
from .main import Main, main
from .netaddr import NetAddr
from .stream import FunctionStream, Routine, Stream, StopStream, YieldAndReset
from ._midiinterface import MidiNrtInterface, MidiRtInterface, MidiScore
from ._oscinterface import (
    OscInterface,
    OscNrtInterface,
    OscScore,
    OscTCPInterface,
    OscUDPInterface,
)

__all__ = [
    "AbstractObject",
    "TempoClock",
    "Main",
    "main",
    "NetAddr",
    "Stream",
    "Routine",
    "FunctionStream",
    "StopStream",
    "YieldAndReset",
    "OscInterface",
    "OscUDPInterface",
    "OscTCPInterface",
    "OscNrtInterface",
    "OscScore",
    "MidiRtInterface",
    "MidiNrtInterface",
    "MidiScore",
]
