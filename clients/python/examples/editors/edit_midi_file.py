#!/usr/bin/env python3
"""A MIDI file with its curves: an MPE chord written, read back and edited.

A MIDI file says a bend, a pressure or a controller as a stream of messages; a
sequence says it as a curve. Which curve depends on what the message is
addressed to: a channel's stream is a **lane** of that channel, and in an MPE
zone each member channel is one note's, so its stream is that note's own
**expression**. So a sequence written for MPE writes each note's curves on a
member channel of its own, and a file read back gives them to the notes again.

Here a three-note chord is written for MPE (`set_midi("mpe")`): the lowest
note bends up a semitone, the top one bends down a quarter tone, the middle
one swells in pressure, and a volume lane runs over the whole zone. The
sequence goes to a Standard MIDI File (`to_smf`), into
`clients/python/examples/out/`, and comes back from it (`from_smf`): the roll
opens on what the file held -- **MPE** under its keyboard, the bends drawn in
the plane as the pitch each note takes, the volume lane as a row under the
grid. The space bar plays it on the server, the bends heard; a MIDI port
(`editor.play(destination=MidiServer(...))`) would hear the same messages the
file holds.

Run it as a script, or step through the cells::

    pip install -e clients/python

    python clients/python/examples/editors/edit_midi_file.py

It self-launches the audio server and the GUI host: this one plays.
"""

# %%
import pathlib
import sys

from clausters import Session
from clausters.gui import edit
from clausters.seq import EventSequence
from clausters.seq.event import Event

OUT = pathlib.Path(__file__).resolve().parent.parent / "out"

# %% [markdown]
# ## A chord written for MPE
#
# Three notes on one beat, each with a curve of its own -- the thing MIDI 1.0
# cannot say, since one channel's bend moves every note on it -- and a lane
# over the zone.

# %%
chord = EventSequence([
    (0.0, Event(midinote=60, dur=4.0)),
    (0.0, Event(midinote=64, dur=4.0)),
    (0.0, Event(midinote=67, dur=4.0)),
])
chord.set_midi("mpe")
low, middle, top = (id for id, _beat, _event in chord.entries())
chord.add_expression(low, {"bend": True}, [(0.0, 0.0), (2.0, 1.0)])
chord.add_expression(middle, {"pressure": True}, [(0.0, 0.2), (3.0, 1.0)])
chord.add_expression(top, {"bend": True}, [(1.0, 0.0), (3.0, -0.5)])
chord.add_lane({"cc": 7}, [(0.0, 90.0), (4.0, 120.0)], name="volume")

# %% [markdown]
# ## To a file and back
#
# The file is MIDI 1.0 bytes: the zone's configuration first, then each note
# on a member channel with its bend and pressure on that channel, and the
# volume on the zone's master channel. Read back, the streams are curves again.

# %%
OUT.mkdir(exist_ok=True)
path = OUT / "mpe_chord.mid"
path.write_bytes(chord.to_smf())
notes = EventSequence.from_smf(path.read_bytes())
print(f"{path.name}: {notes.midi}, {len(notes)} notes")
for held in notes.data()["events"]:
    curves = [list(c["target"])[0] for c in held.get("expression", [])]
    print(f"  #{held['id']:<3} {held['at']:5.2f}  midinote {held['data']['midinote']:5.1f}  {curves}")

# %% [markdown]
# ## On the roll
#
# The roll says which MIDI it is editing under its keyboard, draws each bend
# as the pitch its note takes, and the volume as a row under the grid.

# %%
session = Session.live().activate()
session.gui()
editor = edit(notes,
              sample_rate=session.server.query_info().nominal_sample_rate,
              title="mpe chord")


# %% [markdown]
# ## Play what was read
#
# The editor plays the sequence on its own transport -- the space bar in the
# window is the same verb.

# %%
def play():
    """Play the chord as it now stands, from the position cursor."""
    editor.play()


# %%
def run():
    """Keep the window open until it is closed."""
    print("the space bar plays the chord. Close when done.")
    editor.wait()


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run()
