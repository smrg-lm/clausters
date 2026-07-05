#!/usr/bin/env python3
"""Render an event pattern to a Standard MIDI File (M17 client sub-part 1).

The same `Pbind` you would play live or render to audio can target a **MIDI
destination** instead: `MidiServer` is the double-dispatch counterpart of the
OSC `Server`. A pattern played on a `TempoClock` with a `MidiServer` realizes
each note `Event` as a note on/off pair into a `MidiScore` (in beats), and
`write` serializes it to a `.mid` through the `clausters-midi` crate — the
interop format every DAW reads. No server, no audio device: just a file.

The clock, routine and pattern are unchanged from the audio path; only the
destination differs (the seam). Note number comes from the Event's `midinote`
(or `degree`/`freq`), velocity from `amp`.

    cargo build --release -p clausters-midi
    python3 examples/midi_file.py [out.mid]
    python3 examples/midi_file.py --clip [out.midiclip]   # MIDI 2.0 clip (16-bit vel)
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))

from clausters.base import MidiServer, TempoClock
from clausters.seq import Pbind, Pseq, Pwhite


def phrase() -> Pbind:
    """A short phrase: a walking scale degree, varied dynamics and durations."""
    return Pbind(
        instrument="default",  # only the name matters for MIDI (no synth is made)
        degree=Pseq([0, 1, 2, 3, 4, 3, 2, 1, 0], repeats=2),
        dur=Pseq([0.5, 0.5, 0.25, 0.25, 1.0], repeats=4),
        amp=Pwhite(0.4, 0.9),
        legato=0.9,
    )


def main() -> None:
    args = sys.argv[1:]
    clip = "--clip" in args
    paths = [a for a in args if not a.startswith("--")]
    out = paths[0] if paths else ("out.midiclip" if clip else "out.mid")

    midi = MidiServer(channel=0, ppq=480)
    clock = TempoClock(tempo=2.0)  # 2 beats/second; tempo only scales the clock
    phrase().play(clock, midi)
    clock.render()  # NRT: drive the routine to the end, no sleeping

    midi.write(out, fmt="clip" if clip else "smf")
    notes = sum(1 for _, m in midi.score.sorted() if (m[0] & 0xF0) == 0x90 and m[2] > 0)
    fmt = "MIDI 2.0 clip" if clip else "SMF"
    print(f"wrote {out} ({fmt}): {notes} notes, {len(midi.score.events)} MIDI events")


if __name__ == "__main__":
    main()
