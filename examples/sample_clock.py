#!/usr/bin/env python3
"""Sample-clock client (M8): the server's sample counter as master timebase.

The OS clock and the DAC crystal drift apart (tens of ppm — milliseconds per
minute), so NTP-timetagged bundles re-anchor every event against two clocks
that disagree. This client inverts the relationship: it queries the server's
sample counter with `/clock`, models

    sample(t_local) = a + b * t_local

from (local monotonic time, counter) anchor pairs — a least-squares line
over a sliding window, in the spirit of JACK's DLL and Ableton Link — and
schedules everything **in samples** with `/sched`. Two properties fall out:

- Query latency does not matter: an anchor only needs *bounded* uncertainty
  (the round trip brackets the counter read), because scheduling happens
  ahead of time. An anchor error shifts the whole grid by a constant.
- Relative timing is sample-exact *by construction*: two targets N samples
  apart fire exactly N samples apart — no clock conversion in between.

Run a server first (`cargo run --release`), then:

    python3 examples/sample_clock.py

It prints the measured drift between the two clocks and plays a strictly
periodic 8-note pattern scheduled entirely on the sample clock.
See `docs/sample-clock.md` for the protocol.
"""

import sys
import time

import json_client as osc  # the stdlib-only OSC helpers next to this file

BEATS = 8
BEAT_SECONDS = 0.4  # converted once to samples; spacing is then exact
NOTE_SECONDS = 0.25
LEAD_SECONDS = 0.3  # how far ahead of each target we send its /sched
FREQS = [262.0, 330.0, 392.0, 523.0, 392.0, 330.0, 262.0, 196.0]


class SampleClock:
    """sample(t_local) = a + b·t_local, fitted from /clock anchors."""

    def __init__(self, client: "osc.Client", window: int = 64):
        self.client = client
        self.window = window
        self.anchors: list[tuple[float, int]] = []  # (t_local, samples)
        self.rate = 48_000.0  # nominal, replaced by the first reply
        self.a = 0.0
        self.b = self.rate

    def anchor(self) -> float:
        """One /clock round trip; returns the anchor's uncertainty (s)."""
        t0 = time.monotonic()
        self.client.send("/clock")
        addr, args = self.client.reply(quiet=True)
        t1 = time.monotonic()
        if addr != "/clock.reply":
            raise RuntimeError(f"expected /clock.reply, got {addr}")
        samples, self.rate = args[0], args[1]
        # The counter was read somewhere inside [t0, t1]: pair it with the
        # midpoint. The half-width is the (bounded!) uncertainty — it only
        # shifts the grid, it does not accumulate. Note the counter also
        # advances in device-buffer jumps (the callback processes blocks in
        # bursts), which is more bounded noise of the same kind: the slope
        # only becomes meaningful once the anchors span a few seconds.
        self.anchors.append(((t0 + t1) / 2, samples))
        self.anchors = self.anchors[-self.window:]  # forgetting
        self._fit()
        return (t1 - t0) / 2

    def span(self) -> float:
        """Seconds covered by the anchor window (the slope's baseline)."""
        return self.anchors[-1][0] - self.anchors[0][0]

    def _fit(self):
        """Least squares over the anchor window (2+ anchors), else nominal."""
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
        self.b = cov / var
        self.a = s_mean - self.b * t_mean

    def now(self) -> int:
        """Predicted current value of the server's sample counter."""
        return round(self.a + self.b * time.monotonic())

    def local_time_of(self, sample: int) -> float:
        """Inverse model: when (monotonic) the counter will reach `sample`."""
        return (sample - self.a) / self.b

    def drift_ppm(self) -> float:
        """Measured slope vs. the server's nominal rate, in parts/million."""
        return (self.b / self.rate - 1.0) * 1e6


def sched(client: "osc.Client", target: int, packet: bytes):
    """/sched: fire `packet` (message or bundle) at an absolute sample."""
    client.send_raw(osc.message("/sched", osc.Int64(target), packet))


def main():
    client = osc.Client()
    clock = SampleClock(client)

    # Warm up the model: a handful of anchors a few tens of ms apart. More
    # spread = better slope estimate; a real client would keep anchoring
    # in the background forever (we re-anchor once per beat below).
    uncertainty = 0.0
    for _ in range(5):
        uncertainty = max(uncertainty, clock.anchor())
        time.sleep(0.05)
    print(f"server sample rate: {clock.rate:.0f} Hz")
    print(f"anchor uncertainty: ±{uncertainty * 1e3:.2f} ms "
          f"(constant grid shift, does not accumulate)")

    step = round(BEAT_SECONDS * clock.rate)  # beat spacing, exact in samples
    dur = round(NOTE_SECONDS * clock.rate)
    start = clock.now() + round(0.5 * clock.rate)  # first beat 0.5 s ahead
    print(f"8 beats of exactly {step} samples, starting at sample {start}")

    for i, freq in enumerate(FREQS):
        target = start + i * step
        node = 3100 + i
        # Send each beat LEAD_SECONDS before its target (scheduling ahead).
        time.sleep(max(0.0, clock.local_time_of(target) - LEAD_SECONDS
                       - time.monotonic()))
        sched(client, target,
              osc.message("/s_new", "default", node, 1, 0,
                          "freq", freq, "amp", 0.25))
        sched(client, target + dur, osc.message("/n_free", node))
        # Keep the model fresh while playing (the "forgetting" part).
        clock.anchor()
        print(f"  beat {i}: sample {target:>10} | freq {freq:>5.0f} Hz | "
              f"slope {clock.drift_ppm():+8.1f} ppm ({clock.span():4.1f} s baseline)")

    # Wait for the tail to play out before exiting.
    end = start + (BEATS - 1) * step + dur
    time.sleep(max(0.0, clock.local_time_of(end) + 0.2 - time.monotonic()))
    print("done: every beat fired on its exact sample — the spacing never")
    print("depended on this machine's clock, only the first anchor did.")
    print("(the slope needs minutes of baseline to resolve real crystal")
    print(" drift, tens of ppm; in this short run it shows the counter's")
    print(" device-buffer quantization instead — bounded, so it only")
    print(" matters for how early a /sched is *sent*, never when it fires.)")


if __name__ == "__main__":
    try:
        main()
    except (TimeoutError, OSError):
        sys.exit("no reply — is the server running? (cargo run --release)")
