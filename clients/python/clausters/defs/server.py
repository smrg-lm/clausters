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
from dataclasses import dataclass, field

from .. import _native
from ..config import client_config, server_config
from ..base import _osclib
from ..errors import CommandError, ReplyTimeout
from ..base.main import main
from ..base.moment import Moment
from ..base.netaddr import NetAddr
from ..base._oscinterface import OscNrtInterface, OscTcpInterface, OscUdpInterface, OscWsInterface
from ..base.timebase import SampleClockTimebase
from .bus import (
    AudioBusAllocator,
    Bus,
    ControlBusAllocator,
)
from .buffer import BufferAllocator
from .info import (
    BufferInfo,
    ControlInfo,
    DefInfo,
    NodeInfo,
    NodeMap,
    Tree,
    UgenInfo,
    UgenInput,
    parse_buffer_list,
    parse_def_info,
    parse_query_tree,
    parse_ugen_info,
)
from .node import Group, NodeIdAllocator, ROOT_NODE_ID


# Server defaults, mirroring the Rust server's `DEFAULT_AUDIO_BUSES` /
# `DEFAULT_CONTROL_BUSES` (128 is the hard audio ceiling) and `--sample-rate`.
# They live here, on the server-config object, not in the bus module: how many
# buses exist is the server's property, and these are only the fallback when the
# caller does not specify. The bus allocators carry no defaults of their own.
DEFAULT_AUDIO_BUSES = 128
DEFAULT_CONTROL_BUSES = 16384
DEFAULT_SAMPLE_RATE = 48000
# Boot-time pre-allocated pool sizes, mirroring the Rust server's `Limits`
# defaults (`--max-nodes`/`--max-buffers`/`--max-graph-children`/
# `--max-ugen-inputs`). 32 is the hard ceiling on UGen inputs, like 128 for
# audio buses. Hardware channels default to the device's outputs (``None`` =
# no flag) and no live input.
DEFAULT_MAX_NODES = 8192
DEFAULT_MAX_BUFFERS = 4096
DEFAULT_MAX_GRAPH_CHILDREN = 512
DEFAULT_MAX_UGEN_INPUTS = 32
# The audio-tap region (`--taps`/`--tap-frames`): pre-allocated sample rings
# an audio bus can be routed into with `Server.tap`, read by a GUI host out of
# shared memory or streamed with `Server.stream_taps`. 0 taps disables the
# region; the ring capacity is rounded up to a power of two by the server.
DEFAULT_TAPS = 8
DEFAULT_TAP_FRAMES = 16384


