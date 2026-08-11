#!/usr/bin/env python3
"""Open a real window from one declarative GuiDef: a navigable waveform.

The "first pixels" example. It builds a ``window`` containing a ``label`` and the
heavy ``waveform`` view, fed a generated signal as a binary blob carried in the
same ``/gui_def`` message, and opens it on a ``clausters-gui`` host. The host
opens an actual window and renders the waveform; the wheel zooms toward the
pointer, left-drag pans, ``R`` resets, ``Esc`` (or the close button) closes it.

No audio server is involved -- the signal is generated here and shipped in the
def -- so this boots only the GUI host.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the GUI binary

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/gui_window.py``. It self-launches the windowed
host with `GuiHost().boot()`; by hand that is ``clausters-gui``. Needs a display
and a GPU adapter.

The signal is kept small enough that the whole def (JSON + blob) fits one UDP
datagram (~64 KB); the shared/streamed bulk path for large buffers is
``gui_bulk.py``.
"""

# %%
import math
import sys
import time

from clausters.gui import GuiHost, label, samples_to_blob, waveform, window

# %% [markdown]
# ## Launch the GUI host
# `GuiHost().boot()` starts a windowed `clausters-gui` process and returns a host
# connected to it (stopped by `stop`, or on interpreter exit).

# %%
gui = GuiHost().boot()

# %% [markdown]
# ## A small signal, shipped as a blob in the def
# `samples_to_blob` packs the f32 samples; the waveform reads blob index 0, which
# rides beside the JSON in the same `/gui_def` message.

# %%
def decaying_sine(n: int, cycles: float) -> list:
    """A sine that decays across the buffer, so the waveform shows both the
    cycles and the envelope."""
    return [math.sin(2 * math.pi * cycles * i / n) * math.exp(-3.0 * i / n) for i in range(n)]


# ~8000 f32 (~32 KB) keeps the def (JSON + blob) inside one UDP datagram.
blob = samples_to_blob(decaying_sine(8_000, cycles=120.0))

win = gui.open(window(
    label(name="caption", text="Decaying sine (wheel: zoom, drag: pan, R: reset)"),
    waveform(name="wave", blob=0),
    title="clausters-gui - waveform", w=720, h=360, layout="col"), blob)
win.on_closed(lambda: globals().__setitem__("_closed", True))
print("the host opened a window; zoom/pan the waveform, close it to stop")

# %% [markdown]
# ## Keep it open
# Nothing to drive -- the waveform is navigated with the mouse. Wait for the
# close, then stop the host.

# %%
_closed = False


def run(seconds: float | None = None) -> None:
    """Pumps events for ``seconds`` (the waveform is navigated with the mouse).

    Script-run there is no bound and the window is what ends it; the
    ``seconds`` argument is for a cell run, where a notebook wants the loop to
    give the prompt back.
    """
    start = time.monotonic()
    while not _closed and (seconds is None or time.monotonic() - start < seconds):
        gui.pump(timeout=0.1)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        gui.stop()
else:
    print("window up - run(10) to keep it open, gui.stop() to end")
