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


class Bus:
    def __init__(self, index: int, channels: int = 1, rate: str = "audio"):
        self.index = index
        self.channels = channels
        self.rate = rate  # 'audio' | 'control'

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

    def alloc(self, channels: int = 1) -> Bus:
        """A run of ``channels`` contiguous buses. Raises when no such run is
        free — exhaustion is an explicit failure, never an aliased index."""
        index = self._registry.alloc(channels) if self._registry else None
        if index is None:
            raise RuntimeError(f"out of {self.rate} buses")
        return Bus(index, channels, self.rate)

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