@dataclass
class ServerOptions:
    """Client-owned server configuration, the way SuperCollider's
    ``ServerOptions`` works — the one enumeration of every option a launched
    server takes (`Server.boot` / `Session.live` accept it as ``options``).
    Two families of fields, with different defaulting:

    - **Sizing** (buses, pools, taps, hardware I/O): these also size the
      client's allocators, so their defaults read the same config file the
      server reads and `args` always emits them — the launched server and
      this object agree by construction. Verify a running server with
      `Server.query_info`.
    - **Behavior** (``workers``, ``tcp``, ``ws``, ``midi``, ``persist``,
      ``max_frame``, ``max_clients``, ``pin``): server-only, no client-side
      counterpart. Their default ``None`` emits **no flag**, leaving the
      server's own precedence intact (CLI flag > project config > user
      config > compiled default); a set value emits the flag, which wins.
    """

    # The defaults come from the config file's ``[server]`` section (the same
    # file the Rust server reads), falling back to the compiled constants. A
    # value passed explicitly still wins, as a dataclass field.
    audio_buses: int = field(
        default_factory=lambda: server_config().get("audio_buses", DEFAULT_AUDIO_BUSES)
    )
    control_buses: int = field(
        default_factory=lambda: server_config().get("control_buses", DEFAULT_CONTROL_BUSES)
    )
    sample_rate: int = field(
        default_factory=lambda: server_config().get("sample_rate", DEFAULT_SAMPLE_RATE)
    )
    #: Hardware output channels; ``None`` follows the device default (no flag).
    outputs: "int | None" = field(
        default_factory=lambda: server_config().get("outputs", None)
    )
    #: Hardware input channels; ``0`` opens no input device.
    inputs: int = field(default_factory=lambda: server_config().get("inputs", 0))
    #: Node slab capacity, root included.
    max_nodes: int = field(
        default_factory=lambda: server_config().get("max_nodes", DEFAULT_MAX_NODES)
    )
    #: Buffer pool size.
    max_buffers: int = field(
        default_factory=lambda: server_config().get("max_buffers", DEFAULT_MAX_BUFFERS)
    )
    #: Per-group child capacity.
    max_graph_children: int = field(
        default_factory=lambda: server_config().get("max_graph_children", DEFAULT_MAX_GRAPH_CHILDREN)
    )
    #: Accepted inputs per UGen (clamped to 32 by the server).
    max_ugen_inputs: int = field(
        default_factory=lambda: server_config().get("max_ugen_inputs", DEFAULT_MAX_UGEN_INPUTS)
    )
    #: Audio-tap rings for oscilloscopes (0 disables the tap region).
    taps: int = field(
        default_factory=lambda: server_config().get("taps", DEFAULT_TAPS)
    )
    #: Per-tap ring capacity in samples (rounded up to a power of two).
    tap_frames: int = field(
        default_factory=lambda: server_config().get("tap_frames", DEFAULT_TAP_FRAMES)
    )

    # --- behavior options: ``None`` emits no flag (the server's own config
    # layering decides); a set value emits the flag and wins.
    #: DSP worker threads for parallel groups (``--workers``).
    workers: "int | None" = None
    #: The TCP command plane: ``False`` disables it (``--no-tcp``), ``True``
    #: forces it on at the default port, a number moves it (``--tcp <port>``).
    #: It is on by default server-side; ``None`` leaves that as is.
    tcp: "bool | int | None" = None
    #: OSC over WebSocket: ``True`` opens it at the default port (57120), a
    #: number picks the port (``--ws [port]``). There is no off flag — leave
    #: ``None`` and keep it out of the server's config to run without it.
    ws: "bool | int | None" = None
    #: Virtual MIDI input: ``True`` opens it with the default name, a string
    #: names the port (``--midi [name]``).
    midi: "bool | str | None" = None
    #: Def persistence: ``False`` disables it for this run (``--no-persist``).
    #: There is no force-on flag — ``True`` is expressible only by keeping
    #: ``persist = false`` out of the server's config.
    persist: "bool | None" = None
    #: Largest OSC frame on the stream transports, bytes (``--max-frame``).
    max_frame: "int | None" = None
    #: Concurrent stream clients, TCP + WebSocket (``--max-clients``).
    max_clients: "int | None" = None
    #: CPU affinity list (``--pin``): first CPU for the audio callback, the
    #: rest round-robin over the DSP workers. Experimental, Linux only, and
    #: only accepted by a server built with the ``rtprio`` feature.
    pin: "tuple | list | str | None" = None

    def args(self) -> list[str]:
        """The ``clausters`` CLI flags that launch a server matching these
        options (pass to ``subprocess`` after the binary path). ``outputs`` is
        emitted only when set (otherwise the server follows the device); the
        pre-allocated pool sizes are always emitted so the launched server and
        this object agree by construction."""
        flags = [
            "--audio-buses", str(self.audio_buses),
            "--control-buses", str(self.control_buses),
            "--sample-rate", str(self.sample_rate),
            "--inputs", str(self.inputs),
            "--max-nodes", str(self.max_nodes),
            "--max-buffers", str(self.max_buffers),
            "--max-graph-children", str(self.max_graph_children),
            "--max-ugen-inputs", str(self.max_ugen_inputs),
            "--taps", str(self.taps),
            "--tap-frames", str(self.tap_frames),
        ]
        if self.outputs is not None:
            flags += ["--outputs", str(self.outputs)]
        # Behavior flags: emitted only when set (`None` defers to the
        # server's own config). `is True`/`is False` first — a bool is an
        # int, so the port/number branch must come after.
        if self.workers is not None:
            flags += ["--workers", str(self.workers)]
        if self.tcp is False:
            flags += ["--no-tcp"]
        elif self.tcp is True:
            flags += ["--tcp"]
        elif self.tcp is not None:
            flags += ["--tcp", str(self.tcp)]
        if self.ws is True:
            flags += ["--ws"]
        elif self.ws not in (None, False):
            flags += ["--ws", str(self.ws)]
        if self.midi is True:
            flags += ["--midi"]
        elif self.midi not in (None, False):
            flags += ["--midi", str(self.midi)]
        if self.persist is False:
            flags += ["--no-persist"]
        if self.max_frame is not None:
            flags += ["--max-frame", str(self.max_frame)]
        if self.max_clients is not None:
            flags += ["--max-clients", str(self.max_clients)]
        if self.pin is not None:
            cpus = self.pin if isinstance(self.pin, str) \
                else ",".join(str(c) for c in self.pin)
            flags += ["--pin", cpus]
        return flags


