"""The client-owned server configuration, and what a running server reports.

`ServerOptions` is the enumeration of every option a launched server takes --
it sizes the handle's allocators and builds the process's command line.
`ServerInfo` is the answer to `Server.query_info`: what the server it is
talking to was actually built and booted with, which is not always the same
thing. `ServerStatus` is the answer to `Server.status`: what it is doing right
now, which changes between two calls.
"""

from dataclasses import dataclass, field

from ...config import server_config


# Server defaults, mirroring the Rust server's `DEFAULT_AUDIO_BUSES` /
# `DEFAULT_CONTROL_BUSES` (each a power of two, with no ceiling) and
# `--sample-rate`.
# They live here, on the server-config object, not in the bus module: how many
# buses exist is the server's property, and these are only the fallback when the
# caller does not specify. The bus allocators carry no defaults of their own.
DEFAULT_AUDIO_BUSES = 1024
DEFAULT_CONTROL_BUSES = 16384
DEFAULT_SAMPLE_RATE = 48000
# Boot-time pre-allocated pool sizes, mirroring the Rust server's `Limits`
# defaults (`--max-nodes`/`--max-buffers`/`--max-graph-children`/
# `--max-ugen-inputs`). 256 is the hard ceiling on UGen inputs -- a compile-time
# invariant, unlike the bus counts, which are configured resources with none.
# Hardware channels default to the device's outputs (``None`` = no flag) and no
# live input.
DEFAULT_MAX_NODES = 8192
DEFAULT_MAX_BUFFERS = 4096
DEFAULT_MAX_GRAPH_CHILDREN = 512
# The transports a server boots with when no ``--transports`` says otherwise.
DEFAULT_TRANSPORTS = 16
# The event buses a server boots with when no ``--event-buses`` says otherwise.
DEFAULT_EVENT_BUSES = 64
DEFAULT_MAX_UGEN_INPUTS = 256
# The audio-tap region (`--taps`/`--tap-frames`): pre-allocated sample rings
# an audio bus can be routed into with `Server.tap`, read by a GUI host out of
# shared memory or streamed with `Server.stream_taps`. 0 taps disables the
# region; the ring capacity is rounded up to a power of two by the server.
DEFAULT_TAPS = 8
DEFAULT_TAP_FRAMES = 16384


