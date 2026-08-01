"""Track the server's sample clock: over UDP, or read directly in-process.

Two ways to feed a `SampleClockTimebase`:

- `UdpSampleClock` — over UDP, where the client can't read the sample counter
  directly, it queries the server's ``/clock_query`` and models the counter (below).
- `EmbedSampleClock` — for an in-process embedded server, whose handle exposes
  the counter itself: no socket, no round trips, no model — every read *is* the
  counter. It mirrors the tracker's surface so `TempoClock.lock_to` treats both
  alike.

The UDP tracker models

    sample(t_local) = a + b * t_local

from ``(local monotonic time, counter)`` anchor pairs, a least-squares line over
a sliding window (JACK-DLL / Ableton-Link in spirit; same model as the server's
``examples/sample_clock.py``). The TempoClock then paces against this and the
Server schedules every event by absolute sample with ``/sched_at`` — drift-free.

Query latency does not accumulate: an anchor is paired with the *midpoint* of
its round trip, whose half-width is a bounded uncertainty that only shifts the
whole grid by a constant. Relative timing stays sample-exact by construction.

Why this is drift-free, and what *can* fail. Each event's target is an absolute
sample recomputed from the routine's absolute logical beat — ``round((origin +
beat / tempo) * sample_rate)`` — not stepped from the previous event, so error
never accumulates: at 48 kHz the target stays integer-exact within ``float64``
for hours. The fitted line above only sizes the *lead* (how far ahead the
``/sched_at`` is queued), never the event time, which the server's own counter
resolves exactly. So the only failure mode is **binary, not cumulative**: a
``/sched_at`` that arrives *after* its target sample (lead < worst-case client +
network + model jitter) lands late; one that arrives in time lands exact. A long
run at perfect spacing just means the lead was never violated.

Surviving suspend. The server's counter counts samples *actually emitted*, so it
freezes when the audio device suspends (system sleep, or the sink going idle)
and resumes in place — it is not a wall clock. A ``/sched_at`` keyed to an absolute
sample simply waits in the server's queue and fires when the counter reaches it,
so the audio grid stays sample-exact across the gap (consecutive events keep
their exact spacing; the freeze just drops out of the timeline). The tracker
rides this automatically: while the counter is stalled its anchors fit a flat
slope, so the predictor stops running ahead and the lead/backlog stays bounded;
on resume the slope recovers. Only the wall-clock *phase* shifts, by the suspend
duration — relative sample spacing is preserved.
"""

import threading
import time

from .. import _native
from ..base import _osclib
from ..base._oscinterface import OscUdpInterface
from ..base.timebase import SampleClockTimebase


class SampleClockModel:
    """``sample(t) = a + b·t``, least-squares over a sliding anchor window.

    The fit itself lives in the native core (`clausters._native.ClockSyncModel`
    over ``clausters_core::clocksync``), so every client predicts the same
    sample from the same anchors; this class only adds the local-time
    convenience (`now` reads the monotonic clock)."""

    def __init__(self, nominal_rate: float = 48_000.0, window: int = 64):
        self._model = _native.ClockSyncModel(nominal_rate, window)

    def add_anchor(self, t_local: float, sample: int, rate: float | None = None):
        self._model.add_anchor(t_local, sample, rate if rate is not None else 0.0)

    @property
    def rate(self) -> float:
        return self._model.rate

    @property
    def a(self) -> float:
        """Fitted intercept (samples at local time 0)."""
        return self._model.a

    @property
    def b(self) -> float:
        """Fitted slope (samples per local second)."""
        return self._model.b

    def sample_at(self, t_local: float) -> int:
        return self._model.sample_at(t_local)

    def now(self) -> int:
        """Predicted current value of the server's sample counter."""
        return self.sample_at(time.monotonic())

    def local_time_of(self, sample: int) -> float:
        """Inverse: the monotonic time the counter reaches ``sample``."""
        return self._model.local_time_of(sample)

    def drift_ppm(self) -> float:
        return self._model.drift_ppm()

    def span(self) -> float:
        return self._model.span()

    def close(self):
        self._model.close()


class UdpSampleClock:
    """Tracks a server's sample clock over UDP and yields a timebase.

    Uses its **own** socket (so ``/clock_query`` round trips never contend with the
    Server's command socket). Build one with ``server.sample_clock()``.
    """

    def __init__(self, server, window: int = 64, timeout: float = 2.0):
        self.target = server.target
        self._iface = OscUdpInterface().start()
        self.model = SampleClockModel(window=window)
        self._timeout = timeout
        self._tracking = False
        self._thread = None

    def anchor(self) -> float:
        """One ``/clock_query`` round trip; returns the anchor's uncertainty (s)."""
        t0 = time.monotonic()
        self._iface.send_msg(self.target, "/clock_query")
        packet = self._iface.recv(self._timeout)
        t1 = time.monotonic()
        if packet is None:
            raise TimeoutError("no /clock_query.reply (is the server on UDP?)")
        addr, args = _osclib.decode(packet)
        if addr != "/clock_query.reply":
            raise RuntimeError(f"expected /clock_query.reply, got {addr}")
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
        """A `SampleClockTimebase` reading this tracker's model."""
        return SampleClockTimebase(self.now, self.rate)

    def close(self):
        self.untrack()
        self._iface.close()
        self.model.close()


class EmbedSampleClock:
    """The in-process counterpart of `UdpSampleClock`: reads an embedded
    server's sample counter straight from its handle (`clausters.ipc.Clausters`
    or `ShmClient` — anything with ``clock`` and ``sample_rate``).

    There is nothing to track: the counter is shared memory, so `anchor` /
    `warmup` / `track` are trivial no-ops kept only for surface parity with the
    UDP tracker, and they never block or time out. `close` releases nothing —
    the handle belongs to the interface that opened it.
    """

    def __init__(self, handle):
        self._handle = handle
        self._rate = float(handle.sample_rate)

    def anchor(self) -> float:
        # A direct read has no round trip: probe the handle once (a closed or
        # dead handle raises here, which lock_to turns into a graceful
        # fall-back) and report zero uncertainty.
        self._handle.clock
        return 0.0

    def warmup(self, n: int = 5, gap: float = 0.05) -> float:
        return 0.0

    def track(self, interval: float = 0.5):
        return self

    def untrack(self):
        pass

    def now(self) -> int:
        return self._handle.clock

    @property
    def rate(self) -> float:
        return self._rate

    def timebase(self) -> SampleClockTimebase:
        """A `SampleClockTimebase` reading the handle's counter directly."""
        return SampleClockTimebase(lambda: self._handle.clock, self._rate)

    def close(self):
        pass
