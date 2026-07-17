#!/usr/bin/env python3
"""GraphDef: a node-graph "program" with a named parameter surface (M18).

Where a SynthDef is one node, a `GraphDef` is a whole wired patch the server
stores and instantiates as a unit. It exposes a **named parameter surface** —
ports that map to inner member controls — so you drive the running instance
through the port names, never the private member node ids.

This builds a two-oscillator voice: two `tone` members write a detuned pair
into one private internal bus, and a `gain` member reads that bus and sends it
to the speakers. The surface shows what a bare scsynth group `/n_set` cannot:

  * one port driving **several** inner targets, and
  * **per-target scaling** — the single `freq` port plays a perfect fifth by
    mapping to `tone1.freq` directly and to `tone2.freq` scaled by 1.5.

Renders offline (NRT); build the embed library once:

    cargo build --release --features embed,realtime

then:

    python3 examples/graphdef.py [out.wav]
"""

import json
import os
import struct
import sys
import wave

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))

from clausters.base import OscNrtInterface, Routine, TempoClock
from clausters.defs import GraphDef, Server, SynthDef, control, in_, out, sine

SR = 48000.0


def member_defs():
    """The two SynthDefs the GraphDef wires together. `tone` writes a sine to
    the bus named by its `out` control; `gain` reads its `in` bus, scales it,
    and writes to the hardware buses 0/1."""
    freq, out_bus = control("freq", 440.0), control("out", 0.0)
    tone = SynthDef("tone", out(out_bus, sine(freq) * 0.15))

    in_bus, g = control("in", 0.0), control("gain", 0.4)
    sig = in_(in_bus) * g
    gain = SynthDef("gain", out(0.0, sig), out(1.0, sig))
    return tone, gain


def duo_graph() -> GraphDef:
    g = GraphDef("duo")
    mix = g.bus("mix")                              # one private audio bus
    t1 = g.add("tone", out=mix)                     # both tones sum into `mix`
    t2 = g.add("tone", out=mix)
    amp = g.add("gain", **{"in": mix})              # reads `mix` -> speakers
    # Named surface: one `freq` port -> two targets (the second a fifth up),
    # one `gain` port -> the mixer's gain. External actuation hits these names.
    g.port("freq", t1["freq"], t2["freq"].scaled(1.5), default=220.0)
    g.port("gain", amp["gain"], default=0.4)
    return g


def play(server):
    tone, gain = member_defs()
    duo = duo_graph()
    for sdef in (tone, gain):
        server.add_synthdef(sdef)                   # scored at t=0 in NRT
    server.add_graphdef(duo)                         # /d_graph (validate + store)

    inst = server.graph("duo", {"gain": 0.4})        # /graph_new -> a wired group
    # Drive the instance through its surface. Live this is
    # `server.set(inst, {"freq": freq})`; to schedule it on the (offline) clock
    # use `send_bundle`, stamping each /n_set with the routine's logical beat.
    # /n_set on the instance resolves "freq" against the surface (both
    # oscillators), never the private member node ids.
    for freq in (220.0, 247.0, 196.0, 220.0):
        server.send_bundle(("/n_set", inst.id, "freq", freq))
        yield 0.5
    server.send_bundle(("/n_free", inst.id))         # frees the group + private buses


def main():
    duo = duo_graph()
    print("GraphDef JSON the server validates (/d_graph):")
    print(json.dumps(duo.spec(), indent=2))

    server = Server(interface=OscNrtInterface())
    clock = TempoClock(tempo=1.0)
    clock.play(Routine(lambda: play(server)))
    clock.render()
    samples, frames = server.render(sample_rate=SR, channels=2)

    peak = max(abs(s) for s in samples)
    print(f"\nrendered {frames} frames ({frames / SR:.3f} s) | peak {peak:.3f}")
    if peak < 0.05:
        sys.exit("unexpectedly quiet: the GraphDef did not sound")
    print("the GraphDef played; one `freq` port drove both detuned oscillators.")

    if len(sys.argv) > 1:
        path = sys.argv[1]
        with wave.open(path, "wb") as w:
            w.setnchannels(2)
            w.setsampwidth(2)
            w.setframerate(int(SR))
            w.writeframes(b"".join(
                struct.pack("<h", int(max(-1.0, min(1.0, s)) * 32767)) for s in samples
            ))
        print(f"wrote {path} — listen with: ffplay -autoexit {path}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError) as e:
        sys.exit(str(e))
