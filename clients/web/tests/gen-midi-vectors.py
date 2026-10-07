#!/usr/bin/env python3
"""Generate midi-vectors.json from the Python client's MIDI layer.

Two things have to agree across the languages, and they are of different kinds.

**The parse** is each client's own code -- `parse_midi` there, `parseMidi` here --
so the vectors freeze the reference client's answer for a spread of raw
messages, including the ones that are supposed to decode to nothing.

**The files** are not: `MidiScore.to_smf`/`to_clip` call `clausters-midi`
through the C ABI, and `MidiScore.toSmf`/`toClip` call the same writers through
the core's wasm door, so the bytes are the same by construction rather than by
care. Freezing them is what proves the wasm door was actually wired to those
writers and not to a second implementation -- the failure this whole
arrangement exists to prevent.

**And the note mapping**: an `Event` rendered on a `MidiServer` becomes a note
on/off pair, and where those land in beats and what velocity they carry is
client-side arithmetic in both.

**And a zone**: a `MidiServer` that is an MPE zone puts each note on a member
channel, its expression ahead of it. The channel and the bytes are the crate's,
reached through the C ABI there and the wasm door here; which notes have ended
by the time the next one asks for a channel is each client's own bookkeeping.
So the frozen score proves both at once.

The JSON is committed; regenerate with:

    python3 gen-midi-vectors.py

(from clients/web/tests/, with the Python client importable -- the repo's .venv
has it installed editable -- and libclausters_midi built.)
"""

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "python"))

from clausters.base._midiinterface import (  # noqa: E402
    MidiNrtInterface, MidiScore, MidiServer, parse_midi,
)
from clausters.base import TempoClock  # noqa: E402
from clausters.base.timebase import LogicalTimebase  # noqa: E402
from clausters.seq import Pbind, Pseq  # noqa: E402
from clausters.seq.event import Event  # noqa: E402

PPQ = 480


def parse_cases():
    """Raw bytes in, message dict (or None) out."""
    raw = [
        [0x90, 60, 100],           # note on
        [0x92, 64, 0],             # note on, velocity 0 (running-status off)
        [0x80, 60, 64],            # note off
        [0xA5, 48, 20],            # poly aftertouch
        [0xB0, 7, 127],            # control change
        [0xC3, 12],                # program change (two bytes)
        [0xD1, 90],                # channel aftertouch
        [0xE0, 0x00, 0x40],        # pitch wheel, centre
        [0xE0, 0x7F, 0x7F],        # pitch wheel, top
        [0xEF, 0x01, 0x00],        # pitch wheel, channel 15, one step up
        [0x90, 60],                # truncated: data2 reads 0
        [0xF8],                    # clock: not channel-voice
        [0xFF],                    # reset: not channel-voice
        [60, 100],                 # no status byte at all
        [],                        # nothing
    ]
    return [{"bytes": b, "parsed": parse_midi(b)} for b in raw]


def score() -> MidiScore:
    """A short two-voice figure with a same-beat off/on pair, which is what the
    stable sort is for: the note-off of the held note must stay ahead of the
    re-trigger at the same tick."""
    s = MidiScore()
    s.add(0.0, bytes((0x90, 60, 100)))
    s.add(0.5, bytes((0x90, 67, 80)))
    s.add(1.0, bytes((0x80, 60, 0)))
    s.add(1.0, bytes((0x90, 60, 110)))     # same beat, after the off
    s.add(1.5, bytes((0xB0, 7, 96)))       # a controller in the middle
    s.add(2.0, bytes((0x80, 67, 0)))
    s.add(2.0, bytes((0x80, 60, 0)))
    s.add(0.25, bytes((0xE0, 0x00, 0x40))) # out of order on purpose
    return s


def note_cases():
    """`Event` -> the note pair a MidiServer renders, at beat 0."""
    out = []
    for props in [
        {"midinote": 60, "amp": 0.5, "dur": 1.0},
        {"midinote": 72.4, "amp": 1.0, "dur": 0.5, "legato": 0.5},
        {"freq": 440.0, "amp": 0.0, "dur": 2.0},
        {"degree": 2, "amp": 0.25, "dur": 1.0},
        {"type": "rest", "dur": 1.0},
    ]:
        server = MidiServer(MidiNrtInterface(), channel=3)
        server.play_event(Event(**props))
        out.append({
            "props": props,
            "channel": 3,
            "events": [[beat, list(msg)] for beat, msg in server.score.sorted()],
        })
    return out


# One figure for both zones: four notes that overlap three at a time, so the
# fourth has to take the channel of the first one to end. It states a bend and
# a pressure and no timbre, which therefore goes back to its rest on every note.
ZONE_FIGURE = {
    "midinote": [60, 64, 67, 72],
    "dur": 0.5,
    "legato": 1.5,
    "amp": 0.5,
    "bend": [12.0, 0.0, -24.0, 0.5],
    "press": [1.0, 0.0, 0.5, 0.25],
}


def zone_cases():
    """A pattern played to a zone: the score a `MidiServer(zone=...)` keeps."""
    out = []
    for options, more in [
        ({"zone": 3}, {}),
        # The upper zone, a narrower bend range, and the timbre stated.
        ({"zone": 2, "upper": True, "bend_range": 24.0},
         {"slide": [0.0, 1.0, 0.5, 0.25]}),
    ]:
        figure = {**ZONE_FIGURE, **more}
        server = MidiServer(MidiNrtInterface(), **options)
        clock = TempoClock(tempo=1.0, timebase=LogicalTimebase())
        Pbind(**{
            key: Pseq(value) if isinstance(value, list) else value
            for key, value in figure.items()
        }).play(clock, server)
        clock.render()
        out.append({
            "options": options,
            "figure": figure,
            "events": [[beat, list(msg)] for beat, msg in server.score.sorted()],
        })
    return out


def main():
    s = score()
    vectors = {
        "ppq": PPQ,
        "parse": parse_cases(),
        "score": {
            "events": [[beat, list(msg)] for beat, msg in s.events],
            "sorted": [[beat, list(msg)] for beat, msg in s.sorted()],
            "smf": list(s.to_smf(PPQ)),
            "clip": list(s.to_clip(PPQ)),
        },
        "notes": note_cases(),
        "zones": zone_cases(),
    }
    path = pathlib.Path(__file__).with_name("midi-vectors.json")
    path.write_text(json.dumps(vectors, indent=1) + "\n")
    print(f"wrote {path.name}: {len(vectors['parse'])} parse cases, "
          f"{len(vectors['score']['smf'])} SMF bytes, "
          f"{len(vectors['notes'])} note cases, {len(vectors['zones'])} zones")


if __name__ == "__main__":
    main()
