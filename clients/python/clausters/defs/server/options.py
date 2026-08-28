"""The client-owned server configuration, and what a running server reports.

`ServerOptions` is the enumeration of every option a launched server takes —
it sizes the handle's allocators and builds the process's command line.
`ServerInfo` is the answer to `Server.query_info`: what the server it is
talking to was actually built and booted with, which is not always the same
thing. `ServerStatus` is the answer to `Server.status`: what it is doing right
now, which changes between two calls.
"""

from dataclasses import dataclass, field

from ...config import server_config


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
      ``max_frame``, ``max_stream_buses``, ``max_clients``, ``pin``):
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
    #: forces it on at the default port, a number moves it, and a string binds
    #: it (``--tcp [addr:]port``: ``"0.0.0.0:57110"``, ``"0.0.0.0"``). It is on
    #: by default server-side; ``None`` leaves that as is. Every carrier binds
    #: loopback unless the address says otherwise.
    tcp: "bool | int | str | None" = None
    #: OSC over WebSocket: ``True`` opens it at the default port (57120), a
    #: number picks the port, a string binds it (``--ws [addr:]port``, e.g.
    #: ``"0.0.0.0:57120"`` for a browser on another machine). There is no off
    #: flag — leave ``None`` and keep it out of the server's config to run
    #: without it.
    ws: "bool | int | str | None" = None
    #: Virtual MIDI input: ``True`` opens it with the default name, a string
    #: names the port (``--midi [name]``).
    midi: "bool | str | None" = None
    #: Def persistence: ``False`` disables it for this run (``--no-persist``).
    #: There is no force-on flag — ``True`` is expressible only by keeping
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
    #: The audio host/backend by name (``--host``): ``"jack"``, ``"alsa"``,
    #: ``"pipewire"``, ``"coreaudio"``, ``"wasapi"`` — whatever the build
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
        ])


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
    """

    #: Live UGen instances across every playing node.
    ugens: int
    #: Playing synth nodes.
    synths: int
    #: Groups in the node tree, the root group included.
    groups: int
    #: Defs loaded, both families together (SynthDefs and FaustDefs).
    defs: int
    #: Percentage of the block budget, averaged (~1 s time constant).
    avg_cpu: float
    #: Percentage of the block budget, worst block since the previous call.
    peak_cpu: float
    #: The rate the server was asked for.
    nominal_sample_rate: float
    #: The rate the device actually runs at; it drifts from the nominal one.
    actual_sample_rate: float
    #: Blocks that missed their budget since boot. ``0`` against a server too
    #: old to report it.
    late_blocks: int = 0

    def __str__(self) -> str:
        drift = ("" if self.actual_sample_rate == self.nominal_sample_rate
                 else f" (nominal {self.nominal_sample_rate:g})")
        late = "" if self.late_blocks == 0 else f", {self.late_blocks} late"
        return "\n".join([
            f"server {self.actual_sample_rate:g} Hz{drift}",
            f"  playing {self.synths} synths in {self.groups} groups, "
            f"{self.ugens} ugens",
            f"  loaded  {self.defs} defs",
            f"  cpu     {self.avg_cpu:.1f}% avg, {self.peak_cpu:.1f}% peak"
            f"{late}",
        ])
