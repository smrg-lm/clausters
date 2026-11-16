#!/usr/bin/env python3
"""One window for every window: a GUI host booted with ``dock=True``.

A GUI host opens a window of the platform for each window a script opens. Booted
with ``dock=True`` it opens **one**, and every window is a **pane** of it, side
by side: the panes share its width, and a pane opened later makes room for
itself. A pane is a window in everything but its frame -- it keeps its def, its
events and its picture -- so the script below is the one it would be without the
dock, and a press or a key reaches the pane it lands in.

Every pane is a panel with a bar: a grip, its title, a menu and a close mark.
Drag a panel by its grip onto another -- into its middle to put the two in one
panel, which then shows a tab for each, near one of its edges to stand beside
it -- and drag the line between two panels to give one more room. The panel's
menu splits, moves and closes its window. The same moves are verbs a script
calls, and the arrangement they leave is data it can read and put back.

Along the dock's bottom is a strip of **pages**, each an arrangement of its own: a
window can name the page it opens on, the strip switches them, and a tab
dropped on a page's tab moves its pane there. Switching a page frees nothing.

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
# ## Tabs and splits
# The moves a hand makes on the tabs, by name: ``note`` among ``tone``'s tabs
# (and shown), then ``level`` under ``tone``.

# %%
host.dock_call({"verb": "join", "pane": note, "of": tone})
host.dock_call({"verb": "split", "pane": level, "side": "bottom", "of": tone})

# %% [markdown]
# ## The arrangement is data
# What ``dock_layout`` answers, ``dock_restore`` puts back: a press on
# ``tone``'s tab shows it, and the layout read before shows ``note`` again.

# %%
layout = host.dock_layout()
print(layout)
host.dock_call({"verb": "select", "pane": tone})
host.dock_restore(layout)

# %% [markdown]
# ## Pages
# A window naming its page opens on it, made and shown: ``mix`` on a page of
# its own. ``tone`` is put on it too, beside ``mix``, and the first page is
# shown again as it was left -- ``mix`` waits on its page, open.

# %%
mix = view(knob(name="pan", value=0.5), title="mix", w=360, h=240,
           dock_page="Mix").open(host=host)
host.dock_call({"verb": "place", "pane": tone})
host.dock_call({"verb": "page", "name": "Main"})

# %% [markdown]
# ## Closing a pane gives its room back
# ``tone`` is shown again in the group ``note`` leaves.

# %%
note.close()

# %% [markdown]
# ## Keep it open
# Close the dock's window (or every pane) to end the run.

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        host.wait()
    finally:
        host.stop()
else:
    print("the dock is up - host.wait(10) to hold it, host.stop() to end")