@dataclass
class ServerOptions:
    """Client-owned server configuration, the way SuperCollider's
    ``ServerOptions`` works -- the one enumeration of every option a launched
    server takes (`Server.boot` / `Session.live` accept it as ``options``).
    Two families of fields, with different defaulting:

    - **Sizing** (buses, pools, taps, hardware I/O): these also size the
      client's allocators, so their defaults read the same config file the
      server reads and `args` always emits them -- the launched server and
      this object agree by construction. Verify a running server with
      `Server.query_info`.
    - **Behavior** (``workers``, ``tcp``, ``ws``, ``midi``, ``persist``,
      ``max_frame``, ``max_stream_buses``, ``max_clients``, ``transports``,
      ``event_buses``, ``pin``):
      server-only, no client-side
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
    #: Accepted inputs per UGen (clamped to 256 by the server).
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
    #: forces it on at the default port, a number moves it, and a string binds
    #: it (``--tcp [addr:]port``: ``"0.0.0.0:57110"``, ``"0.0.0.0"``). It is on
    #: by default server-side; ``None`` leaves that as is. Every carrier binds
    #: loopback unless the address says otherwise.
    tcp: "bool | int | str | None" = None
    #: OSC over WebSocket: ``True`` opens it at the default port (57120), a
    #: number picks the port, a string binds it (``--ws [addr:]port``, e.g.
    #: ``"0.0.0.0:57120"`` for a browser on another machine). There is no off
    #: flag -- leave ``None`` and keep it out of the server's config to run
    #: without it.
    ws: "bool | int | str | None" = None
    #: Virtual MIDI input: ``True`` opens it with the default name, a string
    #: names the port (``--midi [name]``).
    midi: "bool | str | None" = None
    #: Def persistence: ``False`` disables it for this run (``--no-persist``).
    #: There is no force-on flag -- ``True`` is expressible only by keeping
    #: ``persist = false`` out of the server's config.
    persist: "bool | None" = None
    #: Largest OSC frame on the stream transports, bytes (``--max-frame``).
    max_frame: "int | None" = None
    #: Bus indices one ``/bus_stream`` subscription may list
    #: (``--max-stream-buses``); the server's default is generous and a client
    #: reads its own effective ceiling from `ServerInfo.max_stream_buses`.
    max_stream_buses: "int | None" = None
    #: Concurrent stream clients, TCP + WebSocket (``--max-clients``).
    max_clients: "int | None" = None
    #: Independent transports, each with its own play, pause, locate, loop
    #: and governed group (``--transports``); the server boots 16 by default.
    #: Transport 0 is the one addressed by number, and a multitrack's; an
    #: audio editor and every sequence playing on its own each take one of
    #: the rest, which a GUI host shares when there is one: raise it to play
    #: more sequences at once. `ServerInfo.transports` reads
    #: back what it has.
    transports: "int | None" = None
    #: Event buses, where MIDI, event lanes and `Server.bus_event` write notes
    #: and a `clausters.defs.Voices` or plugin node reads them
    #: (``--event-buses``); the server boots 64 by default, the top half the
    #: MIDI bindings'. `ServerInfo.event_buses` reads back what it has.
    event_buses: "int | None" = None
    #: The audio host/backend by name (``--host``): ``"jack"``, ``"alsa"``,
    #: ``"pipewire"``, ``"coreaudio"``, ``"wasapi"`` -- whatever the build
    #: has. ``None`` takes the platform's default.
    host: "str | None" = None
    #: The output device by name (``--device``), exact or a substring of one.
    #: Under JACK it is also the client name the ports carry.
    device: "str | None" = None
    #: The input device by name (``--input-device``). Capture belongs to
    #: whoever holds this device.
    input_device: "str | None" = None
    #: What the server calls itself to the audio graph (``--client-name``), so
    #: its ports come back under the same name after a restart and a patchbay
    #: can reconnect them.
    client_name: "str | None" = None
    #: CPU affinity list (``--pin``): first CPU for the audio callback, the
    #: rest round-robin over the DSP workers; ``"fast"`` keeps them all on the
    #: fastest cores of a hybrid CPU. Experimental, Linux only, and only
    #: accepted by a server built with the ``rtprio`` feature.
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
        # server's own config). `is True`/`is False` first -- a bool is an
        # int, so the port/number branch must come after.
        if self.workers is not None:
            flags += ["--workers", str(self.workers)]
        # Devices and the name the audio graph knows this server by: a server
        # meant to be routed by hand is named, or its ports come back under a
        # new name every run and the routing is lost with them.
        for flag, value in (("--host", self.host), ("--device", self.device),
                            ("--input-device", self.input_device),
                            ("--client-name", self.client_name)):
            if value is not None:
                flags += [flag, str(value)]
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
        if self.max_stream_buses is not None:
            flags += ["--max-stream-buses", str(self.max_stream_buses)]
        if self.max_clients is not None:
            flags += ["--max-clients", str(self.max_clients)]
        if self.transports is not None:
            flags += ["--transports", str(self.transports)]
        if self.event_buses is not None:
            flags += ["--event-buses", str(self.event_buses)]
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
    #: How many control buses one ``/bus_stream`` subscription may list
    #: (``--max-stream-buses``), **as it applies to this client's carrier**:
    #: the server's configured ceiling clamped by what one reply can carry
    #: over the transport asking. A subscription is one client's whole live
    #: picture -- a page of many canvases asks for a bus per meter -- so a
    #: client that draws a lot reads the number here instead of assuming one.
    #: Falls back to the historical 128 against a server too old to report it.
    max_stream_buses: int = 128
    #: How many transports the server has (``--transports``), addressed
    #: ``0`` to ``transports - 1`` (`Server.transport_at`). Falls back to the
    #: one transport a server too old to report it had.
    transports: int = 1
    #: How many event buses the server has (``--event-buses``). Falls back to
    #: the default against a server too old to report it.
    event_buses: int = DEFAULT_EVENT_BUSES

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
            f"  stream  {self.max_stream_buses} buses per /bus_stream",
            f"  transports {self.transports}",
            f"  event buses {self.event_buses}",
        ])


#: What a figure that comes from timing reads as when the server has no clock
#: to time with: an engine in a page is the case.
_NOT_TIMED = "not available (this server cannot time itself)"


@dataclass
class Load:
    """One role's share of the server's time, as ``/server_load`` reports it
    (the result of `Server.load`).

    The server times **itself** rather than asking the operating system, so
    the reading is the same on every platform: each piece of work brackets
    itself with a monotonic clock and adds the elapsed time to its role's
    slot. What is measured is therefore the *work* -- a block, a stage of a
    parallel group, a serving turn, an NRT job, a compilation -- and not the
    thread that ran it.

    ``busy`` is cumulative since the server booted, which is what lets several
    clients poll the same server without stealing each other's window:
    ``share`` is computed here, from the previous reading this `Server` took.

    ``busy`` is time the work was **in progress**, not per cent of a core. A
    DSP worker spinning for its next stage is burning a core and is idle by
    this reading, which is the honest answer to how much of the block budget a
    stage took -- a system profiler answers the other question.

    **A server with no clock reports no time.** ``busy`` is then ``None``, and
    so is ``share``: not zero seconds, which would read as an idle role, but
    no reading. ``calls`` is still there, since a run is counted and not
    timed. An engine in a page is that server -- it runs in an AudioWorklet,
    whose scope has no clock fine enough to time a block.
    """

    #: ``audio`` (the callback's block), ``dsp`` (a worker thread), ``net``
    #: (the serving turn), ``nrt`` (the job queue), ``faust`` (compiling), or
    #: ``disk-in`` and ``disk-out`` (streaming: every ``DiskIn`` node's reading
    #: together, and every ``DiskOut`` node's writing).
    role: str
    #: Which instance, for a role that has several. Only ``dsp`` does today.
    index: int
    #: Seconds of work since the server booted, or ``None`` from a server that
    #: cannot time itself.
    busy: "float | None"
    #: Times the work ran: blocks, stages, turns, jobs, compilations.
    calls: int
    #: Fraction of wall time busy **since this client's previous call**, or
    #: ``None`` on the first one, which has no interval to measure -- and on
    #: every one from a server that cannot time itself.
    share: "float | None" = None

    @property
    def name(self) -> str:
        """``role`` for a single-instance role, ``role index`` otherwise."""
        return self.role if self.role != "dsp" else f"{self.role} {self.index}"


def format_load(rows: "list[Load]") -> str:
    """The readable block `Server.load`'s rows print as, one line per role.

    A free function rather than a method because the reading is the *list*,
    and a list has no ``__str__`` of its own to give it.
    """
    if rows and all(row.busy is None for row in rows):
        # No clock on the server: say so once, and keep what was counted.
        lines = [f"server load: {_NOT_TIMED}"]
        for row in rows:
            lines.append(f"  {row.name:<8} {row.calls} calls")
        return "\n".join(lines)
    lines = ["server load"]
    for row in rows:
        share = "     -" if row.share is None else f"{row.share * 100:5.1f}%"
        busy = "        -  " if row.busy is None else f"{row.busy:9.3f} s"
        lines.append(f"  {row.name:<8} {share}  {busy}  {row.calls} calls")
    return "\n".join(lines)


@dataclass
class ProfileRow:
    """One row of a `Profile`: a phase, a synth, a def, a command, a reply or
    an NRT job, and what it cost over the profile's window.

    One shape for the six sections, so a reader goes through them alike:
    ``name`` says what the row is and ``count`` how many times it ran or went
    by; the fields a section has no use for keep their defaults.
    """

    #: The phase (``nodes``), the def of a synth or the def itself, the OSC
    #: address of a command or a reply, or the command of an NRT job.
    name: str
    #: Seconds of work over the window, or ``None`` where there is none to
    #: measure -- a reply, or any row from a server that cannot time itself.
    busy: "float | None"
    #: Blocks, slices, turns, runs, messages or jobs over the window.
    count: int
    #: ``busy`` as a fraction of the window's wall time.
    share: "float | None" = None
    #: The role a phase belongs to (``audio`` or ``net``).
    role: str = ""
    #: The node id of a synth row.
    node: "int | None" = None
    #: How many synths of a def ran in the window.
    nodes: int = 0
    #: Bytes a command or a reply carried over the window.
    bytes: int = 0
    #: Seconds an NRT job waited in the queue before running.
    waited: "float | None" = None


@dataclass
class Profile:
    """What the server spent its time on over a window, in detail (the result
    of `Server.profile`).

    `Server.load` says which role spent the time; this says what inside it:
    which phase of the block or of the serving turn, which synth and which
    def, which command, how much each reply carried and how long each NRT job
    ran and waited. Each section is sorted costliest first and lists only
    what moved during the window.

    **What the window cost** is the difference between two of the server's
    cumulative reports, so several clients can profile one server at once.
    The synth timing is the one switch: the server runs it only while some
    client asks, because it costs the audio thread two clock reads per synth
    per block. ``timed_synths`` says whether it was on for the whole window.
    """

    #: The window, in seconds of the server's clock, or ``None`` from a server
    #: that cannot time itself.
    seconds: "float | None"
    #: Whether every synth was timed throughout the window.
    timed_synths: bool
    #: Where the block (``audio``) and the serving turn (``net``) went.
    phases: "list[ProfileRow]"
    #: The synths still in the tree at the end, with their node ids.
    synths: "list[ProfileRow]"
    #: Every def whose synths ran, those freed during the window included.
    defs: "list[ProfileRow]"
    #: Every OSC address the server received.
    commands: "list[ProfileRow]"
    #: Every OSC address the server sent, pushes and streams included.
    replies: "list[ProfileRow]"
    #: Every command that ran on the NRT queue.
    jobs: "list[ProfileRow]"


#: The fields of each `/server_profile.reply` section, in order.
_PROFILE_WIDTHS = (4, 4, 4, 4, 3, 4)


def _profile_sections(args) -> "tuple[float | None, bool, list[list[list]]]":
    """`/server_profile.reply`'s arguments cut into its six sections."""
    uptime = None if args[0] is None else float(args[0])
    at = 2
    sections = []
    for width in _PROFILE_WIDTHS:
        n = int(args[at])
        at += 1
        sections.append([list(args[at + width * i:at + width * (i + 1)]) for i in range(n)])
        at += width * n
    return uptime, bool(args[1]), sections


