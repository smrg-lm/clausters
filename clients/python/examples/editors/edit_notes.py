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
import time

from clausters import Session
from clausters.gui import edit
from clausters.seq import Timeline
from clausters.seq.event import Event

# %% [markdown]
# ## A timeline, filled the ordinary way
#
# Beats and events. The `instrument` on the last one is the point of the second
# cell below: the roll cannot draw it, and editing must not lose it.

# %%
timeline = Timeline([
    (0.0, Event(midinote=60, dur=1.0)),
    (1.0, Event(midinote=64, dur=1.0)),
    (2.0, Event(midinote=67, dur=2.0)),
    (4.0, Event(midinote=72, dur=1.0, instrument="default", amp=0.4)),
])

# %% [markdown]
# ## One verb
#
# A `Timeline` opens as a `clausters.gui.editing.NotesEditor` — one `pianoroll`
# widget, the crate's ``events`` vocabulary, and the timeline's own editing
# context.

# %%
session = Session.live()
gui = session.gui()
editor = edit(timeline, sample_rate=session.server.sample_rate,
              tempo=session.clock.tempo, title="notes")
editor.open(gui)


# %% [markdown]
# ## Play what was drawn
#
# The timeline is the one the script holds, so playing it needs nothing from the
# editor: `clausters.seq.Timeline` plays itself on the session's clock.

# %%
def play():
    """Play the timeline as it now stands."""
    session.play(timeline)


# %% [markdown]
# ## What a roll cannot say
#
# Five numbers per note — start, length, pitch, velocity, channel. The
# instrument, the amp and anything else the author wrote are none of them, and
# they are still there after an edit.

# %%
def read_back():
    """Every note as it now stands, with what the roll never drew."""
    for beat, event in timeline:
        extra = {k: v for k, v in dict(event).items()
                 if k not in ("midinote", "dur", "sustain", "velocity", "type")}
        print(f"  {beat:5.2f}  midinote {event.midinote():5.1f}   {extra}")


# %%
def run():
    """Keep the window open until it is closed, then print what was drawn."""
    print("edit the notes; space plays nothing here — call play(). Close when done.")
    while editor.window is not None:
        editor.poll(0.05)
        time.sleep(0.01)
    print("the timeline, as it was left:")
    read_back()


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run()
else:
    print("up — play() to hear it, read_back() to see what survived the edit")
