"""Track the server's sample clock over UDP (C6).

To use a :class:`~clausters.base.timebase.SampleClockTimebase` over UDP — where
the client can't read the sample counter directly (as shm/embed can) — it
queries the server's ``/clock`` and models

    sample(t_local) = a + b * t_local

from ``(local monotonic time, counter)`` anchor pairs, a least-squares line over
a sliding window (JACK-DLL / Ableton-Link in spirit; same model as the server's
``examples/sample_clock.py``). The TempoClock then paces against this and the
Server schedules every event by absolute sample with ``/sched`` — drift-free.

Query latency does not accumulate: an anchor is paired with the *midpoint* of
its round trip, whose half-width is a bounded uncertainty that only shifts the
whole grid by a constant. Relative timing stays sample-exact by construction.
"""

import threading
import time

from ..base import _osclib
from ..base._oscinterface import OscUDPInterface
from ..base.timebase import SampleClockTimebase


class SampleClockModel:
    """``sample(t) = a + b·t``, least-squares over a sliding anchor window."""

    def __init__(self, nominal_rate: float = 48_000.0, window: int = 64):
        self.rate = float(nominal_rate)
        self.window = window
        self.anchors: list[tuple[float, int]] = []  # (t_local, sample)
        self.a = 0.0
        self.b = self.rate

    def add_anchor(self, t_local: float, sample: int, rate: float | None = None):
        if rate is not None:
            self.rate = float(rate)
        self.anchors.append((t_local, int(sample)))
        self.anchors = self.anchors[-self.window:]
        self._fit()

    def _fit(self):
        n = len(self.anchors)
        t_ref, s_ref = self.anchors[-1]
        if n < 2:
            self.a, self.b = s_ref - self.rate * t_ref, self.rate
            return
        ts = [t for t, _ in self.anchors]
        ss = [s for _, s in self.anchors]
        t_mean, s_mean = sum(ts) / n, sum(ss) / n
        var = sum((t - t_mean) ** 2 for t in ts)
        cov = sum((t - t_mean) * (s - s_mean) for t, s in self.anchors)
        self.b = cov / var if var > 0 else self.rate
        self.a = s_mean - self.b * t_mean

    def sample_at(self, t_local: float) -> int:
        return round(self.a + self.b * t_local)

    def now(self) -> int:
        """Predicted current value of the server's sample counter."""
        return self.sample_at(time.monotonic())

    def local_time_of(self, sample: int) -> float:
        """Inverse: the monotonic time the counter reaches ``sample``."""
        return (sample - self.a) / self.b

    def drift_ppm(self) -> float:
        return (self.b / self.rate - 1.0) * 1e6

    def span(self) -> float:
        return self.anchors[-1][0] - self.anchors[0][0] if len(self.anchors) >= 2 else 0.0


class UdpSampleClock:
    """Tracks a server's sample clock over UDP and yields a timebase.

    Uses its **own** socket (so ``/clock`` round trips never contend with the
    Server's command socket). Build one with ``server.sample_clock()``.
    """

    def __init__(self, server, window: int = 64, timeout: float = 2.0):
        self.target = server.target.addr()
        self._iface = OscUDPInterface().start()
        self.model = SampleClockModel(window=window)
        self._timeout = timeout
        self._tracking = False
        self._thread = None

    def anchor(self) -> float:
        """One ``/clock`` round trip; returns the anchor's uncertainty (s)."""
        t0 = time.monotonic()
        self._iface.send_msg(self.target, "/clock")
        packet = self._iface.recv(self._timeout)
        t1 = time.monotonic()
        if packet is None:
            raise TimeoutError("no /clock.reply (is the server on UDP?)")
        addr, args = _osclib.decode(packet)
        if addr != "/clock.reply":
            raise RuntimeError(f"expected /clock.reply, got {addr}")
        self.model.add_anchor((t0 + t1) / 2, args[0], args[1])
        return (t1 - t0) / 2

    def warmup(self, n: int = 5, gap: float = 0.05) -> float:
        """A few anchors to seed the model; returns the worst uncertainty."""
        worst = 0.0
        for _ in range(n):
            worst = max(worst, self.anchor())
            time.sleep(gap)
        return worst

    def track(self, interval: float = 0.5):
        """Re-anchor in the background forever (keeps the slope fresh)."""
        if self._tracking:
            return self
        self._tracking = True

        def loop():
            while self._tracking:
                try:
                    self.anchor()
                except (TimeoutError, OSError, RuntimeError):
                    pass
                time.sleep(interval)

        self._thread = threading.Thread(target=loop, name="UdpSampleClock", daemon=True)
        self._thread.start()
        return self

    def untrack(self):
        self._tracking = False
        if self._thread is not None:
            self._thread.join(timeout=1.0)
            self._thread = None

    def now(self) -> int:
        return self.model.now()

    @property
    def rate(self) -> float:
        return self.model.rate

    def timebase(self) -> SampleClockTimebase:
        """A :class:`SampleClockTimebase` reading this tracker's model."""
        return SampleClockTimebase(self.now, self.rate)

    def close(self):
        self.untrack()
        self._iface.close()
