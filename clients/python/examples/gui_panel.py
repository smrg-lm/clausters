#!/usr/bin/env python3
"""A scripted instrument panel: controls that round-trip values and events.

It builds a ``window`` of standard controls -- knobs, sliders, a number, a
toggle, a button and a menu -- opens it as one GuiDef, then both *drives* a
widget live with ``set`` and *listens* for the events your interactions emit
(turn a knob, click the button) and the close the host sends when you close the
window. No audio server is involved, so this boots only the GUI host.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the GUI binary

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/gui_panel.py``. It self-launches the windowed
host with `GuiHost().boot()`; by hand that is ``clausters-gui``. Needs a display
and a GPU adapter.
"""

# %%
import sys
import time

from clausters.gui import GuiHost, button, knob, menu, number, panel, slider, toggle, window

#: The named controls, so the script drives and listens to them by name.
CONTROLS = ("cutoff", "res", "gain", "mix", "bypass", "reset", "wave")

# %% [markdown]
# ## Launch the GUI host
# `GuiHost().boot()` starts a windowed `clausters-gui` process and returns a host
# connected to it (stopped by `stop`, or on interpreter exit).

# %%
gui = GuiHost().boot()

# %% [markdown]
# ## The panel
# A row of knobs over a row of mixed controls. Every widget is *named*, not
# numbered -- the script never picks an id.

# %%
win = gui.open(window(
    panel(knob(name="cutoff", label="cutoff", min=20.0, max=20000.0, value=800.0),
          knob(name="res", label="res", min=0.0, max=1.0, value=0.3),
          number(name="gain", label="gain", min=-24.0, max=24.0, value=0.0),
          layout="row"),
    panel(slider(name="mix", label="mix", min=0.0, max=1.0, value=0.5),
          toggle(name="bypass", label="bypass", value=False),
          button(name="reset", label="reset"),
          menu(name="wave", options=["sine", "saw", "square"], index=1, label="wave"),
          layout="row"),
    title="Filter", w=560, h=300, layout="col"))

# %% [markdown]
# ## Drive and listen
# Nudge the cutoff live (the `set` path), then register a per-widget `on_event`:
# each handle fires with the new value(s) when the host's messages are pumped. No
# ids, no manual matching.

# %%
time.sleep(0.5)
win["cutoff"].set(value=2000.0)
print("set cutoff to 2000; now interact with the window...")

for name in CONTROLS:
    win[name].on_event(lambda *value, name=name: print(f"{name}: {value}"))
_closed = False
win.on_closed(lambda: (print("window closed"), globals().__setitem__("_closed", True)))


def run(seconds: float) -> None:
    """Dispatches panel events for ``seconds``."""
    start = time.monotonic()
    while time.monotonic() - start < seconds and not _closed:
        gui.pump(timeout=0.1)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run(8.0)
    finally:
        gui.stop()
else:
    print("panel up - run(10) to dispatch events, gui.stop() to end")