def _profile_difference(first, second) -> Profile:
    """The `Profile` of the window between two `/server_profile.reply`
    argument lists."""
    start, _, before = _profile_sections(first)
    end, timed, after = _profile_sections(second)
    seconds = None if start is None or end is None else end - start

    def seconds_of(value):
        return None if value is None else float(value)

    def less(now, was):
        if now is None:
            return None
        return now - (was or 0.0)

    def share(busy):
        if busy is None or not seconds:
            return None
        return busy / seconds

    def section(index, key, build):
        was = {key(row): row for row in before[index]}
        rows = []
        for row in after[index]:
            made = build(row, was.get(key(row)))
            if made.count > 0 or (made.busy or 0.0) > 0.0 or made.bytes > 0:
                made.share = share(made.busy)
                rows.append(made)
        rows.sort(key=lambda r: (-(r.busy or 0.0), -r.bytes, -r.count, r.name))
        return rows

    def phase(row, was):
        busy = less(seconds_of(row[2]), seconds_of(was[2]) if was else None)
        return ProfileRow(name=str(row[1]), role=str(row[0]), busy=busy,
                          count=int(row[3]) - (int(was[3]) if was else 0))

    def synth(row, was):
        busy = less(seconds_of(row[2]), seconds_of(was[2]) if was else None)
        return ProfileRow(name=str(row[1]), node=int(row[0]), busy=busy,
                          count=int(row[3]) - (int(was[3]) if was else 0))

    def definition(row, was):
        busy = less(seconds_of(row[1]), seconds_of(was[1]) if was else None)
        return ProfileRow(name=str(row[0]), busy=busy,
                          count=int(row[2]) - (int(was[2]) if was else 0),
                          nodes=int(row[3]) - (int(was[3]) if was else 0))

    def command(row, was):
        busy = less(seconds_of(row[1]), seconds_of(was[1]) if was else None)
        return ProfileRow(name=str(row[0]), busy=busy,
                          count=int(row[2]) - (int(was[2]) if was else 0),
                          bytes=int(row[3]) - (int(was[3]) if was else 0))

    def reply(row, was):
        return ProfileRow(name=str(row[0]), busy=None,
                          count=int(row[1]) - (int(was[1]) if was else 0),
                          bytes=int(row[2]) - (int(was[2]) if was else 0))

    def job(row, was):
        busy = less(seconds_of(row[1]), seconds_of(was[1]) if was else None)
        waited = less(seconds_of(row[3]), seconds_of(was[3]) if was else None)
        return ProfileRow(name=str(row[0]), busy=busy, waited=waited,
                          count=int(row[2]) - (int(was[2]) if was else 0))

    synths = section(1, lambda r: (r[0], r[1]), synth)
    defs = section(2, lambda r: r[0], definition)
    # A def's synths in the window are the ones that ran in it, not the
    # difference of two counts: one alive throughout counts in both. So it is
    # the synths that began in the window (the count's difference) plus those
    # alive at both ends that moved, plus those alive at the start and gone by
    # the end. The server lists at most its costliest synths, so in a tree
    # larger than that this is a floor.
    ended = {(row[0], row[1]) for row in after[1]}
    for row in defs:
        moved = sum(1 for s in synths if s.name == row.name
                    and any(b[0] == s.node and b[1] == s.name for b in before[1]))
        gone = sum(1 for b in before[1] if b[1] == row.name and (b[0], b[1]) not in ended)
        row.nodes += moved + gone
    return Profile(
        seconds=seconds,
        timed_synths=timed,
        phases=section(0, lambda r: (r[0], r[1]), phase),
        synths=synths,
        defs=defs,
        commands=section(3, lambda r: r[0], command),
        replies=section(4, lambda r: r[0], reply),
        jobs=section(5, lambda r: r[0], job),
    )


