"""Buffers, with client-side index allocation.

The server's buffer pool is a finite boot-time resource (``--max-buffers``,
4096 by default), indices allocated by the client (like scsynth). `Buffer` is
a flat handle; the actual allocation/loading happens on the server via
``/b_alloc``/``/b_allocRead``/… driven by `Server`.

The allocator is a **registry** (the core's occupancy map): a freed slot is
always reusable, a double free is refused loudly, exhaustion raises instead of
wrapping. The `Server` sizes it from its `ServerOptions` (``max_buffers``).
"""

from .. import _native

NUM_BUFFERS = 4096


class Buffer:
    def __init__(self, bufnum: int, frames: int = 0, channels: int = 1, sample_rate: float = 0.0):
        self.bufnum = bufnum
        self.frames = frames
        self.channels = channels
        self.sample_rate = sample_rate

    def __repr__(self):
        return f"Buffer(bufnum={self.bufnum}, frames={self.frames}, channels={self.channels})"


class BufferAllocator:
    def __init__(self, size: int = NUM_BUFFERS):
        self.size = size
        self._registry = _native.Registry(0, size)

    def alloc(self) -> int:
        """A free buffer index; raises when the pool is exhausted."""
        bufnum = self._registry.alloc()
        if bufnum is None:
            raise RuntimeError("out of buffer slots")
        return bufnum

    def free(self, bufnum: int):
        """Returns ``bufnum`` to the pool. A double free (or an index this
        allocator never handed out) raises — a lost buffer slot is a client
        bug, never absorbed silently."""
        if self._registry.release(bufnum) != 0:
            raise RuntimeError(
                f"double free of buffer {bufnum}: not currently allocated")

    @property
    def in_use(self) -> int:
        """How many buffer slots are currently allocated."""
        return self._registry.in_use
