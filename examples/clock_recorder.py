#!/usr/bin/env python3
"""Shared-memory sample clock, made audible and *checkable* (M8 + M14).

This demo turns the server's sample clock — read straight out of shared
memory, with no round trip — into something you can verify with your ears
and a recording: it schedules a **pristine one-sample impulse exactly every
N samples** and records the real audio output while it plays. Afterwards it
scans the recording and measures how evenly the impulses actually landed.

Each marker is the `Impulse` UGen at frequency 0, which emits a single 1.0
on the synth's first sample and silence after. A `/sched`'d `/s_new` splits
the processing block at the target sample, so that first sample *is* the
target: one clean impulse on an exact frame, with no envelope and no onset
ramp to blur where it landed.

The point it proves: `ShmClient.clock` mirrors the engine's processed-sample
counter every block, so the client always knows *which sample the engine is
on* without asking. Picking a target sample and firing `/sched <target>`
ahead of time then makes every impulse land sample-accurately. Two impulses
N samples apart in the schedule come out N samples apart in the recording —
the spacing never goes through this machine's wall clock, only the audio
clock. (Contrast `examples/sample_clock.py`, which has to *model* the clock
from `/clock` round trips because it talks UDP; here the clock is just a
field in mapped memory.)

Run (real audio hardware required — the sandbox has no output device):

    cargo run --release -- --shm /dev/shm/clausters     # terminal 1
    python3 examples/clock_recorder.py --seconds 20      # terminal 2

The duration is free: `--seconds 5` for a quick check, `--seconds 7200` to
let it run two hours and watch the jitter stay flat. Scheduling stays a fixed
distance ahead of the clock, so memory is bounded no matter how long it runs.

Recording uses `pw-record` (PipeWire). By default it captures the server's
**own output node** directly: it finds the clausters playback stream via
`pw-dump` and taps its output ports, so the recording is exactly what the
server emits regardless of which sink it is routed to (more robust than the
sink monitor, which only sees the server if it is mixed into that sink). If
the node is not found it falls back to the default sink's monitor (detected
via `wpctl`/`pactl`). Override the source with `--target <node-name>`, the
whole command with `--record-cmd` (for ALSA/PulseAudio/CoreAudio), or use
`--no-record` to only schedule (and analyze a separately captured file later
with `--analyze file.wav`). The sample rate you see is the device's real
rate (e.g. 44100 Hz), read from the segment — recording matches it.
"""

import argparse
import json
import math
import os
import signal
import subprocess
import sys
import time
import wave
from array import array

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))
import json_client as osc  # stdlib-only OSC encode/decode helpers
from clausters import ShmClient

DEF_NAME = "clkimpulse"
NODE_BASE = 4000
NODE_SPAN = 1024  # node ids cycle here; each synth is freed long before reuse


# --------------------------------------------------------------------------
# Scheduling: drive the server through shared memory.
# --------------------------------------------------------------------------


def define_impulse(c: ShmClient, amp: float):
    """`Impulse.ar(0) · amp` out to both channels: a single 1.0·amp on the
    synth's first sample, silence after. Started with `/s_new` at the target
    sample (the engine splits the block there), that one impulse marks the
    exact frame — no envelope, no onset ramp."""
    d = osc.SynthDefBuilder(DEF_NAME)
    sig = d.add("Mul", d.add("Impulse", 0.0), amp)
    d.add("Out", 0, sig)
    d.add("Out", 1, sig)
    addr, args = osc.decode(c.request(osc.message("/d_recv", d.blob())))
    if addr == "/done":
        return
    # A `/fail` almost always means the running server predates the `Impulse`
    # UGen: a stale process keeps serving even after you rebuild. Say so.
    reason = args[-1] if args else ""
    raise RuntimeError(
        f"/d_recv failed: {reason}\n"
        "the running server rejected the Impulse UGen — it is probably a stale "
        "process from before the rebuild. Restart it with the freshly built "
        "binary (e.g. `cargo run --release -- --shm /dev/shm/clausters`) and "
        "run this again.")


