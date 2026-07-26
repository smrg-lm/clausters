#!/usr/bin/env python3
"""Placing sound in the stereo field, and the three things that surprise people
about it (U7).

Panning looks like one idea and is really three. **Where** a source sits is
`pan2` (or `pan_az` on a ring of speakers). **How wide** the whole image is has
nothing to do with position — it is the mid/side pair, `stereo_width` for the
knob and `mid_side` when something has to happen in between. And **which of two
sources you hear** is a crossfade, `xfade2` and `select_x`, which is the same
law again pointed at a different question.

The surprises are all about level, and the report at the end measures them on
what was just rendered rather than asking you to take them on faith. Neither
pan law is free; they spend their error in different places, and which one is
wrong for you depends on who is listening:

* **equal power** (`pan2`) holds the level steady in stereo, and its **mono
  fold-down rises 3 dB** as the source reaches the centre;
* **constant amplitude** (`lin_pan2`) holds the *mono* sum steady instead, and
  loses 3 dB of stereo power at the centre;
* `balance2` **costs 3 dB** when centred, because it applies the pan law to a
  pair that is already stereo — it is the one that charges you for doing
  nothing;
* `stereo_width` changes each channel's level and leaves the **mono sum exactly
  where it was**, because width only scales the part that cancels.

It renders **offline**, so it needs no audio hardware and no running server:

    python3 examples/panning.py             # render and report
    python3 examples/panning.py out.wav     # ...and write it

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
    DoneAction, Env, balance2, env_gen, lf_noise1, lf_tri, lin_pan2, mid_side,
    out, pan2, pan_az, pink_noise, pulse, rlpf, saw, select_x, sine, splay,
    stereo_width, xfade2,
)
from clausters.seq import Pbind, Pseq

SR = 48000.0
SECTION = 2.0  # seconds per section


def envelope(seconds):
    """A rise-hold-fall that frees its own node, so nothing here needs a
    note-off."""
    return env_gen(Env([0.0, 1.0, 1.0, 0.0], [0.05, seconds - 0.2, 0.15]),
                   done_action=DoneAction.FREE_SELF)


# ---- 1. a source crossing the field --------------------------------------------
#
# `pan2` returns *two channels* — a ChannelList, the same container `dup` builds
# — so it goes straight into `out(0, ...)`, which lays them on buses 0 and 1. A
# UGen has one output, so under the hood this is two `Pan2` rows sharing their
# inputs and differing in a trailing channel index; the builder fills that in
# and you never see it.
#
# The position here is an audio-rate `lf_tri`, which is the case worth knowing
# about: the gains are recomputed *per sample* rather than ramped across the
# block, so a sweep this fast has no stair-stepping and no dip in the middle of
# each block.

def sweep() -> SynthDef:
    voice = rlpf(saw(110.0), lf_noise1(0.5) * 400.0 + 900.0, q=2.0) * 0.3
    pos = lf_tri(1.5)  # -1 .. 1, a full crossing per 2/3 second
    return SynthDef("sweep", out(0.0, pan2(voice, pos) * envelope(SECTION)))


# ---- 2. a bank spread across the field -----------------------------------------
#
# `splay` is a client-side helper, not a UGen: it panned each voice with `pan2`
# and folded the results into a pair. The first voice lands hard left, the last
# hard right, the rest evenly in between — so a detuned bank arrives as a wide
# chord instead of a mono lump.
#
# Note that the level is *not* normalized for you. Six voices at equal power
# sum to more than one voice; scale it yourself, as here.

def bank() -> SynthDef:
    voices = [pulse(220.0 * (1 + 0.01 * k), 0.3 + 0.05 * k) for k in range(6)]
    spread = splay(voices, level=0.12)
    return SynthDef("bank", out(0.0, spread * envelope(SECTION)))


# ---- 3. width, which is not position ------------------------------------------
#
# The image gets narrow and then wide without anything moving: `stereo_width`
# scales the side component of an already stereo signal. 0 is mono, 1 is exactly
# the identity, 2 is wide.
#
# `mid_side` is the same matrix with the width left to you, and it is the one to
# reach for when something has to happen *between* the encode and the decode —
# here the centre of the mix is filtered while its sides are not, which is a
# thing a width knob cannot express. The call is its own inverse: the same
# `mid_side` encodes and decodes.

def width() -> SynthDef:
    left = rlpf(saw(147.0), 1200.0, q=1.5) * 0.25
    right = rlpf(saw(148.5), 1400.0, q=1.5) * 0.25
    w = lf_tri(0.5) + 1.0  # 0 .. 2, narrow to wide
    return SynthDef("width", out(0.0, stereo_width(left, right, w) * envelope(SECTION)))


def midside() -> SynthDef:
    left = rlpf(saw(147.0), 1200.0, q=1.5) * 0.25
    right = pink_noise() * 0.5 + rlpf(saw(148.5), 1400.0, q=1.5) * 0.25
    m, s = mid_side(left, right)          # encode
    dulled = rlpf(m, 700.0, q=0.9)        # ...treat the centre alone...
    return SynthDef("midside", out(0.0, mid_side(dulled, s) * envelope(SECTION)))


# ---- 4. the crossfades ---------------------------------------------------------
#
# `xfade2` is the pan law pointed at two sources instead of two channels, and
# `select_x` is the same thing along an array: the index's whole part picks a
# source and its fraction crossfades to the next. Every source runs whether or
# not it is selected — they are UGens in a graph, not branches — so this chooses
# what is *heard*, never what is computed.

def morph() -> SynthDef:
    which = lf_tri(0.35) + 1.0  # 0 .. 2, across three sources
    voice = select_x(which, sine(220.0), pulse(220.0, 0.25), saw(220.0))
    # ...and a plain two-way fade between the dry voice and a bright copy.
    bright = rlpf(voice, 2600.0, q=4.0)
    mixed = xfade2(voice, bright, lf_tri(0.6)) * 0.28
    return SynthDef("morph", out(0.0, pan2(mixed, -0.4) * envelope(SECTION)))


# ---- 5. a ring, folded down to two channels ------------------------------------
#
# `pan_az` places a source on a ring of any size — this one has six channels for
# a listener with two, which is the ordinary case for anyone writing surround
# material on headphones. The ring's channels are then panned into the stereo
# pair by hand, which is exactly what a fold-down is.
#
# `orientation=0` puts channel 0 at the front; the default 0.5 puts the front
# between two channels, which is what an even ring wants for a listener facing
# forward.

def ring() -> SynthDef:
    src = rlpf(pulse(330.0, 0.2), 2000.0, q=3.0) * 0.5
    speakers = pan_az(6, src, lf_tri(0.4), level=0.5, orientation=0.0)
    # Fold: speaker k sits at its own angle in the stereo image.
    folded = None
    for k, chan in enumerate(speakers):
        placed = pan2(chan, -1.0 + 2.0 * k / 5.0)
        folded = placed if folded is None else folded + placed
    return SynthDef("ring", out(0.0, folded * envelope(SECTION)))


# ---- 6. the two laws, measured side by side ------------------------------------
#
# Everything above moves; these hold one steady tone at one position for a whole
# note, so the report can read a level off each channel and compare it against a
# reference that went through no panner at all. Seven notes, seven claims, and
# the numbers in the last three columns are what makes them checkable rather
# than folklore.

def held(name, sig) -> SynthDef:
    return SynthDef(name, out(0.0, sig * envelope(SECTION)))


#: What each measurement note is for, printed beside its measured levels.
CLAIMS = {
    "dry": "the reference: one tone in both channels, no panner",
    "eq_centre": "equal power, centred: 0.71 per channel, mono up 3 dB",
    "eq_side": "equal power, hard left: full level, mono down 3 dB",
    "lin_centre": "constant amplitude, centred: 0.50 per channel",
    "lin_side": "constant amplitude, hard left: same mono as centred",
    "balanced": "balance2, centred: 0.71 — 3 dB for doing nothing",
    "pair": "width 1: exactly the identity — the baseline for the two below",
    "wide": "width 2: channels louder, mono exactly unchanged",
    "narrow": "width 0: both channels are the mid, mono unchanged again",
}


# ---- 7. render it --------------------------------------------------------------

def render(path=None):
    server = Server(interface=OscNrtInterface())
    tone = sine(440.0) * 0.4
    pair = pan2(tone, -0.3)  # an ordinary stereo pair for the width rows
    for sdef in (
        sweep(), bank(), width(), midside(), morph(), ring(),
        held("dry", tone.dup()),
        held("eq_centre", pan2(tone, 0.0)),
        held("eq_side", pan2(tone, -1.0)),
        held("lin_centre", lin_pan2(tone, 0.0)),
        held("lin_side", lin_pan2(tone, -1.0)),
        held("balanced", balance2(tone, tone, 0.0)),
        held("pair", stereo_width(pair[0], pair[1], 1.0)),
        held("wide", stereo_width(pair[0], pair[1], 2.0)),
        held("narrow", stereo_width(pair[0], pair[1], 0.0)),
    ):
        server.add_synthdef(sdef)

    names = ["sweep", "bank", "width", "midside", "morph", "ring", *CLAIMS]
    clock = TempoClock(tempo=1.0)
    Pbind(instrument=Pseq(names), dur=SECTION, amp=0.5).play(clock, server)
    clock.render()
    samples, frames = server.render(sample_rate=SR, channels=2)

    peak = max(abs(s) for s in samples)
    print(f"rendered {frames} frames ({frames / SR:.2f} s) | peak {peak:.3f}")
    if peak == 0.0:
        sys.exit("the render is silent — something is wrong")
    if peak > 1.5:
        sys.exit(f"the render clips hard (peak {peak:.2f})")

    # The claims, measured on what was just rendered. Each section is read in
    # its steady middle, away from the envelope's edges; the measurement notes
    # are also scaled against `dry`, the one that went through no panner.
    print("\n  section        left    right     mono   note")
    reference = None
    for k, name in enumerate(names):
        lo = (int(k * SECTION * SR) + int(0.4 * SR)) * 2
        hi = (int((k + 1) * SECTION * SR) - int(0.4 * SR)) * 2
        cut = samples[lo:hi]
        left, right = cut[0::2], cut[1::2]
        mono = [(a + b) * 0.5 for a, b in zip(left, right)]
        if name == "dry":
            reference = rms(left)
            print(f"  {'':12} {'':>6}   {'':>6}   {'':>6}   "
                  f"(the rows below are x{reference:.3f}, the dry level)")
        scale = reference if name in CLAIMS and reference else 1.0
        print(f"  {name:12} {rms(left) / scale:6.3f}   {rms(right) / scale:6.3f}   "
              f"{rms(mono) / scale:6.3f}   {CLAIMS.get(name, '')}")

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
        print(f"\nwrote {path} — listen with: pw-play {path}")


def rms(x):
    return (sum(s * s for s in x) / len(x)) ** 0.5 if x else 0.0


if __name__ == "__main__":
    try:
        render(sys.argv[1] if len(sys.argv) > 1 else None)
    except (OSError, RuntimeError) as e:
        sys.exit(str(e))
