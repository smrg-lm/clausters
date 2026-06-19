#!/usr/bin/env python3
"""Play an event pattern **live** out a virtual MIDI port (M17 client sub-part 2).

The same `Pbind` that renders to a `.mid` can instead drive a live MIDI output
port: `MidiServer(interface=MidiRtInterface(...))`. Each note goes out the port
in real time (note-on at its beat, note-off scheduled after the sustain) — no
timetags, best-effort, the way live MIDI works. Connect the port to a synth or
to this server's own MIDI input (`clausters --midi`) with `aconnect`.

Needs the live cdylib:

    cargo build --release -p clausters-midi --features live
    python3 examples/midi_live.py [port-name] [seconds]

Then, in another terminal, route it somewhere, e.g. into the server:

    clausters --midi clausters-in
    aconnect <this port> clausters-in        # see `aconnect -l`
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
        amp=Pwhite(0.5, 0.9, seed=2),
        legato=0.9,
    ).play(clock, midi)
    try:
        clock.run(seconds)  # real time
    finally:
        interface.close()


if __name__ == "__main__":
    main()
