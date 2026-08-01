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
import threading
import time

from . import _osclib


class OscReceiver:
    """A UDP listener that demuxes incoming OSC to registered handlers — the
    **input** counterpart of the output interfaces above, and the transport
    under `clausters.responders.OscFunc`.

    It binds its own socket (an ephemeral port by default, or a fixed ``port``
    so external apps can target it), runs a background thread that decodes each
    datagram through `clausters.base._osclib.decode_packet` (bundles unwrapped),
    and calls every registered handler with ``(addr, args, time, src)``. Each
    handler self-filters (by address, args, …); the receiver itself stays a thin
    transport + demux, mirroring the server's single decode door.

    Dispatch threading:

    - With no ``clock``, handlers run **inline on the receiver thread** — keep
      them quick and non-blocking (the golden rule), and to *sequence* in
      response, schedule a routine on a clock (non-blocking) rather than looping
      here.
    - With a ``clock``, each matched handler is dispatched via
      ``clock.sched(0.0, …)`` so it runs on the clock thread with the running
      routine's logical time available. The same golden rule applies: a handler
      must not block the clock thread.
    """

    def __init__(self, port: int = 0, host: str = "127.0.0.1", clock=None):
        self._host = host
        self._port = port
        self.clock = clock
        self._sock = None
        self._thread = None
        self._running = False
        self._handlers = []
        self._lock = threading.Lock()

    @property
    def port(self) -> int:
        """The actually bound UDP port (resolves an ephemeral 0 once started)."""
        if self._sock is not None:
            return self._sock.getsockname()[1]
        return self._port

    def start(self):
        if self._running:
            return self
        self._sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self._sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self._sock.bind((self._host, self._port))
        self._sock.settimeout(0.1)
        self._running = True
        self._thread = threading.Thread(target=self._loop, name="OscReceiver", daemon=True)
        self._thread.start()
        return self

    def stop(self):
        self._running = False
        if self._thread is not None:
            self._thread.join(timeout=1.0)
            self._thread = None
        if self._sock is not None:
            self._sock.close()
            self._sock = None
        return self

    close = stop

    def send(self, target, addr, *args):
        """Send an OSC message out the receiver's own socket. Lets a responder
        reply on the port it listens on, and lets a client register ``/server_notify``
        from here so the server's pushes (e.g. ``/transport_query.reply``) come back to
        *this* socket and reach the responders. ``target`` is ``(host, port)``."""
        if self._sock is None:
            raise RuntimeError("OscReceiver.send before start()")
        self._sock.sendto(_osclib.message(addr, *args), target)

    def add(self, handler):
        """Register ``handler(addr, args, time, src)``; called for every
        decoded message. Returns ``handler`` so it can later be `remove`d."""
        with self._lock:
            self._handlers.append(handler)
        return handler

    def remove(self, handler):
        with self._lock:
            if handler in self._handlers:
                self._handlers.remove(handler)

    def _loop(self):
        while self._running:
            try:
                data, src = self._sock.recvfrom(65536)
            except (TimeoutError, OSError):
                continue
            if not data:
                continue
            try:
                messages = _osclib.decode_packet(data)
            except Exception:
                continue  # untrusted bytes: drop anything that won't decode
            for addr, args, when in messages:
                self._dispatch(addr, args, when, src)

    def _dispatch(self, addr, args, when, src):
        with self._lock:
            handlers = list(self._handlers)
        for handler in handlers:
            if self.clock is not None:
                self.clock.sched(0.0, lambda h=handler: h(addr, args, when, src))
            else:
                handler(addr, args, when, src)


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


#: The most bytes one UDP datagram can carry (the IPv4 ceiling minus headers).
#: A packet over this cannot be sent at all — the OS rejects it — so the UDP
#: interface refuses it early with an error naming the transport that can.
_UDP_MAX = 65507


class OscUdpInterface(OscInterface):
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
        self._sock.sendto(self._checked(_osclib.message(addr, *args), addr), target)

    def send_bundle(self, target, when, *messages):
        self._ensure()
        packets = [_osclib.message(*m) for m in messages]
        self._sock.sendto(self._checked(_osclib.bundle_at(when, *packets), "bundle"), target)

    @staticmethod
    def _checked(data: bytes, what) -> bytes:
        if len(data) > _UDP_MAX:
            raise ValueError(
                f"OSC packet ({what}) is {len(data)} bytes, over the "
                f"{_UDP_MAX}-byte UDP datagram cap; use the TCP transport "
                f"(the default) for payloads this large")
        return data

    def recv(self, timeout):
        self._ensure()
        self._sock.settimeout(timeout)
        try:
            data, _ = self._sock.recvfrom(65536)
            return data
        except (TimeoutError, OSError):
            return None


class OscTcpInterface(OscInterface):
    """Real-time TCP, length-prefixed. Each OSC packet — message or
    bundle — goes out as a 4-byte big-endian length followed by the bytes, the
    same framing scsynth uses and the server's ``osc::tcp`` expects; replies
    arrive framed the same way over the one connection. A drop-in for
    `OscUdpInterface` (the ``target`` argument is ignored: the connection
    already knows its peer). The server listens on TCP by default, at the same
    port as UDP (``--no-tcp`` disables it)."""

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


