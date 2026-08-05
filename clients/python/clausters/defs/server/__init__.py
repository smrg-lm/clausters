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

Where things live: this module holds the `Server` itself — the interface, the
allocators, the raw OSC paths and the server's own lifecycle. Beside it,
`options` (the configuration it is booted with and the configuration it
reports), `queries` (what a running server holds), `transport` (the shared beat
grid) and `streams` (the subscriptions the server pushes).
"""

import time

from ... import _native
from ...config import client_config
from ...base import _osclib
from ...errors import CommandError, ReplyTimeout
from ...base.main import main
from ...base.moment import Moment
from ...base.netaddr import NetAddr
from ...base._oscinterface import (OscNrtInterface, OscTcpInterface, OscUdpInterface,
                                   OscWsInterface)
from ...base.timebase import SampleClockTimebase
from ..bus import AudioBusAllocator, ControlBusAllocator
from ..buffer import BufferAllocator
from ..node import NodeIdAllocator
from ...base.ids import WHOLE as WHOLE_SHARE
from .options import (
    DEFAULT_AUDIO_BUSES,
    DEFAULT_CONTROL_BUSES,
    DEFAULT_MAX_BUFFERS,
    DEFAULT_MAX_GRAPH_CHILDREN,
    DEFAULT_MAX_NODES,
    DEFAULT_MAX_UGEN_INPUTS,
    DEFAULT_SAMPLE_RATE,
    DEFAULT_TAPS,
    DEFAULT_TAP_FRAMES,
    ServerInfo,
    ServerOptions,
)
from .queries import ServerQueries
from .streams import ServerStreams
from .transport import ServerTransport

# The package's public surface: `Server` plus what its configuration is made
# of. The names re-exported here are the ones the module answered to before it
# became a package, so `from clausters.defs.server import ...` is unchanged.
__all__ = [
    "Server",
    "ServerInfo",
    "ServerOptions",
    "ServerQueries",
    "ServerStreams",
    "ServerTransport",
    "DEFAULT_AUDIO_BUSES",
    "DEFAULT_CONTROL_BUSES",
    "DEFAULT_SAMPLE_RATE",
    "DEFAULT_MAX_NODES",
    "DEFAULT_MAX_BUFFERS",
    "DEFAULT_MAX_GRAPH_CHILDREN",
    "DEFAULT_MAX_UGEN_INPUTS",
    "DEFAULT_TAPS",
    "DEFAULT_TAP_FRAMES",
]


class Server(ServerQueries, ServerStreams, ServerTransport):
    def __init__(self, host: "str | None" = None, port: "int | None" = None, interface=None,
                 latency: "float | None" = None, options: "ServerOptions | None" = None,
                 transport: "str | None" = None, timeout: float = 5.0,
                 share=None):
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
        #: Whether this handle built its own interface, and may therefore have
        #: a process behind it that `boot` can start.
        self._own_carrier = interface is None
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
        #: seconds a blocking round trip waits for its reply before raising
        #: `clausters.errors.ReplyTimeout`. Every query, every ``wait=True``
        #: command and every barrier reads it, so a slow machine (or a
        #: deliberately impatient script) is one assignment away rather than an
        #: argument repeated at each call site. A ``timeout=`` argument still
        #: wins wherever one is passed.
        self.timeout = timeout
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
        #: which slice of the server's client id space this handle allocates
        #: from — the whole of it unless a second client shares the server
        #: (`clausters.base.IdShare`). Every space is sliced the same way, so a
        #: handle is one share of everything rather than of one pool.
        self.share = WHOLE_SHARE if share is None else share
        part = _native.node_id_partition(self.options.max_nodes)
        score = getattr(self.interface, "time_mode", "unix") == "score"
        self.nodes = NodeIdAllocator(
            part["client_base"], None if score else part["client_capacity"],
            self.share)
        self.audio_buses = AudioBusAllocator(size=self.options.audio_buses,
                                             share=self.share)
        self.control_buses = ControlBusAllocator(size=self.options.control_buses,
                                                 share=self.share)
        self.buffers = BufferAllocator(size=self.options.max_buffers, share=self.share)
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

    def boot(self, *, shm="auto", verbose: int = 0, workers: "int | None" = None,
             data_dir=None, server_args=(), ready_timeout: float = 10.0,
             adopt_default: bool = True) -> "Server":
        """Start the server **this handle is for**, and return ``self``.

        A `Server` is a handle: constructing one runs nothing and reaches
        nothing, which is what makes it cheap to build one before there is
        anything to talk to. This is the verb that brings up what it points at,
        and what that means is the carrier's to say —

        - the default carriers (TCP/UDP/WS) have a process behind them, so this
          spawns the standalone ``clausters`` server and waits until it
          answers, at **this handle's own address** — it does not move, and a
          handle pointing where a booted server cannot be raises rather than
          launching one elsewhere;
        - an offline or in-process carrier has nothing to start, and this is a
          no-op rather than an error — an NRT score is already "up".

        Pair it with `close`, which stops a process this booted. The server's
        *configuration* belongs to the constructor (``options=``), since it
        sizes this handle's allocators too; what belongs here is the launch
        itself.

        Args:
            shm: the shared-memory segment — ``"auto"`` picks one, a path
                forces it, ``None`` launches without one. Remembered for a GUI
                to map (`shm`).
            verbose: server log verbosity (``1``/``2``/``3`` -> ``-v``/``-vv``/
                ``-vvv``; negative -> ``-q``).
            workers: shortcut for ``options.workers`` (DSP worker threads for
                parallel groups); it wins over a value set there. ``None``
                emits no flag.
            data_dir: the server's ``--data-dir`` for its def store.
            server_args: extra CLI tokens, appended last.
            ready_timeout: seconds to wait for the server to answer.
            adopt_default: make this the default session's server when there is
                none, so the free-standing verbs resolve it. A server already
                adopted is not displaced.

        Returns: ``self``, so ``Server(...).boot()`` reads as one expression.
        """
        if not self._own_carrier:
            return self                  # an offline or in-process carrier
        from ...launch import ServerProcess

        # The address this handle was built with is the address it keeps: the
        # server binary takes no port flag and always listens on the default,
        # so a handle pointing anywhere else names a server that a boot cannot
        # produce. Saying so beats launching one somewhere the caller did not
        # ask for and quietly moving the handle to meet it.
        if (self.target.host, self.target.port) != (ServerProcess.host, ServerProcess.port):
            raise ValueError(
                f"this handle points at {self.target.host}:{self.target.port}, "
                f"and a booted server always listens on {ServerProcess.host}:"
                f"{ServerProcess.port} (the binary takes no port flag, so one "
                "machine runs one at a time). Build the handle without an "
                "address to boot one, or connect to the server already running "
                "there.")
        extra = list(server_args)
        if workers is not None:
            extra = ["--workers", str(workers)] + extra
        self._process = ServerProcess(
            self.options, shm=shm, verbose=verbose, data_dir=data_dir,
            extra_args=extra, ready_timeout=ready_timeout).start()
        if adopt_default and main.server is None:
            main.server = self
        return self

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

    def request(self, addr, *args, timeout: "float | None" = None, expect=None):
        """Sends a message and returns the first matching reply ``(addr, args)``
        (RT only; the interface must reply). ``expect`` filters reply addresses.

        This and `_request_batch` are the two places a ``timeout`` is finally
        read, which is why they are the only two that resolve ``None`` against
        the handle's `timeout` — everything above just passes the argument down
        untouched."""
        timeout = self.timeout if timeout is None else timeout
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

    def _request_batch(self, addr, *args, reply: str, timeout: "float | None" = None):
        """Sends `addr` and collects every `reply` message until the batch's
        ``/done`` terminator (the shape the introspection queries use, whose
        result is a variable number of messages). Returns a list of arg lists.

        Blocking, RT only — like every query here, never call it from a
        routine."""
        timeout = self.timeout if timeout is None else timeout
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
        from ...base._oscinterface import OscReceiver

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

    # ---- buffers ----

    def _bulk_chunk(self, timeout: "float | None" = None) -> int:
        """Samples per bulk round-trip for this interface: datagram-bounded
        transports keep the classic 1024; a stream transport uses the frame
        ceiling from ``/server_query`` (queried once and cached), minus headroom
        for the reply's OSC envelope."""
        if not getattr(self.interface, "stream", False):
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

    def notify(self, flag: bool = True, timeout: "float | None" = None):
        return self.request("/server_notify", 1 if flag else 0, timeout=timeout, expect=("/done",))

    def status(self, timeout: "float | None" = None):
        _, args = self.request("/server_status", timeout=timeout, expect=("/server_status.reply",))
        return args

    def _barrier(self, timeout: "float | None" = None) -> None:
        """`sync`, but a ``/fail`` from the work being waited on ends the wait
        instead of being dropped.

        This is what a **batched** async send needs. Waiting for each command's
        own ``/done`` costs one round trip per command, which is what makes a
        chunked bulk write (`clausters.defs.Buffer.set_samples`) slow in
        proportion to its length rather than to its size; firing the batch and
        closing it with one barrier costs one round trip for the whole of it.
        What that would otherwise give up is the error, since a chunk's
        ``/fail`` arrives while nobody is listening — so the barrier listens for
        both, and the first one wins.
        """
        self._sync_counter += 1
        addr, args = self.request("/server_sync", self._sync_counter, timeout=timeout,
                                  expect=("/server_sync.reply", "/fail"))
        if addr == "/fail":
            raise CommandError(f"{args[0] if args else 'a command'} failed: {args[1:]}")

    def sync(self, timeout: "float | None" = None) -> int:
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
        """Stop the server (``/server_quit``).

        The wire command, so it is the server that stops rather than this end
        of it — `close` is the other half, releasing the interface and any
        process this handle booted.

        What getting another one costs depends on where it was: a launched
        process is booted again from here.
        """
        self.send_msg("/server_quit")

    def sample_clock(self, window: int = 64, timeout: float = 2.0):
        """A sample-clock reader for this server: an `EmbedSampleClock` when the
        server is in-process (the embed interface exposes the counter directly —
        no socket, no round trips), otherwise a `UdpSampleClock` tracking it
        over UDP. Pass its ``.timebase()`` to a ``TempoClock`` to anchor timing
        to the server and schedule by ``/sched_at``."""
        from ...base._oscinterface import OscEmbedInterface
        from ..clocksync import EmbedSampleClock, UdpSampleClock

        if isinstance(self.interface, OscEmbedInterface):
            return EmbedSampleClock(self.interface.server)
        return UdpSampleClock(self, window=window, timeout=timeout)

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

