"""Audio and control buses, with client-side allocation.

Mirrors the server's bus model (`dsp`): audio buses (``0..channels`` are the
hardware outputs) and single-float control buses. Like scsynth, the client owns
allocation; the server just indexes. A :class:`Bus` is a flat
``(index, channels, rate)`` — only flat data ever leaves for the wire.

The allocators carry **no default size of their own**: how many buses exist is a
property of the server, not the bus module. The :class:`~clausters.defs.server.Server`
sizes them from its :class:`~clausters.defs.server.ServerOptions` (which also emits
the matching ``--audio-buses``/``--control-buses`` launch flags), and the live
counts can be read back with :meth:`~clausters.defs.server.Server.query_info`.
"""


class Bus:
    def __init__(self, index: int, channels: int = 1, rate: str = "audio"):
        self.index = index
        self.channels = channels
        self.rate = rate  # 'audio' | 'control'

    def __repr__(self):
        return f"Bus({self.rate}, index={self.index}, channels={self.channels})"


class _Allocator:
    def __init__(self, rate: str, size: int, reserved: int = 0):
        self.rate = rate
        self.size = size
        self._next = reserved   # first freely allocatable index
        self._freed: list[tuple[int, int]] = []  # (index, channels) returned

    def alloc(self, channels: int = 1) -> Bus:
        # reuse an exact-width freed block first
        for i, (index, width) in enumerate(self._freed):
            if width == channels:
                self._freed.pop(i)
                return Bus(index, channels, self.rate)
        if self._next + channels > self.size:
            raise RuntimeError(f"out of {self.rate} buses")
        index = self._next
        self._next += channels
        return Bus(index, channels, self.rate)

    def free(self, bus: Bus):
        self._freed.append((bus.index, bus.channels))


class AudioBusAllocator(_Allocator):
    """Allocates audio buses above the hardware outputs (``reserved``). ``size``
    is the server's audio-bus count (from ``ServerOptions``/``query_info``)."""

    def __init__(self, size: int, reserved: int = 2):
        super().__init__("audio", size, reserved)


class ControlBusAllocator(_Allocator):
    """``size`` is the server's control-bus count (from
    ``ServerOptions``/``query_info``)."""

    def __init__(self, size: int):
        super().__init__("control", size, 0)
