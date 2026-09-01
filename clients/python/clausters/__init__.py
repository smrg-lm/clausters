"""Clausters Python client.

A high-level client for the Clausters audio server, ported selectively from
SuperCollider's class library (sc3). It covers both of the server's def
formats as peers: FaustDefs and UGen-graph SynthDefs.

What the top level holds is what you name while writing a piece: the free
verbs (``play``, ``render``, ``plot``, ``scope``), the three hosts (`Session`,
`Server`, `GuiHost`), the server's resources, the three def formats, the
timing types, the handful of grid and unit conversions beside them (``bar``,
``beat_in_bar``, ``quant_delay``, ``secs_to_samples``, ``samples_to_secs``),
and the layer modules themselves. Everything enumerative — the UGen and
signal callables, the value patterns, the GUI widgets, the sixty-odd numeric
builtins — is named through its module (``defs.sine``, ``seq.Pbind``,
``gui.knob``, ``builtins.midicps``): there are too many of them for a flat
namespace to stay readable. ``builtins`` is one of those modules and is named
at the top level for that reason — it is `clausters.builtins`, which shadows
nothing, and not Python's own. The transports and the
process launchers are named through theirs (`clausters.ipc`,
`clausters.launch`), because you reach them as a return value or an argument
of the layer above, not by instantiating them.

The layers:

- `clausters.ipc` — the low-level local transports (embedded server, shared
  memory, offline render), reached through `Session.embedded` and
  ``Server.shm`` rather than built by hand. The top-level ``render`` is the
  dispatching verb (`clausters.render`), whose ``bytes`` branch is exactly
  `clausters.ipc.render`.
- `clausters._native` — the ctypes binding over the shared native core
  (``clausters-ffi``): builtins, seeded white noise and clock/sample math, all
  matching the server by construction.
- `clausters.base` — the server-agnostic base layer: builtins, absobject,
  stream, clock, netaddr, the OSC/MIDI destination interfaces and the OSC wire
  encoder.
- `clausters.seq` — the sequencing layer: events, value patterns and ``Pbind``,
  the event-stream player, and static timelines with a playhead. `Event`,
  `rest`, `Timeline` and `Playhead` are re-exported here; the ``P*`` patterns
  are not, and stay under `clausters.seq`.
- `clausters.defs` — the definition layer and server resources: the
  ``signals``/`FaustDef` pair, the UGen-graph ``ugens``/`SynthDef` pair, the
  node/bus/buffer handles and the `Server`. Its core names — `Server`,
  `ServerOptions`, `Synth`, `Group`, `AddAction`, `SynthDef`, `FaustDef`,
  `GraphDef`, `Bus`, `Buffer` — are re-exported here; the UGen and signal
  callables are not, and stay under `clausters.defs`.
- `clausters.data` — what the server keeps *sending*, because what is being
  watched changes faster than anything could ask: control buses
  (`clausters.data.BusStream`), an audio bus's samples
  (`clausters.data.TapStream`) and a take as it records
  (`clausters.data.RecordingStream`). The GUI host reads the same three paths
  itself; this is them opened to the script.
- `clausters.form` — the **arrangement**: a recursive algebra of elements
  over the sequencing/def layers, for composing at any granularity.
- `clausters.responders` — `OscFunc`/`MidiFunc`, callbacks on incoming OSC
  replies and live MIDI.
- `clausters.gui` — the `GuiHost` and GuiDef building for the Clausters GUI
  host. The host is re-exported here; the widget callables are not.
- `clausters.play` — the free-standing `play`, one verb for every playable,
  resolved against the ambient session.
- `clausters.render` — the free-standing `render`, one verb for the change
  of state to sound: scores, defs and bare expressions, arrangement
  elements, timelines, patterns and routines, bounced offline to samples or
  a WAV (or delegated to a live destination).
- `clausters.plot` / `clausters.scope` — the free-standing visual verbs: one
  window per call on the ambient GUI host, for a rendered signal (`plot`) or
  a live bus through the server's audio taps (`scope`).
- `clausters.session` — `Session`, an explicit isolated environment; and the
  default session (`default_session`) it falls back to.
- `clausters.launch` — launching and owning the server and GUI processes.
  You drive these through `Session.live` / `Session.gui` and `Server.boot` /
  ``GuiHost.boot``, which own the processes and read their choices back
  (``Server.shm`` is the segment ``shm="auto"`` picked).
- `clausters.config` — the shared TOML configuration, read-only.
"""

from . import _native
from . import base, data, defs, errors, form, gui, ipc, launch, seq
from .base import builtins
from .errors import ClaustersError
from .base.clock import TempoClock
from .base.time import (
    TempoMap,
    bar,
    beat_in_bar,
    quant_delay,
    samples_to_secs,
    secs_to_samples,
)
from .base.main import default_session, main
from .base.rand import Rng, choice, current_rng, seed, spawn_rng, uniform
from .base.stream import Routine
from .responders import MidiFunc, OscFunc, midifunc, oscfunc
from .seq.event import Event, rest
from .seq.timeline import Playhead, Timeline
from .defs import (
    AddAction,
    Buffer,
    Bus,
    FaustDef,
    GraphDef,
    Group,
    Server,
    ServerOptions,
    Synth,
    SynthDef,
)
from .gui import GuiHost
from .play import play
from .plot import plot
from .render import read_soundfile, render
from .scope import scope
from .session import Session

__all__ = [
    # the layers, for the names too many to spell out flat: the UGen and
    # signal callables (defs), the value patterns (seq), the widgets (gui) —
    # and the transports and process launchers (ipc, launch), which you reach
    # through Session and Server rather than by instantiating them.
    "base",
    "builtins",
    "data",
    "defs",
    "errors",
    "form",
    "gui",
    "ipc",
    "launch",
    "seq",
    # the hosts
    "Session",
    "Server",
    "ServerOptions",
    "GuiHost",
    "default_session",
    "main",
    # the server's resources: nodes, definitions, buses, buffers
    "Synth",
    "Group",
    "AddAction",
    "SynthDef",
    "FaustDef",
    "GraphDef",
    "Bus",
    "Buffer",
    # time
    "TempoClock",
    "TempoMap",
    "bar",
    "beat_in_bar",
    "quant_delay",
    "secs_to_samples",
    "samples_to_secs",
    "Routine",
    "Event",
    "rest",
    "Timeline",
    "Playhead",
    # the free verbs
    "play",
    "render",
    "plot",
    "scope",
    "read_soundfile",
    # incoming OSC and MIDI
    "OscFunc",
    "MidiFunc",
    "oscfunc",
    "midifunc",
    # the random context (one seedable source: `seed(n)` on the default
    # session, or `session.seed(n)` on a named one, and these draws from
    # whichever stream the context says; the raw ones, next_f64 and
    # next_below, stay in `base.rand`)
    "Rng",
    "seed",
    "current_rng",
    "spawn_rng",
    "uniform",
    "choice",
    # the root of every error this package raises; the leaves are in `errors`
    "ClaustersError",
]
