"""The three subscriptions of `clausters.data`, over a fake server.

The wire is the whole of them — a subscription sent from the stream's own
socket, a ``/done`` ack, then replies decoded into whatever the stream keeps —
so a fake server standing in for the real one exercises every line and needs no
audio device. What is asserted is the claim rather than the mechanism, and for
the recording one it is the same claim the web client's ``tests/recording.html``
makes about the same wire: the cache the *reports* built and the cache the
*samples* build are the same bytes.
"""

import math
import socket
import struct
import threading
import time

from clausters._native import peaks_cache
from clausters.base import _osclib as osc
from clausters.data import (RECORDING_PERIOD_MS, STREAM_PERIOD_MS, BusStream,
                            RecordingStream, TakeShape, TapStream)
from clausters.defs import Server

BUCKET = 256
FRAMES = 4096
BUFNUM = 7


class FakeServer:
    """A UDP socket that acks ``/buffer_stream`` and pushes reports back to
    whoever subscribed — the server's half of this conversation, and nothing
    else."""

    def __init__(self):
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.bind(("127.0.0.1", 0))
        self.sock.settimeout(0.05)
        self.subscriber = None
        self.subscriptions = []
        self._running = True
        self._thread = threading.Thread(target=self._loop, daemon=True)
        self._thread.start()

    @property
    def port(self):
        return self.sock.getsockname()[1]

    def _loop(self):
        while self._running:
            try:
                data, src = self.sock.recvfrom(65536)
            except (TimeoutError, OSError):
                continue
            addr, args = osc.decode(data)
            if addr in ("/buffer_stream", "/bus_stream", "/bus_tapStream"):
                self.subscriber = src
                self.subscriptions.append((addr, args))
                self.sock.sendto(osc.message("/done", addr), src)

    def report(self, bufnum, start_frame, bucket, stats):
        blob = struct.pack(f"<{len(stats)}f", *stats)
        self.push("/buffer_stream.reply", bufnum, start_frame, bucket, blob)

    def snapshot(self, pairs):
        """One ``/bus_stream.reply bus value ...`` snapshot."""
        flat = [v for pair in pairs for v in pair]
        self.push("/bus_stream.reply", *flat)

    def window(self, bus, end_position, samples):
        """One ``/bus_tapStream.reply bus endPosition blob`` window."""
        self.push("/bus_tapStream.reply", bus, end_position,
                  struct.pack(f"<{len(samples)}f", *samples))

    def push(self, addr, *args):
        self.sock.sendto(osc.message(addr, *args), self.subscriber)

    def close(self):
        self._running = False
        self._thread.join(timeout=1.0)
        self.sock.close()


def _buckets(samples, start, count, bucket):
    """The report a writer would send for ``count`` buckets from ``start``:
    min, max and mean square per bucket, one channel."""
    out = []
    for b in range(count):
        run = samples[start + b * bucket:start + (b + 1) * bucket]
        out += [min(run), max(run), sum(s * s for s in run) / len(run)]
    return out


def _wait(predicate, timeout=2.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.005)
    return False


