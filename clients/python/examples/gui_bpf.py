#!/usr/bin/env python3
"""Draw an envelope in the ``bpf`` editor and hear the server play it.

The first widget that **writes data back**: a drawable break-point function
whose segments use the server's own envelope shape numbers, evaluated in the
host through the same shared math (``clausters-core``) the server's ``EnvGen``
plays — what the editor draws is exactly what you hear.

Editing gestures, all live:

- **drag a point** to move it (times stay monotonic between its neighbors);
- **drag a segment** vertically to bend its curvature (it becomes the custom
  curve shape, like ``Env``'s numeric curvature);
- **Ctrl+click** on empty curve area adds a point there; **Ctrl+click on a
  point** removes it;
- the **curve menu** applies a standard transition shape (or a numeric
  curvature) to every segment at once — the script rewrites the shapes through
  the `Env` round trip and pushes the list back with ``gui.set``, so the same
  points redraw under the chosen curve.

Every edit flows back per the **edit-back pattern**: the host emits
``/gui_event <id> "points" <t v shape curve ...>`` — the breakpoint list as
flat OSC primitives, shapes as ints, everything else floats. This script maps
that list to a `clausters.defs.Env` (`points_to_env`) and, on the **play**
button, sends a fresh SynthDef built from it and spawns a note: the drawn
envelope shapes the tone you hear. (The reverse mapping, `env_to_points`,
seeds the editor with a familiar ADSR to start from.) A *bound* editor
(``GuiHost.bind``) would instead forward the same flat list straight to the
audio server after the binding's fixed prefix, bypassing the script — the
widget-value bypass generalized to a list.

The widget is deliberately more general than an amplitude envelope — the
future automation-lane shape: ``min``/``max`` give any parameter range
(bipolar, unipolar, arbitrary), ``exp=True`` a geometric display scale for
frequency-like values, and the ``"step"`` shape an on/off lane.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention).
Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server + GUI binaries

Run it cell by cell (Shift+Enter) and keep drawing/playing from the live
handles, or as a plain script — ``python clients/python/examples/gui_bpf.py``
— which plays a note on every edit for a while, then tears everything down.
Needs a display and a GPU adapter, plus an audio device.
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
# The editor is seeded from a familiar `Env` (an ADSR played through, no
# sustain) via `env_to_points` — the same helper a live ``points`` set uses.
# Times are seconds over a fixed 2-second domain; values are the unipolar
# amplitude range.

# %%
START_ENV = Env([0.0, 1.0, 0.4, 0.0], [0.05, 0.3, 1.2], ["exp", -4.0, "sin"])

# The curve menu's options: `Env`-style curve specs — shape names plus two
# custom curvatures (positive builds slowly then fast, negative the reverse).
# "hold" is the constant lane: each point's value held until the next point.
# (The wire format also has SC's "step" — jump to the *target* at segment
# start, so a point's level shows one segment early; it expresses nothing
# hold cannot, so the menu leaves it out. `Env.step` builds step sequences.)
CURVES = ["lin", "exp", "sin", "welch", "sqr", "cub", "hold", -4.0, 4.0]


def scene() -> dict:
    return window(
        label(1, "drag points/segments; Ctrl+click adds/removes; play sends it"),
        bpf(10, points=env_to_points(START_ENV), min=0.0, max=1.0,
            duration=2.0, label="amp env"),
        menu(30, [str(c) for c in CURVES], label="curve (all segments)"),
        button(20, label="play"),
        title="BPF envelope -> EnvGen", w=640, h=460, layout="col",
    )


win = gui.open(scene())
print(f"opened window {win} — draw the envelope, then press play")

# %% [markdown]
# ## Hear the drawn envelope
# The latest breakpoint list arrives as ``"points"`` events; `play()` turns it
# into an `Env`, builds a one-shot SynthDef around ``env_gen`` (the envelope
# frees the synth when it finishes) and plays a note through it.

# %%
_points = env_to_points(START_ENV)
_closed = False


def play():
    """One note shaped by the envelope as currently drawn."""
    env = points_to_env(_points)
    sig = sine(330.0) * env_gen(env, done_action=DoneAction.FREE_SELF) * 0.4
    server.add_synthdef(SynthDef("gui_bpf_env", out(0.0, sig), out(1.0, sig)))
    server.synth("gui_bpf_env")
    print(f"played {len(_points) // 4} breakpoints over {env.times} s segments")


def set_curve(spec):
    """Applies one `Env`-style curve spec to every segment of the drawn
    envelope — through the public round trip (`points_to_env` with the new
    curve, back via `env_to_points`) — and pushes it to the window, which
    redraws the same points with the new shapes."""
    global _points
    env = points_to_env(_points)
    _points = env_to_points(Env(env.levels, env.times, spec))
    gui.set(10, points=json.dumps(_points))
    print(f"curve -> {spec}")


def drain_events():
    """Reads pending events: envelope edits update ``_points`` (silently), the
    curve menu reshapes every segment, and the play button triggers the note."""
    global _points, _closed
    while (msg := gui.poll(0.0)) is not None:
        addr, args = msg
        if addr == "/gui_closed":
            _closed = True
        elif addr == "/gui_event" and len(args) >= 2 and args[1] == "points":
            # The edit-back payload: id, "points", then t v shape curve quads.
            _points = list(args[2:])
        elif addr == "/gui_event" and args[0] == 30:
            set_curve(CURVES[int(args[1])])
        elif addr == "/gui_event" and args[0] == 20 and args[1] == 1:
            play()


play()

# %% [markdown]
# ## Set the envelope from the script
# The same flat list is settable live — a ``/gui_set`` value is a scalar, so
# the array rides as its JSON string. Here: a percussive two-segment envelope.

# %%
gui.set(10, points=json.dumps(env_to_points(Env.perc(0.01, 1.2))))
_points = env_to_points(Env.perc(0.01, 1.2))

# %% [markdown]
# ## Plain-script run
# Cell-run: keep drawing and call `play()` / `drain_events()` between cells.
# Script-run: draw for a while — editing is silent, the **play** button sends
# the note — then everything is torn down.

# %%
if __name__ == "__main__":
    try:
        deadline = time.monotonic() + 45.0
        while time.monotonic() < deadline and not _closed:
            drain_events()
            time.sleep(0.05)
        gui.close(win)
        session.close()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
