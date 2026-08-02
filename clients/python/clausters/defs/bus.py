"""Audio and control buses, with client-side allocation.

Mirrors the server's bus model (`dsp`): audio buses (``0..channels`` are the
hardware outputs) and single-float control buses. Like scsynth, the client owns
allocation; the server just indexes. A `Bus` is a flat
``(index, channels, rate)`` — only flat data ever leaves for the wire.

Buses are a finite boot-time resource, so each allocator is a **registry** (the
core's occupancy map): a freed run is always reusable, adjacent runs coalesce,
a double free is refused loudly, and exhaustion raises instead of wrapping. The
allocatable space excludes the hardware outputs at the bottom (``reserved``)
and the GraphDef private-bus range at the top (the core's
``GRAPH_*_BUS_RESERVED``, clamped to the space) — those buses belong to the
server's own registry.

The allocators carry **no default size of their own**: how many buses exist is
a property of the server, not the bus module. The `Server`
sizes them from its `ServerOptions` (which also emits
the matching ``--audio-buses``/``--control-buses`` launch flags), and the live
counts can be read back with `query_info`.
"""

from .. import _native
from ._wire import resolve as _resolve


class Bus:
    """A patch point: where one node writes and another reads.

    Nodes never connect to each other. They read and write **numbered slots**
    on the server, and a bus is a run of those slots plus which kind they are.
    That indirection is what lets a `Synth` be replaced, a whole `Group` be
    freed, or a def be re-sent without anything downstream noticing: the
    reader is aimed at a number, not at a node.

    Two rates, and they are different things:

    - **Audio** — a block of samples per channel every block, the signal
      itself. Buses ``0..outputs`` are the hardware outputs, which is why
      ``out(0, …)`` is what you hear; `audio` allocates above them. Read one
      with the ``in_`` UGen, write one with ``out``.
    - **Control** — one float, updated per block: a parameter, not a signal. A
      slow envelope, a knob, an LFO. Cheap enough to have thousands. Read one
      with ``in_ctl``, and this is the rate `Node.map` binds a control to, so
      one writer drives many synths with the client out of the loop.

    Both are a finite boot-time resource sized by `ServerOptions`, so a bus is
    allocated and `free`d like memory. The `set` / `get` pair works on control
    buses only — that is the client writing a parameter directly — while
    `watch` opens an audio bus for reading, which is how a scope or a meter
    sees a signal without a message per frame.

    One control bus driving three voices at once, which is what `Node.map` is
    for:

    ```python
    from clausters import Bus, Group, Server, Synth, SynthDef
    from clausters.defs import control, out, sine

    s = Server.boot()
    d = SynthDef("voice",
                 out(0, sine(control("freq", 440.0)) * control("amp", 0.1)))
    d.send(s)

    amp = Bus.control(server=s)              # one float the voices follow
    g = Group(server=s)
    for freq in (220.0, 277.0, 330.0):
        n = Synth("voice", {"freq": freq}, target=g, server=s)
        n.map("amp", amp)                    # that control now tracks the bus

    amp.set(0.3)                             # one write, three voices
    print(amp.get())                         # read back (f32: 0.30000001…)

    g.free()
    amp.free()                               # the slot returns to the pool
    ```

    Attributes:
        index: the first slot of the run. This is the number that goes on the
            wire and into a def's ``bus`` control — nodes are aimed at it.
        channels: how many contiguous slots the run covers.
        rate: ``"audio"`` or ``"control"``.
        server: the `Server` this bus was allocated on; `None` falls back to
            the ambient one.
    """

    def __init__(self, index: int, channels: int = 1, rate: str = "audio",
                 server=None):
        """Names an existing run of buses by index — bus 0 is the first
        hardware output on every server, so a fixed index is a meaningful thing
        to write down. To take a **free** run out of the server's pool instead,
        use `audio` or `control`, which is what a piece normally wants.

        Args:
            index: the run's first slot.
            channels: how many contiguous slots it covers.
            rate: ``"audio"`` or ``"control"``.
            server: the `Server` it lives on; `None` takes the ambient one.
        """
        self.index = index
        self.channels = channels
        self.rate = rate  # 'audio' | 'control'
        #: the `Server` this bus was allocated on (set by `audio` / `control`),
        #: so its commands know where to go without being told.
        self.server = server

    @classmethod
    def audio(cls, channels: int = 1, *, server=None) -> "Bus":
        """A run of ``channels`` contiguous audio buses from the server's
        pool, above the hardware outputs."""
        srv = _resolve(server)
        return srv.audio_buses.alloc(channels, srv)

    @classmethod
    def control(cls, channels: int = 1, *, server=None) -> "Bus":
        """A run of ``channels`` contiguous control buses from the server's
        pool."""
        srv = _resolve(server)
        return srv.control_buses.alloc(channels, srv)

    def _server(self):
        return _resolve(self.server)

    def set(self, value):
        """Write a value to this control bus (``/bus_set``)."""
        self._server().send_msg("/bus_set", self.index, float(value))

    def get(self, timeout: "float | None" = None) -> float:
        """Read this control bus's current value (``/bus_get`` -> ``/bus_get.reply``).
        RT only (it needs a reply)."""
        _, args = self._server().request("/bus_get", self.index, timeout=timeout,
                                         expect=("/bus_get.reply",))
        return args[1] if len(args) >= 2 else args[-1]

    def watch(self, flag: bool = True):
        """Asks the server to make this audio bus readable (``/bus_tap``): from the
        next block on, the engine records it into the shared segment, where a
        GUI oscilloscope reads it with zero messages (or a client streams it
        with `clausters.defs.Server.stream_taps`). ``flag=False`` stops.

        **The bus is the only number you name.** Which of the server's finite
        sample rings carries it is the server's own bookkeeping, published in
        the segment for whoever reads the samples. Watches count, so two views
        of one bus share a ring and the last one to stop frees it. No ack, like
        ``/node_map`` (failures reply ``/fail`` -- an unknown bus, no tap region,
        or every ring already taken); sequence with ``sync`` when needed.
        """
        self._server().send_msg("/bus_tap", self.index, 1 if flag else 0)

    def free(self):
        """Returns this bus's run to the server's pool."""
        server = self._server()
        allocator = (server.audio_buses if self.rate == "audio"
                     else server.control_buses)
        allocator.free(self)

    def __repr__(self):
        return f"Bus({self.rate}, index={self.index}, channels={self.channels})"