@dataclass
class ServerInfo:
    """The static configuration a running server reports over ``/server_query``
    (read-only; the result of `Server.query_info`).

    The first six fields are the stable original set; the rest are the
    boot-time capacities the server appends. ``channels`` is the hardware
    **output** channel count, ``input_channels`` the live input count (0 when
    the server was launched without ``--inputs``). Against a pre-S7 server that
    reports only six fields, the appended ones fall back to the compiled
    defaults."""

    audio_buses: int
    control_buses: int
    channels: int
    block_size: int
    nominal_sample_rate: float
    actual_sample_rate: float
    input_channels: int = 0
    max_nodes: int = DEFAULT_MAX_NODES
    max_buffers: int = DEFAULT_MAX_BUFFERS
    max_graph_children: int = DEFAULT_MAX_GRAPH_CHILDREN
    max_ugen_inputs: int = DEFAULT_MAX_UGEN_INPUTS
    #: Audio-tap region shape; ``0``/``0`` when the server has no segment
    #: (or predates taps).
    taps: int = 0
    tap_frames: int = 0
    #: The stream-transport frame ceiling in bytes (``--max-frame``): the
    #: largest OSC frame a TCP/WebSocket client may send or receive, what
    #: bulk requests (`clausters.defs.Buffer.get_samples` chunks) are sized from. Falls back
    #: to the UDP datagram cap against a server too old to report it.
    max_frame: int = 65536

    def __str__(self) -> str:
        drift = ("" if self.actual_sample_rate == self.nominal_sample_rate
                 else f" (nominal {self.nominal_sample_rate:g})")
        taps = (f"{self.taps} x {self.tap_frames} frames" if self.taps
                else "none (no segment)")
        return "\n".join([
            f"server {self.actual_sample_rate:g} Hz{drift}, "
            f"{self.block_size}-sample blocks, "
            f"{self.channels} out / {self.input_channels} in",
            f"  buses   {self.audio_buses} audio, {self.control_buses} control",
            f"  limits  {self.max_nodes} nodes, {self.max_buffers} buffers, "
            f"{self.max_graph_children} graph children, "
            f"{self.max_ugen_inputs} ugen inputs",
            f"  taps    {taps}",
            f"  frame   {self.max_frame} bytes max",
        ])


