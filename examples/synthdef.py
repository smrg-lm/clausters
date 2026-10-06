#!/usr/bin/env python3
"""Build a UGen SynthDef from Python and render it offline.

The UGen-graph counterpart of `examples/json_client.py`'s Faust defs: instead
of formatting the `SynthDefSpec` JSON by hand, compose it with the lowercase
callables in `clausters.defs.ugens` and let `SynthDef` serialize the graph.

The build is **instance-based** -- the graph is just the tree of composed
objects, with no thread-global "current SynthDef" the way sclang has -- so
several defs can be built side by side. The four arithmetic operators map to
the server's dedicated `Add`/`Sub`/`Mul`/`Div` UGens; everything beyond them
(`%`, `min`/`max`, the comparisons, `.midicps()`, `.distort()` ...) composes its
generic `BinaryOpUGen`/`UnaryOpUGen` -- see
`clients/python/examples/basics/graph_maths.py`.

To prove the graph emits exactly what the server expects, this renders a
`Pbind` twice -- once on the server's built-in `default` def, once on a
client-defined graph equivalent to it -- and checks the two renders are
**byte-identical**. Build the embed library once:

    cargo build --release --features embed,realtime

then:

    python3 examples/synthdef.py [out.wav]
"""

import json
import os
import struct
import sys
import wave

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))

from clausters.base import LogicalTimebase, OscNrtInterface, TempoClock
from clausters.defs import (
    DoneAction,
    Env,
    Server,
    SynthDef,
    control,
    env_gen,
    lag,
    out,
)
from clausters.defs.ugens import pan2, rlpf, saw
from clausters.seq import Pbind, Pseq

FREQS = [262.0, 330.0, 392.0, 523.0]
SR = 48000.0


def py_default(name="py_default") -> SynthDef:
    """The client-side twin of the server's built-in `default`: a subtractive
    voice with a control for each dimension an MPE zone plays.
    `freq`/`amp`/`gate`/`pan`/`press`/`slide`/`out` are named controls (the
    `/synth_new`/`/node_set` parameters a `Pbind` drives).

    The tone is two saws detuned by +/-0.4 %, through a resonant lowpass whose cutoff sits half an octave above the
    note and opens with `slide` (the timbre), `press` (the pressure, which
    also lifts the level) and a **bloom** -- a 0.6 s decay on the cutoff,
    deeper the louder the note. `press` and `slide` are smoothed over 50 ms.

    The envelope is the built-in's own: a gated ASR on equal-power sine ramps
    (0.01 s attack, sustain at 1.0 while the gate is held, 0.3 s release) with
    `done_action = FREE_SELF`, so the note ramps in and out without a click and
    frees itself once the release finishes. `pan2` places it at equal power,
    on the bus `out` names and the one after it."""
    freq = control("freq", 440.0)
    amp = control("amp", 0.2)
    gate = control("gate", 1.0)
    pan = control("pan", 0.0)
    press = lag(control("press", 0.0), 0.05)
    slide = lag(control("slide", 0.5), 0.05)
    bus = control("out", 0.0)
    env = env_gen(
        Env.asr(attack=0.01, sustain=1.0, release=0.3, curve="sin"),
        gate=gate,
        done_action=DoneAction.FREE_SELF,
    )
    bloom = env_gen(Env.perc(attack=0.005, release=0.6))
    # `+`, `*` compose Add/Mul UGens; `**` and `.min` the generic BinaryOpUGen.
    tone = (saw(freq * 0.996) + saw(freq * 1.004)) * 0.3
    octaves = 0.5 + slide * 2.5 + press * 2.0 + bloom * (0.3 + amp * 1.2)
    cutoff = (freq * 2.0 ** octaves).min(16000.0)
    sig = rlpf(tone, cutoff, rq=0.8) * env * amp * (1.0 + press * 0.5) * 1.6
    # One `out` control, so each side is written on its own: a channel list
    # is laid on consecutive buses from a constant, not from a control.
    left, right = pan2(sig, pan).items
    return SynthDef(name, out(bus, left), out(bus + 1.0, right))

def render_pbind(instrument: str, sdef: SynthDef | None):
    """Render the arpeggio on `instrument`; if `sdef` is given, score its
    `/def_send synth` first so the offline renderer compiles it before time advances."""
    server = Server(interface=OscNrtInterface())
    if sdef is not None:
        sdef.send(server)      # scored at t=0 in NRT
    clock = TempoClock(tempo=1.0, timebase=LogicalTimebase())
    # `has_gate` releases each note with `gate 0` instead of freeing the node
    # outright, which is what the player does for `default` on its own -- the
    # twin needs it stated so both renders end their notes the same way.
    Pbind(instrument=instrument, has_gate=True,
          freq=Pseq(FREQS), dur=0.5, amp=0.2).play(clock, server)
    clock.render()                     # drain the clock logically
    return server.render(sample_rate=SR, channels=2)


def main():
    sdef = py_default()
    print("SynthDef JSON the server compiles (/def_send synth):")
    print(json.dumps(sdef.spec(), indent=2))
    print("controls:", sdef.control_names())

    a = render_pbind("default", None)
    b = render_pbind("py_default", sdef)
    builtin, b_frames = a.samples, a.frames
    custom, c_frames = b.samples, b.frames

    identical = b_frames == c_frames and list(custom) == list(builtin)
    peak = max(abs(s) for s in custom)
    print(f"\nrendered {c_frames} frames ({c_frames / SR:.3f} s) | peak {peak:.3f}")
    print(f"byte-identical to the built-in `default`: {identical}")
    if not identical:
        sys.exit("MISMATCH: the client graph did not match the built-in def")

    if len(sys.argv) > 1:
        path = sys.argv[1]
        with wave.open(path, "wb") as w:
            w.setnchannels(2)
            w.setsampwidth(2)
            w.setframerate(int(SR))
            w.writeframes(b"".join(
                struct.pack("<h", int(max(-1.0, min(1.0, s)) * 32767)) for s in custom
            ))
        print(f"wrote {path} -- listen with: pw-play {path}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError) as e:
        sys.exit(str(e))
