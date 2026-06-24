"""Base layer (port of ``sc3/base``).

The base layer:

- `builtins` — numeric ops on scalars/lists, dispatched to
  the native core (f32, server-equivalent).
- `absobject` — `AbstractObject`, the operator
  overloading base.
- `stream` — `Stream`/`Routine` (the
  ``yield`` coroutine layer).
- `clock` — `TempoClock` (native-backed, RT + NRT
  drives).
- `main` — the global context (`main`).
- `netaddr` — `NetAddr`.
- `_oscinterface` / `_midiinterface`
  — the RT/NRT destination interfaces.
- `_osclib` — minimal OSC wire encoding.
"""

from .absobject import AbstractObject
from .clock import TempoClock
from .main import Main, main
from .netaddr import NetAddr
from .stream import FunctionStream, Routine, Stream, StopStream, YieldAndReset
from .timebase import MonotonicTimebase, SampleClockTimebase, Timebase
from ._midiinterface import (
    MidiNrtInterface,
    MidiReceiver,
    MidiRtInterface,
    MidiScore,
    MidiServer,
    parse_midi,
)
from ._oscinterface import (
    OscEmbedInterface,
    OscInterface,
    OscNrtInterface,
    OscReceiver,
    OscScore,
    OscTcpInterface,
    OscUdpInterface,
)

__all__ = [
    "AbstractObject",
    "TempoClock",
    "Timebase",
    "MonotonicTimebase",
    "SampleClockTimebase",
    "Main",
    "main",
    "NetAddr",
    "Stream",
    "Routine",
    "FunctionStream",
    "StopStream",
    "YieldAndReset",
    "OscInterface",
    "OscUdpInterface",
    "OscTcpInterface",
    "OscNrtInterface",
    "OscEmbedInterface",
    "OscReceiver",
    "OscScore",
    "MidiRtInterface",
    "MidiNrtInterface",
    "MidiReceiver",
    "MidiScore",
    "MidiServer",
    "parse_midi",
]
