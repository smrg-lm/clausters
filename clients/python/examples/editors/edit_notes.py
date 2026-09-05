#!/usr/bin/env python3
"""``edit(timeline)``: a roll over a timeline, with no composition under it.

The same gesture that edits a track's notes in the multitrack, over a
`clausters.seq.Timeline` a script filled — no arrangement, no document, no
track. Until now the only way to write a roll's edit back was an aggregate's
member list, which needs a tree to be a member *of*; this is the timeline
itself.

What to do in the window: **drag a note** to move it, **drag its edge** to
resize, **Ctrl+click** to add or remove one, and **Ctrl+Z** to step back. The
notes are the timeline's, so playing it after an edit plays what was drawn.

**The lane under the grid is the timeline's OSC markers**, and it is edited the
same way: drag one to move it, Ctrl+click one to remove it, Ctrl+Z to step back.
A marker is matched back to its item by its **label**, which is the address it
sends, so the message survives the drag — print it with ``read_back()``. Adding
one *there* is refused and says why, because a marker is the message it sends
and the lane has no way to type an address: add it here in the script instead.

**Nothing here drives a loop.** ``edit`` opens the window and the host's event
loop delivers each gesture to the timeline, so playing it after an edit plays
what was drawn without a step in between.

**A note keeps what the roll cannot draw.** Order is the only identity the
payload carries, so the i-th note's own `clausters.seq.Event` is *edited* rather
than rebuilt from the five numbers a roll can say — which is what keeps the
instrument, and everything else the author put on it.

Run it as a script, or step through the cells::

    pip install -e clients/python

    python clients/python/examples/editors/edit_notes.py

It self-launches the audio server and the GUI host: this one plays.
"""

# %%
import sys

import clausters
from clausters import Session
from clausters.gui import edit
from clausters.seq import OscItem, Timeline
from clausters.seq.event import Event

# %% [markdown]
# ## A timeline, filled the ordinary way
#
# Beats and events. The `instrument` on the last one is the point of the second
# cell below: the roll cannot draw it, and editing must not lose it — and so are
# the marker's arguments, which the lane draws even less of.

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
# A `Timeline` opens as a `clausters.gui.editing.NotesEditor` — one `pianoroll`
# widget, the crate's ``events`` vocabulary, and the timeline's own editing
# context.

# %%
session = Session.live()
session.gui()          # the host wired to this session's server
editor = edit(timeline,
              sample_rate=session.server.query_info().nominal_sample_rate,
              tempo=session.clock.tempo, title="notes")


# %% [markdown]
# ## Play what was drawn
#
# The timeline is the one the script holds, so playing it needs nothing from the
# editor: `clausters.seq.Timeline` plays itself on the session's clock.

# %%
def play():
    """Play the timeline as it now stands.

    The **free-standing** verb, not `clausters.Session.play`: that one plays an
    event *pattern*, and this is a timeline. `clausters.play` dispatches on what
    the structure is — the same question `clausters.gui.edit` asked to open this
    window — and hands a `clausters.seq.Timeline` to a
    `clausters.seq.timeline.Playhead` over the ambient session's clock and
    server.
    """
    clausters.play(timeline)


# %% [markdown]
# ## What a roll cannot say
#
# Five numbers per note — start, length, pitch, velocity, channel. The
# instrument, the amp and anything else the author wrote are none of them, and
# they are still there after an edit.

# %%
def read_back():
    """Every note as it now stands, with what the roll never drew."""
    for beat, item in timeline:
        if isinstance(item, OscItem):
            print(f"  {beat:5.2f}  osc {item.addr}   {list(item.args)}")
            continue
        extra = {k: v for k, v in dict(item).items()
                 if k not in ("midinote", "dur", "sustain", "velocity", "type")}
        print(f"  {beat:5.2f}  midinote {item.midinote():5.1f}   {extra}")


# %%
def run():
    """Keep the window open until it is closed, then print what was drawn."""
    print("edit the notes; space plays nothing here — call play(). Close when done.")
    editor.wait()
    print("the timeline, as it was left:")
    read_back()


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run()
else:
    print("up — play() to hear it, read_back() to see what survived the edit")