def format_profile(profile: Profile, limit: int = 10) -> str:
    """The readable report `Server.profile`'s result prints as: every phase,
    then the ``limit`` costliest rows of each other section.

    A free function, as `format_load` is, so the report is one call whatever
    the reader wants to do with the rows themselves.
    """
    def seconds(value):
        return "        -  " if value is None else f"{value * 1000:8.2f} ms"

    def share(value):
        return "     -" if value is None else f"{value * 100:5.1f}%"

    window = _NOT_TIMED if profile.seconds is None else f"{profile.seconds:.2f} s"
    lines = [f"server profile over {window}"]
    if not profile.timed_synths:
        lines.append("  (synths were not timed for the whole window)")
    lines.append("  phases")
    for row in profile.phases:
        lines.append(f"    {row.role + ' ' + row.name:<22} {share(row.share)}  "
                     f"{seconds(row.busy)}  {row.count} runs")
    for title, rows, extra in (
        ("synths", profile.synths, lambda r: f"node {r.node}"),
        ("defs", profile.defs, lambda r: f"{r.nodes} synths"),
        ("commands", profile.commands, lambda r: f"{r.bytes} bytes"),
        ("replies", profile.replies, lambda r: f"{r.bytes} bytes"),
        ("jobs", profile.jobs, lambda r: f"waited {seconds(r.waited).strip()}"),
    ):
        if not rows:
            continue
        lines.append(f"  {title}")
        for row in rows[:limit]:
            lines.append(f"    {row.name:<22} {share(row.share)}  {seconds(row.busy)}  "
                         f"{row.count} x  {extra(row)}")
        if len(rows) > limit:
            lines.append(f"    ... {len(rows) - limit} more")
    return "\n".join(lines)