class _Allocator:
    def __init__(self, rate: str, size: int, reserved: int, graph_reserved: int):
        self.rate = rate
        self.size = size
        # The private GraphDef range sits at the top of the space, clamped the
        # same way the server clamps it when the configured count is small. A
        # space the reservations swallow whole leaves no registry: `alloc`
        # reports exhaustion from the first call.
        top = size - min(graph_reserved, size)
        span = max(0, top - reserved)
        self._registry = _native.Registry(reserved, span) if span > 0 else None

    def alloc(self, channels: int = 1, server=None) -> Bus:
        """A run of ``channels`` contiguous buses, stamped with the ``server``
        whose pool this is. Raises when no such run is free — exhaustion is an
        explicit failure, never an aliased index."""
        index = self._registry.alloc(channels) if self._registry else None
        if index is None:
            raise RuntimeError(f"out of {self.rate} buses")
        return Bus(index, channels, self.rate, server)

    def free(self, bus: Bus):
        """Returns the bus's run to the pool. A double free (or a bus this
        allocator never handed out) raises — losing track of a bus is a
        client bug, never absorbed silently."""
        if self._registry is None or self._registry.release(bus.index, bus.channels) != 0:
            raise RuntimeError(
                f"double free of {self.rate} bus {bus.index} "
                f"(channels={bus.channels}): not currently allocated here")

    @property
    def in_use(self) -> int:
        """How many buses are currently allocated."""
        return self._registry.in_use if self._registry else 0


class AudioBusAllocator(_Allocator):
    """Allocates audio buses above the hardware outputs (``reserved``) and
    below the GraphDef private range. ``size`` is the server's audio-bus count
    (from ``ServerOptions``/``query_info``)."""

    def __init__(self, size: int, reserved: int = 2):
        super().__init__("audio", size, reserved, _native.graph_bus_reserved()[0])


class ControlBusAllocator(_Allocator):
    """``size`` is the server's control-bus count (from
    ``ServerOptions``/``query_info``)."""

    def __init__(self, size: int):
        super().__init__("control", size, 0, _native.graph_bus_reserved()[1])
