#!/usr/bin/env python3
"""Build a UGen SynthDef from Python and render it offline (client C5).

The UGen-graph counterpart of `examples/json_client.py`'s Faust defs: instead
of formatting the `SynthDefSpec` JSON by hand, compose it with the lowercase
callables in `clausters.defs.ugens` and let `SynthDef` serialize the graph.

The build is **instance-based** — the graph is just the tree of composed
objects, with no thread-global "current SynthDef" the way sclang has — so
several defs can be built side by side. The four arithmetic operators map to
the server's dedicated `Add`/`Sub`/`Mul`/`Div` UGens; everything beyond them
(`%`, `min`/`max`, the comparisons, `.midicps()`, `.distort()` …) composes its
generic `BinaryOpUGen`/`UnaryOpUGen` — see
`clients/python/examples/graph_maths.py`.

To prove the graph emits exactly what the server expects, this renders a
`Pbind` twice — once on the server's built-in `default` def, once on a
client-defined graph equivalent to it — and checks the two renders are
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

from clausters.base import OscNrtInterface, TempoClock
from clausters.defs import (
    DoneAction,
    Env,
    Server,
    SynthDef,
    control,
    env_gen,
    out,
    sine,
)
from clausters.seq import Pbind, Pseq

FREQS = [262.0, 330.0, 392.0, 523.0]
SR = 48000.0


def py_default(name="py_default") -> SynthDef:
    """`Sine(freq) * EnvGen(gate) * amp` to buses 0 and 1 — the client-side twin
    of the server's built-in `default`. `freq`/`amp`/`gate` are named controls
    (the `/s_new`/`/n_set` parameters a `Pbind` drives).

    The envelope is the built-in's own: a gated ASR on equal-power sine ramps
    (0.01 s attack, sustain at 1.0 while the gate is held, 0.3 s release) with
    `done_action = FREE_SELF`, so the note ramps in and out without a click and
    frees itself once the release finishes."""
    freq = control("freq", 440.0)
    amp = control("amp", 0.2)
    gate = control("gate", 1.0)
    env = env_gen(
        Env.asr(attack=0.01, sustain=1.0, release=0.3, curve="sin"),
        gate=gate,
        done_action=DoneAction.FREE_SELF,
    )
    sig = sine(freq) * env * amp     # `*` composes a Mul UGen
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


def render_pbind(instrument: str, sdef: SynthDef | None):
    """Render the arpeggio on `instrument`; if `sdef` is given, score its
    `/d_recv` first so the offline renderer compiles it before time advances."""
    server = Server(interface=OscNrtInterface())
    if sdef is not None:
        server.add_synthdef(sdef)      # scored at t=0 in NRT
    clock = TempoClock(tempo=1.0)
    # `has_gate` releases each note with `gate 0` instead of freeing the node
    # outright, which is what the player does for `default` on its own — the
    # twin needs it stated so both renders end their notes the same way.
    Pbind(instrument=instrument, has_gate=True,
          freq=Pseq(FREQS), dur=0.5, amp=0.2).play(clock, server)
    clock.render()                     # drain the clock logically
    return server.render(sample_rate=SR, channels=2)


def main():
    sdef = py_default()
    print("SynthDef JSON the server compiles (/d_recv):")
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
        print(f"wrote {path} — listen with: pw-play {path}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError) as e:
        sys.exit(str(e))
