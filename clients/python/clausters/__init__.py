"""Clausters Python client.

A high-level client for the Clausters audio server, ported selectively from
SuperCollider's class library (sc3), Faust-first. It is built in milestones
(see ``clients/PLAN.md``); this is the C1 scaffold.

What is in place now:

- :mod:`clausters.transport` — the low-level transports (embedded server, shared
  memory, offline render). Its public names are re-exported here, so existing
  code using ``from clausters import Clausters, ShmClient, render`` keeps
  working.
- :mod:`clausters._native` — the ctypes binding over the shared native core
  (``clausters-ffi``): builtins, seeded white noise and clock/sample math, all
  matching the server by construction.
- :mod:`clausters.base` — base layer (currently the minimal OSC wire encoder
  ``base._osclib``); the rest (absobject, builtins, stream, clock, netaddr,
  OSC/MIDI interfaces) lands in C2.
- :mod:`clausters.seq`, :mod:`clausters.defs` — placeholders for the
  sequencing layer (C4) and the Faust/SynthDef definitions and server resources
  (C3).
"""

from . import _native
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
]
