#!/usr/bin/env python3
"""Long-running sample-clock check, driven by the high-level Python client.

Locks a `TempoClock` to a running server's **sample clock** (`lock_to`, over
UDP, modelling `/clock_query` — no shared memory) and runs a `Routine` that, once per
second, spawns a synth of a single one-sample-impulse **SynthDef** and fires it
sample-accurately, then frees the synth. The def is defined and loaded **once**
before the run — as on scsynth, a SynthDef is sent to the server one time and
each beat only instantiates a node with `/synth_new`. The server's audio output is
recorded to a WAV in real time; when the run finishes — or on SIGINT / SIGTERM —
the recording is analyzed to confirm there is exactly one one-sample click per
second, evenly spaced.

Each impulse is `Impulse` at frequency 0: a single 1.0 on the synth's first
sample, silence after. Scheduled by `/sched_at` at the sample the routine's logical
beat maps to, that first sample *is* the target, so the click lands on an exact
frame — no envelope, no onset ramp.

Run (real audio hardware required):

    clausters                              # terminal 1: the installed server binary
    python examples/impulse_clock_test.py --seconds 3700 --out clicks.wav

The duration is free: a few seconds for a quick check, `--seconds 7200` for two
hours. Scheduling stays a fixed lead ahead of the clock, so memory is bounded no
matter how long it runs (the single def stays loaded; one or two synths are
alive at a time).

`--live` adds a parallel monitor: a thread tails the growing recording and
reports the click count and spacing every few seconds, so a failure (clicks
stop, spacing drifts) shows up within seconds instead of after the whole run.
The end-of-run analysis is still the authoritative check.

Recording and the final analysis reuse `examples/clock_recorder.py` (pw-record,
the impulse-spacing scan); this file only swaps the low-level shared-memory
scheduling there for the high-level client (Server + lock_to + Routine + SynthDef).
"""

import argparse
import os
import signal
import sys
import threading
import time
import wave
from array import array

_HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, _HERE)  # clock_recorder + json_client live here
sys.path.insert(0, os.path.join(_HERE, "..", "clients", "python"))

import clock_recorder as rec  # noqa: E402  (recording + final analysis, reused)
from clausters.base import Routine, SampleClockTimebase, TempoClock  # noqa: E402
from clausters.defs import Server, SynthDef, impulse, out  # noqa: E402

#: Where a run leaves its file when no path is given: ``examples/out/``, the
#: git-ignored directory every generator in this tree writes to -- beside the
#: examples rather than in whatever directory you ran from.
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "out")
os.makedirs(OUT, exist_ok=True)

NODE_BASE = 4000
NODE_SPAN = 1024  # node ids cycle here; each synth is freed long before reuse
HOLD = 0.1        # beats: free the synth this long after its impulse
IMPULSE_DEF = "clk_impulse"  # the single def, defined and loaded once up front


# --------------------------------------------------------------------------
# Driving the synthesis through the high-level client.
# --------------------------------------------------------------------------


def impulse_def(name: str) -> SynthDef:
    """A def that emits one one-sample impulse: `Impulse(0)` out to both
    channels (the shared node is emitted once, fanned to two `Out`s)."""
    sig = impulse(0.0)
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


def impulse_routine(server: Server, counter: list):
    """Once per beat (= one second at tempo 1.0): spawn a synth of the
    pre-loaded impulse `SynthDef` at the routine's exact logical beat (so
    `/sched_at` lands it on the sample) and free it shortly after. The def is
    defined and loaded once before the run, so the loop only sends `/synth_new` +
    `/node_free` — never a def — and never blocks the clock thread."""

    def routine():
        k = 0
        while True:
            node = NODE_BASE + (k % NODE_SPAN)
            server.send_bundle(("/synth_new", IMPULSE_DEF, node, 1, 0))   # sample-accurate
            server.send_bundle(("/node_free", node), delay_beats=HOLD)   # tidy up the tree
            counter[0] = k + 1
            yield 1.0                                                 # complete the 1 s cycle
            k += 1

    return routine


# --------------------------------------------------------------------------
# Optional live monitor: tail the growing recording, report early.
# --------------------------------------------------------------------------


