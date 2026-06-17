#!/usr/bin/env python3
"""Build a UGen SynthDef from Python and render it offline (client C5).

The UGen-graph counterpart of `examples/json_client.py`'s Faust defs: instead
of formatting the `SynthDefSpec` JSON by hand, compose it with the lowercase
callables in `clausters.defs.ugens` and let `SynthDef` serialize the graph.

The build is **instance-based** — the graph is just the tree of composed
objects, with no thread-global "current SynthDef" the way sclang has — so
several defs can be built side by side. Arithmetic operators map to the
server's `Add`/`Sub`/`Mul`/`Div` UGens (the only math UGens it has; reach for a
Faust def for anything else).

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
from clausters.defs import Server, SynthDef, control, sin_osc, out
from clausters.seq import Pbind, Pseq

FREQS = [262.0, 330.0, 392.0, 523.0]
SR = 48000.0


def py_default(name="py_default") -> SynthDef:
    """`SinOsc(freq) * amp` to buses 0 and 1 — the client-side twin of the
    server's built-in `default`. `freq`/`amp` are named controls (the
    `/s_new`/`/n_set` parameters a `Pbind` drives)."""
    freq = control("freq", 440.0)
    amp = control("amp", 0.2)
    sig = sin_osc(freq) * amp          # `*` composes a Mul UGen
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


def render_pbind(instrument: str, sdef: SynthDef | None):
    """Render the arpeggio on `instrument`; if `sdef` is given, score its
    `/d_recv` first so the offline renderer compiles it before time advances."""
    server = Server(interface=OscNrtInterface())
    if sdef is not None:
        server.add_synthdef(sdef)      # scored at t=0 in NRT
    clock = TempoClock(tempo=1.0)
    Pbind(instrument=instrument, freq=Pseq(FREQS), dur=0.5, amp=0.2).play(clock, server)
    clock.render()                     # drain the clock logically
    return server.render(sample_rate=SR, channels=2)


def main():
    sdef = py_default()
    print("SynthDef JSON the server compiles (/d_recv):")
    print(json.dumps(sdef.spec(), indent=2))
    print("controls:", sdef.control_names())

    builtin, b_frames = render_pbind("default", None)
    custom, c_frames = render_pbind("py_default", sdef)

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
        print(f"wrote {path} — listen with: ffplay -autoexit {path}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError) as e:
        sys.exit(str(e))
