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
import time

from . import _osclib


class OscInterface:
    time_mode = "unix"

    def send_msg(self, target, addr, *args):
        raise NotImplementedError(f"{type(self).__name__}.send_msg")

    def send_bundle(self, target, when, *messages):
        raise NotImplementedError(f"{type(self).__name__}.send_bundle")

    def recv(self, timeout):
        """One reply packet, or ``None``. Only real-time interfaces reply; the
        default (NRT/one-way) has nothing to return."""
        return None

    def close(self):
        pass


class OscUDPInterface(OscInterface):
    """Real-time UDP: messages and bundles go out the socket immediately, and
    the server's replies come back to the bound socket (so the Server can do
    request/reply over the same interface)."""

    time_mode = "unix"

    def __init__(self):
        self._sock = None

    def start(self):
        self._sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        # Bind so the server's replies have somewhere to return.
        self._sock.bind(("127.0.0.1", 0))
        return self

    def stop(self):
        if self._sock is not None:
            self._sock.close()
            self._sock = None

    close = stop

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

    def recv(self, timeout):
        self._ensure()
        self._sock.settimeout(timeout)
        try:
            data, _ = self._sock.recvfrom(65536)
            return data
        except (TimeoutError, OSError):
            return None


class OscTCPInterface(OscInterface):
    """Real-time TCP, length-prefixed (client C8). Each OSC packet — message or
    bundle — goes out as a 4-byte big-endian length followed by the bytes, the
    same framing scsynth uses and the server's ``osc::tcp`` expects; replies
    arrive framed the same way over the one connection. A drop-in for
    :class:`OscUDPInterface` (the ``target`` argument is ignored: the connection
    already knows its peer). Start the server with ``--tcp``."""

    time_mode = "unix"

    def __init__(self, host: str = "127.0.0.1", port: int = 57110):
        self.host = host
        self.port = port
        self._sock = None
        self._buf = b""        # leftover bytes between framed reads

    def start(self):
        self._sock = socket.create_connection((self.host, self.port))
        self._sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        return self

    def stop(self):
        if self._sock is not None:
            self._sock.close()
            self._sock = None
        self._buf = b""

    close = stop

    def _ensure(self):
        if self._sock is None:
            self.start()

    @staticmethod
    def _frame(payload: bytes) -> bytes:
        return len(payload).to_bytes(4, "big") + payload

    def send_msg(self, target, addr, *args):
        self._ensure()
        self._sock.sendall(self._frame(_osclib.message(addr, *args)))

    def send_bundle(self, target, when, *messages):
        self._ensure()
        packets = [_osclib.message(*m) for m in messages]
        self._sock.sendall(self._frame(_osclib.bundle_at(when, *packets)))

    def _recv_into_buf(self, timeout) -> bool:
        """Reads one chunk into ``_buf`` within ``timeout``; False on
        timeout/close."""
        self._sock.settimeout(timeout)
        try:
            chunk = self._sock.recv(65536)
        except (TimeoutError, OSError):
            return False
        if not chunk:
            return False
        self._buf += chunk
        return True

    def recv(self, timeout):
        """One reply packet (the bytes inside a frame), or ``None``. Reassembles
        the 4-byte length prefix and payload across TCP segments."""
        self._ensure()
        deadline = time.monotonic() + timeout
        while True:
            if len(self._buf) >= 4:
                length = int.from_bytes(self._buf[:4], "big")
                if len(self._buf) >= 4 + length:
                    packet = self._buf[4:4 + length]
                    self._buf = self._buf[4 + length:]
                    return packet
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not self._recv_into_buf(remaining):
                return None


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
