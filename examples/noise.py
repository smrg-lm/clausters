#!/usr/bin/env python3
"""The noise sources, and the two things about them that surprise people (U6).

Three shapes with the same name in every synthesis textbook — white, pink,
brown — plus the ones that are noise in a different sense: a random value held
at a rate you choose, impulses that arrive at a *mean* density rather than on a
clock, and a chaotic map with no randomness in it at all.

The surprises are about level, and the report at the end measures them on what
was just rendered rather than asking you to take them on faith: **pink noise is
four times quieter than white** at the same nominal range, while **brown is just
as loud** — it only *sounds* darker, because its energy is all at the bottom.
And **`crackle` carries DC**, because its map takes an absolute value.

It renders **offline**, so it needs no audio hardware and no running server:

    python3 examples/noise.py             # render and report
    python3 examples/noise.py out.wav     # ...and write it

Read it top to bottom; each section is one idea.
"""

import os
import struct
import sys
import wave

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))

from clausters.base import OscNrtInterface, TempoClock
from clausters.defs import Server, SynthDef, control
from clausters.defs.ugens import (
    DoneAction, Env, brown_noise, clip_noise, crackle, decay2, dust, env_gen,
    hpf, impulse, leak_dc, lf_noise1, out, pink_noise, rlpf, white_noise,
)
from clausters.seq import Pbind, Pseq

SR = 48000.0
SECTION = 1.2  # seconds per shape


# ---- 1. the three spectral shapes, one after another --------------------------
#
# Same UGen shape, three spectra: white is flat, pink falls 3 dB per octave,
# brown 6. Played back to back at the *same* nominal amplitude, which is the
# point — and the measured levels separate two things that are easy to conflate.
# Pink really is quieter (a sum of seventeen uniforms spends its time near the
# middle of its range); brown is not quieter at all, it just puts everything it
# has below a few hundred hertz. "Darker" and "softer" are different claims.

def envelope(seconds):
    """A plain rise-hold-fall that frees its own node at the end, so nothing
    here needs a note-off."""
    return env_gen(Env([0.0, 1.0, 1.0, 0.0], [0.02, seconds - 0.1, 0.08]),
                   done_action=DoneAction.FREE_SELF)


def shape(name, sig) -> SynthDef:
    s = sig * envelope(SECTION) * control("amp", 0.5)
    return SynthDef(name, out(0.0, s), out(1.0, s))


# ---- 2. noise as an instrument -------------------------------------------------
#
# A hi-hat is `clip_noise` — the loudest noise there is, every sample at full
# scale — through a highpass and a very short `decay2`. No envelope generator
# and no note: the impulse *is* the note, and `decay2` gives it a shape.
#
# The wind underneath is `brown_noise` through a resonant lowpass whose cutoff
# is itself a slow `lf_noise1`. That is what the LF noises are for: a smooth
# random modulation, not something to listen to directly.

def kit() -> SynthDef:
    clock = impulse(8.0)
    hat = hpf(clip_noise(), 7000.0) * decay2(clock, 0.001, 0.06) * 0.25
    cutoff = lf_noise1(0.4) * 500.0 + 800.0
    wind = rlpf(brown_noise(), cutoff, q=3.0) * 0.6
    sig = (hat + wind) * envelope(2.4) * control("amp", 0.4)
    return SynthDef("kit", out(0.0, sig), out(1.0, sig))


# ---- 3. the two that are not random -------------------------------------------
#
# `dust` fires at a *mean* density: ten per second means ten on average, with
# clusters and gaps, because every sample is an independent trial. Use `impulse`
# when you want them evenly spaced — that difference is audible here.
#
# `crackle` has no RNG at all. The same `chaos` gives the same signal every
# time, so it needs no seed to be reproducible. Its output is one-sided, so it
# carries DC: `leak_dc` before the bus, or it will push everything else off
# centre.

def grit() -> SynthDef:
    sparks = dust(12.0) * decay2(dust(12.0), 0.001, 0.09)
    chatter = leak_dc(crackle(1.7)) * 0.6
    sig = (sparks + chatter) * envelope(SECTION) * control("amp", 0.5)
    return SynthDef("grit", out(0.0, sig), out(1.0, sig))


# ---- 4. render it --------------------------------------------------------------

def render(path=None):
    server = Server(interface=OscNrtInterface())
    for sdef in (
        shape("white", white_noise()),
        shape("pink", pink_noise()),
        shape("brown", brown_noise()),
        grit(),
        kit(),
    ):
        server.add_synthdef(sdef)
    # One event per section, sequenced by a pattern: `instrument` is a pattern
    # like any other key, so a `Pseq` of def names plays them in turn.
    #
    # Note that this is *not* `server.synth()` in a loop. That call is an
    # **immediate** send, which offline means the start of the score — where
    # the setup goes — so five of them would all begin at once whatever the
    # yields in between said. Placing something in time is what a pattern (or
    # `send_bundle`) is for.
    clock = TempoClock(tempo=1.0)
    Pbind(
        instrument=Pseq(["white", "pink", "brown", "grit", "kit"]),
        dur=Pseq([SECTION, SECTION, SECTION, SECTION, 2.4]),
        amp=0.45,
    ).play(clock, server)
    clock.render()
    samples, frames = server.render(sample_rate=SR, channels=2)

    peak = max(abs(s) for s in samples)
    print(f"rendered {frames} frames ({frames / SR:.2f} s) | peak {peak:.3f}")
    if peak == 0.0:
        sys.exit("the render is silent — something is wrong")
    if peak > 1.5:
        sys.exit(f"the render clips hard (peak {peak:.2f})")

    # The claim, measured on what was just rendered.
    for k, name in enumerate(("white", "pink", "brown")):
        lo = int(k * SECTION * SR) * 2 + int(0.1 * SR) * 2
        hi = int((k + 1) * SECTION * SR) * 2 - int(0.2 * SR) * 2
        cut = samples[lo:hi]
        rms = (sum(s * s for s in cut) / len(cut)) ** 0.5
        print(f"  {name:6} rms {rms:.3f}")

    if path:
        with wave.open(path, "wb") as w:
            w.setnchannels(2)
            w.setsampwidth(2)
            w.setframerate(int(SR))
            w.writeframes(
                b"".join(
                    struct.pack("<h", int(max(-1.0, min(1.0, s)) * 32767))
                    for s in samples
                )
            )
        print(f"wrote {path} — listen with: ffplay -autoexit {path}")


if __name__ == "__main__":
    try:
        render(sys.argv[1] if len(sys.argv) > 1 else None)
    except (OSError, RuntimeError) as e:
        sys.exit(str(e))
