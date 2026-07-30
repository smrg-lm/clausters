#!/usr/bin/env python3
"""Subtractive synthesis with no Faust in sight (U1-U3).

Everything here is a UGen graph: a band-limited oscillator, a resonant filter
swept by an envelope, and a delay network. That matters because it means the
whole chain is available in a `synth`-only build — the one that runs where there
is no LLVM to JIT a Faust def, including the browser engine.

It renders **offline**, so it needs no audio hardware and no running server:

    python3 examples/subtractive.py             # render and report
    python3 examples/subtractive.py out.wav     # ...and write it

Read it top to bottom; each section is one idea.
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))

from clausters.base import OscNrtInterface, TempoClock
from clausters.defs import AddAction, Server, SynthDef, control
from clausters.defs.ugens import (
    DoneAction, Env, allpass_c, comb_c, env_gen, in_, lf_tri, lpf, out, pulse,
    rlpf, saw, svf, svf_morph,
)
from clausters.seq import Pbind, Pseq

SR = 48000.0


# ---- 1. a saw through a resonant lowpass -------------------------------------
#
# The classic subtractive voice: a harmonically rich oscillator, then a filter
# that decides which of those harmonics you actually hear. `saw` is
# band-limited, so what reaches the filter are the partials that belong there
# rather than aliases folded down from above Nyquist.
#
# The cutoff rides an envelope, which is what makes a note *speak*: it opens on
# the attack and closes over the tail, so the note is bright then dark. Note
# `q=` rather than `rq=` — the wire carries the reciprocal of Q (so that
# "infinite resonance" is exactly representable), and the client converts.

def voice() -> SynthDef:
    freq = control("freq", 220.0)
    amp = control("amp", 0.2)
    sweep = env_gen(Env([8.0, 1.0], [0.6], "exp"))
    sig = rlpf(saw(freq), freq * sweep, q=6.0)
    env = env_gen(Env.perc(0.01, 0.9), done_action=DoneAction.FREE_SELF)
    return SynthDef("sub_voice", out(0.0, sig * env * amp), out(1.0, sig * env * amp))


# ---- 2. the filter's *response* as an automation lane -------------------------
#
# `svf` exposes the three taps of the state-variable filter as ordinary signal
# inputs, so the response is not fixed when the graph is built. `svf_morph`
# turns one position into those three gains: 0 is a lowpass, 1 a bandpass, 2 a
# highpass, and everything in between is a real crossfade rather than a switch.
#
# This costs the mix and nothing else — the three taps come out of the same pair
# of integrator updates — which is why it exists here and has no scsynth name.

def morphing() -> SynthDef:
    freq = control("freq", 110.0)
    amp = control("amp", 0.15)
    pos = lf_tri(0.7) + 1.0          # 0 -> 2: lowpass -> bandpass -> highpass
    sig = svf(pulse(freq, 0.35), 900.0, 0.3, *svf_morph(pos))
    env = env_gen(Env.perc(0.05, 1.2), done_action=DoneAction.FREE_SELF)
    return SynthDef("sub_morph", out(0.0, sig * env * amp), out(1.0, sig * env * amp))


# ---- 3. a delay network ------------------------------------------------------
#
# `comb_c` is a resonator: its decay time is how long the echo train takes to
# fall 60 dB, counted from the first echo (which is the direct path and always
# returns at full level). `allpass_c` scatters phase without touching the
# magnitude *at all*, which is why a chain of them reads as "space" instead of
# as a row of distinct repeats — three at mutually prime delays is about the
# smallest thing that sounds like a room.
#
# `max_delay` sizes the line the server allocates, once, when the synth is
# built. A constant `delaytime` fills it in for you; a modulated one has to say
# how far it will reach, because the line cannot grow later.

def space() -> SynthDef:
    sig = in_(0.0)
    wet = comb_c(sig, 0.093, 2.5)
    for t in (0.0043, 0.0071, 0.0113):
        wet = allpass_c(wet, t, 1.6)
    wet = lpf(wet, 4500.0)           # take the top off, so it sits behind
    return SynthDef("sub_space", out(0.0, wet * control("mix", 0.3)))


# ---- 4. render it ------------------------------------------------------------

def render(path=None):
    server = Server(interface=OscNrtInterface())
    for sdef in (voice(), morphing(), space()):
        server.add_synthdef(sdef)
    # The reverb reads bus 0 and writes back to it, so it has to run *after* the
    # voices: adding it at the tail of the root group is enough.
    server.synth("sub_space", action=AddAction.TAIL)

    clock = TempoClock(tempo=2.0)
    degrees = [0, 3, 5, 7, 10, 12, 10, 7]
    Pbind(
        instrument="sub_voice",
        freq=Pseq([110.0 * 2 ** (d / 12) for d in degrees]),
        dur=0.5,
        amp=0.18,
    ).play(clock, server)
    Pbind(instrument="sub_morph", freq=Pseq([82.5, 110.0]), dur=2.0, amp=0.12).play(
        clock, server
    )
    clock.render()
    stats = server.render(sample_rate=SR, channels=2, path=path)

    peak = max(stats.peak)
    rms = max(stats.rms, default=0.0)
    print(f"rendered {stats.frames} frames ({stats.duration:.2f} s) | peak {peak:.3f} rms {rms:.4f}")
    if peak == 0.0:
        sys.exit("the render is silent — something is wrong")
    if peak > 1.5:
        sys.exit(f"the render clips hard (peak {peak:.2f})")

        print(f"wrote {path} — listen with: pw-play {path}")


if __name__ == "__main__":
    try:
        render(sys.argv[1] if len(sys.argv) > 1 else None)
    except (OSError, RuntimeError) as e:
        sys.exit(str(e))
