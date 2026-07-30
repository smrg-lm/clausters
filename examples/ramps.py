#!/usr/bin/env python3
"""Ramps, and the three ways a synth ends (U4).

A voice has to stop. `env_gen` can free the node itself, but that is not always
where the decision belongs — sometimes the thing that ends the note is a ramp
that is *not* the amplitude envelope, and sometimes it is an ordinary signal.
This example plays the three answers one after the other.

It renders **offline**, so it needs no audio hardware and no running server:

    python3 examples/ramps.py               # render and report
    python3 examples/ramps.py out.wav       # ...and write it

Read it top to bottom; each section is one idea.
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))

from clausters.base import OscNrtInterface, TempoClock
from clausters.defs import Server, SynthDef, control
from clausters.defs.ugens import (
    DoneAction, Env, env_gen, free_self, free_self_when_done, line, lpf, out,
    saw, x_line,
)
from clausters.seq import Pbind, Pseq

SR = 48000.0


# ---- 1. the ramp decides, and something else frees ---------------------------
#
# `x_line` moves in equal *ratios*, which is what a pitch sweep wants: two
# octaves down sounds like two octaves down at every point along the way, where
# a linear ramp would spend most of its time near the bottom.
#
# The interesting part is who ends the note. The amplitude envelope here has
# `DoneAction.NONE` — it is not in charge. `free_self_when_done` watches the
# *pitch* ramp and frees the synth when that finishes. What it reads is the
# ramp's **done flag**, not its value: the ramp ends at 110 Hz, and no test on
# the number 110 would tell you it had arrived rather than passed through.

def zap() -> SynthDef:
    amp = control("amp", 0.2)
    sweep = x_line(1760.0, 110.0, 0.35)
    sig = saw(free_self_when_done(sweep)) * env_gen(Env.perc(0.005, 0.4))
    return SynthDef("zap", out(0.0, sig * amp), out(1.0, sig * amp))


# ---- 2. a ramp at control rate ------------------------------------------------
#
# `line` at `rate="kr"` produces one value per block instead of one per sample —
# a sixty-fourth of the work. It still takes exactly as long in seconds and
# still says its duration in seconds; choosing `kr` changes a ugen's cost, not
# its meaning. A filter cutoff sliding over a second is precisely the case for
# it: nothing audible happens between two blocks.
#
# This one *does* free itself, the ordinary way — the ramp carries a
# `done_action` because `line` is an `env_gen` with its header filled in, so it
# takes the whole set.

def sweep() -> SynthDef:
    amp = control("amp", 0.12)
    cutoff = line(200.0, 6000.0, 1.2, DoneAction.FREE_SELF).at_rate("kr")
    sig = lpf(saw(110.0), cutoff) * env_gen(Env([0.0, 1.0, 1.0, 0.0], [0.05, 1.05, 0.1]))
    return SynthDef("sweep", out(0.0, sig * amp), out(1.0, sig * amp))


# ---- 3. an ordinary signal ends it --------------------------------------------
#
# `free_self` takes no envelope and no flag — just a signal, and it frees the
# node while that signal is above zero, passing it through meanwhile. Here a
# slow ramp crosses a threshold, which is a stand-in for anything a graph can
# compute: a level detector, a counter, a comparison against a control.
#
# `pause_self` is its twin and parks the node instead. Neither latches, so a
# paused node really does resume with `Server.run` and re-pauses only if its
# input is still up.

def fade() -> SynthDef:
    amp = control("amp", 0.15)
    ramp = line(0.0, 1.0, 0.8)
    # Positive from 0.6 s on: the node lives 0.6 s whatever the envelope says.
    # It is passed as a **root** of the def, next to the two `out`s: a def keeps
    # what its roots reach, and this one is wired to nothing downstream, so
    # leaving it as a loose expression would drop it from the graph entirely.
    stop = free_self(ramp - 0.75)
    sig = saw(220.0) * env_gen(Env([0.0, 1.0, 1.0], [0.05, 2.0]))
    return SynthDef("fade", out(0.0, sig * amp), out(1.0, sig * amp), stop)


# ---- 4. render it ------------------------------------------------------------

def render(path=None):
    server = Server(interface=OscNrtInterface())
    for sdef in (zap(), sweep(), fade()):
        server.add_synthdef(sdef)

    clock = TempoClock(tempo=1.0)
    Pbind(instrument="zap", dur=Pseq([0.5, 0.5, 0.5, 1.5]), amp=0.2).play(clock, server)
    Pbind(instrument="sweep", dur=Pseq([3.0]), amp=0.12).play(clock, server)
    Pbind(instrument="fade", dur=Pseq([1.0]), delta=3.0, amp=0.15).play(clock, server)
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
