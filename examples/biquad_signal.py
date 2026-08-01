#!/usr/bin/env python3
"""A biquad filter built only from Faust Signal API primitives (no Faust source).

This example reproduces `filters.lib`'s `fi.tf2` / `TF2` -- a second-order
direct-form-II biquad -- using *only* the lowercase callables of
`clausters.defs.signals`, which map one-to-one to the bound Faust Signal API.
No `.dsp` text is ever written: the whole DSP is assembled as a Python
expression that becomes the JSON signal tree `/def_send faust` compiles.

The transfer function realized is the standard monic biquad

        b0 + b1 z^-1 + b2 z^-2
    H = ----------------------- ,
        1  + a1 z^-1 + a2 z^-2

with `fi.TF2` written in Faust as `sub ~ conv2(a1,a2) : conv3(b0,b1,b2)`, i.e.

    w[n] = x[n] - a1*w[n-1] - a2*w[n-2]      (feedback / poles)
    y[n] = b0*w[n] + b1*w[n-1] + b2*w[n-2]   (feedforward / zeros)

`~` (one-sample feedback) is `signals.rec`/`self_`; `'` (one-sample memory) is
`signals.delay1`; the arithmetic is just Python operators on `Signal`s.

On top of the raw biquad we cook the coefficients of a resonant low-pass
(Audio EQ Cookbook, RBJ) from `cutoff`/`q` controls -- again only with signal
primitives (`sin`, `cos`, `*`, `/`). The sample rate is read *from the server*
with `signals.sr()` (the port of Faust's `ma.SR`, a foreign constant resolved
when the def compiles), so the filter is in tune at whatever rate the engine
runs -- nothing about SR is baked into the graph. `TWO_PI` is just a literal
(Faust's `ma.PI` is a literal too), so `signals.TAU` is a plain Python float.

To make it audible standalone the def filters an internal sawtooth (rich in
harmonics) and sweeps the cutoff, so a low-pass is plainly heard.

Run offline (no server, renders to a WAV; needs the embed library with faust):

    cargo build --release --features embed,realtime,faust
    python3 examples/biquad_signal.py [out.wav]

Run live (needs a server built with faust in another terminal):

    cargo run --release --features faust        # terminal 1
    python3 examples/biquad_signal.py --live     # terminal 2
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))

from clausters.defs import signals as S
from clausters.defs import FaustDef, Server
from clausters.seq import Event

# Host-side render rate for the offline path: the rate we ask the NRT renderer
# for and write into the WAV header. It is NOT baked into the DSP -- the graph
# reads its rate from the server via `S.sr()`, so changing this stays in tune.
RENDER_SR = 48000.0
FDEF_NAME = "rlpf_signal"

# --- the sequence (shared by the NRT and live paths) ---
CUTOFF_HZ = (400, 900, 1600, 3000, 6000, 3000, 1600, 900, 400)  # the sweep
STEP = 0.25     # beats per note (also each note's length, with legato 1.0)
TEMPO = 1.0     # beats per second
LATENCY = 0.2   # live: place OSC timetags this far ahead (scsynth s.latency)


def biquad(x, b0, b1, b2, a1, a2):
    """A second-order biquad, direct form II -- the heart of the example.

    Pure Signal API: the pole recursion is `rec` (Faust `~`, one implicit
    sample of feedback delay) and the inner taps are `delay1` (Faust `'`).
    `b*`/`a*` may be plain numbers or `Signal`s (so coefficients can be
    modulated). Returns the filtered `Signal`.
    """
    # w[n] = x[n] - a1*w[n-1] - a2*w[n-2].  Inside `rec`, `w1` is the
    # one-sample-delayed output (w[n-1]); `delay1(w1)` is w[n-2].
    w = S.rec(lambda w1: x - (a1 * w1 + a2 * S.delay1(w1)))
    w1 = S.delay1(w)        # w[n-1]
    w2 = S.delay1(w1)       # w[n-2]
    # y[n] = b0*w[n] + b1*w[n-1] + b2*w[n-2].
    return b0 * w + b1 * w1 + b2 * w2


def rbj_lowpass_coeffs(cutoff, q):
    """Cook the 5 monic biquad coefficients of a resonant low-pass from
    `cutoff` (Hz) and `q`, both `Signal`s, with the RBJ Audio EQ Cookbook
    formulas -- built entirely from signal primitives. The sample rate comes
    from the server via `S.sr()` (Faust's `ma.SR`), so the coefficients are
    correct at the engine's actual rate."""
    w0 = cutoff * (S.TAU / S.sr())       # normalized angular frequency
    cosw = S.cos(w0)
    sinw = S.sin(w0)
    alpha = sinw / (q * 2.0)
    a0 = 1.0 + alpha                     # normalization (make denominator monic)
    b1 = (1.0 - cosw) / a0
    b0 = b1 * 0.5                        # b0 = b2 = (1 - cos w0) / 2 / a0
    b2 = b0
    a1 = (-2.0 * cosw) / a0
    a2 = (1.0 - alpha) / a0
    return b0, b1, b2, a1, a2


def build_def(name=FDEF_NAME):
    """A self-contained resonant low-pass synth: internal sawtooth -> biquad.

    Controls: `freq` (saw pitch), `cutoff`, `q`, `amp`. Output is duplicated to
    two channels so it lands on the stereo `out` bus the server adds.
    """
    freq = S.hslider("freq", 110.0, 20.0, 2000.0, 0.01)
    cutoff = S.hslider("cutoff", 800.0, 20.0, 18000.0, 0.01)
    q = S.hslider("q", 4.0, 0.5, 20.0, 0.001)
    amp = S.hslider("amp", 0.2, 0.0, 1.0, 0.0001)

    # Internal source: a naive sawtooth via a phasor (same idiom as the other
    # examples), mapped from [0,1) to [-1,1). Plenty of harmonics to filter.
    phasor = S.rec(lambda s: (s + freq / S.sr()) % 1.0)
    saw = phasor * 2.0 - 1.0

    coeffs = rbj_lowpass_coeffs(cutoff, q)
    out = biquad(saw, *coeffs) * amp
    return FaustDef.from_signals(name, out, out)   # stereo (same on both)


def voice(server):
    """The sequence as a `Routine` body (a generator). For each cutoff value we
    build a note `Event` and call `event.play(server)`: the Event emits the
    bundles for us -- `/synth_new` of our synth with these controls, then a
    scheduled `/node_free` after its sustain -- so we never hand-write a bundle.
    `yield event.delta()` hands control back to the `TempoClock`, which resumes
    us at the next note's exact logical beat (timing is the clock's job, never a
    `time.sleep`). `legato=1.0` makes each note fill its `dur`, so the stepping
    low-pass sounds continuous. The *same* generator drives both paths."""
    for hz in CUTOFF_HZ:
        event = Event(
            instrument=FDEF_NAME,
            freq=110.0,             # saw pitch (a control of the def)
            cutoff=float(hz),       # the moving control
            q=6.0,
            amp=0.3,
            dur=STEP,
            legato=1.0,
        )
        event.play(server)          # Event sends the /synth_new (+ scheduled /node_free)
        yield event.delta()         # advance the clock by dur * stretch


def render_offline(path):
    """NRT path: drive the voice with a clock in non-real time, then render."""
    from clausters.base import Routine, TempoClock, OscNrtInterface

    fdef = build_def()
    server = Server(interface=OscNrtInterface())
    clock = TempoClock(tempo=TEMPO)
    fdef.send(server)             # NRT: scores /def_send faust at time 0
    clock.play(Routine(lambda: voice(server)))
    clock.render()                       # drain the queue in beat order, no sleep
    stats = server.render(sample_rate=int(RENDER_SR), channels=2, path=path)

    peak = max(stats.peak, default=0.0)
    print(f"NRT: {stats.frames} frames, peak {peak:.3f}")
    if path:
        print(f"wrote {path}")


def run_live():
    """RT path: the same voice over UDP, driven by the clock in real time."""
    from clausters.base import Routine, TempoClock

    fdef = build_def()
    server = Server(latency=LATENCY)                # 127.0.0.1:57110
    print("status:", server.status()[:5])
    fdef.send(server)                       # RT: blocks until /done compiles
    clock = TempoClock(tempo=TEMPO)
    clock.play(Routine(lambda: voice(server)))
    total_beats = len(CUTOFF_HZ) * STEP
    # Drive in real time long enough for the last (latency-delayed) note + tail.
    clock.run(clock.beats2secs(total_beats) + LATENCY + 0.3)
    server.sync()
    print("LIVE OK")
    server.close()


def main(argv):
    if "--live" in argv:
        run_live()
    else:
        out = next((a for a in argv[1:] if not a.startswith("-")), None)
        render_offline(out)


if __name__ == "__main__":
    main(sys.argv)
