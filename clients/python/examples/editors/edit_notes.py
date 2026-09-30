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
**The space bar plays and stops** -- the editor's own playback, on a transport
of its own -- and a stop goes back to the position cursor. An edit while it
plays is heard at once: a note moved ahead of the line sounds where it lands,
and a note already sounding ends as it would have. The pass does not stop at
the last note: it rolls on, as a multitrack's does, until the space bar stops
it -- ``editor.end = "contents"`` stops it where the last note ends, and a
beat is an end marker.

**Click the ruler under the grid** to place the position cursor: the play line
goes there, and the next play starts from it. While it plays, the line is
where the transport is. **Home** puts the cursor at the start and **End** where
the last note ends. **Alt and a drag** anywhere on the roll marks a **time range**, as a drag does
in the audio editor: the space bar then plays it, from its start to its end,
and **L** loops it (with no range, L loops every note); an Alt click still
toggles the note it lands on. **The wheel over the keyboard scrolls through the octaves**
-- every MIDI note, and in the hertz window up to 20 kHz -- and **Ctrl** with
it zooms; the roll opens on the notes. In hertz a note is a bar of one height
whatever the zoom, centred on the line of its frequency, and a drag lands on a
round frequency -- finer as you zoom in. On the keys a note moved in hertz sits
on its nearest key, the line inside the box its bend off that key; dragging it
there transposes by semitones and keeps the bend, and a note past MIDI 127 is
a strip at the top edge.

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

**Curves are drawn and edited like a multitrack's automation.** The row under
the grid, *brightness*, is a lane of the sequence -- a CC 74 curve over all of
it -- and the first note carries a curve of its own, a **bend**, drawn in the
plane as the pitch it glides to. **Drag a point** to move it, **Ctrl+click** to
add one or remove the one under the cursor; one gesture is one edit, and
Ctrl+Z takes it back.

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
# arguments. A marker is a message the server runs when it plays; this one
# writes a value to a control bus.

# %%
timeline = Timeline([
    (0.0, Event(midinote=60, dur=1.0)),
    (1.0, Event(midinote=64, dur=1.0)),
    (2.0, Event(midinote=67, dur=2.0)),
    (4.0, Event(midinote=72, dur=1.0, instrument="default", amp=0.4)),
    (3.0, OscItem("/bus_set", 100, 0.5)),
])

# %% [markdown]
# ## One verb, spelled out
#
# `edit(timeline)` renders the timeline into the events it plays and opens
# them as a `clausters.gui.editing.NotesEditor`: one roll, each note with its
# id. Here the render is its own line, so the sequence gets its curves before
# it opens: a CC lane on the first channel, and a bend on its first note that
# glides up two semitones while the note is held and falls back one in its
# release -- a note's curve runs past its box, which is its on and its off.

# %%
# `activate` is what makes this session the **ambient** one, and the free-standing
# verbs are what need it: `play` below resolves its server from here, the way
# `edit` resolves its host.
session = Session.live().activate()
session.gui()          # the host wired to this session's server
notes = timeline.render_events()   # what the roll edits, in place
notes.add_lane({"cc": 74, "channel": 0}, [(0.0, 20.0), (4.0, 110.0)], name="brightness")
first = notes.entries()[0][0]
notes.add_expression(first, {"bend": True}, [(0.0, 0.0), (0.8, 2.0), (1.2, 1.0)])
editor = edit(notes,
              sample_rate=session.server.query_info().nominal_sample_rate,
              title="notes")

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
