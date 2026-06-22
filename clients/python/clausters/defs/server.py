"""Server facade: the running Clausters server, its resources and comms.

This is the **server-application** side of the client: it owns the
**communication interface** (RT over UDP by default; an ``OscNrtInterface`` for
offline; shared-memory/embed would be further interfaces), the client-side
resource allocators (`node`/`bus`/`buffer`), builds the OSC and
handles the async replies (``/done`` / ``/fail``) and notifications.

Timing is **not** here and not in the clock-as-sender: the clock
(`clausters.base.clock.TempoClock`) only schedules and tells time; the
Server emits, reading the logical time from the clock of the routine in flight.
A routine sequences events by calling `send_bundle`; swapping the
Server's interface retargets every routine from live RT to an NRT score without
touching clock or routine.
"""

import time
from dataclasses import dataclass

from ..base import _osclib
from ..errors import CommandError, ReplyTimeout
from ..base.main import main
from ..base.netaddr import NetAddr
from ..base._oscinterface import OscNrtInterface, OscUDPInterface
from ..base.timebase import SampleClockTimebase
from .bus import (
    AudioBusAllocator,
    Bus,
    ControlBusAllocator,
)
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


def _control_key(key):
    """A control identifier in a reply is a name string, or an int index when
    the server could not resolve a name."""
    return key if isinstance(key, str) else int(key)


def _parse_tree_nodes(args, i, count, flag):
    """Recursively parse `count` nodes of a ``/g_queryTree.reply`` starting at
    index `i`; returns (nodes, next_index). A synth has child-count -1."""
    out = []
    for _ in range(count):
        node_id, child_count = int(args[i]), int(args[i + 1])
        i += 2
        if child_count == -1:
            node = {"id": node_id, "def": str(args[i])}
            i += 1
            if flag:
                ncon = int(args[i])
                i += 1
                controls = {}
                for _ in range(ncon):
                    controls[_control_key(args[i])] = float(args[i + 1])
                    i += 2
                node["controls"] = controls
            out.append(node)
        else:
            children, i = _parse_tree_nodes(args, i, child_count, flag)
            out.append({"id": node_id, "children": children})
    return out, i


def _parse_query_tree(args) -> dict:
    """``/g_queryTree.reply`` -> a nested ``{"id", "children"|"def"+"controls"}``
    tree. A standalone function so it can be unit-tested without a server."""
    flag = int(args[0])
    root_id = int(args[1])
    count = int(args[2])
    children, _ = _parse_tree_nodes(args, 3, count, flag)
    return {"id": root_id, "children": children}


def _parse_n_info(args) -> dict:
    """``/n_info`` -> a per-node dict (see ``CmdTranslator::node_info``)."""
    info = {
        "id": int(args[0]),
        "parent": int(args[1]),
        "prev": int(args[2]),
        "next": int(args[3]),
        "is_group": bool(int(args[4])),
    }
    if info["is_group"]:
        info["head"], info["tail"] = int(args[5]), int(args[6])
        return info
    i = 5
    info["def"] = str(args[i])
    i += 1
    ncon = int(args[i])
    i += 1
    controls = {}
    for _ in range(ncon):
        controls[_control_key(args[i])] = float(args[i + 1])
        i += 2
    info["controls"] = controls
    nmaps = int(args[i])
    i += 1
    maps = []
    for _ in range(nmaps):
        maps.append({"control": int(args[i]), "bus": int(args[i + 1]), "audio": bool(args[i + 2])})
        i += 3
    info["maps"] = maps
    info["reads"], info["writes"] = str(args[i]), str(args[i + 1])
    return info


# Server defaults, mirroring the Rust server's `DEFAULT_AUDIO_BUSES` /
# `DEFAULT_CONTROL_BUSES` (128 is the hard audio ceiling) and `--sample-rate`.
# They live here, on the server-config object, not in the bus module: how many
# buses exist is the server's property, and these are only the fallback when the
# caller does not specify. The bus allocators carry no defaults of their own.
DEFAULT_AUDIO_BUSES = 128
DEFAULT_CONTROL_BUSES = 1024
DEFAULT_SAMPLE_RATE = 48000


