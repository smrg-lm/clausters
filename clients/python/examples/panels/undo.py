#!/usr/bin/env python3
"""A panel that keeps its own undo: what the hand did to a synth's controls.

A panel of a synth's controls has no document behind it, and a control
**bound** to the synth sends its value straight to the server with this script
out of the loop (``bind.py``), so the script cannot keep what the hand did. The
window can: ``view(..., history=True)`` gives it an undo order of its own, kept
by the GUI host, and Ctrl+Z / Ctrl+Shift+Z walk it there.

Two windows, one synth each, show what the order holds and what it does not:

- **A gesture is one entry.** Drag the ``freq`` knob around and let go: one
  Ctrl+Z takes the whole drag back. A run of wheel notches or arrow keys on one
  control is one entry too, and so is what was typed in a field while it held
  the keys.
- **The target is the parameter.** The ``freq`` field is bound to the same synth
  control as the knob, so the two are one target: undo a drag of the knob and
  the field shows it too.
- **Undo writes the value it had.** Click ``nudge from the script``: the script
  sets the synth's ``freq`` itself, the controls follow it on their read-back,
  and nothing is recorded. Ctrl+Z then writes the value the last entry stored,
  whatever the synth holds now.
- **The clipboard is the host's one.** Click the knob in one window, Ctrl+C,
  click the knob in the other, Ctrl+V: the value travels as text (paste it in a
  text editor too), and the paste is one entry of the window it landed in.
  Anything else on the clipboard (a range of samples an editor copied) is
  refused, and the status line says what it holds.
- **Two windows are two orders**, and Ctrl+Z in one never reaches the other.
- **A target that is gone leaves the order.** Click ``free the synth`` in the
  second window: its node ends, and Ctrl+Z there says there is nothing left to
  undo. The status line says each step.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server + GUI binaries

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/panels/undo.py``. It self-launches the audio
server and the GUI host (`Session.live` + `Session.gui`). Run this with no
server already up on 57110, so the session boots its own. Needs a display and a
GPU adapter.
"""

# %%
import sys

from clausters import Session
from clausters.defs import Synth, SynthDef, control, out, sine
from clausters.gui import button, knob, number, slider, view

# %% [markdown]
# ## Launch the server and the GUI, and two synths to drive
# The same def twice, a fifth apart, so the two windows are two instruments.

# %%
session = Session.live()
server = session.server
gui = session.gui()

FREQ = control("freq", 440.0)
AMP = control("amp", 0.1)


def tone(name: str = "undo_tone") -> SynthDef:
    """A quiet stereo sine over the `freq` and `amp` controls."""
    sig = sine(freq=FREQ) * AMP
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


tone().send(server)
first = Synth("undo_tone", {"freq": 440.0}, server=server)
second = Synth("undo_tone", {"freq": 660.0}, server=server)

# %% [markdown]
# ## Two panels that keep their own undo
# `history=True` is the whole of it: the host records what a hand does to the
# window's controls and walks it on Ctrl+Z. The knob and the slider are built
# from the def's controls and bound in one verb, `win.bind(synth)`; the field
# is bound by hand to the same `freq`, which makes it the knob's target too.
# The button is not bound: its click comes here, and a command is never an
# entry.


def panel(title: str, freq: float, act: str):
    """One synth's panel: its pitch twice, its level and one button."""
    return view(knob(FREQ, min=220.0, max=1760.0, warp="exp", value=freq),
                number(name="typed", label="freq", min=220.0, max=1760.0,
                       step=1.0, value=freq),
                slider(AMP, min=0.0, max=0.3),
                button(name="act", label=act),
                title=title, history=True, flow="col")


a = panel("first", 440.0, "nudge from the script").open()
a.bind(first)
a["typed"].bind("/node_set", first.id, "freq")
# Another writer: the script sets the parameter itself. The controls follow
# on their read-back and the order records nothing.
a["act"].on_click(lambda: first.set({"freq": 880.0}))

b = panel("second", 660.0, "free the synth").open()
b.bind(second)
b["typed"].bind("/node_set", second.id, "freq")
# The target goes: the node ends, and its entries leave the second order.
b["act"].on_click(lambda: second.free())

print("drag, turn, type and paste in either window; Ctrl+Z and Ctrl+Shift+Z "
      "walk that window's own order, and its status line says each step.")

# %% [markdown]
# ## Play with it
# Nothing to drive from here: the bindings send every value to the server and
# the host keeps the order, so the script only stays alive until both windows
# are closed.

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        gui.wait()
    finally:
        session.close()   # the server it booted goes, and the synths with it
else:
    print("undo up - gui.wait(10) to hold it, session.close() to end")
