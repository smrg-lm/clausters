"""Clausters Python client.

A high-level client for the Clausters audio server, ported selectively from
SuperCollider's class library (sc3), Faust-first.

What is in place now:

- `clausters.transport` — the low-level transports (embedded server, shared
  memory, offline render). Its public names are re-exported here, so existing
  code using ``from clausters import Clausters, ShmClient, render`` keeps
  working.
- `clausters._native` — the ctypes binding over the shared native core
  (``clausters-ffi``): builtins, seeded white noise and clock/sample math, all
  matching the server by construction.
- `clausters.base` — the server-agnostic base layer: builtins, absobject,
  stream, clock, netaddr, the OSC/MIDI destination interfaces and the OSC wire
  encoder.
- `clausters.seq` — the sequencing layer: events, value patterns and ``Pbind``,
  and the event-stream player.
- `clausters.defs` — the definition layer and server resources: the Faust-first
  ``signals``/`FaustDef` pair, the UGen-graph ``ugens``/`SynthDef` pair, the
  node/bus/buffer handles and the `Server`.
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
from .session import Session
from .transport import (
    ABI_VERSION,
    SEGMENT_SIZE,
    Clausters,
    ShmClient,
    render,
)

__all__ = [
    "ABI_VERSION",
    "SEGMENT_SIZE",
    "Clausters",
    "ShmClient",
    "Session",
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
