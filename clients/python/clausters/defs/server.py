"""Server facade: resources, definitions and the OSC round-trip.

Ties the client-side allocators (:mod:`node`/:mod:`bus`/:mod:`buffer`) to a live
Clausters server, builds the OSC and handles the async replies (``/done`` /
``/fail``) and notifications (``/n_go`` / ``/n_end``). By default it talks UDP;
pass any connection exposing ``send(packet)`` / ``recv(timeout)`` (e.g. an
adapter over the shared-memory transport) to reuse the same logic.

Offline (NRT) work does not go through here: build a score with
:mod:`clausters.defs.signals` + :mod:`clausters.defs.faustdef` and a clock with
an ``OscNrtInterface``, then ``render`` it (see ``GUIA.md``).
"""

import socket
import time

from ..base import _osclib
from ..base.netaddr import NetAddr
from .bus import AudioBusAllocator, Bus, ControlBusAllocator
from .buffer import Buffer, BufferAllocator
from .faustdef import FaustDef
from .node import AddAction, Group, NodeIDAllocator, ROOT_NODE_ID, Synth


class UdpConnection:
    """A bound UDP socket; replies come back to the address it binds."""

    def __init__(self, host: str = "127.0.0.1", port: int = 57110):
        self.target = (host, port)
        self._sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self._sock.bind(("127.0.0.1", 0))

    def send(self, packet: bytes):
        self._sock.sendto(packet, self.target)

    def recv(self, timeout: float):
        self._sock.settimeout(timeout)
        try:
            data, _ = self._sock.recvfrom(65536)
            return data
        except (TimeoutError, OSError):
            return None

    def close(self):
        self._sock.close()


def _flatten_controls(controls) -> list:
    """Accepts a dict or a list of (name, value) pairs (so the reserved
    ``in``/``out`` controls, which are Python keywords, are expressible)."""
    if controls is None:
        return []
    items = controls.items() if isinstance(controls, dict) else controls
    flat = []
    for name, value in items:
        flat += [name, value]
    return flat


class Server:
    def __init__(self, host: str = "127.0.0.1", port: int = 57110, conn=None):
        self.target = NetAddr(host, port)
        self.conn = conn if conn is not None else UdpConnection(host, port)
        self.nodes = NodeIDAllocator()
        self.audio_buses = AudioBusAllocator()
        self.control_buses = ControlBusAllocator()
        self.buffers = BufferAllocator()

    # ---- raw OSC ----

    def send_msg(self, addr, *args):
        self.conn.send(_osclib.message(addr, *args))

    def request(self, addr, *args, timeout: float = 5.0, expect=None):
        """Sends a message and returns the first matching reply ``(addr, args)``.
        ``expect`` is a set of reply addresses to accept (defaults to any)."""
        self.conn.send(_osclib.message(addr, *args))
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            packet = self.conn.recv(timeout)
            if packet is None:
                continue
            raddr, rargs = _osclib.decode(packet)
            if expect is None or raddr in expect:
                return raddr, rargs
        raise TimeoutError(f"no reply to {addr}")

    # ---- definitions ----

    def add_def(self, fdef: FaustDef, timeout: float = 10.0) -> str:
        """Sends ``/d_faust`` and blocks until it compiles (or raises)."""
        addr, args = self.request(
            "/d_faust", fdef.name, fdef.payload(), timeout=timeout, expect=("/done", "/fail")
        )
        if addr == "/fail":
            raise RuntimeError(f"/d_faust {fdef.name!r} failed: {args}")
        return fdef.name

    def free_def(self, *names: str):
        self.send_msg("/d_free", *names)

    # ---- nodes ----

    def synth(self, defname, controls=None, *, target=ROOT_NODE_ID,
              action=AddAction.TAIL) -> Synth:
        node_id = self.nodes.alloc()
        self.send_msg("/s_new", defname, node_id, int(action), int(target),
                      *_flatten_controls(controls))
        return Synth(node_id, defname)

    def group(self, *, target=ROOT_NODE_ID, action=AddAction.TAIL) -> Group:
        node_id = self.nodes.alloc()
        self.send_msg("/g_new", node_id, int(action), int(target))
        return Group(node_id)

    def set(self, node, controls):
        flat = _flatten_controls(controls)
        self.send_msg("/n_set", node.id if hasattr(node, "id") else node, *flat)

    def map(self, node, name, bus, *, audio=False):
        index = bus.index if isinstance(bus, Bus) else bus
        self.send_msg("/n_mapa" if audio else "/n_map",
                      node.id if hasattr(node, "id") else node, name, index)

    def free(self, *nodes):
        for n in nodes:
            nid = n.id if hasattr(n, "id") else n
            self.send_msg("/n_free", nid)
            if hasattr(n, "id"):
                self.nodes.free(nid)

    # ---- buses ----

    def audio_bus(self, channels: int = 1) -> Bus:
        return self.audio_buses.alloc(channels)

    def control_bus(self) -> Bus:
        return self.control_buses.alloc(1)

    def set_bus(self, bus, value):
        index = bus.index if isinstance(bus, Bus) else bus
        self.send_msg("/c_set", index, float(value))

    def get_bus(self, bus, timeout: float = 5.0) -> float:
        index = bus.index if isinstance(bus, Bus) else bus
        _, args = self.request("/c_get", index, timeout=timeout, expect=("/c_set",))
        return args[1] if len(args) >= 2 else args[-1]

    # ---- buffers ----

    def alloc_buffer(self, frames: int, channels: int = 1, timeout: float = 5.0) -> Buffer:
        bufnum = self.buffers.alloc()
        addr, args = self.request("/b_alloc", bufnum, frames, channels,
                                  timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            self.buffers.free(bufnum)
            raise RuntimeError(f"/b_alloc {bufnum} failed: {args}")
        return Buffer(bufnum, frames, channels)

    def free_buffer(self, buf):
        bufnum = buf.bufnum if isinstance(buf, Buffer) else buf
        self.send_msg("/b_free", bufnum)
        self.buffers.free(bufnum)

    # ---- server control ----

    def notify(self, flag: bool = True, timeout: float = 5.0):
        return self.request("/notify", 1 if flag else 0, timeout=timeout, expect=("/done",))

    def status(self, timeout: float = 5.0):
        _, args = self.request("/status", timeout=timeout, expect=("/status.reply",))
        return args

    def sync(self, timeout: float = 5.0):
        """Round-trips ``/status`` so earlier async commands have landed."""
        self.status(timeout=timeout)

    def quit(self):
        self.send_msg("/quit")

    def close(self):
        if hasattr(self.conn, "close"):
            self.conn.close()
