"""Definitions and server resources (port of ``sc3/synth``, Faust-first).

The Faust-first definition layer and the server resources:

- `signals` — lowercase callables mapping Faust's Signal
  API; compose them (operators or functions) into the JSON signal tree.
- `faustdef` — `FaustDef`: build the ``/d_faust``
  payload (signal tree, source, or box tree) and list its controls.
- `node` / `bus` /
  `buffer` — `Synth`/`Group`/`Bus`/
  `Buffer` and their client-side allocators.
- `server` — `Server`: the live OSC round-trip
  (definitions, nodes, buses, buffers, ``/done``/``/fail``, ``/notify``).
- `ugens` / `synthdef` — the UGen
  graph (lowercase callables → `Ugen`/`Control`) and
  `SynthDef` (``/d_recv``), the UGen-graph counterpart of the Faust
  `signals` / `FaustDef` pair.
"""

from . import signals
from . import ugens
from .bus import AudioBusAllocator, Bus, ControlBusAllocator
from .clocksync import SampleClockModel, UdpSampleClock
from .buffer import Buffer, BufferAllocator
from .faustdef import FaustDef
from .graphdef import GraphDef
from .node import AddAction, Group, NodeIdAllocator, ROOT_NODE_ID, Synth
from .server import Server, ServerInfo, ServerOptions
from .signals import Signal
from .synthdef import SynthDef
from .ugens import (
    Control,
    DoneAction,
    Env,
    Ugen,
    buf_rd,
    control,
    demand,
    dseq,
    env_gen,
    impulse,
    in_,
    in_ctl,
    lag,
    local_in,
    local_out,
    out,
    play_buf,
    rand,
    replace_out,
    sample_rate,
    sin_osc,
    var_lag,
    white_noise,
)

__all__ = [
    "signals",
    "ugens",
    "Signal",
    "FaustDef",
    "SynthDef",
    "GraphDef",
    "Ugen",
    "Control",
    "control",
    "sin_osc",
    "impulse",
    "white_noise",
    "in_",
    "in_ctl",
    "out",
    "replace_out",
    "play_buf",
    "buf_rd",
    "local_in",
    "local_out",
    "lag",
    "var_lag",
    "sample_rate",
    "rand",
    "dseq",
    "demand",
    "env_gen",
    "Env",
    "DoneAction",
    "Bus",
    "AudioBusAllocator",
    "ControlBusAllocator",
    "Buffer",
    "BufferAllocator",
    "AddAction",
    "Group",
    "Synth",
    "NodeIdAllocator",
    "ROOT_NODE_ID",
    "Server",
    "ServerOptions",
    "ServerInfo",
    "UdpSampleClock",
    "SampleClockModel",
]
