"""Following a take while it records: `clausters.data.RecordingStream`.

The wire is the whole of it — a subscription sent from the stream's own socket,
a ``/done`` ack, then ``/buffer_stream.reply`` reports folded into a cache — so
a fake server standing in for the real one exercises every line of the class
and needs no audio device. What is asserted is the claim rather than the
mechanism, and it is the same claim the web client's ``tests/recording.html``
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
from clausters.data import RECORDING_PERIOD_MS, RecordingStream, TakeShape
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
            if addr == "/buffer_stream":
                self.subscriber = src
                self.subscriptions.append(args)
                self.sock.sendto(osc.message("/done", "/buffer_stream"), src)

    def report(self, bufnum, start_frame, bucket, stats):
        blob = struct.pack(f"<{len(stats)}f", *stats)
        self.sock.sendto(
            osc.message("/buffer_stream.reply", bufnum, start_frame, bucket, blob),
            self.subscriber,
        )

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
        assert fake.subscriptions == [[RECORDING_PERIOD_MS, BUCKET, BUFNUM]]
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
        assert fake.subscriptions[-1] == [0, BUCKET]
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
