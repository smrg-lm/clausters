#!/usr/bin/env python3
"""The UGen bench: hear the row that is under test.

Every UGen on the bench has asserts — a measured alias SNR, an analytic
transfer function, a decay envelope that matches its formula. What none of that
tells you is whether the thing *sounds* like what it claims to be. This plays
it.

It reads the same declaration the asserts do, `tests/common/subjects.json`, so
there is one table and two consumers: `tests/common/bench.rs` renders a subject
and measures it, and this renders the same subject and puts it out a speaker.
A row that gained a test is audible here with no further work, and a number you
want to argue with is a number you can go and listen to.

    python3 examples/audition.py                     # what is on the bench
    python3 examples/audition.py saw                 # play it
    python3 examples/audition.py saw --secs 4 --set freq=55
    python3 examples/audition.py <section>           # a whole section, in order
    python3 examples/audition.py rlpf --sweep freq 100 8000
    python3 examples/audition.py saw --wav /tmp/saw.wav      # offline, no device
    python3 examples/audition.py <section> --wav /tmp/noise.wav

`--wav` renders **offline**, so it needs no audio hardware and no running
server; without it the script boots a server and plays through the sound card.
Either way the measured level of what was produced is printed, which is the
first thing to check when something sounds wrong.

**One honest difference between the ear and the asserts.** A subject with a
`source` is driven, in the tests, by an exact array of samples the test writes
onto a bus. A live graph has no way to be handed an array, so here the source is
its **UGen equivalent** — `noise` becomes a `WhiteNoise`, `sine` a `Sine` at the
subject's `hz`, `impulse_train` an `Impulse` at it. Same character, different
samples: this bench is for judging a sound, not for reproducing an assert.

Read it top to bottom; each section is one idea.
"""

import argparse
import copy
import json
import os
import struct
import sys
import wave

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, os.path.join(ROOT, "clients", "python"))

SUBJECTS = os.path.join(ROOT, "tests", "common", "subjects.json")
SR = 48000.0


# ---- 1. the table ------------------------------------------------------------
#
# The file is shared with the Rust bench, and its `_doc` key is the prose that
# describes it. Every other key is a section of the bench, and running the
# script with no argument prints them with their rows.
#
# Pending: those sections are still the batches the rows were written in, not
# families of UGen, so a new row lands in whichever batch added it. What this
# bench wants is to follow the *registry* -- every implemented UGen audible
# here with no table to edit -- which needs a source this file does not have
# yet.

def load_table():
    with open(SUBJECTS, encoding="utf-8") as f:
        table = json.load(f)
    return {k: v for k, v in table.items() if not k.startswith("_")}


def resolve(table, target):
    """A target is a section of the table, a subject handle (`combc`) or
    `all`."""
    if target == "all":
        return [(m, s) for m, rows in sorted(table.items()) for s in rows]
    if target in table:
        return [(target, s) for s in table[target]]
    for section, rows in sorted(table.items()):
        for s in rows:
            if s["name"] == target:
                return [(section, s)]
    known = sorted(s["name"] for rows in table.values() for s in rows)
    sys.exit(f"no subject or section {target!r}\non the bench: {', '.join(known)}")


# ---- 2. the source ------------------------------------------------------------
#
# `bench.rs` writes an exact array onto a bus and the subject reads it with an
# `In`. Here the same slot is filled by a UGen, because that is what a live
# graph can hold. The two agree on *what kind of signal* it is, which is what a
# listening test needs, and disagree on the samples, which it does not.

def source_ugen(kind, hz, secs):
    if kind == "noise":
        return {"kind": "WhiteNoise", "inputs": []}
    if kind == "impulse":
        # A lone impulse is inaudible; a slow train is the audible equivalent.
        return {"kind": "Impulse", "inputs": [{"const": 1.0}]}
    if kind == "impulse_train":
        return {"kind": "Impulse", "inputs": [{"const": hz}]}
    if kind == "sine":
        return {"kind": "Sine", "inputs": [{"const": hz}]}
    if kind == "dc":
        return {"kind": "Line", "inputs": [{"const": 1.0}, {"const": 1.0},
                                           {"const": secs}, {"const": 0.0}]}
    if kind == "silence":
        return {"kind": "Line", "inputs": [{"const": 0.0}, {"const": 0.0},
                                           {"const": secs}, {"const": 0.0}]}
    sys.exit(f"unknown source {kind!r} in subjects.json")


