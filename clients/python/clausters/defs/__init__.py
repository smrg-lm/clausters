"""Definitions and server resources (port of ``sc3/synth``).

The definition layer — both def formats, FaustDefs and UGen-graph SynthDefs —
and the server resources:

- `signals` — lowercase callables mapping Faust's Signal
  API; compose them (operators or functions) into the JSON signal tree.
- `boxes` — the same pattern over Faust's Box API: point-free composition
  plus the `boxes.faust` escape hatch that turns any Faust expression
  (the stdlib included) into a composable `Box`.
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

from . import boxes
from . import signals
from . import ugens
from .asdef import as_def
from .boxes import Box
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
    buf_sample_rate,
    control,
    demand,
    dseq,
    env_gen,
    fft,
    ifft,
    impulse,
    in_,
    in_ctl,
    lag,
    local_in,
    local_out,
    mul_add,
    out,
    play_buf,
    poll,
    pv_brick_wall,
    pv_mag_above,
    pv_mag_below,
    rand,
    replace_out,
    sample_rate,
    send_reply,
    send_trig,
    sin_osc,
    sum3,
    sum4,
    var_lag,
    white_noise,
)

__all__ = [
    "boxes",
    "signals",
    "ugens",
    "as_def",
    "Box",
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
    "send_trig",
    "send_reply",
    "poll",
    "fft",
    "ifft",
    "pv_mag_above",
    "pv_mag_below",
    "pv_brick_wall",
    "play_buf",
    "buf_rd",
    "buf_sample_rate",
    "local_in",
    "local_out",
    "mul_add",
    "sum3",
    "sum4",
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
