"""Server facade: the running Clausters server, its resources and comms.

This is the **server-application** side of the client (see the memory note
``separacion-cliente-servidor-clausters`` and ``clients/PLAN.md``): it owns the
**communication interface** (RT over UDP by default; an ``OscNrtInterface`` for
offline; shared-memory/embed would be further interfaces), the client-side
resource allocators (:mod:`node`/:mod:`bus`/:mod:`buffer`), builds the OSC and
handles the async replies (``/done`` / ``/fail``) and notifications.

Timing is **not** here and not in the clock-as-sender: the clock
(:class:`clausters.base.clock.TempoClock`) only schedules and tells time; the
Server emits, reading the logical time from the clock of the routine in flight.
A routine sequences events by calling :meth:`send_bundle`; swapping the
Server's interface retargets every routine from live RT to an NRT score without
touching clock or routine.
"""

import time

from ..base import _osclib
from ..base.main import main
from ..base.netaddr import NetAddr
from ..base._oscinterface import OscNrtInterface, OscUDPInterface
from .bus import AudioBusAllocator, Bus, ControlBusAllocator
from .buffer import Buffer, BufferAllocator
from .faustdef import FaustDef
from .node import AddAction, Group, NodeIDAllocator, ROOT_NODE_ID, Synth


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
    def __init__(self, host: str = "127.0.0.1", port: int = 57110, interface=None,
                 latency: float = 0.0):
        self.target = NetAddr(host, port)
        #: the communication interface (RT/UDP, NRT/score, …). The Server owns
        #: it; swapping it is the RT/NRT seam.
        self.interface = interface if interface is not None else OscUDPInterface().start()
        #: seconds added to RT timetags so they land in the (near) future,
        #: sample-accurate, instead of "as soon as possible" (scsynth latency).
        self.latency = latency
        self.nodes = NodeIDAllocator()
        self.audio_buses = AudioBusAllocator()
        self.control_buses = ControlBusAllocator()
        self.buffers = BufferAllocator()

    # ---- raw OSC: immediate and timed ----

    def send_msg(self, addr, *args):
        """Send one message immediately."""
        self.interface.send_msg(self.target.addr(), addr, *args)

    def send_bundle(self, *messages, delay_beats: float = 0.0, clock=None):
        """Emit a timetagged bundle of ``(addr, *args)`` messages at the running
        routine's **exact logical beat** (+ optional lookahead). Call it from a
        routine playing on a clock (found via ``main.current_tt``) or pass
        ``clock=``. The timetag comes from the yield-accumulated beat, not from
        wall-clock now, so inter-event timing is exact; the interface decides
        the wire time (wall clock for RT, seconds-from-start for NRT)."""
        tt = main.current_tt
        if clock is None:
            clock = getattr(tt, "clock", None)
            if clock is None:
                raise RuntimeError(
                    "send_bundle needs a clock: call it from a routine playing "
                    "on a TempoClock, or pass clock=..."
                )
        base = getattr(tt, "_logical_beat", None)
        if base is None:
            base = clock.beats()
        self.interface.send_bundle(
            self.target.addr(), self._when(clock, base + delay_beats), *messages
        )

    def _when(self, clock, beat: float) -> float:
        secs = clock.beats2secs(beat)
        if getattr(self.interface, "time_mode", "unix") == "score":
            return secs  # seconds from render start
        base = clock.start_time if clock.start_time is not None else time.time()
        return base + secs + self.latency  # absolute wall clock (+ latency)

    def request(self, addr, *args, timeout: float = 5.0, expect=None):
        """Sends a message and returns the first matching reply ``(addr, args)``
        (RT only; the interface must reply). ``expect`` filters reply addresses."""
        self.send_msg(addr, *args)
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            packet = self.interface.recv(timeout)
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
        self.send_msg("/n_set", node.id if hasattr(node, "id") else node,
                      *_flatten_controls(controls))

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

    # ---- offline render (NRT interface only) ----

    def render(self, sample_rate: float = 48_000.0, channels: int = 2):
        """Renders the accumulated score (the interface must be an
        :class:`OscNrtInterface`). Schedule a closing bundle (e.g. ``/n_free 0``)
        so the render has a defined duration."""
        if not isinstance(self.interface, OscNrtInterface):
            raise RuntimeError("render() needs a Server with an OscNrtInterface")
        return self.interface.render(sample_rate=sample_rate, channels=channels)

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
        self.interface.close()