# ---- 3. the def ---------------------------------------------------------------
#
# The same assembly `Subject::def_json` does on the Rust side — source, prelude,
# one row per channel with the channel index appended, an `Out` per channel —
# plus the two things only a listener needs: a level, and the option to sweep an
# input so the row is heard *moving* rather than at one setting.

def build(subject, secs, amp, sets, sweep, catalog):
    hz = subject.get("hz", 220.0)
    channels = subject.get("channels", 1)
    inputs = copy.deepcopy(subject["inputs"])

    ugens = []
    if subject.get("source"):
        ugens.append(source_ugen(subject["source"], hz, secs))
    ugens.extend(copy.deepcopy(subject.get("prelude", [])))

    for slot, value in sets.items():
        inputs[slot_index(subject, slot, catalog)] = {"const": value}
    if sweep:
        slot, lo, hi = sweep
        # The ramp goes at the end of the prelude, so every index the subject
        # already refers to keeps pointing where it did.
        ugens.append({"kind": "Line", "inputs": [
            {"const": lo}, {"const": hi}, {"const": secs}, {"const": 0.0}]})
        inputs[slot_index(subject, slot, catalog)] = {"ugen": len(ugens) - 1}

    first = len(ugens)
    for chan in range(channels):
        row = {"kind": subject["kind"], "inputs": list(inputs)}
        if channels > 1:
            row["inputs"] = row["inputs"] + [{"const": float(chan)}]
        row.update(subject.get("static", {}))
        ugens.append(row)
    # The level: a bare row runs at full scale, and several of them (`Crackle`,
    # `PulseCount`, `Phasor`) are not in [-1, 1] at all.
    for chan in range(channels):
        ugens.append({"kind": "Mul", "inputs": [
            {"ugen": first + chan}, {"const": amp}]})
    scaled = first + channels
    for chan in range(channels):
        ugens.append({"kind": "Out", "inputs": [
            {"const": float(chan)}, {"ugen": scaled + chan}]})
    return {"name": f"aud_{subject['name']}", "ugens": ugens}


def slot_index(subject, slot, catalog):
    """An input by name (`freq`) or by position (`0`). Names come from the
    server's own `/u_query`, so a typo is an error here rather than a value
    quietly landing in the wrong slot."""
    if slot.isdigit():
        return int(slot)
    names = catalog.get(subject["kind"])
    if names is None:
        sys.exit(
            f"cannot resolve the input name {slot!r} offline; pass its position "
            f"instead (the subject declares {len(subject['inputs'])} inputs), "
            "or drop --wav so the bench can ask the server"
        )
    if slot not in names:
        sys.exit(f"{subject['kind']} has no input {slot!r}; it takes {names}")
    return names.index(slot)


class RawDef:
    """What `Server.add_synthdef` needs from a def: a name and its wire text.
    The subjects are already `SynthDefSpec` JSON, which is that text — there is
    nothing for the client-side builders to rebuild."""

    def __init__(self, spec):
        self.name = spec["name"]
        self._text = json.dumps(spec)

    def dump_def(self):
        return self._text


# ---- 4. play it, or render it -------------------------------------------------

def report(name, samples, channels, secs):
    if not samples:
        print(f"  {name:14} rendered nothing")
        return
    peak = max(abs(s) for s in samples)
    rms = (sum(s * s for s in samples) / len(samples)) ** 0.5
    frames = len(samples) // channels
    note = ""
    if peak == 0.0:
        note = "  <- silent"
    elif peak > 1.0:
        note = "  <- clipping"
    print(f"  {name:14} {frames / SR:5.2f} s  {channels} ch  "
          f"peak {peak:6.3f}  rms {rms:6.3f}{note}")


