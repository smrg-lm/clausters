#!/usr/bin/env python3
"""A scripted instrument panel: controls that round-trip values and events.

It builds a ``window`` of standard controls -- knobs, sliders, a number, a
toggle, a button and a menu, two of them non-linear (a curved knob and a
stepped number) -- opens it **twice**, then both *drives* a widget
live with ``set`` and *listens* for the events your interactions emit (turn a
knob, click the button) and the close the host sends when you close a window.
No audio server is involved, so this boots only the GUI host.

The two windows are the point of the second half: a view is a definition, so
opening it again gives a second instance with widget ids of its own. The
script holds one handle per window and addresses every control by name through
it -- the same names, two independent panels.

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

from clausters.gui import GuiHost, button, knob, menu, number, panel, slider, toggle, view

#: The named controls, so the script drives and listens to them by name.
CONTROLS = ("cutoff", "res", "gain", "mix", "bypass", "reset", "wave")

# %% [markdown]
# ## Launch the GUI host
# `GuiHost().boot()` starts a windowed `clausters-gui` process and returns a host
# connected to it (stopped by `stop`, or on interpreter exit).

# %%
gui = GuiHost().boot()

# %% [markdown]
# ## The panel, as a view
# A row of knobs over a row of mixed controls. Every widget is *named*, not
# numbered -- the script never picks an id. The builders return a `View`: a tree
# you compose and then open, the way a `SynthDef` is a graph you compose and
# then send.

# %%
v = view(
    panel(knob(name="cutoff", label="cutoff", min=20.0, max=20000.0, value=800.0,
               curve=4.0),
          knob(name="res", label="res", min=0.0, max=1.0, value=0.3),
          number(name="gain", label="gain", min=-24.0, max=24.0, value=0.0,
                 step=1.0),
          layout="row"),
    panel(slider(name="mix", label="mix", min=0.0, max=1.0, value=0.5),
          toggle(name="bypass", label="bypass", value=False),
          button(name="reset", label="reset"),
          menu(name="wave", options=["sine", "saw", "square"], index=1, label="wave"),
          layout="row"),
    title="Filter", w=560, h=300, layout="col")

# %% [markdown]
# ## How the travel becomes the value
# Two of those are not linear. The cutoff knob carries `curve=4.0`, so half its
# travel is spent below 2.5 kHz instead of leaving 20..2000 Hz in a hairline --
# the bend `lincurve` runs, read by the host out of the shared core. The gain
# number carries `step=1.0`, so a drag lands on whole decibels and prints
# `-3.0`, never `-3.0417`. Turn the `res` knob beside them to feel the
# difference: it is linear and continuous, which is the default.
#
# The step is a rule about the hand: the `set` below sends 2000.0 to a curved
# knob and it is drawn at 2000.0, because a control shows what it was told.

# %% [markdown]
# ## Two windows from one view
# `open()` is what makes an *instance*: it allocates the widget ids and sends the
# document, leaving `v` as it was written. So the same view opens twice and the
# two panels share nothing but their names.

# %%
a = v.open()
b = v.open()
b.set(title="Filter (2)")
print(f"two windows from one view: cutoff is {a['cutoff'].id} in one "
      f"and {b['cutoff'].id} in the other")

# %% [markdown]
# ## Drive and listen
# Nudge the first panel's cutoff live (the `set` path), then register a
# per-widget `on_event` on both: each handle fires with the new value(s) when the
# host's messages are pumped. No ids, no manual matching -- and turning the knob
# in one window leaves the other where it was.

# %%
time.sleep(0.5)
a["cutoff"].set(value=2000.0)
print("set the first panel's cutoff to 2000; now interact with the windows...")

for tag, win in (("1", a), ("2", b)):
    for name in CONTROLS:
        win[name].on_event(
            lambda *value, tag=tag, name=name: print(f"{tag} {name}: {value}"))

_open = {int(a), int(b)}


def _closed_one(win):
    """Stop only once both windows are gone."""
    _open.discard(int(win))
    print(f"window closed ({len(_open)} left)")
    globals()["_closed"] = not _open


_closed = False
a.on_closed(lambda: _closed_one(a))
b.on_closed(lambda: _closed_one(b))


def run(seconds: float | None = None) -> None:
    """Dispatches panel events for ``seconds``.

    Script-run there is no bound and the windows are what end it; the
    ``seconds`` argument is for a cell run, where a notebook wants the loop to
    give the prompt back.
    """
    start = time.monotonic()
    while not _closed and (seconds is None or time.monotonic() - start < seconds):
        gui.pump(timeout=0.1)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        gui.stop()
else:
    print("panels up - run(10) to dispatch events, gui.stop() to end")
