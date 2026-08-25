#!/usr/bin/env python3
"""A live node-tree view: the GUI mirroring the audio server's node graph.

A ``nodetree`` widget shows the audio server's tree -- its groups, synths, def
names and control values -- and keeps it current: the GUI host mirrors the tree
over its **client leg** (``/group_queryTree``), refreshing the moment a node is
created or freed (``/node_start``/``/node_end`` notifications) and on a low-rate poll that
catches ``/node_set`` control changes. Nothing in this script pushes the tree to the
GUI; the host reads it from the server itself (so its client leg must point at
the server -- which `Session.gui` wires up).

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server + GUI binaries

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/views/nodetree.py``. It self-launches the audio
server and the GUI host (`Session.live` + `Session.gui`); by hand that is
``clausters`` and ``clausters-gui --server 127.0.0.1:57110``. Run this with no
server already up on 57110, so the session boots its own. Needs a display and a
GPU adapter.
"""

# %%
import math
import sys
import time

from clausters import Session
from clausters.defs import SynthDef, control, out, sine
from clausters.defs.node import AddAction
from clausters.gui import nodetree, view
from clausters.defs import Group, Synth

# %% [markdown]
# ## Launch the server and the GUI, and build a few nodes
# `Session.gui()` points the host's client leg at this session's server, which is
# what lets the `nodetree` view mirror it.

# %%
session = Session.live()
server = session.server
gui = session.gui()

SynthDef(
    "beep", out(0.0, sine(control("freq", 220.0)) * control("amp", 0.1))).send(server)
group = Group(server=server)
sweeper = Synth("beep", {"freq": 220.0}, target=group.id,
                    action=AddAction.TAIL, server=server)
Synth("beep", {"freq": 330.0}, target=group.id, action=AddAction.TAIL, server=server)

# %% [markdown]
# ## The view
# One `nodetree` widget rooted at the root group (0), named so `open` resolves it.

# %%
win = view(
    nodetree(name="tree", group=0, controls=True, label="node tree"),
    title="Live node tree", w=420, h=520).open()
win.on_closed(lambda: globals().__setitem__("_closed", True))
print("the window mirrors the server's node tree; watch freq sweep and "
      "a synth come and go; close the window to stop")

# %% [markdown]
# ## Drive it
# Sweep one synth's `freq` (an `/node_set` per tick, shown by the host's poll) and
# let a third synth appear and disappear (the host catches the `/node_start`/`/node_end`).
# None of this is pushed to the GUI -- it reads the tree from the server.

# %%
_closed = False


def run(seconds: float | None = None) -> None:
    """Sweeps a control and cycles a synth in and out for ``seconds``.

    Script-run there is no bound and the window is what ends it; the
    ``seconds`` argument is for a cell run, where a notebook wants the loop to
    give the prompt back.
    """
    extra = None
    start = time.monotonic()
    while not _closed and (seconds is None or time.monotonic() - start < seconds):
        t = time.monotonic() - start
        sweeper.set({"freq": 330.0 + 220.0 * math.sin(t)})
        if extra is None and int(t) % 4 == 2:
            extra = Synth("beep", {"freq": 550.0, "amp": 0.08},
                                 target=group.id, action=AddAction.TAIL, server=server)
        elif extra is not None and int(t) % 4 == 0:
            extra.free()
            extra = None
        gui.pump(timeout=0.1)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        group.free()
        session.close()
else:
    print("nodetree up - run(10) to drive the tree, session.close() to end")
