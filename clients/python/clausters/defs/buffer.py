"""Buffers, with client-side index allocation.

The server has 1024 buffer slots, indices allocated by the client (like
scsynth). :class:`Buffer` is a flat handle; the actual allocation/loading
happens on the server via ``/b_alloc``/``/b_allocRead``/… driven by
:class:`~clausters.defs.server.Server`.
"""

NUM_BUFFERS = 1024


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
        self._next = 0
        self._freed: list[int] = []

    def alloc(self) -> int:
        if self._freed:
            return self._freed.pop()
        if self._next >= self.size:
            raise RuntimeError("out of buffer slots")
        bufnum = self._next
        self._next += 1
        return bufnum

    def free(self, bufnum: int):
        self._freed.append(bufnum)
