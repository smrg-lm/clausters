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

The two windows are two `clausters.gui.FormEditor`s over one arrangement. Nothing
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

from clausters import Session
from clausters.defs import DoneAction, Env, SynthDef, control, env_gen, out, sine
from clausters.form import Aggregate, Sequence
from clausters.gui import FormEditor
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
# composition — `FormEditor` asks the arrangement for it rather than making one.

# %%
left = FormEditor(piece, sample_rate=SR, tempo=BPM / 60.0, quant=0.25, follow=True,
              title="Composition (left)", width=760, height=380)
right = FormEditor(piece, sample_rate=SR, tempo=BPM / 60.0, quant=0.25, follow=True,
               title="Composition (right)", width=760, height=380)

a = left.open(gui)
b = right.open(gui)


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
# The read-out below prints what each window says about the history — both
# directions — after every event, so the agreement is legible without pressing
# anything twice. Watch `can_redo`: it turns true in *both* windows on an undo
# in either, and at the end of the history both sides stay false and nothing
# redraws.

# %%
_shown = None


def show_history():
    """Print what each window says about the history; asks again in 50 ms.

    **Both directions, both windows.** The redo side is what makes "one order,
    not two" legible: undo in either window and `can_redo` turns true in *both*,
    because there is one pile and both are standing at the same place in it. It
    is also what says a step did nothing -- at the end of the history both stay
    False, and no window redraws.

    A history label is not an event, so it is *read* rather than waited for:
    that is a periodic read-out, and it belongs on the application clock
    (`clausters.base.appclock.AppClock`), which reschedules a function by the
    number it returns.
    """
    global _shown
    state = tuple((editor.can_undo, editor.undo_label,
                   editor.can_redo, editor.redo_label)
                  for editor in (left, right))
    if state != _shown:
        for name, (can_undo, undo, can_redo, redo) in zip(("left ", "right"), state):
            print(f"{name}: can_undo={can_undo!s:5} {undo!r:24}"
                  f" can_redo={can_redo!s:5} {redo!r}")
        print("-" * 72)
        _shown = state
    return 0.05


def run(seconds=None):
    """Hold both windows open, with the read-out running.

    Each editor is subscribed to the host's event loop by its own ``open``, so
    every message reaches **both** -- every route resolves through an editor's
    own registries, and the other window's events fall through untouched. That
    used to be a shared poll loop written here; it is now the one loop the host
    already has.

    Script-run there is no bound and the windows are what end it; the
    ``seconds`` argument is for a cell run, where a notebook wants the wait to
    give the prompt back.
    """
    gui.clock.sched(0.05, show_history)
    gui.wait(seconds)


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
