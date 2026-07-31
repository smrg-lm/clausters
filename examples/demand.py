#!/usr/bin/env python3
"""Sequencing inside the def: the demand family.

A `d*` builder is not a signal. It is a **stream**: it has no samples, only a
next value, and between two pulls it does nothing at all. What turns a stream
into sound is a **driver**, and there are three. `demand` is *told* when to
pull, by a trigger. `duty` and `tduty` bring their own clock — every ``dur``
seconds they pull one ``level`` — and since **both** of those are pulled, a
stream of durations against a stream of pitches is a sequencer whose two parts
need not be the same length. `duty` holds each level until the next is due;
`tduty` emits it on that one sample and is silent in between, which makes it a
trigger train whose amplitudes are the levels.

Two conventions carry the whole family:

* **``repeats=0`` is endless.** sclang writes ``inf``; a def cannot (the wire
  refuses a non-finite constant), so the count of none is the endless one. It
  counts *passes over the list* for `dseq`/`dshuf` and *items* for
  `drand`/`dxrand`, which have no pass to complete.
* **A stream goes anywhere a number goes.** `dseq([dseries(3, 1, 1), 9])` is
  four items, not two: a list source **drains** a nested stream before moving
  on, and restarts it when it comes round to it again. That is the whole reason
  the family exists — a sequence of *phrases* rather than of numbers.

The file has two halves. The first is a short piece; the second is a bench that
plays each claim above as a stream straight onto a bus and then **measures it
off the render** rather than asking you to take it on faith.

It renders **offline**, so it needs no audio hardware and no running server:

    python3 examples/demand.py             # play the piece, run the bench
    python3 examples/demand.py out.wav     # ...and write the piece

Read it top to bottom; each section is one idea.
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))

from clausters.base import OscNrtInterface, TempoClock
from clausters.render import read_soundfile
from clausters.defs import Server, SynthDef
from clausters.defs.ugens import (
    DoneAction, Env, dbrown, decay2, drand, dseq, dseries, dshuf, dstutter,
    dswitch1, dwhite, dxrand, duty, env_gen, line, lpf, out, pan2, rlpf, saw,
    sine, tduty,
)
from clausters.seq import Pbind, Pseq

SR = 48000.0
SECTION = 2.5    # seconds per section of the piece
SLOT = 0.05      # the bench's clock: one pulled value per slot
BENCH = 1.0      # seconds per bench row (20 slots)


def envelope(seconds):
    """A rise-hold-fall that frees its own node, so nothing here needs a
    note-off."""
    return env_gen(Env([0.0, 1.0, 1.0, 0.0], [0.05, seconds - 0.2, 0.15]),
                   done_action=DoneAction.FREE_SELF)


# ---- 1. the sequencer: two streams of different lengths -------------------------
#
# `duty` pulls a duration *and* a level, so the melody is two independent
# streams. Five pitches against three durations is not a five-note phrase — the
# pair only realigns after fifteen notes, which is the cheapest polyrhythm there
# is and something a fixed note list cannot express at all.
#
# `repeats=0` on both keeps them going for the whole section.

def melody() -> SynthDef:
    pitch = dseq([62, 65, 69, 72, 67], repeats=0)
    dur = dseq([0.25, 0.25, 0.5], repeats=0)
    voice = saw(duty(dur, level=pitch).midicps()) * 0.15
    return SynthDef("melody", out(0.0, pan2(lpf(voice, 2500.0), -0.3)
                                  * envelope(SECTION)))


# ---- 2. phrases, not numbers ----------------------------------------------------
#
# The nesting. A slot of this `dseq` is itself a stream, so the outer sequence
# yields a *phrase* — the four rising notes of the `dseries`, then two drawn
# from a small set, then the fixed 60 — and comes back round to restart each of
# them. This is the property the whole family is built around.

def phrase() -> SynthDef:
    pitch = dseq([dseries(4, 60, 2), drand([72, 75, 79], repeats=2), 60],
                 repeats=0)
    voice = sine(duty(0.2, level=pitch).midicps()) * 0.2
    return SynthDef("phrase", out(0.0, pan2(voice, 0.3) * envelope(SECTION)))


# ---- 3. stutter --------------------------------------------------------------
#
# `dstutter` repeats each item of another stream, and its count is pulled per
# item — so the count can be a stream too. Here a `drand` decides between one
# and four repetitions, which is a rhythm made of nothing but repetition.

def stutter() -> SynthDef:
    pitch = dstutter(drand([1, 2, 4], repeats=0), drand([64, 67, 71, 76], repeats=0))
    voice = saw(duty(0.08, level=pitch).midicps()) * 0.12
    return SynthDef("stutter", out(0.0, pan2(rlpf(voice, 1800.0, q=3.0), 0.0)
                                   * envelope(SECTION)))


# ---- 4. one order, replayed -----------------------------------------------------
#
# `dshuf` shuffles **once** and then replays that order — which is what makes it
# a riff rather than a random walk. `drand` in its place would give a different
# bassline every pass; here the ear gets to learn one.

def shuffle() -> SynthDef:
    bass = dshuf([36, 43, 39, 48], repeats=0)
    voice = saw(duty(0.25, level=bass).midicps()) * 0.25
    return SynthDef("shuffle", out(0.0, pan2(lpf(voice, 600.0), 0.0)
                                   * envelope(SECTION)))


# ---- 5. a stream as a control ---------------------------------------------------
#
# Nothing says a demand stream has to carry pitch. `dbrown` walks between two
# bounds by at most `step` per item — **folded** at a bound, so it turns around
# instead of piling up against it — and here it walks a filter's cutoff, one
# step per sixteenth.

def walk() -> SynthDef:
    cutoff = duty(0.125, level=dbrown(repeats=0, lo=400.0, hi=4000.0, step=700.0))
    voice = rlpf(saw(55.0), cutoff, q=6.0) * 0.2
    return SynthDef("walk", out(0.0, pan2(voice, -0.15) * envelope(SECTION)))


# ---- 6. tduty: the levels are the amplitudes ------------------------------------
#
# `tduty` puts each level on its own single sample and nothing in between, so it
# is a trigger train that carries dynamics. Feed it to a percussive decay and
# the stream's numbers *are* the accents.

def perc() -> SynthDef:
    hits = tduty(dseq([0.125, 0.125, 0.25], repeats=0),
                 level=dseq([1.0, 0.3, 0.5, 0.3], repeats=0))
    voice = decay2(hits, 0.005, 0.12) * sine(1800.0) * 0.3
    return SynthDef("perc", out(0.0, pan2(voice, 0.4) * envelope(SECTION)))


# ---- 7. the bench: each claim as a stream on a bus ------------------------------
#
# Every row below is one `duty` at the same slow clock, writing its pulled
# values straight to bus 0 — no oscillator, no envelope, so a sample read in the
# middle of slot *k* is exactly the *k*-th value the stream yielded. The values
# are all non-zero on purpose: `done_action` frees the node the moment a stream
# ends, so **silence marks the end of the stream** and the number of sounding
# slots is the number of items.
#
# An endless stream has no such end, so those rows carry a `line` that frees
# them instead — an ordinary UGen root beside the `out`, which is all a
# "second root" ever is.

def bench(name, stream, endless=False) -> SynthDef:
    roots = [out(0.0, duty(SLOT, level=stream, done_action=DoneAction.FREE_SELF))]
    if endless:
        roots.append(line(0.0, 1.0, BENCH - SLOT, DoneAction.FREE_SELF))
    return SynthDef(name, *roots)


def bench_rows():
    """Each row: the def, and the claim its measured values have to support."""
    return [
        (bench("passes", dseq([1, 2, 3], repeats=2)),
         "repeats counts PASSES for dseq: 1 2 3 1 2 3, then the stream ends"),
        (bench("items", drand([4, 5, 6], repeats=2)),
         "repeats counts ITEMS for drand: two picks from {4,5,6}, then it ends"),
        (bench("nested", dseq([dseries(3, 1, 1), 9], repeats=2)),
         "a nested stream is DRAINED and restarted: 1 2 3 9 1 2 3 9"),
        (bench("stutter", dstutter(3, dseries(3, 1, 1))),
         "dstutter repeats each item three times: 1 1 1 2 2 2 3 3 3"),
        (bench("switch", dswitch1(dseq([0, 1, 0, 1], repeats=1),
                                  dseries(4, 1, 1), dseries(4, 10, 10))),
         "dswitch1 takes ONE item and leaves the others where they were: "
         "1 10 2 20"),
        (bench("shuffled", dshuf([1, 2, 3, 4], repeats=2)),
         "dshuf shuffles once and replays it: the second pass repeats the first"),
        (bench("norepeat", dxrand([1, 2, 3], repeats=0), endless=True),
         "dxrand never picks the value it just used"),
        (bench("bounded", dwhite(repeats=0, lo=2.0, hi=5.0), endless=True),
         "dwhite draws inside [2, 5]"),
        (bench("folded", dbrown(repeats=0, lo=2.0, hi=5.0, step=0.5), endless=True),
         "dbrown steps by at most 0.5 and folds back at a bound"),
    ]


#: The bench rows whose exact value sequence is fixed, checked item by item.
EXACT = {
    "passes": [1, 2, 3, 1, 2, 3],
    "nested": [1, 2, 3, 9, 1, 2, 3, 9],
    "stutter": [1, 1, 1, 2, 2, 2, 3, 3, 3],
    "switch": [1, 10, 2, 20],
}


def slots(samples, section):
    """The values one bench row yielded: the sample at the middle of each slot
    (where a held value is unambiguous), up to the point the node freed itself
    and the bus went quiet — which is exactly where the stream ended."""
    values = []
    for k in range(int(BENCH / SLOT)):
        i = int((section * BENCH + (k + 0.5) * SLOT) * SR)
        if i >= len(samples) or samples[i] == 0.0:
            break
        values.append(samples[i])
    return values


def run_bench(path=None):
    server = Server(interface=OscNrtInterface())
    rows = bench_rows()
    for sdef, _ in rows:
        server.add_synthdef(sdef)
    clock = TempoClock(tempo=1.0)
    Pbind(instrument=Pseq([s.name for s, _ in rows]), dur=BENCH).play(clock, server)
    clock.render()
    stats = server.render(sample_rate=SR, channels=1, path=path)
    # The claims below read individual slots, not a summary, so the samples
    # come back from the file the server just wrote.
    samples = read_soundfile(path).samples if path else \
        server.render(sample_rate=SR, channels=1).samples

    print(f"\nthe bench: {len(rows)} streams, one pulled value every "
          f"{SLOT * 1000:.0f} ms ({stats.duration:.1f} s)\n")
    failures = []
    for i, (sdef, claim) in enumerate(rows):
        values = slots(samples, i)
        shown = " ".join(f"{v:g}" for v in values[:10])
        print(f"  {sdef.name:10} {shown}")
        print(f"  {'':10} {claim}")
        failures += check(sdef.name, values)
    return failures


def check(name, values):
    """The claims, read off the numbers the render actually produced."""
    bad = []
    if name in EXACT and values != EXACT[name]:
        bad.append(f"{name}: expected {EXACT[name]}, got {values}")
    if name == "items" and len(values) != 2:
        bad.append(f"items: drand(repeats=2) yielded {len(values)} items, not 2")
    if name == "items" and not all(v in (4, 5, 6) for v in values):
        bad.append(f"items: drew {values}, which is not from {{4, 5, 6}}")
    if name == "shuffled":
        if len(values) != 8:
            bad.append(f"shuffled: {len(values)} items, not two passes of four")
        elif values[:4] != values[4:]:
            bad.append(f"shuffled: the second pass {values[4:]} differs from "
                       f"the first {values[:4]}")
        elif sorted(values[:4]) != [1, 2, 3, 4]:
            bad.append(f"shuffled: {values[:4]} is not a permutation of the list")
    if name == "norepeat":
        pairs = [(a, b) for a, b in zip(values, values[1:]) if a == b]
        if pairs:
            bad.append(f"norepeat: dxrand repeated a value immediately ({pairs[0]})")
    if name == "bounded" and not all(2.0 <= v <= 5.0 for v in values):
        bad.append(f"bounded: {min(values):.3f}..{max(values):.3f} leaves [2, 5]")
    if name == "folded":
        if not all(2.0 <= v <= 5.0 for v in values):
            bad.append(f"folded: {min(values):.3f}..{max(values):.3f} leaves [2, 5]")
        # Folding reflects, so a step never *lands* further than it was told —
        # the reflection is inside the same 0.5 the walk was given.
        jumps = [abs(b - a) for a, b in zip(values, values[1:])]
        if jumps and max(jumps) > 0.5 + 1e-4:
            bad.append(f"folded: a step of {max(jumps):.3f} exceeds 0.5")
    return bad


# ---- 8. the clock: a driver that does not drift ---------------------------------
#
# `duty` keeps its countdown in f64 and carries the remainder across pulls
# (``count += dur * sr``, never ``count = dur * sr``), so a duration that is not
# a whole number of samples does not accumulate error. At 48 kHz a 1/300 s slot
# is 160 samples exactly, but 1/700 is 68.57 — this measures where the 700th
# change actually lands.

def drift_check():
    server = Server(interface=OscNrtInterface())
    dur = 1.0 / 700.0
    server.add_synthdef(SynthDef(
        "drift",
        out(0.0, duty(dur, level=dseq([1.0, 2.0], repeats=0))),
        line(0.0, 1.0, 1.0, DoneAction.FREE_SELF),
    ))
    clock = TempoClock(tempo=1.0)
    Pbind(instrument=Pseq(["drift"]), dur=1.2).play(clock, server)
    clock.render()
    samples = server.render(sample_rate=SR, channels=1).samples

    changes = [i for i, (a, b) in enumerate(zip(samples, samples[1:]))
               if a != b and a != 0.0 and b != 0.0]
    if len(changes) < 600:
        return [f"drift: only {len(changes)} changes in a second at 700 Hz"]
    last = changes[599]                   # the 600th change
    ideal = 600 * dur * SR                # where it belongs, to the sample
    print(f"\n  drift      the 600th pull at {dur * 1000:.3f} ms lands on sample "
          f"{last + 1}, ideal {ideal:.1f}")
    print(f"  {'':10} a naive counter would be "
          f"{600 * (round(dur * SR) - dur * SR):.0f} samples off by now")
    return ([f"drift: the 600th pull is {abs(last + 1 - ideal):.1f} samples out"]
            if abs(last + 1 - ideal) > 1.5 else [])


# ---- 9. render the piece --------------------------------------------------------

def render_piece(path=None):
    server = Server(interface=OscNrtInterface())
    parts = [melody(), phrase(), stutter(), shuffle(), walk(), perc()]
    for sdef in parts:
        server.add_synthdef(sdef)
    clock = TempoClock(tempo=1.0)
    Pbind(instrument=Pseq([s.name for s in parts]), dur=SECTION).play(clock, server)
    clock.render()
    stats = server.render(sample_rate=SR, channels=2, path=path)
    # Per-section RMS below needs the samples, not the whole-render summary.
    samples = read_soundfile(path).samples if path else \
        server.render(sample_rate=SR, channels=2).samples

    peak = max(stats.peak)
    print(f"the piece: {len(parts)} sections, {stats.frames} frames "
          f"({stats.duration:.2f} s) | peak {peak:.3f}")
    if peak == 0.0:
        sys.exit("the render is silent - something is wrong")
    if peak > 1.5:
        sys.exit(f"the render clips hard (peak {peak:.2f})")

    print("\n  section      what it demonstrates")
    for k, sdef in enumerate(parts):
        lo = (int(k * SECTION * SR) + int(0.4 * SR)) * 2
        hi = (int((k + 1) * SECTION * SR) - int(0.4 * SR)) * 2
        print(f"  {sdef.name:12} rms {rms(samples[lo:hi]):.3f}   {NOTES[sdef.name]}")

        print(f"\nwrote {path} - listen with: pw-play {path}")


#: One line per section of the piece, printed beside its measured level.
NOTES = {
    "melody": "five pitches against three durations - they realign after 15",
    "phrase": "a stream inside a stream: a phrase per slot, restarted each pass",
    "stutter": "dstutter, its repeat count itself a stream",
    "shuffle": "dshuf: one order, learned by the ear because it comes back",
    "walk": "dbrown on a filter cutoff - a stream need not carry pitch",
    "perc": "tduty: one sample per hit, the levels are the accents",
}


def rms(x):
    return (sum(s * s for s in x) / len(x)) ** 0.5 if x else 0.0


if __name__ == "__main__":
    try:
        render_piece(sys.argv[1] if len(sys.argv) > 1 else None)
        problems = run_bench() + drift_check()
    except (OSError, RuntimeError) as e:
        sys.exit(str(e))
    if problems:
        print("\nthe render does not support these claims:")
        for p in problems:
            print(f"  - {p}")
        sys.exit(1)
    print("\nevery claim above holds on the render just produced.")