def send_blocking(c: ShmClient, packet: bytes, drain: bool):
    """Push one packet, retrying on ring backpressure. Drain any server
    replies meanwhile so the reply ring never fills (a `/sched` only replies
    on error — a `/fail` here means a malformed packet, worth surfacing)."""
    while not c.send(packet):
        if drain:
            _drain(c)
        time.sleep(0.0005)


def _drain(c: ShmClient):
    while (reply := c.poll()) is not None:
        addr, args = osc.decode(reply)
        if addr == "/fail":
            print(f"  server /fail: {args}", file=sys.stderr)


def schedule_impulses(c: ShmClient, *, seconds: float, period: int, hold: int,
                      amp: float, lead: float) -> tuple[int, int, float]:
    """Schedule an impulse every `period` samples for `seconds`, staying
    `lead` seconds ahead of the clock. Returns (count, first_target, rate)."""
    rate = c.sample_rate
    define_impulse(c, amp)

    start = c.clock + int(0.5 * rate)  # first impulse half a second out
    count = int(seconds * rate // period)
    lead_samples = int(lead * rate)
    print(f"sample rate: {rate:.0f} Hz   period: {period} samples "
          f"({period / rate * 1e3:.1f} ms)")
    print(f"scheduling {count} impulses over ~{seconds:.0f} s, first at sample {start}")

    for k in range(count):
        target = start + k * period
        # Wait until the clock (read directly from shared memory) is within
        # `lead` of the target, then schedule. No /clock round trip.
        while True:
            ahead = target - lead_samples - c.clock
            if ahead <= 0:
                break
            time.sleep(min(ahead / rate, 0.1))
        node = NODE_BASE + (k % NODE_SPAN)
        # The impulse fires on the synth's first sample (= the target); `hold`
        # samples later it is pure silence, so free it to keep the tree clean.
        on = osc.message("/s_new", DEF_NAME, node, 1, 0)
        off = osc.message("/n_free", node)
        send_blocking(c, osc.message("/sched", osc.Int64(target), on), drain=True)
        send_blocking(c, osc.message("/sched", osc.Int64(target + hold), off), drain=True)
        if k % 50 == 0 or k == count - 1:
            print(f"  scheduled impulse {k + 1}/{count} at sample {target} "
                  f"(clock now {c.clock})", end="\r", flush=True)
    print()

    # Let the last impulse play out: the clock counts *processed* samples and
    # runs a device buffer ahead of the speakers, so wait past it plus a tail.
    last_end = start + (count - 1) * period + hold
    while c.clock < last_end + int(0.5 * rate):
        _drain(c)
        time.sleep(0.05)
    return count, start, rate


# --------------------------------------------------------------------------
# Recording: capture the real output.
# --------------------------------------------------------------------------


def find_server_node() -> str | None:
    """The clausters playback stream's PipeWire `node.name`, via `pw-dump`.
    pw-record taps a stream's output ports when `--target` names it, so this
    records exactly what the server emits — independent of routing, and
    immune to the server not being mixed into the default sink."""
    try:
        data = json.loads(subprocess.check_output(["pw-dump"], stderr=subprocess.DEVNULL))
    except (OSError, subprocess.CalledProcessError, ValueError):
        return None
    for obj in data:
        if obj.get("type") != "PipeWire:Interface:Node":
            continue
        props = (obj.get("info") or {}).get("props") or {}
        if "Stream/Output/Audio" not in props.get("media.class", ""):
            continue
        haystack = " ".join(str(props.get(k, "")) for k in (
            "application.name", "node.name", "application.process.binary",
            "node.description", "media.name")).lower()
        if "clausters" in haystack:
            return props.get("node.name") or str(props.get("object.serial", ""))
    return None


def default_sink_node() -> str | None:
    """The default sink's PipeWire `node.name`. pw-record captures a sink's
    **monitor** when `--target` names the sink node (no `.monitor` suffix —
    that is the PulseAudio/`parec` convention, not pw-record's). Tries the
    PipeWire-native `wpctl` first, then `pactl` (PulseAudio/pipewire-pulse)."""
    try:
        out = subprocess.check_output(
            ["wpctl", "inspect", "@DEFAULT_AUDIO_SINK@"], stderr=subprocess.DEVNULL
        ).decode()
        for line in out.splitlines():
            key = line.strip().lstrip("* ").strip()
            if key.startswith("node.name") and "=" in key:
                return key.split("=", 1)[1].strip().strip('"')
    except (OSError, subprocess.CalledProcessError):
        pass
    try:
        sink = subprocess.check_output(
            ["pactl", "get-default-sink"], stderr=subprocess.DEVNULL
        ).decode().strip()
        return sink or None
    except (OSError, subprocess.CalledProcessError):
        return None


def build_record_cmd(out: str, rate: int, target: str | None) -> list[str]:
    """A pw-record command to a 16-bit WAV. `target` is a node name: a stream
    (taps its output ports) or a sink (taps its monitor ports)."""
    cmd = ["pw-record", "--rate", str(rate), "--channels", "1", "--format", "s16"]
    if target:
        cmd += ["--target", target]
    return cmd + [out]


def start_recorder(cmd: list[str]) -> subprocess.Popen:
    print(f"recording: {' '.join(cmd)}")
    try:
        return subprocess.Popen(cmd)
    except OSError as e:
        sys.exit(f"could not start recorder ({e}). Pass --record-cmd or --no-record.")


def stop_recorder(proc: subprocess.Popen):
    # SIGINT lets pw-record finalize the WAV header cleanly.
    proc.send_signal(signal.SIGINT)
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.terminate()
        proc.wait(timeout=5)


# --------------------------------------------------------------------------
# Analysis: how evenly did the impulses land?
# --------------------------------------------------------------------------


def _chunks(w: wave.Wave_read, frames_per_chunk: int = 1 << 20):
    """Yield channel-0 samples as normalized floats, streaming so multi-hour
    files never load whole into memory."""
    width = w.getsampwidth()
    channels = w.getnchannels()
    typecode = {1: "b", 2: "h", 4: "i"}.get(width)
    if typecode is None:
        sys.exit(f"unsupported WAV sample width: {width} bytes")
    full_scale = float(1 << (8 * width - 1))
    while raw := w.readframes(frames_per_chunk):
        samples = array(typecode)
        samples.frombytes(raw)
        if width == 1:  # 8-bit WAV is unsigned
            samples = array("h", (s - 128 for s in samples))
            full_scale = 128.0
        ch0 = samples[::channels] if channels > 1 else samples
        yield [s / full_scale for s in ch0]


def find_onsets(path: str, threshold_frac: float, refractory: int) -> tuple[list[int], float]:
    """Two streaming passes: find the peak, then the leading edge of every
    impulse (latched, with hysteresis and a refractory gap, so any sinc-like
    ringing the audio path adds around a sample cannot re-trigger). For the
    direct node capture each impulse is a single sample. Returns (onsets, peak)."""
    with wave.open(path, "rb") as w:
        peak = max((max((abs(x) for x in chunk), default=0.0) for chunk in _chunks(w)),
                   default=0.0)
    if peak <= 0.0:
        return [], 0.0
    hi = threshold_frac * peak
    lo = 0.5 * hi
    onsets: list[int] = []
    armed = True  # ready to fire; disarmed until the level drops back below lo
    i = 0
    last = -refractory
    with wave.open(path, "rb") as w:
        for chunk in _chunks(w):
            for x in chunk:
                a = abs(x)
                if armed:
                    if a >= hi and i - last >= refractory:
                        onsets.append(i)
                        last = i
                        armed = False
                elif a < lo:
                    armed = True
                i += 1
    return onsets, peak


def survey(path: str, abs_threshold: float = 0.01) -> tuple[int, float, list[float]]:
    """A diagnostic, independent of the relative threshold: count distinct
    transient regions above a low absolute level and time the first few.
    Tells us whether the recording actually holds the impulses at all."""
    regions = 0
    in_region = False
    i = 0
    rec_rate = 1
    first_times: list[float] = []
    rms_acc = 0.0
    n = 0
    with wave.open(path, "rb") as w:
        rec_rate = w.getframerate()
        gap = max(1, rec_rate // 20)  # 50 ms minimum spacing between regions
        last = -gap
        for chunk in _chunks(w):
            for x in chunk:
                rms_acc += x * x
                n += 1
                a = abs(x)
                if not in_region and a >= abs_threshold and i - last >= gap:
                    regions += 1
                    last = i
                    in_region = True
                    if len(first_times) < 8:
                        first_times.append(i / rec_rate)
                elif in_region and a < abs_threshold * 0.5:
                    in_region = False
                i += 1
    rms = math.sqrt(rms_acc / n) if n else 0.0
    return regions, rms, first_times


def _linfit(xs: list[float], ys: list[float]) -> tuple[float, float]:
    """Least-squares slope and intercept of ys against xs."""
    n = len(xs)
    mx, my = sum(xs) / n, sum(ys) / n
    var = sum((x - mx) ** 2 for x in xs)
    cov = sum((x - mx) * (y - my) for x, y in zip(xs, ys))
    slope = cov / var if var else 0.0
    return slope, my - slope * mx


def analyze(path: str, *, period: int, server_rate: float, expected: int | None,
            threshold_frac: float) -> bool:
    """Scan the recording and report impulse spacing, jitter and drift."""
    with wave.open(path, "rb") as w:
        rec_rate = w.getframerate()
        rec_frames = w.getnframes()
    print(f"\nanalyzing {path}: {rec_rate} Hz, {rec_frames} frames "
          f"({rec_frames / rec_rate:.1f} s)")

    # The recorder's clock may differ from the server's; the spacing we expect
    # in *recorded* samples scales by the rate ratio.
    expected_gap = period * rec_rate / server_rate
    refractory = max(1, int(expected_gap * 0.5))
    onsets, peak = find_onsets(path, threshold_frac, refractory)
    print(f"detected {len(onsets)} impulses (peak {peak:.3f} of full scale)")

    # Diagnostic: when far fewer impulses turn up than were scheduled, scan
    # once at a low absolute level to see what the recording really holds.
    if expected is not None and len(onsets) < 0.8 * expected:
        regions, rms, times = survey(path)
        head = ", ".join(f"{t:.2f}s" for t in times)
        print(f"  diagnostic: {regions} transient regions above 0.01, "
              f"overall rms {rms:.4f}; first onsets at [{head}]")
        if regions < 0.8 * expected:
            print("  -> the recording is missing most impulses even though you "
                  "hear them: the captured node is not the server's output. "
                  "Pass --target with the clausters node (see `pw-dump`/`wpctl "
                  "status`), or record its output node directly.")

    if len(onsets) < 2:
        print("FAIL: need at least two impulses to measure spacing — was the "
              "right node captured? (try --target, or --record-cmd)")
        return False

    gaps = [onsets[i + 1] - onsets[i] for i in range(len(onsets) - 1)]
    mean = sum(gaps) / len(gaps)
    std = math.sqrt(sum((g - mean) ** 2 for g in gaps) / len(gaps))
    ks = [float(k) for k in range(len(onsets))]
    slope, _ = _linfit(ks, [float(o) for o in onsets])  # samples per impulse
    # Jitter = scatter of onsets around the fitted straight line.
    fit_resid = [onsets[k] - (slope * k + onsets[0]) for k in range(len(onsets))]
    jitter = math.sqrt(sum(r * r for r in fit_resid) / len(fit_resid))
    drift_ppm = (slope / expected_gap - 1.0) * 1e6

    to_ms = 1e3 / rec_rate
    print(f"  expected gap : {expected_gap:10.2f} samples ({expected_gap * to_ms:.2f} ms)")
    print(f"  measured gap : {mean:10.2f} samples ({mean * to_ms:.2f} ms)  "
          f"min {min(gaps)} / max {max(gaps)}")
    print(f"  jitter (rms) : {jitter:10.2f} samples ({jitter * to_ms:.3f} ms) "
          f"around the fitted line")
    print(f"  drift (slope): {drift_ppm:+10.1f} ppm vs the scheduled period")

    ok = True
    if expected is not None and not (expected - 2 <= len(onsets) <= expected):
        print(f"  note: expected ~{expected} impulses, found {len(onsets)} "
              "(threshold? clipped recording? overlapping impulses?)")
        ok = False
    if jitter * to_ms > 2.0:
        print("  note: jitter above 2 ms — onset detection is noisy or the "
              "audio path is loose; inspect the WAV.")
        ok = False
    print("PASS: impulses are evenly spaced — the shared-memory clock drove "
          "sample-accurate scheduling." if ok else
          "see notes above; open the WAV to inspect.")
    return ok


# --------------------------------------------------------------------------


def parse_args(argv):
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--shm", default="/dev/shm/clausters", help="server segment path")
    p.add_argument("--seconds", type=float, default=20.0,
                   help="test duration (a few seconds to many hours)")
    p.add_argument("--period", type=int, default=24000,
                   help="samples between impulses (default 24000 = 0.5 s @ 48 kHz)")
    p.add_argument("--hold-ms", type=float, default=2.0,
                   help="how long to keep each impulse synth before /n_free")
    p.add_argument("--amp", type=float, default=0.5, help="impulse amplitude 0..1")
    p.add_argument("--lead", type=float, default=1.0,
                   help="seconds to schedule ahead of the clock")
    p.add_argument("--out", default="/tmp/clausters_clock.wav", help="recording path")
    p.add_argument("--target", default=None,
                   help="recorder node (default: the server's output node)")
    p.add_argument("--record-cmd", default=None,
                   help="full recorder command; {out} is replaced by --out")
    p.add_argument("--no-record", action="store_true",
                   help="only schedule; do not record or analyze")
    p.add_argument("--analyze", metavar="WAV", default=None,
                   help="skip the server: just analyze an existing recording")
    p.add_argument("--threshold", type=float, default=0.2,
                   help="onset threshold as a fraction of the recording's peak")
    p.add_argument("--server-rate", type=float, default=48000.0,
                   help="server sample rate, only for --analyze")
    return p.parse_args(argv)


def main(argv):
    args = parse_args(argv)

    # Analyze-only: no server, no recording — re-check a captured file.
    if args.analyze:
        ok = analyze(args.analyze, period=args.period, server_rate=args.server_rate,
                     expected=None, threshold_frac=args.threshold)
        return 0 if ok else 1

    try:
        c = ShmClient(args.shm)
    except (FileNotFoundError, ValueError) as e:
        sys.exit(f"{e}\nstart the server first: "
                 f"cargo run --release -- --shm {args.shm}")
    print(f"attached to {args.shm}")
    rate_int = int(round(c.sample_rate))
    hold = max(1, int(args.hold_ms * 1e-3 * c.sample_rate))

    recorder = None
    if not args.no_record:
        if args.record_cmd:
            cmd = args.record_cmd.replace("{out}", args.out).split()
        else:
            if args.target:
                target = args.target
                print(f"capturing target: {target}")
            elif (node := find_server_node()) is not None:
                target = node
                print(f"capturing the server's output node directly: {target}")
            elif (sink := default_sink_node()) is not None:
                target = sink
                print(f"server node not found; capturing the monitor of sink: {sink}")
            else:
                sys.exit(
                    "could not find the clausters output node or a default sink "
                    "(need pw-dump, wpctl or pactl). Recording the default source "
                    "would capture the mic, not the server. Run `wpctl status` to "
                    "find the target and pass it with --target, or override the "
                    "whole command with --record-cmd.")
            cmd = build_record_cmd(args.out, rate_int, target)
        recorder = start_recorder(cmd)
        time.sleep(0.5)  # let the capture stream settle before the first impulse

    try:
        count, _, server_rate = schedule_impulses(
            c, seconds=args.seconds, period=args.period, hold=hold,
            amp=args.amp, lead=args.lead)
    except RuntimeError as e:
        sys.exit(str(e))  # clean message, not a traceback (finally still runs)
    finally:
        if recorder:
            stop_recorder(recorder)
        c.close()

    if recorder:
        return 0 if analyze(args.out, period=args.period, server_rate=server_rate,
                            expected=count, threshold_frac=args.threshold) else 1
    print("done (no recording requested).")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
