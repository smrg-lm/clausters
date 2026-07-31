#!/usr/bin/env python3
"""A triggered audio-rate oscilloscope over a server audio bus.

The GUI's ``scope`` widget names a **bus** and a **rate**. At audio rate (the
default) it is a real **oscilloscope**, showing the actual samples of a live
signal with a level trigger that holds the trace still; at control rate (see
``gui_meters.py``) it plots a control bus's history one sample per frame.

A script only names the bus. Behind it, the GUI host asks the server to record
that bus into the shared-memory segment and reads the newest window straight
out of it each frame -- zero per-frame OSC -- and stops the recording when no
open view is drawing it. (A browser host cannot map the segment; it subscribes
``/tap_stream`` instead. ``Server.stream_taps`` exposes that path to Python
too, for headless capture of a live signal.)

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server + GUI binaries

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/gui_scope.py``. It self-launches the audio
server (with a shared-memory segment) and the GUI host mapping
that segment; by hand that is ``clausters --shm <path>`` and ``clausters-gui
--shm <path>``. Run this with no server already up on 57110, so the session
boots its own. Needs a display and a GPU adapter.
"""

# %%
import math
import sys
import time

from clausters import Session
from clausters.defs import SynthDef, control, out, sine
from clausters.gui import panel, scope, window
from clausters.defs import Synth

# %% [markdown]
# ## Launch the server and the GUI, and a tone on the output bus
# `Session.live()` boots the server with a shared-memory segment;
# `session.gui()` maps the same segment, so the scopes read bus 0 from it.

# %%
session = Session.live()
server = session.server
gui = session.gui()

SynthDef(
    "tone", out(0.0, sine(control("freq", 220.0)) * control("amp", 0.2))).send(server)
synth = Synth.new("tone", {"freq": 220.0}, server=server)

# %% [markdown]
# ## Two scopes on the same bus: triggered vs free-running
# The triggered one aligns each redraw to a rising crossing of level 0.0, so the
# sine stays locked; the free-running one has a trigger the signal never reaches,
# so it shows the same signal drifting -- why triggering exists. Both named.

# %%
win = gui.open(window(
    panel(scope(0, name="triggered", window_ms=15.0, trigger=0.0,
                label="triggered (level 0.0)"),
          scope(0, name="free", window_ms=15.0, trigger=9.0,
                label="free-running"),
          layout="col"),
    title="Audio-rate oscilloscope", w=560, h=420))
win.on_closed(lambda: globals().__setitem__("_closed", True))
print("the top trace stays locked while the pitch sweeps; "
      "the bottom one drifts; close the window to stop")

# %% [markdown]
# ## Drive it
# Sweep the frequency 220..440 Hz and back so the triggered trace visibly
# re-locks. The scopes read the bus from shared memory on their own.

# %%
_closed = False


def run(seconds: float) -> None:
    """Sweeps the tone's frequency for ``seconds``."""
    start = time.monotonic()
    while time.monotonic() - start < seconds and not _closed:
        phase = (time.monotonic() - start) / 8.0
        synth.set({"freq": 330.0 + 110.0 * math.sin(2 * math.pi * phase)})
        gui.pump(timeout=0.03)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run(20.0)
    finally:
        synth.free()
        session.close()
else:
    print("scope up - run(10) to sweep the tone, session.close() to end")
