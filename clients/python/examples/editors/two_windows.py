#!/usr/bin/env python3
"""Two windows over one composition, and **one** history between them.

An undo stack belongs to the data, not to the view. Open the same piece in two
windows, drag a clip in one, and press **Ctrl+Z** over the *other*: the clip
springs back in both. That is the whole of what this example is for, and it is
worth a file of its own because the arrangement it protects against is the one a
person reaches for without thinking — a multitrack up here, a closer look at one
lane over there.

**What would happen without it.** A history an editor kept would see only the
gestures *that* editor made. So window A's undo would revert across window B's
edit, and window B's undo would then put back a value A had already replaced —
a state nobody was ever in, reached by two buttons that both look right. The
history lives with the arrangement instead, so both windows find the same one
and step one order.

Two consequences you can watch here:

- the **label** agrees. `undo_label` reads the same in both windows, because it
  names the entry the next keystroke is about to move and there is one pile;
- the **selection does not travel**. Sweep in one window and the other keeps its
  own: what a window can see is a window's, and none of it is ever an entry in a
  history. That is the same line drawn from the other side.

The two windows are two `clausters.gui.Editor`s over one arrangement. Nothing
wires them together — the sharing is not a feature of this script, it is where
the history lives.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server + GUI binaries

Run it cell by cell (Shift+Enter) or as a plain script --
``python clients/python/examples/editors/two_windows.py``. Needs a display and a
GPU adapter, plus an audio device.
"""

# %%
import sys
import time

from clausters import Session
from clausters.defs import DoneAction, Env, SynthDef, control, env_gen, out, sine
from clausters.form import Aggregate, Sequence
from clausters.gui import Editor
from clausters.seq import Pbind, Pseq

# %% [markdown]
# ## The session, the host and one instrument
# `Session.live` connects to a running audio server or starts one; `session.gui()`
# starts ``clausters-gui`` wired to it. The clock runs at the piece's tempo, so a
# clip lasts as long as it looks.

# %%
BPM = 110.0
SR = 48_000.0
session = Session.live(tempo=BPM / 60.0).activate()
gui = session.gui()

bell = SynthDef("bell", out(0, sine(control("freq", 440.0))
                            * env_gen(Env.perc(attack=0.005, release=0.5),
                                      done_action=DoneAction.FREE_SELF)
                            * control("amp", 0.2)))
bell.send()
session.server.sync()

# %% [markdown]
# ## The composition
# Two lanes of patterned notes — the material is beside the point here, so it is
# the smallest thing that draws as clips a hand can drag. Every fundamental is
# well clear of the bass register.

# %%
def lane(name, pitches, dur):
    """One lane: a pattern, placed at the top of the piece."""
    seq = Sequence(Pbind(instrument="bell", freq=Pseq(pitches, 2), dur=dur),
                   name=name)
    return Aggregate([(0.0, seq)], name=name)


piece = Aggregate([
    (0.0, lane("lead", [660.0, 880.0, 770.0, 990.0], 0.5)),
    (0.0, lane("counter", [440.0, 550.0], 1.0)),
], name="two windows")

# %% [markdown]
# ## Two windows over it
# Two editors, one arrangement. Each opens its own window; neither is told about
# the other. They share a history because the history belongs to the
# composition — `Editor` asks the arrangement for it rather than making one.

# %%
left = Editor(piece, sample_rate=SR, tempo=BPM / 60.0, quant=0.25, follow=True,
              title="Composition (left)", width=760, height=380)
right = Editor(piece, sample_rate=SR, tempo=BPM / 60.0, quant=0.25, follow=True,
               title="Composition (right)", width=760, height=380)

a = left.open(gui)
b = right.open(gui)

_closed = False
a.on_closed(lambda: globals().__setitem__("_closed", True))
b.on_closed(lambda: globals().__setitem__("_closed", True))

# %% [markdown]
# ## What to do, and what to watch
# 1. **Drag a clip in the left window.** It snaps to the quarter-beat grid — the
#    crate decides that, not the editor.
# 2. **Press Ctrl+Z over the right window.** The clip springs back *in both*.
#    Nothing in this script forwarded anything: the right window undid an edit it
#    never made, because it is showing the data that moved.
# 3. **Ctrl+Shift+Z over either** redoes it, once — one order, not two.
# 4. **Sweep a selection in one window.** The other keeps its own.
#
# The read-out below prints what each window says about the history after every
# event, so the agreement is legible without pressing anything twice.

# %%
def run(seconds=None):
    """Pump both windows until one is closed.

    Script-run there is no bound and the windows are what end it; the
    ``seconds`` argument is for a cell run, where a notebook wants the loop to
    give the prompt back.
    """
    start = time.monotonic()
    shown = None
    while not _closed and (seconds is None or time.monotonic() - start < seconds):
        gui.pump(timeout=0.05)
        state = (left.can_undo, left.undo_label, right.can_undo, right.undo_label)
        if state != shown:
            print(f"left: can_undo={state[0]} label={state[1]!r} | "
                  f"right: can_undo={state[2]} label={state[3]!r}")
            shown = state


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        session.close()
    sys.exit(0)
else:
    print("two windows up - drag in one, Ctrl+Z over the other; "
          "run(10) to drive it, session.close() to end")