def spread(samples, ch, into):
    """One subject's interleaved frames widened to the file's channel count. A
    mono row goes to every channel — it is meant to be heard, not placed — and
    a narrower multi-channel one keeps its channels and leaves the rest
    silent."""
    if ch == into:
        return samples
    out = []
    for f in range(len(samples) // ch):
        frame = samples[f * ch:(f + 1) * ch]
        out.extend(list(frame) * into if ch == 1 else
                   list(frame) + [0.0] * (into - ch))
    return out


def render_offline(items, secs, amp, sets, sweep, path):
    from clausters.base import OscNrtInterface
    from clausters.defs import Server

    catalog = {}  # no server to ask; --sweep/--set take positions
    # The file is as wide as its widest subject; each one still renders at its
    # own width, or a 4-channel `PanAz` would silently make everything after it
    # four channels too.
    width = max(s.get("channels", 1) for _, s in items)
    out = []
    for section, subject in items:
        ch = subject.get("channels", 1)
        spec = build(subject, secs, amp, sets, sweep, catalog)
        server = Server(interface=OscNrtInterface())
        server.add_synthdef(RawDef(spec))
        node = server.synth(spec["name"])
        # A bare subject has no envelope and no done action, so nothing would
        # end the score: the render's duration is the closing bundle.
        server.send_bundle_after(secs, ("/n_free", node.id))
        samples = server.render(sample_rate=SR, channels=ch).samples
        report(f"{section}/{subject['name']}", samples, ch, secs)
        out.extend(spread(samples, ch, width))
        out.extend([0.0] * int(0.15 * SR) * width)   # a gap to hear the seam

    if path:
        with wave.open(path, "wb") as w:
            w.setnchannels(width)
            w.setsampwidth(2)
            w.setframerate(int(SR))
            w.writeframes(b"".join(
                struct.pack("<h", int(max(-1.0, min(1.0, s)) * 32767))
                for s in out))
        print(f"\nwrote {path} -- listen with: pw-play {path}")


def play_live(items, secs, amp, sets, sweep):
    import time

    from clausters import Session

    session = Session.live()
    try:
        catalog = {u.name: [i.name for i in u.inputs]
                   for u in session.server.query_ugens()}
        for section, subject in items:
            spec = build(subject, secs, amp, sets, sweep, catalog)
            session.server.add_synthdef(RawDef(spec))
            print(f"  {section}/{subject['name']:14} {subject['kind']}")
            node = session.server.synth(spec["name"])
            time.sleep(secs)
            node.free()
            time.sleep(0.15)
    finally:
        session.close()


# ---- 5. the command line ------------------------------------------------------

def main(argv=None):
    p = argparse.ArgumentParser(
        description="Play a UGen from the test bench.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="With no target, lists what is on the bench.")
    p.add_argument("target", nargs="?",
                   help="a subject handle (saw), a section of the table, or all")
    p.add_argument("--secs", type=float, default=2.0,
                   help="seconds per subject (default 2)")
    p.add_argument("--amp", type=float, default=0.2,
                   help="output level (default 0.2 -- a bare row is full scale)")
    p.add_argument("--set", action="append", default=[], metavar="SLOT=VALUE",
                   help="override an input, by name (freq=55) or position (0=55)")
    p.add_argument("--sweep", nargs=3, metavar=("SLOT", "LO", "HI"),
                   help="ramp an input from LO to HI over the render")
    p.add_argument("--wav", metavar="PATH",
                   help="render offline to a WAV instead of playing")
    args = p.parse_args(argv)

    table = load_table()
    if not args.target:
        print(f"{sum(len(v) for v in table.values())} subjects on the bench:\n")
        for section, rows in sorted(table.items()):
            names = " ".join(s["name"] for s in rows)
            print(f"  {section}  {names}")
        print("\nplay one:  python3 examples/audition.py <name>")
        return 0

    items = resolve(table, args.target)
    sets = {}
    for pair in args.set:
        slot, _, value = pair.partition("=")
        if not value:
            sys.exit(f"--set wants SLOT=VALUE, got {pair!r}")
        sets[slot] = float(value)
    sweep = None
    if args.sweep:
        sweep = (args.sweep[0], float(args.sweep[1]), float(args.sweep[2]))

    if args.wav:
        render_offline(items, args.secs, args.amp, sets, sweep, args.wav)
    else:
        play_live(items, args.secs, args.amp, sets, sweep)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, RuntimeError, ValueError) as e:
        sys.exit(str(e))
