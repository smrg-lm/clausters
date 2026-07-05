#!/usr/bin/env python3
"""Play an event pattern **live** out a virtual MIDI port (M17 client sub-part 2).

The same `Pbind` that renders to a `.mid` can instead drive a live MIDI output
port: `MidiServer(interface=MidiRtInterface(...))`. Each note goes out the port
in real time (note-on at its beat, note-off scheduled after the sustain) — no
timetags, best-effort, the way live MIDI works. Connect the port to a synth or
to this server's own MIDI input (`clausters --midi`) with `pw-link` (or wire it
visually in qpwgraph).

Needs the live cdylib:

    cargo build --release -p clausters-midi --features live

The script **creates a virtual MIDI output port** (named `clausters` by
default, or the `[port-name]` argument) and plays into it:

    python3 examples/midi_live.py [port-name] [seconds]

That port is just a loose cable until you wire it to a destination. With the
script still running, in another terminal start something that has a MIDI input
-- e.g. this server's own MIDI input port `clausters-in`:

    cargo run -- --midi clausters-in

then connect the two ports with `pw-link` (list MIDI ports with `pw-link -o`
for sources and `pw-link -i` for sinks), or wire them in qpwgraph:

    pw-link clausters clausters-in
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))

from clausters.base import MidiRtInterface, MidiServer, TempoClock
from clausters.seq import Pbind, Pseq, Pwhite


def main() -> None:
    port = sys.argv[1] if len(sys.argv) > 1 else "clausters"
    seconds = float(sys.argv[2]) if len(sys.argv) > 2 else 4.0

    interface = MidiRtInterface(port=port)
    print(f'MIDI output on virtual port "{port}" (connect with aconnect); playing...')
    midi = MidiServer(interface=interface, channel=0)
    clock = TempoClock(tempo=2.0)
    Pbind(
        instrument="default",
        degree=Pseq([0, 2, 4, 2, 0, 4, 7, 4], repeats=8),
        dur=0.5,
        amp=Pwhite(0.5, 0.9),
        legato=0.9,
    ).play(clock, midi)
    try:
        clock.run(seconds)  # real time
    finally:
        interface.close()


if __name__ == "__main__":
    main()
