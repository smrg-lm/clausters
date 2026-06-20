"""Definitions and server resources (port of ``sc3/synth``, Faust-first).

C3 ships the Faust-first definition layer and the server resources:

- :mod:`~clausters.defs.signals` — lowercase callables mapping Faust's Signal
  API; compose them (operators or functions) into the JSON signal tree.
- :mod:`~clausters.defs.faustdef` — :class:`FaustDef`: build the ``/d_faust``
  payload (signal tree, source, or box tree) and list its controls.
- :mod:`~clausters.defs.node` / :mod:`~clausters.defs.bus` /
  :mod:`~clausters.defs.buffer` — :class:`Synth`/:class:`Group`/:class:`Bus`/
  :class:`Buffer` and their client-side allocators.
- :mod:`~clausters.defs.server` — :class:`Server`: the live OSC round-trip
  (definitions, nodes, buses, buffers, ``/done``/``/fail``, ``/notify``).
- :mod:`~clausters.defs.ugens` / :mod:`~clausters.defs.synthdef` — the UGen
  graph (lowercase callables → :class:`Ugen`/:class:`Control`) and
  :class:`SynthDef` (``/d_recv``), the UGen-graph counterpart of the Faust
  :mod:`~clausters.defs.signals` / :class:`FaustDef` pair.
"""

from . import signals
from . import ugens
from .bus import AudioBusAllocator, Bus, ControlBusAllocator
from .clocksync import SampleClockModel, UdpSampleClock
from .buffer import Buffer, BufferAllocator
from .faustdef import FaustDef
from .graphdef import GraphDef
from .node import AddAction, Group, NodeIDAllocator, ROOT_NODE_ID, Synth
from .server import Server
from .signals import Signal
from .synthdef import SynthDef
from .ugens import (
    Control,
    Ugen,
    buf_rd,
    control,
    impulse,
    in_,
    in_ctl,
    local_in,
    local_out,
    out,
    play_buf,
    replace_out,
    sin_osc,
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
    "Bus",
    "AudioBusAllocator",
    "ControlBusAllocator",
    "Buffer",
    "BufferAllocator",
    "AddAction",
    "Group",
    "Synth",
    "NodeIDAllocator",
    "ROOT_NODE_ID",
    "Server",
    "UdpSampleClock",
    "SampleClockModel",
]
