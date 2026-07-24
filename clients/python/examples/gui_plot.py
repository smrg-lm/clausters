#!/usr/bin/env python3
"""Plotting an NRT render's output: the static ``plot`` view.

A ``plot`` is the lightweight counterpart of the heavy ``waveform``: it draws a
signal once (a line, or a min/max envelope when there are more samples than
pixels) with no zoom or pan -- "a simple static plot of an NRT-generated
signal/file". Here the signal is produced **offline**, by the bundled NRT
renderer, with no server and no audio device, then handed to the GUI host as a
**mapped local file** (the bulk path: the samples never ride OSC).

So no audio server is involved -- the audio was already rendered offline -- and
this boots only the GUI host.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the renderer + GUI binary

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/gui_plot.py``. It renders offline with
`Session.nrt` and self-launches the windowed host with `GuiHost.boot()`; by hand
the host is ``clausters-gui`` (no ``--server`` needed). Needs a display and a GPU
adapter.
"""

# %%
import os
import sys
import tempfile
import time

from clausters import Session
from clausters.gui import GuiHost, plot, samples_to_file, window
from clausters.seq import Pbind, Pseq, Pwhite

SR = 48_000.0

# %% [markdown]
# ## Render a phrase offline (no server, no audio device)
# A one-bar arpeggio walking a major scale, rendered by the bundled NRT engine;
# channel 0 is de-interleaved and written to a raw f32 file the host will map.

# %%
def phrase() -> Pbind:
    return Pbind(degree=Pseq([0, 2, 4, 7, 4, 2], repeats=2), dur=0.25,
                 amp=Pwhite(0.1, 0.2))


nrt = Session.nrt(tempo=2.0)
nrt.play(phrase())
samples, frames = nrt.render(sample_rate=SR, channels=2)
print(f"rendered {frames} frames ({frames / SR:.2f} s) offline, no server")

fd, path = tempfile.mkstemp(prefix="clausters_plot_", suffix=".f32")
os.close(fd)
samples_to_file(list(samples[0::2]), path)  # de-interleave channel 0
print(f"wrote {os.path.getsize(path)} B of raw f32; the host maps it (no OSC)")

# %% [markdown]
# ## Launch the GUI host and plot the file
# `GuiHost.boot()` starts a windowed host (no server needed); the plot maps the
# rendered file. Named, so `open` resolves it.

# %%
gui = GuiHost.boot()
win = gui.open(window(
    plot(name="render", path=path, min=-1.0, max=1.0, label="NRT render (mono)"),
    title="Plot of an NRT render", w=720, h=300))
win.on_closed(lambda: globals().__setitem__("_closed", True))
print("a window plots the rendered signal; close it to stop")

# %% [markdown]
# ## Keep it open, then clean up

# %%
_closed = False


def run(seconds: float) -> None:
    """Pumps events for ``seconds`` (the plot is static)."""
    start = time.monotonic()
    while time.monotonic() - start < seconds and not _closed:
        gui.pump(timeout=0.1)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run(30.0)
    finally:
        gui.stop()
        if os.path.exists(path):
            os.remove(path)
else:
    print("plot up - run(10) to keep it open, gui.stop() to end")
