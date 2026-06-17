"""OSC destination interfaces (port of ``sc3/base/_oscinterface.py``).

The swap point that lets one clock + routine target real time **or** an NRT
score without changing the routine. Every interface speaks the same two calls —
``send_msg(target, addr, *args)`` and ``send_bundle(target, when, *messages)``
— and declares a ``time_mode`` so the clock knows what ``when`` means:

- ``'unix'``  — ``when`` is an absolute wall-clock instant (RT, sent now).
- ``'score'`` — ``when`` is seconds from the render start (NRT, accumulated).

A *message* is a tuple ``(addr, arg1, …)``. The timetag↔sample math lives in
the native core; this layer only encodes and routes.
"""

import socket

from . import _osclib


class OscInterface:
    time_mode = "unix"

    def send_msg(self, target, addr, *args):
        raise NotImplementedError(f"{type(self).__name__}.send_msg")

    def send_bundle(self, target, when, *messages):
        raise NotImplementedError(f"{type(self).__name__}.send_bundle")


class OscUDPInterface(OscInterface):
    """Real-time UDP: messages and bundles go out the socket immediately."""

    time_mode = "unix"

    def __init__(self):
        self._sock = None

    def start(self):
        self._sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        return self

    def stop(self):
        if self._sock is not None:
            self._sock.close()
            self._sock = None

    def _ensure(self):
        if self._sock is None:
            self.start()

    def send_msg(self, target, addr, *args):
        self._ensure()
        self._sock.sendto(_osclib.message(addr, *args), target)

    def send_bundle(self, target, when, *messages):
        self._ensure()
        packets = [_osclib.message(*m) for m in messages]
        self._sock.sendto(_osclib.bundle_at(when, *packets), target)


class OscTCPInterface(OscInterface):
    """Real-time TCP (length-prefixed). The Clausters server does not speak TCP
    yet, so this is a deliberate stub kept for interface symmetry."""

    time_mode = "unix"

    def __init__(self, *args, **kwargs):
        raise NotImplementedError(
            "TCP transport is not implemented in the server yet "
            "(clients/PLAN.md: OscTCPInterface stub)"
        )


class OscScore:
    """Accumulated NRT bundles, ordered by time, serialized to a binary score
    (`[i32 len][packet]…`) that the offline renderer consumes."""

    def __init__(self):
        self.bundles = []  # (time_seconds, packet_bytes)

    def add(self, time_seconds, packet_bytes):
        self.bundles.append((time_seconds, packet_bytes))

    def bytes(self) -> bytes:
        ordered = sorted(self.bundles, key=lambda b: b[0])
        return _osclib.score(*[pkt for _, pkt in ordered])


class OscNrtInterface(OscInterface):
    """Non-real-time: instead of sending, accumulate timetagged bundles into an
    :class:`OscScore` for an offline render."""

    time_mode = "score"

    def __init__(self):
        self.score = OscScore()

    def send_msg(self, target, addr, *args):
        self.send_bundle(target, 0.0, (addr, *args))

    def send_bundle(self, target, when, *messages):
        packets = [_osclib.message(*m) for m in messages]
        self.score.add(when, _osclib.score_bundle(when, *packets))

    def render(self, sample_rate: float = 48_000.0, channels: int = 2):
        """Renders the accumulated score through the embed transport (C1).

        Schedule a closing bundle (e.g. ``/n_free 0``) at the end so the render
        has a defined duration — scsynth semantics (its commands do not sound).
        """
        from .. import transport

        return transport.render(self.score.bytes(), sample_rate=sample_rate, channels=channels)
