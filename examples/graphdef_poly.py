#!/usr/bin/env python3
"""A polyphonic GraphDef: shared part + per-voice part (M18).

A GraphDef member can be marked `voice=True`. The **shared** members are
instantiated once at `server.graph(...)` (the always-on part: the private bus
and the mixer); each **voice** member is instantiated per note with
`server.graph_voice(...)`, wired into the same shared bus. This is the model a
MIDI binding uses too: `/midi_bind <ch> poly` spawns the shared instance and
each note spawns a voice — drive it from a controller with
`clausters --midi` and `aconnect`.

Here a routine spawns an arpeggio of overlapping voices into one instance and
frees each after its note. Renders offline; build the embed library once:

    cargo build --release --features embed,realtime

then:

    python3 examples/graphdef_poly.py [out.wav]
"""

import json
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))

from clausters.base import OscNrtInterface, Routine, TempoClock
from clausters.defs import GraphDef, Server, SynthDef, control, in_, out, sine

SR = 48000.0
NOTES = (220.0, 277.0, 330.0, 440.0)  # an A-major arpeggio


def member_defs():
    """`vtone` (per voice): a sine at `freq`·`level` into the bus `out`.
    `vgain` (shared): reads the bus `in`, scales by `gain`, to the speakers."""
    freq, out_bus, level = control("freq", 440.0), control("out", 0.0), control("level", 0.2)
    vtone = SynthDef("vtone", out(out_bus, sine(freq) * level))

    in_bus, gain = control("in", 0.0), control("gain", 0.3)
    sig = in_(in_bus) * gain
    vgain = SynthDef("vgain", out(0.0, sig), out(1.0, sig))
    return vtone, vgain


def poly_graph() -> GraphDef:
    g = GraphDef("poly")
    mix = g.bus("mix")
    amp = g.add("vgain", **{"in": mix})                 # shared mixer (always on)
    voice = g.add("vtone", out=mix, voice=True)         # per-voice oscillator
    g.port("gain", amp["gain"], default=0.3)            # shared port
    g.port("freq", voice["freq"], default=220.0)        # voice ports
    g.port("amp", voice["level"], default=0.2)
    return g


def play(server):
    vtone, vgain = member_defs()
    for sdef in (vtone, vgain):
        server.add_synthdef(sdef)
    server.add_graphdef(poly_graph())

    inst = server.graph("poly", {"gain": 0.3})          # the shared instance, once
    for i, freq in enumerate(NOTES):
        vid = server.nodes.alloc()                      # a per-note voice id
        # Spawn a voice into the instance at this beat, then free it later
        # (overlapping: each rings for 0.6 beats while notes step every 0.3).
        server.send_bundle(("/graph_voice", inst.id, vid, "freq", freq, "amp", 0.2))
        server.send_bundle(("/n_free", vid), delay_beats=0.6)
        yield 0.3
    yield 0.6
    server.send_bundle(("/n_free", inst.id))            # tear the instance down


def main():
    print("GraphDef JSON the server validates (/d_graph):")
    print(json.dumps(poly_graph().spec(), indent=2))

    server = Server(interface=OscNrtInterface())
    clock = TempoClock(tempo=2.0)
    clock.play(Routine(lambda: play(server)))
    clock.render()
    path = sys.argv[1] if len(sys.argv) > 1 else None
    stats = server.render(sample_rate=SR, channels=2, path=path)

    peak = max(stats.peak)
    print(f"\nrendered {stats.frames} frames ({stats.duration:.3f} s) | peak {peak:.3f}")
    if peak < 0.05:
        sys.exit("unexpectedly quiet: the polyphonic GraphDef did not sound")
    print("the shared instance played four overlapping per-voice notes.")

    if path:
        print(f"wrote {path} — listen with: pw-play {path}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError) as e:
        sys.exit(str(e))