@dataclass
class ServerOptions:
    """Client-owned server configuration, the way SuperCollider's
    ``ServerOptions`` works: it both **sizes the client's bus allocators** and
    emits the **CLI flags** to launch a matching server (`args`), so the
    two agree by construction. Verify a running server with
    `Server.query_info`.
    """

    audio_buses: int = DEFAULT_AUDIO_BUSES
    control_buses: int = DEFAULT_CONTROL_BUSES
    sample_rate: int = DEFAULT_SAMPLE_RATE

    def args(self) -> list[str]:
        """The ``clausters`` CLI flags that launch a server matching these
        options (pass to ``subprocess`` after the binary path)."""
        return [
            "--audio-buses", str(self.audio_buses),
            "--control-buses", str(self.control_buses),
            "--sample-rate", str(self.sample_rate),
        ]


@dataclass
class ServerInfo:
    """The static configuration a running server reports over ``/server_info``
    (read-only; the result of `Server.query_info`)."""

    audio_buses: int
    control_buses: int
    channels: int
    block_size: int
    nominal_sample_rate: float
    actual_sample_rate: float


class Server:
    def __init__(self, host: str = "127.0.0.1", port: int = 57110, interface=None,
                 latency: float = 0.0, options: "ServerOptions | None" = None):
        self.target = NetAddr(host, port)
        #: the communication interface (RT/UDP, NRT/score, …). The Server owns
        #: it; swapping it is the RT/NRT seam.
        self.interface = interface if interface is not None else OscUDPInterface().start()
        #: seconds added to RT timetags so they land in the (near) future,
        #: sample-accurate, instead of "as soon as possible" (scsynth latency).
        self.latency = latency
        #: the client-owned server configuration; sizes the allocators below so
        #: they never hand out a bus the server does not have. Override it to
        #: match a server launched with `--audio-buses`/`--control-buses`, or
        #: reconcile it from a running server with `query_info`.
        self.options = options if options is not None else ServerOptions()
        self.nodes = NodeIDAllocator()
        self.audio_buses = AudioBusAllocator(size=self.options.audio_buses)
        self.control_buses = ControlBusAllocator(size=self.options.control_buses)
        self.buffers = BufferAllocator()
        self._sync_counter = 0      # ids for /sync -> /synced round-trips

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
        beat = base + delay_beats
        secs = clock.beats2secs(beat)

        if getattr(self.interface, "time_mode", "unix") == "score":
            # NRT: seconds from render start (logical, timebase-independent).
            self.interface.send_bundle(self.target.addr(), secs, *messages)
            return

        timebase = getattr(clock, "timebase", None)
        if isinstance(timebase, SampleClockTimebase):
            # Anchored to the server's sample clock: schedule by absolute sample,
            # drift-free and sample-accurate, via /sched.
            origin = clock.pacing_origin or 0.0      # seconds in the sample timebase
            sample = round((origin + secs + self.latency) * timebase.sample_rate)
            self._send_sched(sample, messages)
        else:
            # Wall clock: an NTP-timetagged bundle.
            wall = (clock.start_time if clock.start_time is not None else time.time())
            self.interface.send_bundle(self.target.addr(), wall + secs + self.latency, *messages)

    def play_event(self, event):
        """Realize a note `Event` as OSC: `/s_new`
        at the routine's logical beat, then `/n_free` (or `gate 0`) after the
        sustain. The OSC side of the double dispatch — a MIDI destination
        realizes the same event as note on/off. Returns the synth node id (or
        None for a rest)."""
        if event.get("type") == "rest":
            return None
        node_id = self.nodes.alloc()
        self.send_bundle(
            ("/s_new", event["instrument"], node_id, int(event["add_action"]),
             int(event["target"]), *event._control_args())
        )
        sustain = event.sustain()
        if event.get("has_gate"):
            self.send_bundle(("/n_set", node_id, "gate", 0.0), delay_beats=sustain)
        else:
            self.send_bundle(("/n_free", node_id), delay_beats=sustain)
        return node_id

    def _send_sched(self, sample: int, messages):
        inner = _osclib.immediate_bundle(*[_osclib.message(*m) for m in messages])
        self.send_msg("/sched", _osclib.Int64(sample), inner)

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
        raise ReplyTimeout(f"no reply to {addr}")

    def query_info(self, timeout: float = 5.0) -> ServerInfo:
        """Asks the running server for its static configuration (RT only):
        bus counts, output channels, block size and sample rate. Use it to
        size or check allocators against a server you did not launch; compare
        the result with `options`."""
        _, args = self.request(
            "/server_info", timeout=timeout, expect=("/server_info.reply",)
        )
        return ServerInfo(
            audio_buses=int(args[0]),
            control_buses=int(args[1]),
            channels=int(args[2]),
            block_size=int(args[3]),
            nominal_sample_rate=float(args[4]),
            actual_sample_rate=float(args[5]),
        )

    # ---- node tree introspection (RT only) ----

    def query_tree(self, group=ROOT_NODE_ID, *, controls: bool = True,
                   timeout: float = 5.0) -> dict:
        """The node tree from `group` down (scsynth ``/g_queryTree``), as a
        nested dict: a group is ``{"id", "children": [...]}``, a synth is
        ``{"id", "def", "controls": {name: value}}`` (controls only when
        ``controls=True``). This is the **structured** way to read the tree —
        never scrape the server's logs."""
        gid = group.id if hasattr(group, "id") else group
        addr, args = self.request("/g_queryTree", int(gid), 1 if controls else 0,
                                  timeout=timeout, expect=("/g_queryTree.reply", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/g_queryTree failed: {args}")
        return _parse_query_tree(args)

    def node_query(self, node, timeout: float = 5.0) -> dict:
        """Per-node detail (``/n_query`` -> ``/n_info``): ``id``, ``parent``,
        ``prev``/``next`` siblings, ``is_group``; for a group ``head``/``tail``;
        for a synth ``def``, ``controls``, ``maps`` (``/n_map`` bindings) and the
        inferred ``reads``/``writes`` bus lists."""
        nid = node.id if hasattr(node, "id") else node
        addr, args = self.request("/n_query", int(nid),
                                  timeout=timeout, expect=("/n_info", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/n_query failed: {args}")
        return _parse_n_info(args)

    def dump_graph(self, group=ROOT_NODE_ID, timeout: float = 5.0) -> str:
        """The inferred bus graph of `group` as a human-readable string
        (``/g_dumpGraph``): what each child reads/writes and the current order.
        A debugging aid; for machine use prefer `query_tree`."""
        gid = group.id if hasattr(group, "id") else group
        addr, args = self.request("/g_dumpGraph", int(gid),
                                  timeout=timeout, expect=("/g_dumpGraph.reply", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/g_dumpGraph failed: {args}")
        return str(args[1])

    # ---- definitions ----

    def add_faustdef(self, fdef: FaustDef, *, wait: bool = True,
                     timeout: float = 10.0) -> str:
        """Sends a `FaustDef` via ``/d_faust``.

        ``/d_faust`` JIT-compiles **asynchronously** on the server's network
        thread (answered later by ``/done``/``/fail``). In RT, ``wait=True``
        (the default) blocks until that reply -- raising `CommandError`
        on ``/fail`` or `ReplyTimeout` if it never lands; ``wait=False``
        returns immediately (fire-and-forget), so use `sync` as a barrier
        before relying on the def (e.g. ``yield`` it from a routine, never block
        in one). In NRT it always *scores* ``/d_faust`` at time 0 -- the
        renderer compiles it before time advances -- so ``wait`` does not
        apply."""
        if getattr(self.interface, "time_mode", "unix") == "score":
            self.send_msg("/d_faust", fdef.name, fdef.payload())
            return fdef.name
        if not wait:
            self.send_msg("/d_faust", fdef.name, fdef.payload())
            return fdef.name
        addr, args = self.request(
            "/d_faust", fdef.name, fdef.payload(), timeout=timeout, expect=("/done", "/fail")
        )
        if addr == "/fail":
            raise CommandError(f"/d_faust {fdef.name!r} failed: {args}")
        return fdef.name

    def add_synthdef(self, sdef, *, wait: bool = True, timeout: float = 10.0) -> str:
        """Sends a UGen `SynthDef` via
        ``/d_recv``. Like `add_faustdef`: ``wait=True`` (default) blocks
        in RT until ``/done``/``/fail``; ``wait=False`` is fire-and-forget
        (pair with `sync`). In NRT it scores ``/d_recv`` at time 0 so the
        renderer compiles it before time advances."""
        payload = sdef.payload()
        if getattr(self.interface, "time_mode", "unix") == "score":
            self.send_msg("/d_recv", payload)
            return sdef.name
        if not wait:
            self.send_msg("/d_recv", payload)
            return sdef.name
        addr, args = self.request(
            "/d_recv", payload, timeout=timeout, expect=("/done", "/fail")
        )
        if addr == "/fail":
            raise CommandError(f"/d_recv {sdef.name!r} failed: {args}")
        return sdef.name

    def add_graphdef(self, gdef, *, wait: bool = True, timeout: float = 10.0) -> str:
        """Sends a `GraphDef` via ``/d_graph``.
        Like `add_synthdef`/`add_faustdef`: ``wait=True``
        (default) blocks in RT until ``/done``/``/fail``; ``wait=False`` is
        fire-and-forget (pair with `sync`). In NRT it scores ``/d_graph``
        at time 0. Loading a GraphDef is cheap on the server (no JIT — it only
        validates and references the member defs), but it is still asynchronous,
        so the same barrier discipline applies."""
        payload = gdef.payload()
        if getattr(self.interface, "time_mode", "unix") == "score":
            self.send_msg("/d_graph", payload)
            return gdef.name
        if not wait:
            self.send_msg("/d_graph", payload)
            return gdef.name
        addr, args = self.request(
            "/d_graph", payload, timeout=timeout, expect=("/done", "/fail")
        )
        if addr == "/fail":
            raise CommandError(f"/d_graph {gdef.name!r} failed: {args}")
        return gdef.name

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

    def graph(self, defname, ports=None, *, target=ROOT_NODE_ID,
              action=AddAction.TAIL) -> Group:
        """Instantiates a GraphDef (``/graph_new``) as a wired group, with
        ``ports`` (a ``{name: value}`` dict) overriding the def defaults. The
        returned `Group` is the instance: drive it
        through the surface with `set` (``/n_set`` resolves names against
        the surface, not the private members) and tear it down with
        `free` (which also reclaims its private buses)."""
        node_id = self.nodes.alloc()
        self.send_msg("/graph_new", defname, node_id, int(action), int(target),
                      *_flatten_controls(ports))
        return Group(node_id)

    def graph_voice(self, instance, ports=None) -> Group:
        """Spawns a per-voice sub-graph (``/graph_voice``) inside a running
        GraphDef ``instance`` (a `Group` from
        `graph`), wired to its shared private buses. ``ports`` overrides
        the voice-port defaults. The returned group is the voice: drive it
        through its surface with `set` and free it with `free`."""
        inst_id = instance.id if hasattr(instance, "id") else instance
        node_id = self.nodes.alloc()
        self.send_msg("/graph_voice", inst_id, node_id, *_flatten_controls(ports))
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
            raise CommandError(f"/b_alloc {bufnum} failed: {args}")
        return Buffer(bufnum, frames, channels)

    def free_buffer(self, buf):
        bufnum = buf.bufnum if isinstance(buf, Buffer) else buf
        self.send_msg("/b_free", bufnum)
        self.buffers.free(bufnum)

    # ---- offline render (NRT interface only) ----

    def render(self, sample_rate: float = 48_000.0, channels: int = 2):
        """Renders the accumulated score (the interface must be an
        `OscNrtInterface`). Schedule a closing bundle (e.g. ``/n_free 0``)
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

    def sync(self, timeout: float = 5.0) -> int:
        """The async barrier (scsynth ``/sync``): sends ``/sync id`` and blocks
        until the server answers ``/synced id``, which it does only once every
        async command sent earlier -- Faust/SynthDef compiles, buffer jobs --
        has completed. Use it after a ``wait=False`` `add_faustdef` /
        `add_synthdef` / buffer alloc. RT only (in NRT the renderer
        already serializes async work at time 0). Returns the id used.

        **Blocking — never call from a routine.** This (and any ``wait=True``)
        blocks the calling thread on a reply: fine on your own thread, but it
        would freeze the clock thread if called from inside a routine generator
        (see `Routine`). It also polls the socket
        synchronously; a non-blocking, notification-driven barrier you can
        ``yield`` from a routine is future work (``OSCFunc``)."""
        self._sync_counter += 1
        sync_id = self._sync_counter
        self.request("/sync", sync_id, timeout=timeout, expect=("/synced",))
        return sync_id

    def quit(self):
        self.send_msg("/quit")

    def sample_clock(self, window: int = 64, timeout: float = 2.0):
        """A `UdpSampleClock` tracking this
        server's sample clock over UDP. Pass its ``.timebase()`` to a
        ``TempoClock`` to anchor timing to the server and schedule by ``/sched``."""
        from .clocksync import UdpSampleClock

        return UdpSampleClock(self, window=window, timeout=timeout)

    def close(self):
        self.interface.close()
