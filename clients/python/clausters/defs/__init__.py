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

SynthDef-based definitions (``synthdef``/``ugens``) come later.
"""

from . import signals
from .bus import AudioBusAllocator, Bus, ControlBusAllocator
from .clocksync import SampleClockModel, UdpSampleClock
from .buffer import Buffer, BufferAllocator
from .faustdef import FaustDef
from .node import AddAction, Group, NodeIDAllocator, ROOT_NODE_ID, Synth
from .server import Server
from .signals import Signal

__all__ = [
    "signals",
    "Signal",
    "FaustDef",
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
