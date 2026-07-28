#!/usr/bin/env python3
"""A triggered audio-rate oscilloscope over a server audio tap.

The GUI's ``scope`` widget has two rates. Its control-rate form (see
``gui_meters.py``) plots a control bus's history one sample per frame; this
example uses the audio-rate form -- a real **oscilloscope** showing the actual
samples of a live signal, with a level trigger that holds the trace still.

The data path is the server's **audio taps**: pre-allocated sample rings inside
the shared-memory segment. ``Server.tap(tap, bus)`` routes an audio bus into a
ring; from then on the engine appends that bus's samples every block, and the
GUI host reads the newest window straight out of shared memory each frame --
zero per-frame OSC. (A browser host cannot map the segment; it subscribes
``/tap_stream`` instead. ``Server.stream_taps`` exposes that path to Python too,
for headless capture of a live signal.)

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server + GUI binaries

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/gui_scope.py``. It self-launches the audio
server (with a shared-memory segment and audio taps) and the GUI host mapping
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

# %% [markdown]
# ## Launch the server and the GUI, and a tone routed into a tap
# `Session.live()` boots the server with a shared-memory segment and taps;
# `session.gui()` maps the same segment, so the scopes read tap 0 from it.

# %%
session = Session.live()
server = session.server
gui = session.gui()

server.add_synthdef(SynthDef(
    "tone", out(0.0, sine(control("freq", 220.0)) * control("amp", 0.2))))
synth = server.synth("tone", {"freq": 220.0})
server.tap(0, 0)  # audio bus 0 (the hardware out) -> audio tap 0

# %% [markdown]
# ## Two scopes on the same tap: triggered vs free-running
# The triggered one aligns each redraw to a rising crossing of level 0.0, so the
# sine stays locked; the free-running one has a trigger the signal never reaches,
# so it shows the same signal drifting -- why triggering exists. Both named.

# %%
win = gui.open(window(
    panel(scope(name="triggered", tap=0, window_ms=15.0, trigger=0.0,
                label="triggered (level 0.0)"),
          scope(name="free", tap=0, window_ms=15.0, trigger=9.0,
                label="free-running"),
          layout="col"),
    title="Audio-rate oscilloscope", w=560, h=420))
win.on_closed(lambda: globals().__setitem__("_closed", True))
print("the top trace stays locked while the pitch sweeps; "
      "the bottom one drifts; close the window to stop")

# %% [markdown]
# ## Drive it
# Sweep the frequency 220..440 Hz and back so the triggered trace visibly
# re-locks. The scopes read the tap from shared memory on their own.

# %%
_closed = False


def run(seconds: float) -> None:
    """Sweeps the tone's frequency for ``seconds``."""
    start = time.monotonic()
    while time.monotonic() - start < seconds and not _closed:
        phase = (time.monotonic() - start) / 8.0
        server.set(synth, {"freq": 330.0 + 110.0 * math.sin(2 * math.pi * phase)})
        gui.pump(timeout=0.03)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run(20.0)
    finally:
        server.tap(0, -1)  # stop the tap; the ring goes quiet
        server.free(synth)
        session.close()
else:
    print("scope up - run(10) to sweep the tone, session.close() to end")