class OscWsInterface(OscInterface):
    """Real-time WebSocket. Each OSC packet — message or bundle — goes out as one
    WebSocket binary message and replies arrive the same way, the framing the
    server's ``osc::ws`` expects (the frame *is* the packet boundary, so there is
    no length prefix, unlike `OscTcpInterface`). A drop-in for the other
    interfaces (the ``target`` argument is ignored: the connection already knows
    its peer). Start the server with ``--ws`` (a ``ws``-feature build).

    The handshake and framing live in the **native core**
    (`clausters._native.WsClient`, ``tungstenite``) — the same WebSocket
    implementation the server uses, reached by ctypes like the shm/embed
    transports — so there is no second implementation to maintain here. This is
    the transport a browser can reach (it cannot open raw UDP or map shared
    memory); the browser itself uses the native ``WebSocket`` API
    (`examples/ws_ping.html`), not this class. ``wss://`` (TLS) is out of scope."""

    time_mode = "unix"

    def __init__(self, host: str = "127.0.0.1", port: int = 57120, path: str = "/"):
        self.host = host
        self.port = port
        self.path = path
        self._conn = None

    def start(self):
        from .._native import WsClient

        self._conn = WsClient(self.host, self.port, self.path)
        return self

    def stop(self):
        if self._conn is not None:
            self._conn.close()
            self._conn = None

    close = stop

    def _ensure(self):
        if self._conn is None:
            self.start()

    def send_msg(self, target, addr, *args):
        self._ensure()
        self._conn.send(_osclib.message(addr, *args))

    def send_bundle(self, target, when, *messages):
        self._ensure()
        packets = [_osclib.message(*m) for m in messages]
        self._conn.send(_osclib.bundle_at(when, *packets))

    def recv(self, timeout):
        """One reply packet (a binary message's bytes), or ``None`` on timeout."""
        self._ensure()
        return self._conn.recv(timeout)


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
    `OscScore` for an offline render."""

    time_mode = "score"

    def __init__(self):
        self.score = OscScore()

    def send_msg(self, target, addr, *args):
        self.send_bundle(target, 0.0, (addr, *args))

    def send_bundle(self, target, when, *messages):
        packets = [_osclib.message(*m) for m in messages]
        self.score.add(when, _osclib.score_bundle(when, *packets))

    def render(self, sample_rate: float = 48_000.0, channels: int = 2,
               workers: int = 0, path=None, seed: int | None = None,
               sample_format: str = "float"):
        """Renders the accumulated score, and reports what that did.

        This is where a score becomes audio — `clausters.defs.Server.render`
        and `clausters.Session.render` are the surfaces, this is the one
        implementation, because the score is this interface's own.

        Schedule a closing bundle (e.g. ``/node_free 0``) at the end so the render
        has a defined duration — scsynth semantics (its commands do not sound).
        ``workers`` adds DSP threads for the score's parallel groups.

        Returns a `clausters.render.RenderStats` either way. ``path`` chooses
        **where the audio goes, not whether there is a result**: without it the
        samples ride in ``stats.samples`` (through the embed transport); with
        it the server's own ``--nrt`` renderer writes the file and
        ``stats.samples`` is ``None``.

        ``seed`` starts the render's stochastic UGens; ``None`` draws a fresh
        one, so a score with noise in it is a new take every time. The seed
        used comes back in ``stats.seed`` — hand it back to replay that take.
        """
        from .. import ipc
        from ..render import RenderStats, render_to_file

        if path is not None:
            return render_to_file(self.score.bytes(), path, sample_rate,
                                  channels, workers, seed, sample_format)
        samples, frames, events, used = ipc.render(
            self.score.bytes(), sample_rate=sample_rate, channels=channels,
            workers=workers, seed=seed)
        peak, rms = ipc.channel_stats(samples, channels)
        return RenderStats(frames=frames, channels=channels,
                           sample_rate=sample_rate, events=events, peak=peak,
                           rms=rms, seed=used, samples=samples)


class OscEmbedInterface(OscInterface):
    """Real-time, in-process: the embedded server as just another OSC
    destination.

    It encodes exactly like `OscUdpInterface` — same wire bytes, same
    NTP-timetagged bundles — but instead of a socket it delivers each packet to
    an in-process `clausters.ipc.Clausters` server by function call, and reads
    replies by polling it. The embedded server decodes those bytes through the
    very same command path as the networked one, and (running in this process)
    shares the wall clock the timetags are written against, so the timing
    semantics match UDP exactly.

    The point is uniformity: a `Server` / `Session` driven through this
    interface behaves identically to one over UDP/TCP — the same routines,
    patterns and defs — because the only thing that changed is the transport.
    ``target`` is ignored, like `OscTcpInterface` (the handle already knows its
    server).

    Pass an existing `Clausters` handle to share one embedded server across
    interfaces, or leave ``server=None`` to open (and own) a fresh one;
    `close` shuts down only a handle this interface created.
    """

    time_mode = "unix"

    def __init__(self, server=None, *, workers: int = 0, lib_path: str | None = None):
        from .. import ipc

        #: the embedded `clausters.ipc.Clausters` server this interface drives;
        #: reach its sample clock / control buses through ``interface.server``.
        self.server = server if server is not None else ipc.Clausters(workers=workers, lib_path=lib_path)
        #: only close the handle if we opened it (a shared one outlives us).
        self._owned = server is None

    def send_msg(self, target, addr, *args):
        self.server.send(_osclib.message(addr, *args))

    def send_bundle(self, target, when, *messages):
        packets = [_osclib.message(*m) for m in messages]
        self.server.send(_osclib.bundle_at(when, *packets))

    def recv(self, timeout):
        """One reply packet from the embedded server, or ``None`` within
        ``timeout``. Polls the reply ring (the server never blocks the caller)."""
        deadline = time.monotonic() + timeout
        while True:
            reply = self.server.poll()
            if reply is not None:
                return reply
            if time.monotonic() >= deadline:
                return None
            time.sleep(0.001)

    def close(self):
        if self._owned and self.server is not None:
            self.server.close()
            self.server = None
