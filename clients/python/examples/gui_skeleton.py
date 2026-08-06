#!/usr/bin/env python3
"""Drive a headless ``clausters-gui`` host: build a GuiDef, read a widget back.

The smallest round trip over the widget protocol, the GUI counterpart of
``live_udp.py``. A GuiDef is built exactly the way a ``SynthDef``/``GraphDef`` is
-- a tree of ``{id, type, ...props, children}`` nodes serialized to JSON -- and
sent in one ``/gui_def`` message; the host registers the tree and answers
``/gui_query`` with ``/gui_info``. This exercises the protocol and the dual-role
host with **no display** (see ``gui_window.py`` for the windowed version).

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the GUI binary

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/gui_skeleton.py``. It self-launches the host
**headless** (`GuiHost().boot(extra_args=("--headless",))`); by hand that is
``clausters-gui --headless``. No display or GPU needed.
"""

# %%
import sys

from clausters.gui import GuiHost, knob, slider, waveform, window

# %% [markdown]
# ## Launch a headless host
# `GuiHost.boot` starts a `clausters-gui` process; `--headless` runs it with no
# window, so this exercises the pure protocol path.

# %%
gui = GuiHost().boot(extra_args=("--headless",))

# %% [markdown]
# ## Build a small panel and open it
# Two controls and a waveform view. The root `window` carries no id (the id comes
# from the `/gui_def` argument), and the children are *named*, not numbered --
# `open` assigns each a fresh id and hands back a handle that resolves the names.

# %%
win = gui.open(window(
    knob(name="cutoff", label="cutoff", min=20.0, max=20000.0, value=800.0),
    slider(name="res", label="res", min=0.0, max=1.0, value=0.2),
    waveform(name="wave", buffer=0),
    title="Filter", w=480, h=240, layout="col"))

# %% [markdown]
# ## Read a widget back
# `/gui_query` -> `/gui_info`, through the handle. The float `value` comes back as
# a float and the int `buffer` as an int -- the wire keeps them apart.

# %%
info = win["cutoff"].query()
if info is None:
    sys.exit("no /gui_info reply -- did the headless host boot?")
kind, props = info
print(f'the "cutoff" widget (id {win["cutoff"].id}) is a {kind!r} with {props}')

root = gui.query(win)
print(f"root (def {int(win)}) is a {root[0]!r}")

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    gui.stop()
else:
    print("skeleton up - gui.stop() to end")
