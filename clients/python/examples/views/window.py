#!/usr/bin/env python3
"""Open a real window from one view: a navigable waveform, fed by a source.

The "first pixels" example. It builds a ``view`` containing a ``label`` and the
heavy ``waveform``, fed a generated signal through a `source`, and opens it on a
``clausters-gui`` host. The host opens an actual window and renders the
waveform; the wheel zooms toward the pointer, left-drag pans, ``R`` resets,
``Esc`` (or the close button) closes it. Then the source is pointed at other
samples and the same window redraws.

No audio server is involved -- the signal is generated here -- so this boots
only the GUI host.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the GUI binary

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/views/window.py``. It self-launches the windowed
host with `GuiHost().boot()`; by hand that is ``clausters-gui``. Needs a display
and a GPU adapter.

A source picks how its samples travel: a short list rides inline in the JSON,
and this one -- 8000 floats, past that ceiling -- spills to a mapped file, so
nothing rides OSC at all. Naming a carrier by hand (``blob=``, ``path=``,
``cache=``, ``buffer=``) is still supported and is what ``bulk.py`` shows.
"""

# %%
import math
import sys
import time

from clausters.gui import GuiHost, label, source, view, waveform

# %% [markdown]
# ## Launch the GUI host
# `GuiHost().boot()` starts a windowed `clausters-gui` process and returns a host
# connected to it (stopped by `stop`, or on interpreter exit). It also becomes
# the *ambient* host, which is why `open()` below needs no argument -- the same
# first-wins rule `Server.boot()` follows for the default session.

# %%
gui = GuiHost().boot()

# %% [markdown]
# ## A signal, held as a source
# A `source` is the samples as something you keep: the view is handed the source,
# not a carrier, and the source decides how they get there. 8000 floats is past
# the inline ceiling, so it spills to a mapped file -- `sig.carrier` says
# `"path"` -- and the `/gui_def` message carries no samples at all.

# %%
def decaying_sine(n: int, cycles: float) -> list:
    """A sine that decays across the buffer, so the waveform shows both the
    cycles and the envelope."""
    return [math.sin(2 * math.pi * cycles * i / n) * math.exp(-3.0 * i / n) for i in range(n)]


sig = source(decaying_sine(8_000, cycles=120.0))

v = view(
    label(name="caption", text="Decaying sine (wheel: zoom, drag: pan, R: reset)"),
    waveform(name="wave", data=sig),
    title="clausters-gui - waveform", w=720, h=360, layout="col")

win = v.open()
print("the host opened a window; zoom/pan the waveform, close it to stop")

# %% [markdown]
# ## Point the source at other samples
# The window is not rebuilt and the view is not touched: the source is the
# entry point, so changing it is what redraws. A spilled source rewrites its own
# file and tells the view to read it again; an inline one sends the new samples.

# %%
time.sleep(2.0)
sig.set(decaying_sine(8_000, cycles=40.0))
print("the same window now draws a slower sine")

# %% [markdown]
# ## Keep it open
# Nothing else to drive -- the waveform is navigated with the mouse. A script
# holds the main thread until the window is closed with
# `clausters.gui.handle.WindowHandle.wait`; the host's event loop is already
# running underneath, so this is a wait and not a drain. Cell by cell there is
# nothing to call at all.

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        win.wait()
    finally:
        gui.stop()
else:
    print("window up - win.wait(10) to hold it, gui.stop() to end")
