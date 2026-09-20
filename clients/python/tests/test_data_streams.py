"""The three subscriptions of `clausters.data`, over a fake server.

The wire is the whole of them -- a subscription sent from the stream's own
socket, a ``/done`` ack, then replies decoded into whatever the stream keeps --
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
    whoever subscribed -- the server's half of this conversation, and nothing
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


# ---- the true peak: what happened between the samples ----


def test_the_true_peak_reads_above_the_sample_peak():
    """The reconstructed peak, which is the number a delivery specification
    means by dBTP.

    The standard's own worst case (ITU-R BS.1770-4, Appendix 1 to Annex 2): a
    tone at a quarter of the sample rate, sampled at 45 degrees. Every sample
    sits at full scale, so a meter watching samples has nothing to report --
    and the signal between them reaches sqrt(2), three decibels over.
    """
    from array import array

    from clausters import ipc

    x = array("f", [1.0 if (i // 2) % 2 == 0 else -1.0 for i in range(512)])
    (sample_peak,), _ = ipc.channel_stats(x, 1)
    (true_peak,) = ipc.true_peak(x, 1)
    assert sample_peak == 1.0
    over = 20 * math.log10(true_peak / sample_peak)
    assert abs(over - 3.01) < 0.25, f"{over} dB over the samples"


def test_a_true_peak_is_never_below_the_sample_peak():
    """Every sample lies on the reconstructed curve, so the measurement can
    only ever find more than a scan of the samples does."""
    from array import array

    from clausters import ipc

    x = array("f", [0.8 * math.sin(i * 0.37) + 0.2 * math.sin(i * 1.9)
                    for i in range(1024)])
    (sample_peak,), _ = ipc.channel_stats(x, 1)
    (true_peak,) = ipc.true_peak(x, 1)
    assert true_peak >= sample_peak - 1e-6


def test_each_channel_is_measured_through_its_stride():
    """One channel of an interleaved buffer, without deinterleaving it, and
    nothing at all for a request that cannot be met."""
    from array import array

    from clausters import ipc

    loud = [0.9 * math.sin(i * 0.31) for i in range(512)]
    quiet = [0.05 * v for v in loud]
    x = array("f", [v for pair in zip(quiet, loud) for v in pair])
    peaks = ipc.true_peak(x, 2)
    assert peaks[0] < 0.2 < 0.8 < peaks[1]
    assert ipc.true_peak(x, 0) == ()
    assert ipc.true_peak(array("f", []), 1) == ()


# ---- loudness: how loud it sounds, as broadcast measures it ----


def _tone(seconds, levels, rate=48000):
    """An interleaved 1 kHz sine, one peak level in dBFS per channel (``None``
    for a silent channel), for ``seconds`` -- EBU Tech 3341's test tone."""
    from array import array

    gains = [0.0 if db is None else 10 ** (db / 20) for db in levels]
    out = array("f")
    for i in range(int(seconds * rate)):
        s = math.sin(2 * math.pi * 1000 * i / rate)
        out.extend(g * s for g in gains)
    return out


def test_a_stereo_tone_reads_its_peak_level_in_lufs():
    """EBU Tech 3341's first case: a stereo 1 kHz sine at -23 dBFS reads
    -23.0 LUFS, integrated, momentary and short-term alike -- the calibration
    the -0.691 in BS.1770's formula exists for."""
    from clausters import ipc

    measured = ipc.loudness(_tone(5.0, [-23.0, -23.0]), 2, 48000)
    assert measured is not None
    assert abs(measured.integrated - -23.0) < 0.1
    assert abs(measured.momentary_max - -23.0) < 0.1
    assert abs(measured.short_term_max - -23.0) < 0.1
    assert measured.range < 0.1


def test_the_range_is_the_spread_of_the_short_term_loudness():
    """EBU Tech 3342's first case, shortened: a tone 10 dB quieter after the
    first spreads the short-term loudness by 10 LU."""
    from array import array

    from clausters import ipc

    x = array("f", _tone(8.0, [-20.0, -20.0]) + _tone(8.0, [-30.0, -30.0]))
    measured = ipc.loudness(x, 2, 48000)
    assert measured is not None
    assert abs(measured.range - 10.0) < 1.0


def test_a_zero_weight_leaves_a_channel_out():
    """A weight per channel, stated: 0.0 measures nothing of that channel
    whatever it holds, which is how an LFE is left out."""
    from clausters import ipc

    loud_second = _tone(2.0, [-20.0, 0.0])
    silent_second = _tone(2.0, [-20.0, None])
    assert ipc.loudness(loud_second, 2, 48000, weights=[1.0, 0.0]) == \
        ipc.loudness(silent_second, 2, 48000, weights=[1.0, 0.0])
    assert ipc.loudness(loud_second, 2, 48000) != \
        ipc.loudness(silent_second, 2, 48000)


def test_nothing_to_measure_and_a_request_that_cannot_be_met():
    """Silence reads no loudness at all, and a request the core cannot answer
    reads nothing rather than a number."""
    from array import array

    from clausters import ipc

    silence = ipc.loudness(array("f", bytes(4 * 48000)), 1, 48000)
    assert silence is not None
    assert silence.integrated == -math.inf and silence.range == 0.0
    x = _tone(1.0, [-20.0])
    assert ipc.loudness(x, 0, 48000) is None
    assert ipc.loudness(x, 1, 4) is None
    assert ipc.loudness(x, 1, 48000, weights=[1.0, 1.0]) is None
    assert ipc.loudness(array("f"), 1, 48000) is None
