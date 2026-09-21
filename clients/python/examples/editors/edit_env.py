#!/usr/bin/env python3
"""``edit(env)``: draw an envelope and hear the server play it.

An `clausters.defs.Env` is a **curve**, so ``edit`` opens it -- one ``bpf``
widget, the ``points`` vocabulary, and the editing history that comes with the
verb. The envelope's segments carry the server's own shape numbers and the host
evaluates them through the same shared math (``clausters-core``) the server's
``EnvGen`` plays, so what is drawn is exactly what is heard.

What to do in the window:

- **drag a point** to move it (times stay monotonic between its neighbours);
- **drag a segment** vertically to bend its curvature (it becomes the custom
  curve shape, like an `Env`'s numeric curvature);
- **Ctrl+click** on empty curve area adds a point, on a point removes it;
- **Ctrl+Z** / **Ctrl+Shift+Z** undo and redo -- the history belongs to the
  envelope, not to the window;
- the **curve menu** applies one shape to every segment at once, which is a
  write to the `clausters.defs.Env` and not a gesture.

**Every gesture sounds.** ``editor.on_change`` is called once per gesture that
changed the data, and this file answers it by sending a fresh def built from the
envelope and spawning a note. Nothing here keeps a break-point list of its own:
the `clausters.defs.Env` passed to ``edit`` *is* the edited one, and reading it
after a gesture reads what the hand left there.

The sibling `edit_curve.py` opens the same widget over a `clausters.defs.Bpf` --
the same envelope in absolute coordinates -- and never plays it. This one is the
other half: the same curve as the thing `EnvGen` reads.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server + GUI binaries

Run it cell by cell (Shift+Enter) and keep drawing between cells, or as a plain
script -- ``python clients/python/examples/editors/edit_env.py``. It
self-launches the audio server and the GUI host (`clausters.Session.live` +
`clausters.Session.gui`). Needs a display and a GPU adapter, plus an audio
device.
"""

# %%
import sys

from clausters import Session, Synth
from clausters.defs import DoneAction, Env, SynthDef, env_gen, out, sine
from clausters.gui import button, edit, label, menu

# %% [markdown]
# ## Launch the server and the GUI
# `clausters.Session.live` connects to a running audio server or starts one;
# ``session.gui()`` starts ``clausters-gui`` wired to it. Both are owned by the
# session and torn down with it.

# %%
session = Session.live()
server = session.server
session.gui()

# %% [markdown]
# ## The envelope
# A familiar ADSR-shaped `clausters.defs.Env`: levels, segment times in seconds,
# and a shape per segment (a name, or a numeric curvature). Nothing about this
# object knows it is going to be edited.

# %%
env = Env([0.0, 1.0, 0.4, 0.0], [0.05, 0.3, 1.2], ["exp", -4.0, "sin"])

#: The curve specs the menu offers -- shape names plus two custom curvatures,
#: exactly the values an `clausters.defs.Env`'s ``curve`` takes. "hold" is the
#: constant lane (each point's value held until the next); ``Env.step`` builds
#: SuperCollider's "step" sequences separately.
CURVES = ["lin", "exp", "sin", "welch", "sqr", "cub", "hold", -4.0, 4.0]


# %% [markdown]
# ## One note, shaped by the envelope as it now stands
# The envelope is read at the moment the note is built, so this always plays
# what is on screen.

# %%
def play():
    """One note through the envelope as currently drawn."""
    sig = sine(330.0) * env_gen(env, done_action=DoneAction.FREE_SELF) * 0.4
    SynthDef("edit_env_voice", out(0.0, sig), out(1.0, sig)).send(server)
    Synth("edit_env_voice", server=server)
    print(f"played {len(env.times)} segments over {env.times} s")


def set_curve(spec):
    """One curve spec applied to **every** segment.

    A write to the envelope rather than a gesture on the curve: the shapes are
    the `clausters.defs.Env`'s own field, so this sets them and asks the window
    to come in step. Nothing converts, because there is no second copy of the
    curve to keep aligned.
    """
    env.curves = [spec] * len(env.times)
    editor.adopt()
    print(f"curve -> {spec}")


# %% [markdown]
# ## One verb, with two widgets of the script's own
# `clausters.gui.edit` dispatches on **what the structure holds**: an `Env` can
# give and take break points, so it opens as a
# `clausters.gui.editing.PointsEditor`. ``extra`` appends widgets after the
# picture -- they are the script's, and the editor never touches their ids --
# and `clausters.gui.editing.Editor.window` resolves them by name.
#
# `clausters.gui.editing.Editor.on_change` is called once per gesture that
# changed the envelope -- once for a whole drag, because that is what a hand
# did -- so drawing plays.

# %%
editor = edit(
    env, sample_rate=48_000.0, title="amp env -> EnvGen",
    extra=[menu(name="curve", options=[str(c) for c in CURVES],
                label="curve (all segments)"),
           button(name="play", label="play again"),
           label(name="hint", text="drag points/segments; Ctrl+click adds/removes")],
)
editor.on_change = play
editor.window["play"].on_click(lambda *_: play())
editor.window["curve"].on_event(lambda index: set_curve(CURVES[int(index)]))
print(f"opened window {editor.id} -- draw, and every gesture sounds")

# %% [markdown]
# ## Hear the seed, then rewrite it from the script
# The envelope is the script's object, so a script may write it too:
# `clausters.defs.Env.set_points` takes the break points of another envelope,
# and `clausters.gui.editing.Editor.adopt` brings the window in step.

# %%
play()
env.set_points(Env.perc(0.01, 1.2).to_points())
editor.adopt()
print("the editor now holds a percussive envelope instead")

# %% [markdown]
# ## Drive it
# Cell-run: keep drawing, and call `play()` between cells when you want the same
# envelope again. Script-run: hold the window open until you close it, then tear
# everything down.

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        editor.wait()
    finally:
        session.close()
else:
    print("up -- draw to hear it; play() replays; session.close() to end")
