#!/usr/bin/env python3
"""An event lane: notes the transport plays by its position, as it plays a take.

A take follows the transport because its reader reads the position -- a
locate, a loop and a stop are the transport's, and nothing is sent per pass.
An **event lane** does the same for notes and messages: they are data, at
samples of the transport's position, and the server plays them there. This
script sends one lane a four-note figure and then only moves the transport:

- it plays the figure through once;
- it seeks back into the middle of it, and the note that was sounding is
  released on the jump -- the notes ahead play from there;
- it loops the figure's first half, and the lane plays again on every pass
  with no message from here;
- while the loop turns it sends the lane new data -- the same figure a fifth up
  -- and the next pass is the new one, while the note sounding keeps its end;
- it stops in the middle of a note, which is released there and rings out,
  as a DAW's stop sends its note-offs;
- it sends the lane the same figure as **MIDI messages** -- a note-on and a
  note-off per note, on channel 0 -- and binds that channel to the default
  instrument with `/midi_bind`: the server plays them as though they had
  reached its MIDI input at those positions, and a locate releases the MIDI
  note sounding as it released the synth.

Listen for each change at the moment the script names it. Needs an audio
device (it boots its own server and plays through the sound card). Run it:

    python3 examples/event_lane.py

`docs/sample-clock.md` ("Events on the transport's position") explains which
axis a lane is on, and `docs/schemas.md` has `/lane_new`, `/lane_set` and
`/lane_free`.
"""

import os
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, os.path.join(ROOT, "clients", "python"))

from clausters import Session  # noqa: E402
from clausters.defs import Group  # noqa: E402

SR = 48000
LANE = 7
BEAT = SR // 2  # two notes a second


def figure(transpose=1.0):
    """Four notes, one a beat, each held for most of it. Every position is a
    sample of the transport."""
    notes = []
    for i, hz in enumerate([220.0, 277.183, 329.628, 440.0]):
        start = i * BEAT
        notes.append([start, start + int(0.8 * BEAT), "default",
                      {"freq": hz * transpose, "amp": 0.2}, "gate"])
    return {"notes": notes}


def midi_figure():
    """The same four notes as MIDI messages on channel 0: a note-on and, most
    of a beat later, its note-off, each ``[position, status, key, velocity]``."""
    midi = []
    for i, key in enumerate([57, 61, 64, 69]):  # A3, C#4, E4, A4
        start = i * BEAT
        midi.append([start, 0x90, key, 90])
        midi.append([start + int(0.8 * BEAT), 0x80, key, 0])
    return {"midi": midi}


def main():
    with Session.live() as session:
        server = session.server
        # The transport rolls in the engine once it governs a group. The
        # lane's notes are made in another one, which a stop does not freeze:
        # a stop releases them, and their releases ring out.
        group = Group()
        server.transport_group(group)
        voices = Group()
        server.lane_new(LANE, voices)
        server.lane_set(LANE, figure())
        server.transport_locate_sample(0)

        print("the figure, once through")
        server.transport_play()
        time.sleep(2.2)

        print("back to its second beat: what sounds is released, the rest plays on")
        server.transport_locate_sample(BEAT + BEAT // 2)
        time.sleep(1.8)

        print("looping its first half: the lane plays on every pass, nothing sent")
        server.transport_loop((0, 2 * BEAT))
        server.transport_locate_sample(0)
        time.sleep(2.0)

        print("new data mid-loop: the next pass is a fifth up")
        server.lane_set(LANE, figure(transpose=1.5))
        time.sleep(2.0)

        print("a stop mid-note: the note is released, and its release rings out")
        server.transport_loop(None)
        server.transport_locate_sample(0)
        time.sleep(0.25)
        server.transport_stop()
        time.sleep(1.0)

        print("the figure as MIDI, through channel 0's binding")
        # `/midi_bind channel instrument target addAction gate`: voices of
        # the default instrument at the tail of `voices`, released by gate.
        server.send_msg("/midi_bind", 0, "default", voices.id, 1, 1)
        server.lane_set(LANE, midi_figure())
        server.transport_locate_sample(0)
        server.transport_play()
        time.sleep(1.2)

        print("back to its first beat: the MIDI note sounding is released")
        server.transport_locate_sample(BEAT // 2)
        time.sleep(2.0)
        server.transport_stop()
        server.send_msg("/midi_unbind", 0)
        time.sleep(0.5)

        server.lane_free(LANE)
        server.transport_group(None)
        voices.free()
        group.free()
        print("done")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
