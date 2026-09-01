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
- `time` — the piece's beat<->second map (`TempoMap`) and the beat-grid and
  sample-axis conversions: the questions about time a clock is not needed to
  answer.
- `environment` — `Environment`, an isolated place to make sound (server +
  random context); the base of both the default session and `Session`.
- `main` — the default session (`main`), the ambient `Environment` and the
  process-wide execution registry.
- `rand` — the random context: one seedable source (``main.seed`` +
  per-routine derived generators) behind every random value in the library.
- `netaddr` — `NetAddr`, a target's host and port.
- `moment` — `Moment`, when something is happening (a clock and an exact
  beat on it).
- `destination` — `Destination` and `OscDestination`: where OSC goes, and how
  a `Moment` becomes wire time.
- `_oscinterface` / `_midiinterface`
  — the RT/NRT destination interfaces.
- `_osclib` — minimal OSC wire encoding.
"""

from .absobject import AbstractObject
from .clock import TempoClock
from .time import BEATS, EXPONENTIAL, LINEAR, SECONDS, STEP, TempoMap
from .destination import Destination, OscDestination
from .environment import Environment, RandomContext
from .ids import IdShare, WHOLE as WHOLE_SHARE, share_of
from .main import Main, main
from .moment import Moment
from .netaddr import NetAddr
from .rand import choice, current_rng, next_below, next_f64, spawn_rng, uniform
from .stream import FunctionStream, Routine, Stream, StopStream, YieldAndReset
from .timebase import ManualTimebase, MonotonicTimebase, SampleClockTimebase, Timebase
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
    OscWsInterface,
)

__all__ = [
    "BEATS",
    "SECONDS",
    "STEP",
    "LINEAR",
    "EXPONENTIAL",
    "AbstractObject",
    "TempoClock",
    "TempoMap",
    "Timebase",
    "MonotonicTimebase",
    "ManualTimebase",
    "SampleClockTimebase",
    "Environment",
    "RandomContext",
    "IdShare",
    "WHOLE_SHARE",
    "share_of",
    "Main",
    "main",
    "Moment",
    "NetAddr",
    "Destination",
    "OscDestination",
    "Stream",
    "Routine",
    "FunctionStream",
    "StopStream",
    "YieldAndReset",
    "OscInterface",
    "OscUdpInterface",
    "OscTcpInterface",
    "OscWsInterface",
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
