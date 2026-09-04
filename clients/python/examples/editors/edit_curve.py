#!/usr/bin/env python3
"""``edit(curve)``: a break-point curve in a window of its own.

The smallest of the three structures `clausters.gui.edit` opens, and the one
that shows what the verb is for. There is **no composition here** — no
arrangement, no document, no track. A script builds a
`clausters.seq.Automation`, hands it to ``edit``, and reads the edited curve
back out of the object it already holds.

What to do in the window:

- **drag a point** to move it (times stay monotonic between its neighbours);
- **drag a segment** vertically to bend its curvature;
- **Ctrl+click** on empty curve area adds a point, on a point removes it;
- **Ctrl+Z** / **Ctrl+Shift+Z** undo and redo — the history belongs to the
  curve, not to the window, which is what the second cell shows.

**Nothing here drives a loop.** ``edit`` opens the window and the host's event
loop delivers each gesture to the curve on its own thread, so reading the
`clausters.seq.Automation` at any moment reads what the hand has left there.

**How an edit inverts is the shared crate's.** The payload goes in with the
curve as it stands and comes back as the curve it now is *plus* the payload that
puts it back — one call, because the inverse has to be read before the edit
lands. Nothing in this client computes it, and nothing in the web client does
either.

Run it as a script, or step through the cells. Install once, from the repo
root::

    pip install -e clients/python

    python clients/python/examples/editors/edit_curve.py

It self-launches the GUI host. Needs a display and a GPU adapter; no audio
server, because a curve is data and this example never plays it.
"""

# %%
import sys
import time

from clausters import Session
from clausters.gui import edit

# %% [markdown]
# ## A curve, built the ordinary way
#
# Four break points in seconds, with the shape of each segment on the point that
# starts it. Nothing about this object knows it is going to be edited.

# %%
from clausters.seq.automation import Automation

curve = Automation.from_points(
    [(0.0, 200.0, 1, 0.0),      # linear up
     (0.5, 4000.0, 2, 0.0),     # exponential down
     (2.0, 800.0, 1, 0.0),
     (3.0, 200.0, 1, 0.0)],
    target=None, name="cutoff")

# %% [markdown]
# ## One verb
#
# `clausters.gui.edit` dispatches on **what the structure is**: an `Automation`
# opens as a `clausters.gui.editing.PointsEditor` — one `bpf` widget, the
# ``points`` vocabulary, and the curve's own editing context.

# %%
session = Session.live(boot=False)
editor = edit(curve, sample_rate=48_000.0, title="cutoff")


# %% [markdown]
# ## Two windows, one stack
#
# Calling `edit` again over the **same** curve gives a second window. It is not a
# copy and it does not get a history of its own: an undo stack belongs to the
# data, so `Ctrl+Z` in either window steps the one order both of them made.

# %%
def second_window():
    """Open a second view of the same curve. Edit in one, undo in the other."""
    return edit(curve, sample_rate=48_000.0, title="cutoff (again)")


# %% [markdown]
# ## Read it back
#
# Nothing is handed back at the end: the `Automation` passed in *is* the edited
# one, so reading `clausters.seq.Automation.to_points` is how a script sees what
# was drawn.

# %%
def read_back() -> list:
    """The curve as it now stands, as ``[t, value, shape, curve, ...]``."""
    points = curve.to_points()
    for t, value, shape, bend in (points[i:i + 4] for i in range(0, len(points), 4)):
        print(f"  {t:6.3f}s  {value:9.2f}   shape {int(shape)}  curve {bend:+.2f}")
    return points


# %%
def run():
    """Keep the window open until it is closed, then print what was drawn."""
    print("draw in the window; Ctrl+Z undoes. Close it when you are done.")
    while not editor.closed:
        time.sleep(0.05)
    print("the curve, as it was left:")
    read_back()


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run()
else:
    print("up — read_back() for the points, second_window() for a second view")
