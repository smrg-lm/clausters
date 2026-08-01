#!/usr/bin/env python3
"""A sequencer that lives inside one synth.

Every other example here drives the server from the client: a pattern computes
an event, the clock sends it, the server plays it. This one sends **one synth,
once**, and never speaks again. The clock, the pitches, the accents, the timbre
changes and the note envelopes are all UGens, so the sequence keeps running with
nothing on the other end of the socket — which is what triggers are for.

It renders **offline**, so it needs no audio hardware and no running server:

    python3 examples/sequencer.py             # render and report
    python3 examples/sequencer.py out.wav     # ...and write it

Read it top to bottom; each section is one idea.
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))

from clausters.base import OscNrtInterface, Routine, TempoClock
from clausters.defs import Server, Synth, SynthDef, control
from clausters.defs.ugens import (
    decay2, demand, dseq, impulse, latch, lpf, out, pulse_divider, rlpf, saw,
    toggle_ff, white_noise,
)

SR = 48000.0


# ---- the patch --------------------------------------------------------------
#
# One `impulse` is the clock, and everything downstream reads it as a
# **trigger** — a signal crossing from at-or-below zero up to above it. That one
# definition is shared by every UGen here, so "on each step" means exactly the
# same thing to the pitch sequence, the accent divider and the sample-and-hold.
#
# Note that the whole graph runs at audio rate, counters included. A control-rate
# UGen reads one sample per block from an audio-rate input, so a `kr` counter fed
# an `ar` clock would see one trigger in 64 and drop the rest: a counter and its
# clock belong at the same rate.

def sequencer() -> SynthDef:
    amp = control("amp", 0.2)
    clock = impulse(control("tempo", 6.0))

    # Pitch: a demand-rate sequence, pulled one value per trigger and held in
    # between. The list loops forever (`repeats=0`).
    degree = demand(clock, 0.0, dseq([0, 3, 5, 7, 10, 12, 10, 3], repeats=0))
    freq = (degree + 45.0).midicps()

    # Accent: one step in four gets a second impulse on top, so it hits harder
    # and rings longer. `start=3` phases the divider to fire on the *first*
    # step rather than the fourth.
    accent = pulse_divider(clock, 4.0, 3.0)

    # The note envelope is not an `env_gen` at all: `decay2` turns each impulse
    # into an exponential with a rounded attack, and its height follows the
    # impulse's, so the accent is louder for free.
    env = decay2(clock + accent, 0.004, 0.22)

    # Timbre: `toggle_ff` flips on every step, and `latch` samples noise once
    # per step and holds it — the classic sample-and-hold filter sweep, which
    # is a *stepped* random rather than a smooth one because the value only
    # moves when a trigger says so.
    bright = toggle_ff(clock)
    cutoff = latch(white_noise() * 1500.0 + 2500.0, clock) * (1.0 + bright)

    sig = rlpf(saw(freq), cutoff, q=5.0) * env * amp
    return SynthDef("seq", out(0.0, sig), out(1.0, sig))


# ---- a second voice, half as fast --------------------------------------------
#
# The same clock divided by four drives a bass an octave and a half down. It is
# the same patch with `pulse_divider` in front of the sequence, which is the
# point of a divider: two voices that cannot drift apart, because they are the
# same clock.

def bass() -> SynthDef:
    amp = control("amp", 0.25)
    clock = pulse_divider(impulse(control("tempo", 6.0)), 4.0, 3.0)
    degree = demand(clock, 0.0, dseq([0, 5], repeats=0))
    env = decay2(clock, 0.01, 0.5)
    sig = lpf(saw((degree + 21.0).midicps()), 900.0) * env * amp
    return SynthDef("seq_bass", out(0.0, sig), out(1.0, sig))


# ---- render it ---------------------------------------------------------------

def play(server):
    """Two synths, sent once, then four seconds of silence on this side. The
    only later message is the one that ends the render."""
    voices = [Synth("seq", {"amp": 0.18}, server=server),
              Synth("seq_bass", {"amp": 0.22}, server=server)]
    yield 4.0
    for v in voices:
        server.send_bundle(("/node_free", v.id))


def render(path=None):
    server = Server(interface=OscNrtInterface())
    for sdef in (sequencer(), bass()):
        sdef.send(server)
    clock = TempoClock(tempo=1.0)
    clock.play(Routine(lambda: play(server)))
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