def test_reports_build_the_cache_the_samples_would_have_built():
    samples = [math.sin(2 * math.pi * 3 * i / FRAMES) * (i / FRAMES)
               for i in range(FRAMES)]
    fake = FakeServer()
    server = Server("127.0.0.1", fake.port)
    stream = None
    try:
        take = TakeShape(BUFNUM, FRAMES, 1)
        stream = RecordingStream.open(server, [take], timeout=2.0)

        # The subscription is the stream's own: the cadence, the grid the
        # caches were built on, and the takes it asked for.
        assert fake.subscriptions == [("/buffer_stream",
                                       [RECORDING_PERIOD_MS, BUCKET, BUFNUM])]
        assert stream.written(take) == 0

        seen = []
        stream.on_report(lambda bufnum, s: seen.append(s.written(bufnum)))

        # Four reports, as a writer would send them: whole buckets, in order,
        # each starting where the last ended.
        per = FRAMES // BUCKET // 4
        for i in range(4):
            start = i * per * BUCKET
            fake.report(BUFNUM, start, BUCKET, _buckets(samples, start, per, BUCKET))
            assert _wait(lambda i=i: stream.reports == i + 1)

        assert stream.written(take) == FRAMES
        assert seen == [FRAMES // 4, FRAMES // 2, 3 * FRAMES // 4, FRAMES]
        # The claim: nothing was measured on this side, so what the reports
        # left is what reading the samples would have produced.
        assert stream.peaks(take) == peaks_cache(samples, BUCKET, 1)
        assert stream.peaks(BUFNUM) == stream.peaks(take)
    finally:
        if stream is not None:
            stream.free()
        server.close()
        fake.close()


def test_a_report_off_the_grid_changes_nothing():
    fake = FakeServer()
    server = Server("127.0.0.1", fake.port)
    stream = None
    try:
        take = TakeShape(BUFNUM, FRAMES, 1)
        stream = RecordingStream.open(server, [take], timeout=2.0)
        before = stream.peaks(take)

        # A start off a bucket boundary: the core refuses it, and so does the
        # count -- a refused report is not a repaint.
        fake.report(BUFNUM, 13, BUCKET, [0.0, 1.0, 0.5])
        # A take this stream never subscribed to.
        fake.report(BUFNUM + 1, 0, BUCKET, [0.0, 1.0, 0.5])
        time.sleep(0.2)

        assert stream.reports == 0
        assert stream.written(take) == 0
        assert stream.peaks(take) == before
        assert stream.peaks(BUFNUM + 1) is None
    finally:
        if stream is not None:
            stream.free()
        server.close()
        fake.close()


def test_stop_cancels_the_subscription_and_leaves_the_caches_readable():
    fake = FakeServer()
    server = Server("127.0.0.1", fake.port)
    stream = None
    try:
        take = TakeShape(BUFNUM, FRAMES, 1)
        stream = RecordingStream.open(server, [take], timeout=2.0)
        fake.report(BUFNUM, 0, BUCKET, _buckets([0.5] * FRAMES, 0, 4, BUCKET))
        assert _wait(lambda: stream.reports == 1)

        stream.stop(timeout=2.0)
        # Cancelled on the server's own terms: no period, no buffers.
        assert fake.subscriptions[-1] == ("/buffer_stream", [0, BUCKET])
        # A finished take is still a picture.
        assert stream.written(take) == 4 * BUCKET
        assert stream.peaks(take) is not None

        # Nothing is decoded afterwards.
        fake.report(BUFNUM, 4 * BUCKET, BUCKET, _buckets([0.5] * FRAMES, 0, 4, BUCKET))
        time.sleep(0.2)
        assert stream.reports == 1
    finally:
        if stream is not None:
            stream.free()
        server.close()
        fake.close()


# ---- the control-bus stream ----


def test_a_snapshot_lands_in_bus_order_not_wire_order():
    fake = FakeServer()
    server = Server("127.0.0.1", fake.port)
    stream = None
    try:
        stream = BusStream.open(server, [12, 5, 30], timeout=2.0)
        assert fake.subscriptions == [("/bus_stream",
                                       [STREAM_PERIOD_MS, 12, 5, 30])]
        assert stream.buses == (12, 5, 30)
        assert stream.snapshots == 0

        seen = []
        stream.on_snapshot(lambda values, s: seen.append(list(values)))

        # The server names each bus in its snapshot, so what orders `values` is
        # the order this stream asked in -- never the order the wire arrived in.
        fake.snapshot([(5, 0.25), (30, -1.0), (12, 0.5)])
        assert _wait(lambda: stream.snapshots == 1)
        assert [round(v, 4) for v in stream.values] == [0.5, 0.25, -1.0]
        assert seen == [[0.5, 0.25, -1.0]]
        assert round(stream.value(5), 4) == 0.25

        # A bus this stream never asked for is not a snapshot.
        fake.snapshot([(99, 1.0)])
        time.sleep(0.2)
        assert stream.snapshots == 1
        assert math.isnan(stream.value(99))
    finally:
        if stream is not None:
            stream.free()
        server.close()
        fake.close()


def test_a_bus_stream_is_a_latest_value_store():
    fake = FakeServer()
    server = Server("127.0.0.1", fake.port)
    stream = None
    try:
        stream = BusStream.open(server, [1], timeout=2.0)
        for i, value in enumerate((0.1, 0.2, 0.3)):
            fake.snapshot([(1, value)])
            assert _wait(lambda i=i: stream.snapshots == i + 1)
        # Three snapshots, one value: a stream keeps no history, and a trace is
        # kept by whoever wants one.
        assert round(stream.value(1), 4) == 0.3
        assert len(stream.values) == 1

        stream.stop(timeout=2.0)
        assert fake.subscriptions[-1] == ("/bus_stream", [0])
    finally:
        if stream is not None:
            stream.free()
        server.close()
        fake.close()


# ---- the audio-tap stream ----


def test_a_window_carries_its_place_on_the_bus_axis():
    fake = FakeServer()
    server = Server("127.0.0.1", fake.port)
    stream = None
    try:
        stream = TapStream.open(server, [8, 9], frames=4, timeout=2.0)
        assert fake.subscriptions == [("/bus_tapStream",
                                       [STREAM_PERIOD_MS, 4, 8, 9])]
        assert stream.window(8) is None      # nothing recorded yet

        seen = []
        stream.on_data(lambda bus, w: seen.append((bus, w.end_position)))

        fake.window(8, 1024, [0.0, 0.5, 1.0, 0.5])
        assert _wait(lambda: stream.window(8) is not None)
        window = stream.window(8)
        assert [round(s, 4) for s in window.samples] == [0.0, 0.5, 1.0, 0.5]
        # The position is what places consecutive windows on the bus's own
        # timeline: they overlap or gap by its delta, never by the period.
        assert window.end_position == 1024
        assert seen == [(8, 1024)]

        fake.window(99, 0, [1.0])
        time.sleep(0.2)
        assert seen == [(8, 1024)]           # a bus this stream never asked for

        stream.stop(timeout=2.0)
        # The command takes a frame count before its buses, so cancelling it
        # takes one too.
        assert fake.subscriptions[-1] == ("/bus_tapStream", [0, 0])
    finally:
        if stream is not None:
            stream.free()
        server.close()
        fake.close()


def test_interleaving_pairs_the_freshest_samples():
    fake = FakeServer()
    server = Server("127.0.0.1", fake.port)
    stream = None
    try:
        stream = TapStream.open(server, [8, 9], frames=4, timeout=2.0)
        # Empty until every bus of the run has a window.
        fake.window(8, 100, [1.0, 2.0, 3.0, 4.0])
        assert _wait(lambda: stream.window(8) is not None)
        assert len(stream.interleaved(8, 2)) == 0

        # The two windows differ in length, which the server is allowed to do:
        # `frames` is clamped, and a fill that has not caught up is shorter.
        fake.window(9, 100, [30.0, 40.0])
        assert _wait(lambda: stream.window(9) is not None)
        # Aligned on the newest sample of each, not on their starts.
        assert list(stream.interleaved(8, 2)) == [3.0, 30.0, 4.0, 40.0]
    finally:
        if stream is not None:
            stream.free()
        server.close()
        fake.close()
