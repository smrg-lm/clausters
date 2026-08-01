"""Clausters Python client.

A high-level client for the Clausters audio server, ported selectively from
SuperCollider's class library (sc3). It covers both of the server's def
formats as peers: FaustDefs and UGen-graph SynthDefs.

The layers:

- `clausters.ipc` — the low-level local transports (embedded server, shared
  memory, offline render). Its public names are re-exported here; the
  top-level ``render`` is now the dispatching verb (`clausters.render`),
  whose ``bytes`` branch is exactly the historical `clausters.ipc.render`,
  so ``from clausters import Clausters, ShmClient, render`` keeps working.
- `clausters._native` — the ctypes binding over the shared native core
  (``clausters-ffi``): builtins, seeded white noise and clock/sample math, all
  matching the server by construction.
- `clausters.base` — the server-agnostic base layer: builtins, absobject,
  stream, clock, netaddr, the OSC/MIDI destination interfaces and the OSC wire
  encoder.
- `clausters.seq` — the sequencing layer: events, value patterns and ``Pbind``,
  the event-stream player, and static timelines with a playhead.
- `clausters.defs` — the definition layer and server resources: the
  ``signals``/`FaustDef` pair, the UGen-graph ``ugens``/`SynthDef` pair, the
  node/bus/buffer handles and the `Server`. Its core names — `Server`,
  `Synth`, `Group`, `AddAction`, `SynthDef`, `FaustDef`, `Bus`, `Buffer` —
  are re-exported here; the UGen and signal callables are not, and stay
  under `clausters.defs`.
- `clausters.form` — the **arrangement**: a recursive algebra of elements
  over the sequencing/def layers, for composing at any granularity.
- `clausters.responders` — `OscFunc`/`MidiFunc`, callbacks on incoming OSC
  replies and live MIDI.
- `clausters.gui` — GuiDef building for the Clausters GUI host.
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
- `clausters.launch` — launching and owning the server and GUI processes
  (`Session.live` / `Session.gui`, and `Server.boot` / `GuiHost.boot`, drive
  these under the hood).
- `clausters.config` — the shared TOML configuration, read-only.
"""

from . import _native
from .errors import (
    AbiMismatchError,
    ClaustersError,
    CommandError,
    CommandRingFull,
    LibraryError,
    LibraryFeatureError,
    LibraryNotFoundError,
    RenderError,
    ReplyTimeout,
    SegmentError,
    ServerError,
)
from .base.clock import TempoClock
from .base.main import default_session, main
from .base.rand import choice, next_below, next_f64, uniform
from .base.stream import Routine
from .responders import MidiFunc, OscFunc, midifunc, oscfunc
from .seq.event import Event, rest
from .defs import (
    AddAction,
    Buffer,
    Bus,
    FaustDef,
    Group,
    Server,
    Synth,
    SynthDef,
)
from .play import play
from .plot import PlotWindow, plot
from .render import render
from .scope import ScopeWindow, scope
from .session import Session
from .launch import GuiProcess, ServerProcess, default_shm_path
from .ipc import (
    ABI_VERSION,
    SEGMENT_SIZE,
    Clausters,
    ShmClient,
)

__all__ = [
    "ABI_VERSION",
    "SEGMENT_SIZE",
    "Clausters",
    "ShmClient",
    "Session",
    "Server",
    # the server's resources: nodes, definitions, buses, buffers
    "Synth",
    "Group",
    "AddAction",
    "SynthDef",
    "FaustDef",
    "Bus",
    "Buffer",
    "TempoClock",
    "Routine",
    "Event",
    "rest",
    "play",
    "plot",
    "PlotWindow",
    "scope",
    "ScopeWindow",
    "default_session",
    "ServerProcess",
    "GuiProcess",
    "default_shm_path",
    "main",
    # the random context (one seedable source: main.seed(n) + these draws)
    "next_f64",
    "uniform",
    "next_below",
    "choice",
    "OscFunc",
    "MidiFunc",
    "oscfunc",
    "midifunc",
    "render",
    "_native",
    # error types
    "ClaustersError",
    "LibraryError",
    "LibraryNotFoundError",
    "LibraryFeatureError",
    "AbiMismatchError",
    "RenderError",
    "ServerError",
    "CommandError",
    "SegmentError",
    "CommandRingFull",
    "ReplyTimeout",
]
