#!/usr/bin/env python3
"""Draw an envelope in the ``bpf`` editor and hear the server play it.

The first widget that **writes data back**: a drawable break-point function
whose segments use the server's own envelope shape numbers, evaluated in the
host through the same shared math (``clausters-core``) the server's ``EnvGen``
plays -- what the editor draws is exactly what you hear.

Editing gestures, all live:

- **drag a point** to move it (times stay monotonic between its neighbors);
- **drag a segment** vertically to bend its curvature (it becomes the custom
  curve shape, like ``Env``'s numeric curvature);
- **Ctrl+click** on empty curve area adds a point there; **Ctrl+click on a
  point** removes it;
- the **curve menu** applies a standard transition shape (or a numeric
  curvature) to every segment at once -- the script rewrites the shapes through
  the `Env` round trip and pushes the list back with ``set``, so the same
  points redraw under the chosen curve.

Every edit flows back per the **edit-back pattern**: the host emits
``"points" <t v shape curve ...>`` -- the breakpoint list as flat OSC
primitives, shapes as ints, everything else floats. This script maps that list
to a `clausters.defs.Env` (`points_to_env`) and, on the **play** button, sends a
fresh SynthDef built from it and spawns a note: the drawn envelope shapes the
tone you hear. (The reverse mapping, `env_to_points`, seeds the editor with a
familiar ADSR to start from.)

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server + GUI binaries

Run it cell by cell (Shift+Enter) and keep drawing/playing from the live
handles, or as a plain script -- ``python clients/python/examples/gui_bpf.py``.
It self-launches the audio server and the GUI host (`Session.live` +
`Session.gui`). Needs a display and a GPU adapter, plus an audio device.
"""

# %%
import json
import sys
import time

from clausters import Session
from clausters.defs import DoneAction, Env, SynthDef, env_gen, out, sine
from clausters.gui import bpf, button, env_to_points, label, menu, points_to_env, window

# %% [markdown]
# ## Launch the server and the GUI
# `Session.live` connects to a running audio server or starts one; `session.gui()`
# starts ``clausters-gui`` wired to it. Both are owned by the session and torn
# down with it.

# %%
session = Session.live()
server = session.server
gui = session.gui()

# %% [markdown]
# ## Open the editor window
# Seeded from a familiar `Env` (an ADSR played through) via `env_to_points`.
# Every widget is *named*, not numbered -- the script drives them by name.

# %%
START_ENV = Env([0.0, 1.0, 0.4, 0.0], [0.05, 0.3, 1.2], ["exp", -4.0, "sin"])

# `Env`-style curve specs the menu offers: shape names plus two custom
# curvatures. "hold" is the constant lane (each point's value held until the
# next). `Env.step` builds SC's "step" sequences separately.
CURVES = ["lin", "exp", "sin", "welch", "sqr", "cub", "hold", -4.0, 4.0]

win = gui.open(window(
    label(name="hint", text="drag points/segments; Ctrl+click adds/removes; play sends it"),
    bpf(name="env", points=env_to_points(START_ENV), min=0.0, max=1.0,
        duration=2.0, label="amp env"),
    menu(name="curve", options=[str(c) for c in CURVES], label="curve (all segments)"),
    button(name="play", label="play"),
    title="BPF envelope -> EnvGen", w=640, h=460, layout="col"))
print(f"opened window {win} -- draw the envelope, then press play")

# %% [markdown]
# ## The handlers, wired by name
# The `env` view reports its breakpoints as they are edited; the `curve` menu
# reshapes every segment; the `play` button spawns a note through the drawn
# envelope. Each is a handle callback -- no ids, no manual event matching.

# %%
_points = env_to_points(START_ENV)
_closed = False


def play(*_):
    """One note shaped by the envelope as currently drawn."""
    env = points_to_env(_points)
    sig = sine(330.0) * env_gen(env, done_action=DoneAction.FREE_SELF) * 0.4
    server.add_synthdef(SynthDef("gui_bpf_env", out(0.0, sig), out(1.0, sig)))
    server.synth("gui_bpf_env")
    print(f"played {len(_points) // 4} breakpoints over {env.times} s segments")


def set_curve(spec):
    """Applies one `Env`-style curve spec to every segment (through the public
    `points_to_env` / `env_to_points` round trip) and pushes it back, so the same
    points redraw under the new shapes."""
    global _points
    env = points_to_env(_points)
    _points = env_to_points(Env(env.levels, env.times, spec))
    win["env"].set(points=json.dumps(_points))
    print(f"curve -> {spec}")


def on_points(_tag, *values):
    """The edit-back payload (`"points"` then t v shape curve quads): keep the
    latest breakpoints so `play` hears what is drawn."""
    global _points
    _points = list(values)


win["env"].on_event(on_points)
win["curve"].on_event(lambda index: set_curve(CURVES[int(index)]))
win["play"].on_event(lambda value: play() if value == 1 else None)  # 1 = press
win.on_closed(lambda: globals().__setitem__("_closed", True))

# %% [markdown]
# ## Hear it now, and set it from the script
# Play the seed once, then set a percussive two-segment envelope live -- a
# `/gui_set` value is a scalar, so the array rides as its JSON string.

# %%
play()
win["env"].set(points=json.dumps(env_to_points(Env.perc(0.01, 1.2))))
_points = env_to_points(Env.perc(0.01, 1.2))

# %% [markdown]
# ## Drive it
# Cell-run: keep drawing and call `play()` between cells. Script-run: pump events
# for a while -- editing is silent, the **play** button sends the note -- then
# tear everything down.

# %%
def run(seconds: float) -> None:
    """Dispatches editor events for ``seconds``."""
    start = time.monotonic()
    while time.monotonic() - start < seconds and not _closed:
        gui.pump(timeout=0.05)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run(45.0)
    finally:
        session.close()
else:
    print("bpf up - run(10) to dispatch events, session.close() to end")
