#!/usr/bin/env python3
"""One window for every window: a GUI host booted with ``dock=True``.

A GUI host opens a window of the platform for each window a script opens. Booted
with ``dock=True`` it opens **one**, and every window is a **pane** of it, side
by side: the panes share its width, and a pane opened later makes room for
itself. A pane is a window in everything but its frame -- it keeps its def, its
events and its picture -- so the script below is the one it would be without the
dock, and a press or a key reaches the pane it lands in.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the GUI binary

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/panels/dock.py``. No audio server is
involved, so this boots only the GUI host. Needs a display and a GPU adapter.
"""

# %%
import sys

from clausters.gui import GuiHost, knob, label, slider, view

# %% [markdown]
# ## A host with a dock
# One window of the host's own, which every window opened on it joins.

# %%
host = GuiHost().boot(dock=True)

# %% [markdown]
# ## Three windows, three panes
# Each is opened as it would be without the dock; the panes share the width.

# %%
tone = view(slider(name="freq", value=0.4), title="tone", w=360, h=240).open(host=host)
level = view(knob(name="level", value=0.7), title="level", w=360, h=240).open(host=host)
note = view(label("a third pane makes room for itself", name="note"),
            title="note", w=360, h=240).open(host=host)

# %% [markdown]
# ## Each pane answers its own hand
# Drag the slider or the knob: the event names the widget it came from.

# %%
tone.widget("freq").on_event(lambda *payload: print("freq", *payload))
level.widget("level").on_event(lambda *payload: print("level", *payload))

# %% [markdown]
# ## Closing a pane gives its room back
# The two left share the width again.

# %%
note.close()

# %% [markdown]
# ## Keep it open
# Close the dock's window to end the run.

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        tone.wait()
    finally:
        host.stop()
else:
    print("two panes up - tone.wait(10) to hold them, host.stop() to end")
