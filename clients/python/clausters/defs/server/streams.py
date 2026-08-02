"""The subscriptions the server pushes without being asked again.

A stream is the network counterpart of reading the shared-memory segment: one
subscription per client, replaced on each call, cancelled by a period of zero.
The snapshots arrive on their own reply address, so they are received with an
`OscFunc` rather than returned from here — what these calls block on is only
the ``/done`` ack.
"""

from ..bus import Bus


class ServerStreams:
    """The subscription half of `Server`; never instantiated on its own."""

    # ---- bus and tap subscriptions (one per client, over a set) ----

    def stream_buses(self, period_ms: int, *buses, timeout: "float | None" = None):
        """Subscribes this client to a periodic ``/bus_stream.reply`` snapshot of the
        given control buses (``/bus_stream``): the server sends one snapshot
        immediately and then one every ``period_ms`` (floor 10 ms, at most 128
        buses) with no further requests -- the network counterpart of reading
        the shared-memory segment, e.g. for meters over WebSocket. One
        subscription per client, replaced on each call; ``period_ms <= 0`` (or
        no buses) cancels it. Receive the snapshots with an `OscFunc` on
        ``/bus_stream.reply``. Blocks on the ``/done`` ack."""
        indices = [b.index if isinstance(b, Bus) else int(b) for b in buses]
        return self.request("/bus_stream", int(period_ms), *indices,
                            timeout=timeout, expect=("/done", "/fail"))

    def stream_taps(self, period_ms: int, frames: int, *buses, timeout: "float | None" = None):
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
