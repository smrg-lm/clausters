#!/usr/bin/env python3
"""MPE: each note bends, presses and colours on its own.

Plain MIDI puts a whole keyboard on one channel, so a bend moves every note at
once. **MIDI Polyphonic Expression** gives each note a channel of its own --
a *zone* of member channels -- and the server plays a zone that way: each
note is a voice, and the bend, pressure and third dimension ("timbre") on its
channel reach that voice alone, the bend as pitch.

This script is both ends. It boots a server with its MIDI input open, binds
the lower zone (fifteen members) to the built-in `default` -- which declares
the three dimensions a zone's voice starts with: `freq`, `press` (the
pressure: brighter and louder) and `slide` (the timbre: opening its filter)
-- and plays an MPE stream into that input from a MIDI port of its own:

- a chord whose three notes are bent apart by different amounts (a quarter
  tone up, a quarter tone down, none) -- one channel could not say it;
- the same chord, each note pressed and coloured differently: a soft dark
  note, a medium one and a loud bright one.

It needs an audio device, the live MIDI cdylib (`scripts/refresh-bin.sh`
stages it) and the `aconnect` tool, which it uses to wire its output port to
the server's input. Run it:

    python3 examples/midi_mpe.py

`docs/schemas.md` ("MPE zones") has the rules, `/midi_bindZone` and
`/midi_query`.
"""

import os
import re
import subprocess
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))

from clausters import Session  # noqa: E402
from clausters.base import MidiRtInterface, MidiServer, TempoClock  # noqa: E402
from clausters.seq import Pbind, Pseq  # noqa: E402

SERVER_PORT = "clausters-in"
OUR_PORT = "clausters-mpe"


def port_address(name: str) -> str | None:
    """The ALSA sequencer address (`client:port`) of the port called `name`.
    Both ends are clients named `clausters`, so the port's own name is what
    tells them apart."""
    listing = subprocess.run(["aconnect", "-l"], capture_output=True, text=True,
                             env={**os.environ, "LC_ALL": "C"}).stdout
    client = None
    for line in listing.splitlines():
        if m := re.match(r"client (\d+):", line):
            client = m.group(1)
        elif (m := re.match(r"\s+(\d+) '(.*?)\s*'", line)) and m.group(2) == name:
            return f"{client}:{m.group(1)}"
    return None


def main():
    with Session.live(server_args=("--midi", SERVER_PORT)) as session:
        server = session.server
        # The lower zone, fifteen members, gate-aware: a note-off closes the
        # def's gate rather than freeing it, and the release rings out.
        server.midi_bind_zone(0, 15, "default", gate=True)

        interface = MidiRtInterface(port=OUR_PORT)
        try:
            time.sleep(0.3)
            ours, theirs = port_address(OUR_PORT), port_address(SERVER_PORT)
            if ours is None or theirs is None:
                sys.exit(f"could not find the ports {OUR_PORT} and {SERVER_PORT} (aconnect -l)")
            subprocess.run(["aconnect", ours, theirs], check=True)
            # A MIDI destination that is an MPE zone: the RPN declaring it
            # goes out now, and each note takes a member channel of its own.
            midi = MidiServer(interface=interface, zone=15)
            clock = TempoClock(tempo=1.0)
            chord = [64, 67, 71]  # E4, G4, B4

            def play(**per_note):
                # One pattern per note of the chord, all on the same beat, each
                # with its own value of every key in `per_note`.
                for i, key in enumerate(chord):
                    keys = {name: values[i] for name, values in per_note.items()}
                    Pbind(instrument="default", midinote=Pseq([key]), dur=2.0,
                          legato=0.9, **keys).play(clock, midi)

            print("a chord bent apart: a quarter tone up, one down, none")
            play(bend=[0.5, -0.5, 0.0])
            clock.run(2.2)

            print("the same chord, each note pressed and coloured on its own")
            play(press=[0.1, 0.5, 1.0], slide=[0.0, 0.4, 1.0])
            clock.run(2.5)
        finally:
            interface.close()
        server.midi_unbind(0)
        print("done")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
