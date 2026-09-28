#!/usr/bin/env python3
"""``edit(timeline)``: a timeline's events on a roll, edited note by note.

A `clausters.seq.Timeline` is code -- events, patterns, routines, other
timelines -- and a roll edits data. So ``edit`` renders the timeline first
(`clausters.seq.Timeline.render_events`) and opens the events it produced: a
`clausters.seq.EventSequence`, in the timeline's beats with its tempo map, each
event with an id of its own. The editor edits that sequence in place, and the
timeline is left as it was.

What to do in the window: **drag a note** to move it, **drag its edge** to
resize, **Shift and drag it up or down** to change its velocity (drawn as the
note's fill), **Ctrl+click** to add or remove one, and **Ctrl+Z** to step back.
**The space bar plays and pauses** -- the editor's own playback, on a transport
of its own -- and an edit while it plays is heard at once: a note moved ahead
of the line sounds where it lands, and a note already sounding ends as it
would have.

**Click the ruler under the grid** to place the position cursor: the play line
goes there, and the next play starts from it. While it plays, the line is
where the transport is.

**A second window shows the same notes in hertz** -- the same sequence, the
roll's vertical axis a frequency on a log scale, ruled in round frequencies. A
note is where it is in the first window, an octave is the same height, and a
drag there moves it continuously, writing its ``freq``: the MIDI note it was
written with follows, and the first window redraws it between the keys. An
edit in either window is one history.

**The lane under the grid is the sequence's OSC markers**, and it is edited the
same way: drag one to move it, Ctrl+click one to remove it. A marker is matched
by its **label**, the address it sends, so the message survives the drag. Adding
one *there* is refused and says why: the lane has no way to type an address.

**A note keeps what the roll cannot draw.** Every note on the roll carries the
id of its event, so an edit names the note it touched: its instrument, its
amplitude and anything else the author put on it stay, and removing one note
leaves every other note its own.

Run it as a script, or step through the cells::

    pip install -e clients/python

    python clients/python/examples/editors/edit_notes.py

It self-launches the audio server and the GUI host: this one plays.
"""

# %%
import sys

from clausters import Session
from clausters.gui import edit
from clausters.seq import OscItem, Timeline
from clausters.seq.event import Event

# %% [markdown]
# ## A timeline, filled the ordinary way
#
# Beats and events. The `instrument` and `amp` on the last one are what the
# roll cannot draw and editing must not lose -- and so are the marker's
# arguments.

# %%
timeline = Timeline([
    (0.0, Event(midinote=60, dur=1.0)),
    (1.0, Event(midinote=64, dur=1.0)),
    (2.0, Event(midinote=67, dur=2.0)),
    (4.0, Event(midinote=72, dur=1.0, instrument="default", amp=0.4)),
    (3.0, OscItem("/mark", 1, "cue")),
])

# %% [markdown]
# ## One verb
#
# The timeline is rendered into the events it plays, and those open as a
# `clausters.gui.editing.NotesEditor`: one roll, each note with its id.

# %%
# `activate` is what makes this session the **ambient** one, and the free-standing
# verbs are what need it: `play` below resolves its server from here, the way
# `edit` resolves its host.
session = Session.live().activate()
session.gui()          # the host wired to this session's server
editor = edit(timeline,
              sample_rate=session.server.query_info().nominal_sample_rate,
              title="notes")
notes = editor.sequence        # what the roll edits, in place

# %% [markdown]
# ## The same notes, in hertz
#
# A second editor over the very same sequence, whose vertical axis is the
# frequency: the same pitch in another coordinate.

# %%
hertz = edit(notes,
             sample_rate=session.server.query_info().nominal_sample_rate,
             y_axis="hz", title="notes in hertz")


# %% [markdown]
# ## Play what was drawn
#
# The editor plays the sequence it edits, on its own transport -- the space bar
# in the window is the same verb.

# %%
def play():
    """Play the sequence as it now stands, from the position cursor."""
    editor.play()


# %% [markdown]
# ## What a roll cannot say
#
# Five numbers per note -- start, length, pitch, velocity, channel -- and the
# id that names its event. The instrument, the amp and anything else the author
# wrote are none of them, and they are still there after an edit.

# %%
def read_back():
    """Every event as it now stands, with its id and what the roll never drew."""
    for id, beat, item in notes.entries():
        if item.get("type") == "osc":
            print(f"  #{id:<3} {beat:5.2f}  osc {item['addr']}   {list(item['args'])}")
            continue
        extra = {k: v for k, v in dict(item).items()
                 if k in ("instrument", "amp", "velocity")}
        print(f"  #{id:<3} {beat:5.2f}  midinote {item.midinote():5.1f}"
              f"  freq {item.freq():7.1f}   {extra}")


# %%
def run():
    """Keep the window open until it is closed, then print what was drawn."""
    print("edit the notes; the space bar plays them. Close when done.")
    editor.wait()
    print("the sequence, as it was left:")
    read_back()


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run()
else:
    print("up -- play() to hear it, read_back() to see what survived the edit")
