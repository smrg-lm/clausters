#!/usr/bin/env python3
"""A window's verbs, asked for from a script: ``editor.select_all()``,
``editor.cut()``, ``editor.verb("view_all")``.

Every command a window has -- a menu entry, a key, a tool -- is a **verb**, and
the verbs are an API: the script that opened the window asks for them the way
the hand does, and the host performs them through the same dispatch, on what
the window's focus or its main view holds. So ``editor.cut()`` does exactly
what Ctrl+X does in the window, and what it edits comes back as the edit a
hand made: recorded, drawn, undone with Ctrl+Z.

Each verb has a **named member** -- ``select_all``, ``copy``, ``paste``,
``to_end``, ``quantize``, ``split``, ``join`` -- and all of them are one line
over one door, ``editor.verb(name)``, which reaches any verb of the window,
the ones with no member of their own included.

What to look at: the roll opens with four notes. Run ``again()``: the notes
are copied and pasted after themselves, and the view zooms out to show all of
it (``verb("view_all")``, the R key). Run ``clear()``: every note is cut and
the roll is empty; ``back()`` pastes them at the start again. **Ctrl+Z** in
the window takes each step back, as it would one made by hand.

The verbs arrive through the window's event loop, as a gesture does, so what
a verb edited is in the sequence a moment after the call returns rather than
when it does.

Run it as a script, or step through the cells::

    pip install -e clients/python

    python clients/python/examples/editors/window_verbs.py

It self-launches the audio server and the GUI host, and needs a display.
"""

# %%
import sys

from clausters import Session
from clausters.gui import edit
from clausters.seq import Timeline
from clausters.seq.event import Event

# %% [markdown]
# ## Four notes
#
# An arpeggio, a beat each.

# %%
timeline = Timeline([
    (0.0, Event(midinote=72, dur=0.9)),
    (1.0, Event(midinote=76, dur=0.9)),
    (2.0, Event(midinote=79, dur=0.9)),
    (3.0, Event(midinote=84, dur=1.0)),
])

# %% [markdown]
# ## The editor
#
# The notes, rendered into the sequence the roll edits, in a window.

# %%
session = Session.live().activate()
session.gui()
notes = timeline.render_events()
editor = edit(notes,
              sample_rate=session.server.query_info().nominal_sample_rate,
              title="window verbs")

# %% [markdown]
# ## The window's verbs, by name
#
# `select_all` holds every note, `copy` puts them on the clipboard, `to_end`
# moves the position cursor to where the last note ends and `paste` lands the
# block there: what Ctrl+A, Ctrl+C, End and Ctrl+V do in the window, and the
# paste one entry of its history. The last line asks for the R key through
# the door itself, `verb("view_all")`: what `view_all()` is one line over, and
# what reaches a verb no member names.

# %%
def again():
    """The notes again, after themselves, and all of it in view."""
    editor.select_all()
    editor.copy()
    editor.to_end()
    editor.paste()
    editor.verb("view_all")


# %% [markdown]
# ## Cut, and put back
#
# `cut` takes what is held off the roll and onto the clipboard; `to_start`
# and `paste` put it back at the beginning.

# %%
def clear():
    """Every note, cut."""
    editor.select_all()
    editor.cut()


def back():
    """What was cut, pasted at the start."""
    editor.to_start()
    editor.paste()


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        again()
        editor.wait()
    finally:
        session.close()
else:
    print("again() repeats the notes, clear() cuts them, back() pastes them; "
          "session.close() to end")
