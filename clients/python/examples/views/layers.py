#!/usr/bin/env python3
"""The layer stack: a spectrogram, the wave over it, and the level inside that.

A view of a signal draws several things on one body, and ``layers`` is the list
of them, **back to front**. It is one element and not three stacked widgets,
because every view of a signal paints its own field before it draws: a second
one on the same rectangle would be a lid over the first, not a layer over it.
One element is also one time axis, one selection, one playhead and one upload.

What this window shows, and what to look for:

- **the order is yours**. The stack here is the texture, then the wave, then the
  level: the spectrogram is drawn first and the curves over it. Reverse the two
  words and the texture is a lid;
- **the vertical belongs to one layer**. A spectrogram measures hertz and a wave
  measures amplitude, and two quantities cannot share a ruler — so the texture
  keeps the axis (the Hz ruler on the left is its) and the curves say
  ``y="box"``, which normalizes each of them into the body it is drawn on. Two
  layers claiming the axis is **refused**, not resolved: the window prints what
  the host answers when the second cell asks for it;
- **alpha is per layer**. The two sliders re-state the stack live, so the wave
  and the level fade in and out over the texture while the picture under them
  stays where it is. That is not the widget's ``opacity``, which would fade the
  rulers and the caption with it;
- **solo and hide read a stack while you build it**. The buttons turn one layer
  off, or leave one on alone, without rewriting the list — a hidden layer keeps
  its place, so showing it again puts it back where it was rather than on top;
- **one axis for all of it**. Sweep a selection across the body and it crosses
  every layer at once, and the cursor read-out names one position in one unit:
  three pictures, one coordinate system.

The take is four seconds of a **sweep from 300 Hz to 3 kHz** whose level steps
down halfway, built at 16 kHz: the sweep is a diagonal line in the texture and
the step is a cliff in the wave, so the two layers are saying visibly different
things about the same samples.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server + GUI binaries

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/views/layers.py``. It self-launches the audio
server and the GUI host; by hand that is ``clausters`` and ``clausters-gui
--server 127.0.0.1:57110``. Needs a display and a GPU adapter; it makes no
sound, the take is looked at rather than played.
"""

# %%
import math
import sys
from array import array

from clausters import Session
from clausters.defs import Buffer
from clausters.gui import button, panel, slider, spectrogram, view

#: The take's own rate. A take carries its rate with it, and 16 kHz is enough
#: for a sweep that stops at 3 kHz -- it keeps what travels over the wire small
#: and puts the interesting part of the spectrum in the lower half of the
#: texture.
RATE = 16_000.0

# %% [markdown]
# ## Launch the server and the GUI
# The host fetches the take over its client leg, so it needs `--server`, which
# `Session.gui()` wires up.

# %%
session = Session.live()
server = session.server
gui = session.gui()

# %% [markdown]
# ## The take: a sweep whose level steps down halfway
# The sweep is a diagonal in the texture; the step is a cliff in the wave. One
# source, two layers saying different things about it.

# %%
def sweep(seconds: float, f0: float, f1: float) -> list:
    """A sine sweeping `f0` to `f1`, at full level for the first half of its
    length and a quarter of it for the second."""
    frames = int(seconds * RATE)
    out = []
    phase = 0.0
    for i in range(frames):
        t = i / frames
        freq = f0 * (f1 / f0) ** t
        phase += 2 * math.pi * freq / RATE
        out.append(0.9 * math.sin(phase) * (1.0 if t < 0.5 else 0.25))
    return out


take = Buffer.from_samples(array("f", sweep(4.0, 300.0, 3000.0)), 1, server=server)
server.sync()

# %% [markdown]
# ## The stack
# Back to front: the texture, the envelope over it, the level inside that. The
# curves take `y="box"` because the axis is the texture's — it is the layer
# whose quantity the Hz ruler reports.

# %%
STACK = [
    "spectrogram",
    {"draw": "peak", "y": "box", "alpha": 0.7},
    {"draw": "rms", "y": "box", "alpha": 0.45},
]


def with_alpha(peak: float, rms: float) -> list:
    """The same stack at two weights — what a slider re-states."""
    return [
        "spectrogram",
        {"draw": "peak", "y": "box", "alpha": peak},
        {"draw": "rms", "y": "box", "alpha": rms},
    ]


win = view(
    spectrogram(buffer=take.bufnum, sample_rate=RATE, name="stack", layers=STACK,
                window_size=512, freq_scale="linear",
                label="one body: the texture, the wave over it, the level inside that"),
    panel(
        slider(name="peak_alpha", label="wave", min=0.0, max=1.0, value=0.7),
        slider(name="rms_alpha", label="level", min=0.0, max=1.0, value=0.45),
        button(name="solo", label="solo the wave"),
        button(name="hide", label="hide the level"),
        layout="row", h=70,
    ),
    title="The layer stack", w=900, h=560, layout="col",
).open()

# %% [markdown]
# ## The stack is re-stated, not patched
# A layer's alpha is a property of the stack, so what a slider sends is the
# stack again — one prop, one message, the picture unchanged underneath.

# %%
state = {"peak": 0.7, "rms": 0.45, "solo": False, "hide": False}


def restate() -> None:
    """Sends the stack the buttons and sliders currently describe."""
    layers = with_alpha(state["peak"], state["rms"])
    if state["hide"]:
        layers[2]["visible"] = False
    if state["solo"]:
        layers[1]["solo"] = True
    win["stack"].set(layers=layers)


def on_peak(value: float) -> None:
    state["peak"] = float(value)
    restate()


def on_rms(value: float) -> None:
    state["rms"] = float(value)
    restate()


def toggle_solo() -> None:
    state["solo"] = not state["solo"]
    win["solo"].set(label="the whole stack" if state["solo"] else "solo the wave")
    restate()


def toggle_hide() -> None:
    state["hide"] = not state["hide"]
    win["hide"].set(label="show the level" if state["hide"] else "hide the level")
    restate()


win["peak_alpha"].on_event(on_peak)
win["rms_alpha"].on_event(on_rms)
# On the **click**, not on every event a button emits: `on_event` fires for the
# press *and* the release, so a flag flipped there comes back to where it
# started.
win["solo"].on_click(toggle_solo)
win["hide"].on_click(toggle_hide)
print("the sliders fade the two curves over the texture; solo leaves the wave "
      "alone; sweep a selection across the body and it crosses every layer at "
      "once. Close the window to stop")

# %% [markdown]
# ## What the host refuses
# The wave measures amplitude and the texture measures hertz. Asking for both
# on the vertical axis is refused rather than resolved — whichever lost would
# be drawn on a scale that is not its own.

# %%
win["stack"].set(layers=["spectrogram", "peak"])
print("after asking for two layers on one axis, the stack is still:",
      win["stack"].query().props.get("layers"))

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        win.wait()
    finally:
        session.close()
else:
    print("up - session.close() to end")