@dataclass
class ServerStatus:
    """The live counters a running server reports over ``/server_status``
    (read-only; the result of `Server.status`).

    The reply carries exactly these fields, in this order: the four counts, the
    two CPU meters, the two sample rates and ``late_blocks``.

    ``avg_cpu`` and ``peak_cpu`` are the audio thread's per-block processing
    time as a **percentage of the block budget**, not of a core: the average is
    an exponential moving average with a ~1 s time constant, the peak is the
    worst single block **since the previous call**, so every call reports the
    peak of its own interval and reading it resets the window. Expect the peak
    to sit well above the average -- the callback must fit its worst block, not
    its mean.

    ``late_blocks`` counts, cumulatively since boot, the blocks whose
    processing exceeded that budget. An occasional increment is a warning (a
    device quantum larger than one block absorbs it); a steady climb is audible
    trouble.

    In an offline render both meters measure render speed rather than a real
    callback, since there is none.

    **A server with no clock reports none of the three**: ``avg_cpu``,
    ``peak_cpu`` and ``late_blocks`` are ``None`` -- not zero, which would read
    as an idle engine. An engine in a page is that server.
    """

    #: Live UGen instances across every playing node.
    ugens: int
    #: Playing synth nodes.
    synths: int
    #: Groups in the node tree, the root group included.
    groups: int
    #: Defs loaded, both families together (SynthDefs and FaustDefs).
    defs: int
    #: Percentage of the block budget, averaged (~1 s time constant); ``None``
    #: from a server that cannot time itself.
    avg_cpu: "float | None"
    #: Percentage of the block budget, worst block since the previous call;
    #: ``None`` from a server that cannot time itself.
    peak_cpu: "float | None"
    #: The rate the server was asked for.
    nominal_sample_rate: float
    #: The rate the device actually runs at; it drifts from the nominal one.
    actual_sample_rate: float
    #: Blocks that missed their budget since boot. ``0`` against a server too
    #: old to report it, ``None`` from one that cannot time itself.
    late_blocks: "int | None" = 0

    def __str__(self) -> str:
        drift = ("" if self.actual_sample_rate == self.nominal_sample_rate
                 else f" (nominal {self.nominal_sample_rate:g})")
        if self.avg_cpu is None or self.peak_cpu is None:
            cpu = _NOT_TIMED
        else:
            late = "" if not self.late_blocks else f", {self.late_blocks} late"
            cpu = f"{self.avg_cpu:.1f}% avg, {self.peak_cpu:.1f}% peak{late}"
        return "\n".join([
            f"server {self.actual_sample_rate:g} Hz{drift}",
            f"  playing {self.synths} synths in {self.groups} groups, "
            f"{self.ugens} ugens",
            f"  loaded  {self.defs} defs",
            f"  cpu     {cpu}",
        ])