class Server:
    def __init__(self, host: "str | None" = None, port: "int | None" = None, interface=None,
                 latency: "float | None" = None, options: "ServerOptions | None" = None,
                 transport: "str | None" = None):
        # An explicit argument wins; otherwise the config file's ``[client]``
        # section provides the default, then the built-in fallback. This is the
        # client end of the same config the server reads.
        cfg = client_config()
        host = host if host is not None else cfg.get("host", "127.0.0.1")
        port = port if port is not None else cfg.get("port", 57110)
        latency = latency if latency is not None else cfg.get("latency", None)
        self.target = NetAddr(host, port)
        #: the communication interface (RT/TCP by default, NRT/score, …). The
        #: Server owns it; swapping it is the RT/NRT seam. ``transport`` picks
        #: the default carrier when no explicit ``interface`` is given:
        #: ``"tcp"`` (the command plane — reliable, and a def or a bulk read is
        #: not bounded by a datagram; the connection opens lazily on first
        #: use), ``"udp"`` (each packet must fit a datagram) or ``"ws"``. UDP
        #: remains the *discovery* protocol: `boot` probes over it before
        #: connecting.
        if interface is not None:
            self.interface = interface
        else:
            transport = transport if transport is not None else cfg.get("transport", "tcp")
            if transport == "tcp":
                # Not started here: it connects on first send, so a handle may
                # be built before (or without) a reachable server.
                self.interface = OscTcpInterface(host, port)
            elif transport == "udp":
                self.interface = OscUdpInterface().start()
            elif transport == "ws":
                self.interface = OscWsInterface(host)
            else:
                raise ValueError(f"unknown transport {transport!r} (tcp, udp or ws)")
        #: seconds added to RT timetags so they land in the (near) future,
        #: sample-accurate, instead of "as soon as possible" (scsynth latency).
        #: When neither the argument nor the config sets it, the default is
        #: transport-aware: a real-time interface (UDP/TCP/WS/embed, all
        #: wall-clock timetagged) gets 0.1 s of lead so events land on time
        #: instead of late, while an NRT/score interface keeps 0.0 (an offline
        #: score has no real deadline).
        if latency is None:
            latency = 0.0 if getattr(self.interface, "time_mode", None) == "score" else 0.1
        self.latency = latency
        #: the client-owned server configuration; sizes the allocators below so
        #: they never hand out a bus the server does not have. Override it to
        #: match a server launched with `--audio-buses`/`--control-buses`, or
        #: reconcile it from a running server with `query_info`.
        self.options = options if options is not None else ServerOptions()
        # Allocators are registries of the server's finite boot-time resources,
        # sized from the options so client and server agree by construction.
        # The node-id range comes from the shared partition formula
        # (`--max-nodes` scales every range); in score (NRT) mode it is
        # unbounded — an offline render has no live `/node_end` stream to recycle
        # from, and no real-time bound on ids over the score's length.
        part = _native.node_id_partition(self.options.max_nodes)
        score = getattr(self.interface, "time_mode", "unix") == "score"
        self.nodes = NodeIdAllocator(
            part["client_base"], None if score else part["client_capacity"])
        self.audio_buses = AudioBusAllocator(size=self.options.audio_buses)
        self.control_buses = ControlBusAllocator(size=self.options.control_buses)
        self.buffers = BufferAllocator(size=self.options.max_buffers)
        #: the `/node_end` side-channel that returns node ids to the registry
        #: (an `OscReceiver` + `/server_notify`), started lazily by `_ensure_recycler`.
        self._recycler = None
        self._sync_counter = 0      # ids for /server_sync -> /server_sync.reply round-trips
        #: the server's stream-frame ceiling, queried lazily by `_bulk_chunk`.
        self._max_frame: "int | None" = None
        #: the server *process* this handle started and owns (`boot`), if any;
        #: stopped by `close`. ``None`` when attached to a server it did not
        #: start.
        self._process = None

    @classmethod
    def boot(cls, options: "ServerOptions | None" = None, *, shm="auto",
             transport: "str | None" = None,
             verbose: int = 0, workers: "int | None" = None,
             data_dir=None, server_args=(),
             latency: "float | None" = None, ready_timeout: float = 10.0,
             _adopt_default: bool = True) -> "Server":
        """Start a **separate** ``clausters`` server process and return a `Server`
        connected to and owning it.

        The launcher's ergonomic non-`Session` entry point: it spawns the
        standalone server (choosing a shared-memory segment), waits until it
        answers, and hands back a `Server` whose `close` also stops the process
        (and interpreter exit stops it too). Pair it with
        `clausters.gui.GuiHost.boot` for the GUI, or use `clausters.Session.live`
        for the bundled, clock-included path.

        Args:
            options: a `ServerOptions` — the enumeration of **every** option a
                launched server takes (sizing *and* behavior: transports, MIDI,
                persistence, workers, ...) — sizing this handle's allocators
                alike; ``None`` uses the server's defaults.
            shm: the shared-memory segment — ``"auto"`` picks one, a path forces
                it, ``None`` launches without one. Remembered for a GUI to map.
            transport: the carrier this handle talks over — ``"tcp"``
                (default), ``"udp"`` or ``"ws"`` (a ``--ws`` server). The
                boot-or-attach probe itself always rides UDP.
            verbose: server log verbosity (``1``/``2``/``3`` -> ``-v``/``-vv``/
                ``-vvv``; negative -> ``-q``).
            workers: shortcut for ``options.workers`` (DSP worker threads for
                parallel groups); it wins over a value set there. ``None``
                emits no flag.
            data_dir: the server's ``--data-dir``; ``None`` uses its default.
            server_args: raw CLI tokens appended **last** (they win over
                everything above) — an escape hatch for flags newer than this
                client; prefer `ServerOptions` fields.
            latency: seconds added to RT timetags (see the constructor).
            ready_timeout: seconds to wait for the server to answer.

        A server booted free-standing (not from within a `Session`) is adopted
        as the **default session's** server, first-wins: the first such boot sets
        ``clausters.default_session.server``, so ``Event().play()`` and
        ``clausters.play(...)`` find it with no session wiring. A later boot does
        not displace it, and an explicit `Session` never adopts.

        Returns:
            A booted `Server`; ``server.shm`` is the segment path (or ``None``).
        """
        from ..launch import ServerProcess

        extra = list(server_args)
        if workers is not None:
            extra = ["--workers", str(workers)] + extra
        proc = ServerProcess(options, shm=shm, verbose=verbose, data_dir=data_dir,
                             extra_args=extra, ready_timeout=ready_timeout).start()
        server = cls(proc.host, proc.port, latency=latency, options=options,
                     transport=transport)
        server._process = proc
        if _adopt_default and main.server is None:
            main.server = server
        return server

    @property
    def shm(self) -> "str | None":
        """The shared-memory segment path of the server this handle `boot`-ed, or
        ``None`` (attached, or booted without a segment). A GUI maps this."""
        return getattr(self._process, "shm", None)

    # ---- raw OSC: immediate and timed ----

    def send_msg(self, addr, *args):
        """Send one message. **A message has no time**: in a bundle it would
        carry the immediate timetag, and alone it means exactly that.

        The interface is the same one in real time and offline, and so is this —
        immediate is immediate. Logical time belongs to the **bundle** path:
        `send_bundle`, `Event.play`, and the patterns built on them. So creating
        a node with `send_msg` from inside a routine is an **error**, not
        something that behaves differently offline. Use it for what has no place
        in a timeline: sending defs, allocating buffers, opening the groups a
        piece is built on."""
        self.interface.send_msg(self.target, addr, *args)

    def send_bundle(self, *messages, delay_beats: float = 0.0, clock=None, at=None):
        """Emit a timetagged bundle of ``(addr, *args)`` messages at ``at``
        (default: the ambient `Moment`) plus ``delay_beats``, plus this
        server's `latency`.

        Inside a routine the moment is the routine's **exact logical beat** —
        the yield-accumulated one, not wall-clock now — so inter-event timing
        stays exact. Outside any routine it is wall-clock now, and the delay
        reads as seconds.

        What this adds to a plain OSC bundle is what belongs to *this* server:
        its `latency`, scheduling by absolute sample when the clock is anchored
        to the server's own, and accumulating into a score offline. For any
        other application, `clausters.base.OscDestination` sends standard
        bundles with the same logical timing."""
        when = (at if at is not None else Moment.current(clock)).at(delay_beats)

        if getattr(self.interface, "time_mode", "unix") == "score":
            # NRT: seconds from render start (logical, timebase-independent).
            self.interface.send_bundle(self.target, when.secs(), *messages)
            return

        timebase = getattr(when.clock, "timebase", None)
        if isinstance(timebase, SampleClockTimebase):
            # Anchored to the server's sample clock: schedule by absolute sample,
            # drift-free and sample-accurate, via /sched_at. The seconds->sample
            # rounding is the core's (shared with the server).
            origin = when.clock.pacing_origin or 0.0  # seconds in the sample timebase
            sample = timebase.sample_at(origin + when.secs() + self.latency)
            self._send_sched(sample, messages)
        else:
            # Wall clock: an NTP-timetagged bundle.
            self.interface.send_bundle(
                self.target, when.instant() + self.latency, *messages
            )

    def play_event(self, event):
        """Play a note `Event` as OSC: `/synth_new`
        then `/node_free` (or `gate 0`) after the sustain. The OSC side of the
        double dispatch — a MIDI destination renders the same event as note
        on/off. Returns the synth node id (or None for a rest).

        Release is by ``gate 0`` when the event sets ``has_gate`` **or** the
        instrument is the built-in ``"default"`` (which carries a gated,
        self-freeing envelope); otherwise it is a direct ``/node_free``.

        One timing path, whatever the context. Both messages go out as
        timetagged bundles at the ambient `Moment`: inside a routine that is
        its exact logical beat, so a sequence stays sample-tight; outside any
        clock it is wall-clock now, and the sustain reads as seconds
        (tempo 1.0) — so a bare ``Event().play()`` sounds now and frees itself
        without a `TempoClock`."""
        if event.get("type") == "rest":
            return None
        node_id = self._node_id()
        s_new = ("/synth_new", event["instrument"], node_id, int(event["add_action"]),
                 int(event["target"]), *event._control_args())
        # The built-in "default" instrument carries a gated envelope that frees
        # itself on release, so it is released by closing its gate even though
        # the global `has_gate` default is False (which keeps gate-less custom
        # defs freed directly). Any def can opt in per event with `has_gate`.
        gate_release = event.get("has_gate") or event["instrument"] == "default"
        release = (("/node_set", node_id, "gate", 0.0) if gate_release
                   else ("/node_free", node_id))
        self.send_bundle(s_new)
        self.send_bundle(release, delay_beats=event.sustain())
        return node_id

    def send_bundle_after(self, delay_secs: float, *messages):
        """Emit a timetagged bundle of ``(addr, *args)`` messages at wall-clock
        now + ``delay_secs`` (+ `latency`), ignoring whatever clock is in
        flight — the **clockless** entry point to `send_bundle`, for a delay
        that is a duration in seconds rather than a position in the music. In
        score (NRT) mode the delay is seconds from the render start."""
        self.send_bundle(*messages, at=Moment(None, delay_secs))

    def _send_sched(self, sample: int, messages):
        inner = _osclib.immediate_bundle(*[_osclib.message(*m) for m in messages])
        self.send_msg("/sched_at", _osclib.Int64(sample), inner)

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

    def _request_batch(self, addr, *args, reply: str, timeout: float = 5.0):
        """Sends `addr` and collects every `reply` message until the batch's
        ``/done`` terminator (the shape the introspection queries use, whose
        result is a variable number of messages). Returns a list of arg lists.

        Blocking, RT only — like every query here, never call it from a
        routine."""
        self.send_msg(addr, *args)
        out = []
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            packet = self.interface.recv(timeout)
            if packet is None:
                continue
            raddr, rargs = _osclib.decode(packet)
            if raddr == "/done" and rargs and str(rargs[0]) == addr:
                return out
            if raddr == "/fail" and rargs and str(rargs[0]) == addr:
                raise CommandError(f"{addr} failed: {rargs}")
            if raddr == reply:
                out.append(rargs)
        raise ReplyTimeout(f"no reply to {addr}")

    # ---- server introspection: what a running server actually holds ----

    def query_defs(self, *names, timeout: float = 5.0) -> "list[DefInfo]":
        """The defs the server holds, each with its control surface
        (``/def_query``). With `names`, details exactly those — an unknown one
        comes back with an empty ``family`` (see `DefInfo.exists`) rather than
        raising; with no argument, every loaded def of every family.

        The def store persists across restarts, so a server may well hold defs
        this client never sent: this is how you find out. Blocking, RT only —
        never call it from a routine."""
        rows = self._request_batch("/def_query", *[str(n) for n in names],
                                   reply="/def_query.reply", timeout=timeout)
        return [parse_def_info(r) for r in rows]

    def query_buffers(self, timeout: float = 5.0) -> "list[BufferInfo]":
        """Every **allocated** buffer with its shape (an argument-less
        ``/buffer_query``). Like `query_defs`, this reports what the server holds rather
        than what this client allocated. Blocking, RT only."""
        _, args = self.request("/buffer_query", timeout=timeout, expect=("/buffer_query.reply",))
        return parse_buffer_list(args)

    def query_ugens(self, *kinds, timeout: float = 5.0) -> "list[UgenInfo]":
        """The server's UGen catalog (``/ugen_query``): every kind with its named
        inputs, defaults and rate rules, or just `kinds` if given.

        This is the catalog **this** server was built with, which is why it is
        worth asking instead of assuming: a build without the ``synth`` feature
        has no UGens at all and returns an empty list (its defs would all be
        FaustDefs, whose box vocabulary is Faust's own and lives client-side).
        Blocking, RT only."""
        rows = self._request_batch("/ugen_query", *[str(k) for k in kinds],
                                   reply="/ugen_query.reply", timeout=timeout)
        return [parse_ugen_info(r) for r in rows]

    def query_info(self, timeout: float = 5.0) -> ServerInfo:
        """Asks the running server for its static configuration (RT only): bus
        counts, output/input channels, block size, sample rate and the
        boot-time pool sizes. Use it to size or check allocators against a
        server you did not launch; compare the result with `options`. The
        appended capacity fields degrade to the defaults against a server too
        old to report them."""
        _, args = self.request(
            "/server_query", timeout=timeout, expect=("/server_query.reply",)
        )

        def at(i, cast, default):
            return cast(args[i]) if i < len(args) else default

        return ServerInfo(
            audio_buses=int(args[0]),
            control_buses=int(args[1]),
            channels=int(args[2]),
            block_size=int(args[3]),
            nominal_sample_rate=float(args[4]),
            actual_sample_rate=float(args[5]),
            input_channels=at(6, int, 0),
            max_nodes=at(7, int, DEFAULT_MAX_NODES),
            max_buffers=at(8, int, DEFAULT_MAX_BUFFERS),
            max_graph_children=at(9, int, DEFAULT_MAX_GRAPH_CHILDREN),
            max_ugen_inputs=at(10, int, DEFAULT_MAX_UGEN_INPUTS),
            taps=at(11, int, 0),
            tap_frames=at(12, int, 0),
            max_frame=at(13, int, 65536),
        )

    # ---- node tree introspection (RT only) ----

    def query_tree(self, group=ROOT_NODE_ID, timeout: float = 5.0) -> Tree:
        """The node tree from `group` down (``/group_queryTree``) as a `Tree`:
        every entry is the same `NodeInfo` that `clausters.defs.Node.info`
        returns, so reading a subtree needs no follow-up query. This is the
        **structured** way to read the tree — never scrape the server's logs.

        ``print(tree)`` draws it indented. Blocking, RT only."""
        gid = group.id if hasattr(group, "id") else group
        addr, args = self.request("/group_queryTree", int(gid), 2,
                                  timeout=timeout, expect=("/group_queryTree.reply", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/group_queryTree failed: {args}")
        return parse_query_tree(args)

    def group_at(self, path: str, timeout: float = 5.0):
        """The group a path names (``/group_query``), as a
        `clausters.defs.Group` handle — or ``None`` when nothing answers to it.

        A path is the group names from the root down, ``/mixer/drums``; a group
        with no name contributes its id instead (``/1000/drums``), so every
        group is reachable whether it was labelled or not. Resolve once and keep
        the handle: the id is the identity, the path is how you found it, and a
        group that is renamed or freed leaves the handle pointing at the id it
        resolved to. Blocking, RT only."""
        addr, args = self.request("/group_query", str(path),
                                  timeout=timeout, expect=("/group_query.reply", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/group_query failed: {args}")
        node_id = int(args[1])
        return Group.from_id(node_id, self) if node_id >= 0 else None

    def dump_graph(self, group=ROOT_NODE_ID, timeout: float = 5.0) -> str:
        """The inferred bus graph of `group` as a human-readable string
        (``/group_dumpGraph``): what each child reads/writes and the current order.
        A debugging aid; for machine use prefer `query_tree`."""
        gid = group.id if hasattr(group, "id") else group
        addr, args = self.request("/group_dumpGraph", int(gid),
                                  timeout=timeout, expect=("/group_dumpGraph.reply", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/group_dumpGraph failed: {args}")
        return str(args[1])

    # ---- definitions ----

    def free_def(self, *names: str):
        """Removes defs from the server's def table by name (``/def_free``).

        A def is not freed by itself: in use it is *overwritten* by sending
        another under the same name. This is the table's own command, for
        reclaiming what a session no longer names."""
        self.send_msg("/def_free", *names)

    # ---- nodes ----

    def _node_id(self) -> int:
        """A free node id from the registry, with the recycling side-channel
        up: every id stays tracked until its ``/node_end`` returns it to the
        pool, so the client range never exhausts while nodes keep dying."""
        self._ensure_recycler()
        return self.nodes.alloc()

    def _ensure_recycler(self):
        """Starts the ``/node_end`` listener once per server handle: a dedicated
        `OscReceiver` registered with ``/server_notify 1`` **from its own socket**, so
        the server's node-lifecycle pushes land here whatever transport the
        command path uses (UDP, TCP, WS — notify registration is per source).
        Ids outside the client range (the server's auto/MIDI ranges, other
        clients) are ignored by `NodeIdAllocator.free`. Score
        (NRT) interfaces skip this: their registry is unbounded and an offline
        score has no live notifications."""
        if self._recycler is not None or \
                getattr(self.interface, "time_mode", "unix") == "score":
            return
        from ..base._oscinterface import OscReceiver

        def on_node_end(addr, args, when, src):
            if addr == "/node_end" and args:
                self.nodes.free(int(args[0]))
            elif addr == "/fail" and len(args) >= 3 and isinstance(args[2], int):
                # An engine rejection (duplicate id / full table) is async:
                # the node never existed, so no /node_end will come — reconcile
                # the in-flight id here instead of losing it.
                self.nodes.free(int(args[2]))

        recv = OscReceiver().start()
        recv.add(on_node_end)
        recv.send(self.target, "/server_notify", 1)
        self._recycler = recv

    # ---- bus and tap subscriptions (one per client, over a set) ----

    def stream_buses(self, period_ms: int, *buses, timeout: float = 5.0):
        """Subscribes this client to a periodic ``/bus_set`` snapshot of the
        given control buses (``/bus_stream``): the server sends one snapshot
        immediately and then one every ``period_ms`` (floor 10 ms, at most 128
        buses) with no further requests -- the network counterpart of reading
        the shared-memory segment, e.g. for meters over WebSocket. One
        subscription per client, replaced on each call; ``period_ms <= 0`` (or
        no buses) cancels it. Receive the snapshots with an `OscFunc` on
        ``/bus_set``. Blocks on the ``/done`` ack."""
        indices = [b.index if isinstance(b, Bus) else int(b) for b in buses]
        return self.request("/bus_stream", int(period_ms), *indices,
                            timeout=timeout, expect=("/done", "/fail"))

    def stream_taps(self, period_ms: int, frames: int, *buses, timeout: float = 5.0):
        """Subscribes this client to a periodic ``/bus_tapStream.reply`` snapshot of the
        given audio **buses** (``/bus_tapStream``): every ``period_ms`` (floor
        10 ms) the server sends, per bus, its **newest** ``frames`` samples as
        ``/bus_tapStream.reply bus endPosition blob`` -- the bus, its stream position
        (total samples recorded) at the window's end, and the window as raw
        little-endian ``float32``. The network counterpart of reading the
        samples out of shared memory, e.g. for a browser oscilloscope or
        headless capture.

        The subscription **is** the watch: it starts recording each bus it
        lists and stops when it is replaced, cancelled or the connection dies,
        so a streaming client never calls `watch` itself. ``frames`` is clamped
        to 8192 and to half the server's ring; at most 8 buses per
        subscription; one subscription per client, replaced on each call;
        ``period_ms <= 0`` (or no buses) cancels. Receive the snapshots with an
        `OscFunc` on ``/bus_tapStream.reply``. Blocks on the ``/done`` ack."""
        return self.request("/bus_tapStream", int(period_ms), int(frames),
                            *[b.index if isinstance(b, Bus) else int(b) for b in buses],
                            timeout=timeout, expect=("/done", "/fail"))

    # ---- buffers ----

    def _bulk_chunk(self, timeout: float) -> int:
        """Samples per bulk round-trip for this interface: datagram-bounded
        transports keep the classic 1024; a stream transport uses the frame
        ceiling from ``/server_query`` (queried once and cached), minus headroom
        for the reply's OSC envelope."""
        if not isinstance(self.interface, (OscTcpInterface, OscWsInterface)):
            return 1024
        if self._max_frame is None:
            try:
                self._max_frame = self.query_info(timeout=timeout).max_frame
            except ReplyTimeout:
                return 1024  # no reply: stay conservative, retry next call
        return max(1024, (self._max_frame - 256) // 4)

    # ---- offline render (NRT interface only) ----

    def render(self, sample_rate: float = 48_000.0, channels: int = 2,
               workers: int = 0, path=None, seed: int | None = None,
               sample_format: str = "float") -> "RenderStats":
        """Renders the accumulated score (the interface must be an
        `OscNrtInterface`). Schedule a closing bundle (e.g. ``/node_free 0``)
        so the render has a defined duration. ``workers`` adds DSP threads
        for the score's parallel groups — bit-identical, only faster.

        Always returns a `clausters.render.RenderStats`. **``path`` chooses
        where the audio goes, not whether there is any**:

        - without it the samples come back in ``stats.samples``, interleaved;
        - with it the **server** writes the file and ``stats.samples`` is
          ``None``. Read it back with `clausters.render.read_soundfile`.

        The file is written by the server, not here: the score goes to the
        ``clausters --nrt`` renderer, which streams straight to disk. That is
        why nothing has to cross into Python — a long bounce never
        materializes millions of floats just to be written out — and why
        ``sample_format`` (``"float"``, ``"int24"``, ``"int16"``) is
        available at all. It also means the binary must be findable, the same
        way `clausters.launch` finds it.

        ``seed`` starts the render's stochastic UGens. Left ``None`` the
        render draws a fresh one, so a score with noise in it is a new take
        every time — a random process is unpredictable first. The seed it used
        comes back in ``stats.seed``; pass that back here to replay exactly
        that take.
        """
        if not isinstance(self.interface, OscNrtInterface):
            raise RuntimeError("render() needs a Server with an OscNrtInterface")
        return self.interface.render(sample_rate=sample_rate, channels=channels,
                                     workers=workers, path=path, seed=seed,
                                     sample_format=sample_format)

    # ---- server control ----

    def notify(self, flag: bool = True, timeout: float = 5.0):
        return self.request("/server_notify", 1 if flag else 0, timeout=timeout, expect=("/done",))

    def status(self, timeout: float = 5.0):
        _, args = self.request("/server_status", timeout=timeout, expect=("/server_status.reply",))
        return args

    def sync(self, timeout: float = 5.0) -> int:
        """The async barrier (scsynth ``/server_sync``): sends ``/server_sync id`` and blocks
        until the server answers ``/server_sync.reply id``, which it does only once every
        async command sent earlier -- Faust/SynthDef compiles, buffer jobs --
        has completed. Use it after a ``wait=False`` def send or
        buffer alloc. RT only (in NRT the renderer
        already serializes async work at time 0). Returns the id used.

        **Blocking — never call from a routine.** This (and any ``wait=True``)
        blocks the calling thread on a reply: fine on your own thread, but it
        would freeze the clock thread if called from inside a routine generator
        (see `Routine`). It also polls the socket
        synchronously; a non-blocking, notification-driven barrier you can
        ``yield`` from a routine is future work (``OSCFunc``)."""
        self._sync_counter += 1
        sync_id = self._sync_counter
        self.request("/server_sync", sync_id, timeout=timeout, expect=("/server_sync.reply",))
        return sync_id

    def quit(self):
        self.send_msg("/server_quit")

    def sample_clock(self, window: int = 64, timeout: float = 2.0):
        """A sample-clock reader for this server: an `EmbedSampleClock` when the
        server is in-process (the embed interface exposes the counter directly —
        no socket, no round trips), otherwise a `UdpSampleClock` tracking it
        over UDP. Pass its ``.timebase()`` to a ``TempoClock`` to anchor timing
        to the server and schedule by ``/sched_at``."""
        from ..base._oscinterface import OscEmbedInterface
        from .clocksync import EmbedSampleClock, UdpSampleClock

        if isinstance(self.interface, OscEmbedInterface):
            return EmbedSampleClock(self.interface.server)
        return UdpSampleClock(self, window=window, timeout=timeout)

    def transport(self, timeout: float = 5.0):
        """The server's shared transport grid (``/transport_query``) as
        ``(origin_sample, tempo)``, or ``None`` if none is set. The grid lets
        several clients phase-align on the master clock; join it from a clock
        with `clausters.base.clock.TempoClock.join_transport`. RT only."""
        _, args = self.request("/transport_query", timeout=timeout, expect=("/transport_query.reply",))
        origin, tempo, defined = int(args[0]), float(args[1]), int(args[2])
        return (origin, tempo) if defined else None

    def set_transport(self, origin_sample: int, tempo: float, timeout: float = 5.0):
        """Define the server's shared transport grid (``/transport_set``): beat 0 at
        ``origin_sample`` on the sample clock, advancing at ``tempo`` beats per
        second. One client (the conductor) sets it; the others
        `join_transport`. Last writer wins. Defining the grid resets the rolling
        state to stopped at position 0."""
        addr, args = self.request("/transport_set", _osclib.Int64(int(origin_sample)), float(tempo),
                                  timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/transport_set failed: {args}")
        return self

    def transport_state(self, timeout: float = 5.0):
        """The full shared transport state as a dict ``{origin_sample, tempo,
        playing, position}``, or ``None`` if no grid is defined. ``playing`` is
        whether the transport is rolling and ``position`` the song-position beat
        (where play starts, or where a stopped transport sits). A
        `clausters.seq.timeline.Playhead` follows this with `follow_transport`.
        RT only."""
        _, args = self.request("/transport_query", timeout=timeout, expect=("/transport_query.reply",))
        if not int(args[2]):
            return None
        return {
            "origin_sample": int(args[0]),
            "tempo": float(args[1]),
            "playing": bool(int(args[3])),
            "position": float(args[4]),
        }

    def transport_play(self, position: "float | None" = None, timeout: float = 5.0):
        """Start the shared transport rolling (``/transport_play``). With
        ``position`` playback starts from that song-position beat; without it,
        from where it last stopped or located. The server broadcasts the change
        to every `/server_notify` client, so all playheads following the transport roll
        together. Needs a grid defined (`set_transport`)."""
        extra = [float(position)] if position is not None else []
        addr, args = self.request("/transport_play", *extra,
                                  timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/transport_play failed: {args}")
        return self

    def transport_stop(self, timeout: float = 5.0):
        """Stop the shared transport (``/transport_stop``); every following
        playhead halts. Broadcast to `/server_notify` clients."""
        addr, args = self.request("/transport_stop", timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/transport_stop failed: {args}")
        return self

    def transport_locate(self, position: float, timeout: float = 5.0):
        """Set the shared transport's song position (``/transport_locate``) —
        where play starts, or where it seeks to while playing. Every following
        playhead locates to it. Broadcast to `/server_notify` clients."""
        addr, args = self.request("/transport_locate", float(position),
                                  timeout=timeout, expect=("/done", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/transport_locate failed: {args}")
        return self

    def close(self):
        """Close the communication interface (and the ``/node_end`` recycling
        listener) and, if this handle `boot`-ed a server process, stop it
        too."""
        if self._recycler is not None:
            self._recycler.close()
            self._recycler = None
        self.interface.close()
        if self._process is not None:
            self._process.close()
            self._process = None