def _wav_data_region(f):
    """Find the PCM `data` chunk in a (possibly still-growing) WAV: returns
    `(data_offset, channels, sampwidth)` or `None` if the header is not there
    yet."""
    f.seek(0)
    if f.read(12)[:4] != b"RIFF":
        return None
    channels = width = None
    while True:
        head = f.read(8)
        if len(head) < 8:
            return None
        cid, size = head[:4], int.from_bytes(head[4:8], "little")
        if cid == b"fmt ":
            fmt = f.read(size)
            channels = int.from_bytes(fmt[2:4], "little")
            width = int.from_bytes(fmt[14:16], "little") // 8
        elif cid == b"data":
            return (f.tell(), channels or 1, width or 2)
        else:
            f.seek(size + (size & 1), 1)


class LiveMonitor(threading.Thread):
    """Tails the recording while it is written and prints a running tally of
    clicks and their spacing, so a stalled or drifting run is caught early.
    Streaming onset detection with hysteresis + a refractory gap, on an
    absolute threshold (the impulse is full scale, so no global peak needed)."""

    def __init__(self, path, server_rate, stop, report_every=10.0, threshold=0.2):
        super().__init__(daemon=True)
        self.path, self.server_rate, self.stop = path, server_rate, stop
        self.report_every = report_every
        self.hi, self.lo = threshold, 0.5 * threshold
        self.onsets = []  # recorded-sample index of each click

    def run(self):
        # Wait for the header, then open and seek to the audio data.
        region = None
        while not self.stop.is_set() and region is None:
            if os.path.exists(self.path) and os.path.getsize(self.path) > 64:
                with open(self.path, "rb") as f:
                    region = _wav_data_region(f)
            if region is None:
                time.sleep(0.2)
        if region is None:
            return
        data_off, channels, width = region
        typecode = {1: "b", 2: "h", 4: "i"}.get(width, "h")
        full = float(1 << (8 * width - 1))
        with wave.open(self.path, "rb") as w:
            rec_rate = w.getframerate()
        refractory = max(1, rec_rate // 2)  # at most one click per half second

        armed, last = True, -refractory
        i = 0                       # sample index into the audio data
        reported_at = 0
        leftover = b""
        f = open(self.path, "rb")
        f.seek(data_off)
        bytes_per = width * channels
        try:
            while not self.stop.is_set():
                chunk = f.read(1 << 18)
                if not chunk:
                    time.sleep(0.3)
                    self._maybe_report(i, rec_rate, reported_at)
                    reported_at = i
                    continue
                buf = leftover + chunk
                n = (len(buf) // bytes_per) * bytes_per
                leftover = buf[n:]
                samples = array(typecode)
                samples.frombytes(buf[:n])
                ch0 = samples[::channels] if channels > 1 else samples
                for s in ch0:
                    a = abs(s) / full
                    if armed:
                        if a >= self.hi and i - last >= refractory:
                            self.onsets.append(i)
                            last = i
                            armed = False
                    elif a < self.lo:
                        armed = True
                    i += 1
                if i - reported_at >= self.report_every * rec_rate:
                    self._maybe_report(i, rec_rate, reported_at)
                    reported_at = i
        finally:
            f.close()

    def _maybe_report(self, i, rec_rate, reported_at):
        n = len(self.onsets)
        secs = i / rec_rate
        recent = [b - a for a, b in zip(self.onsets[-11:], self.onsets[-10:])]
        gap = sum(recent) / len(recent) if recent else 0.0
        new = sum(1 for o in self.onsets if o >= reported_at)
        flag = "" if new else "  <-- no new clicks this interval!"
        print(f"  [monitor t={secs:6.1f}s] clicks={n:5d}  recent gap≈{gap:9.1f} "
              f"samples (≈{gap / rec_rate:.3f}s)  +{new} since last{flag}", flush=True)


# --------------------------------------------------------------------------
# Orchestration.
# --------------------------------------------------------------------------


def parse_args(argv):
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--host", default="127.0.0.1")
    p.add_argument("--port", type=int, default=57110)
    p.add_argument("--latency", type=float, default=0.1,
                   help="scheduling lookahead added to each /sched_at target")
    p.add_argument("--seconds", type=float, default=3700.0,
                   help="run duration (one impulse per second)")
    p.add_argument("--out", default=os.path.join(OUT, "impulse_clock.wav"))
    p.add_argument("--target", default=None,
                   help="recorder node (default: the server's output node)")
    p.add_argument("--record-cmd", default=None,
                   help="full recorder command; {out} is replaced by --out")
    p.add_argument("--no-record", action="store_true")
    p.add_argument("--analyze", metavar="WAV", default=None,
                   help="skip the server: just analyze an existing recording")
    p.add_argument("--server-rate", type=float, default=48000.0,
                   help="server sample rate, only for --analyze")
    p.add_argument("--threshold", type=float, default=0.2,
                   help="onset threshold as a fraction of the recording's peak")
    p.add_argument("--live", action="store_true",
                   help="tail the recording and report clicks while it runs")
    p.add_argument("--report-every", type=float, default=10.0,
                   help="seconds between --live reports")
    return p.parse_args(argv)


def _pick_target(args, rate_int):
    if args.record_cmd:
        return args.record_cmd.replace("{out}", args.out).split()
    if args.target:
        target = args.target
        print(f"capturing target: {target}")
    elif (node := rec.find_server_node()) is not None:
        target = node
        print(f"capturing the server's output node directly: {target}")
    elif (sink := rec.default_sink_node()) is not None:
        target = sink
        print(f"server node not found; capturing the monitor of sink: {sink}")
    else:
        sys.exit("could not find the clausters output node or a default sink "
                 "(need pw-dump, wpctl or pactl). Pass --target or --record-cmd.")
    return rec.build_record_cmd(args.out, rate_int, target)


def main(argv):
    args = parse_args(argv)

    if args.analyze:
        period = int(round(args.server_rate))
        ok = rec.analyze(args.analyze, period=period, server_rate=args.server_rate,
                         expected=None, threshold_frac=args.threshold)
        return 0 if ok else 1

    server = Server(args.host, args.port, latency=args.latency)
    clock = TempoClock(tempo=1.0)            # 1 beat = 1 second
    print(f"locking the clock to {args.host}:{args.port} ...")
    clock.lock_to(server)
    if not isinstance(clock.timebase, SampleClockTimebase):
        server.close()
        sys.exit("could not lock to the server's sample clock — is a server "
                 "running on UDP at that address? (start the installed `clausters`)")
    rate = clock.timebase.sample_rate
    period = int(round(rate))                # SR samples between impulses
    print(f"locked: sample rate {rate:.0f} Hz, one impulse every {period} samples (1 s)")

    recorder = monitor = None
    stop = threading.Event()
    if not args.no_record:
        recorder = rec.start_recorder(_pick_target(args, int(round(rate))))
        time.sleep(0.5)                      # let the capture stream settle
        if args.live:
            monitor = LiveMonitor(args.out, rate, stop, args.report_every, args.threshold)
            monitor.start()

    # SIGINT / SIGTERM end the run cleanly (finalize the WAV, then analyze).
    signal.signal(signal.SIGINT, lambda *_: stop.set())
    signal.signal(signal.SIGTERM, lambda *_: stop.set())

    counter = [0]
    impulse_def(IMPULSE_DEF).send(server)   # define + load the def once, up front
    print(f"running for {args.seconds:.0f} s (Ctrl-C to stop early and analyze) ...")
    clock.play(Routine(impulse_routine(server, counter)))
    clock.start()
    deadline = time.monotonic() + args.seconds
    try:
        while time.monotonic() < deadline and not stop.is_set():
            time.sleep(0.25)
    finally:
        stop.set()
        clock.close()                        # stop the clock + release the tracker
        if monitor is not None:
            monitor.join(timeout=2.0)
        if recorder is not None:
            time.sleep(0.5)                  # let the tail of the last impulse land
            rec.stop_recorder(recorder)
        server.free_def(IMPULSE_DEF)         # the def loaded once, freed once
        server.close()

    print(f"fired {counter[0]} impulses.")
    if recorder is None:
        print("done (no recording requested).")
        return 0
    ok = rec.analyze(args.out, period=period, server_rate=rate,
                     expected=counter[0], threshold_frac=args.threshold)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
