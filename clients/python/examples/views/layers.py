#!/usr/bin/env python3
"""The layer stack: every picture a signal view can draw, on one body.

A view of a signal draws several things on one body, and ``layers`` is the list
of them, **back to front**. It is one element and not a pile of widgets, because
every view of a signal paints its own field before it draws: a second one on the
same rectangle would be a lid over the first, not a layer over it. One element
is also one time axis, one selection, one playhead and one upload.

The panel on the right is the stack itself, a row per layer: a **checkbox** (is
it drawn), a **fader** (its own weight over what is under it) and **top** (move
it to the front).

What to look for:

- **the order is yours, and it is live**. ``top`` re-states the list with that
  layer last, and the picture re-composites between two frames. The texture
  under the curves and the texture over them are the same layers in two orders;
- **the vertical belongs to one layer, and here it is the top one**. A
  spectrogram measures hertz and a wave measures amplitude, and two quantities
  cannot share a ruler -- so whichever layer is on top takes ``y="axis"`` (the
  left ruler and the cursor read-out are its) and the rest take ``y="box"``,
  which normalizes each into the body it is drawn on. Raise the wave over the
  texture and the ruler goes from hertz to amplitude. That the *top* layer
  rules is **this example's** policy, not the host's: the host asks only that
  exactly one layer claim the axis, and refuses a stack where two claim it for
  different quantities;
- **alpha is per layer**. A fader fades one picture over the ones under it and
  leaves the rulers, the caption and the selection alone -- which the widget's
  own ``opacity`` would not;
- **a hidden layer keeps its place**. Uncheck one and check it again: it comes
  back where it was in the order, not on top;
- **one axis for all of it**. Sweep a selection across the body and it crosses
  every layer at once, and the cursor read-out names one position in one unit:
  several pictures, one coordinate system.

The two loudness curves are in the list for completeness and are the one pair
that never takes the ruler: a loudness is read in LU against its own target, so
that layer draws **its own** scale on the right of the body whatever else is
happening (see ``views/loudness.py``). ``signal`` is the band-limited
reconstruction and draws only where the samples are separate points, so it
appears once you have zoomed into them.

The take is four seconds of a **sweep from 300 Hz to 3 kHz** whose level steps
down halfway, built at 16 kHz: the sweep is a diagonal in the texture and the
step is a cliff in the wave, so the layers say visibly different things about
the same samples.

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
from clausters.gui import button, label, panel, slider, spectrogram, toggle, view

#: The take's own rate. A take carries its rate with it, and 16 kHz is enough
#: for a sweep that stops at 3 kHz -- it keeps what travels over the wire small
#: and puts the interesting part of the spectrum in the lower half of the
#: texture.
RATE = 16_000.0

#: Every layer a signal view can draw, with the weight and the checkbox each one
#: starts at. The order here is the stack's, back to front.
CATALOGUE = [
    ("spectrogram", "the STFT, as a texture", 1.0, True),
    ("peak", "the min/max envelope", 0.7, True),
    ("rms", "the level inside it", 0.45, True),
    ("signal", "the curve between the samples", 1.0, False),
    ("momentary", "loudness, 400 ms", 1.0, False),
    ("short", "loudness, 3 s", 1.0, False),
]

#: The layers read in LU against their own target: they draw their own scale on
#: the right of the body and never take the view's vertical, so the search for
#: the top layer skips them.
LOUDNESS = ("momentary", "short")

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
# source, several layers saying different things about it.

# %%
def sweep(seconds: float, f0: float, f1: float) -> list:
    """A sine sweeping `f0` to `f1`, at full level for the first half of its
    length and a quarter of it for the second."""
    frames = int(seconds * RATE)
    out = []
    phase = 0.0
    for i in range(frames):
        t = i / frames
        phase += 2 * math.pi * (f0 * (f1 / f0) ** t) / RATE
        out.append(0.9 * math.sin(phase) * (1.0 if t < 0.5 else 0.25))
    return out


take = Buffer.from_samples(array("f", sweep(4.0, 300.0, 3000.0)), 1, server=server)
server.sync()

# %% [markdown]
# ## The stack, and the panel that is it
# `order` is the list back to front; `state` is what each layer says about
# itself. The ruler goes to the layer on top, which is this example's rule for
# picking the one that claims the axis.

# %%
order = [name for name, _, _, _ in CATALOGUE]
state = {name: {"on": on, "alpha": alpha} for name, _, alpha, on in CATALOGUE}


def ruler_layer() -> str:
    """The drawn layer whose quantity the ruler reports: the top one, skipping
    the loudness curves, which carry a scale of their own."""
    for name in reversed(order):
        if state[name]["on"] and name not in LOUDNESS:
            return name
    return ""


def stack() -> list:
    """The stack as the panel currently describes it: one entry per layer, in
    drawing order, each saying whether it is drawn, at what weight and on which
    vertical."""
    top = ruler_layer()
    return [
        {"draw": name, "alpha": state[name]["alpha"], "visible": state[name]["on"],
         "y": "axis" if name == top else "box"}
        for name in order
    ]


def row(name: str, what: str, alpha: float, on: bool):
    """One layer's controls: is it drawn, at what weight, and move it to the
    front."""
    return panel(
        label(f"{name} - {what}", h=18),
        panel(
            toggle(name=f"on_{name}", value=on, w=36),
            slider(name=f"alpha_{name}", min=0.0, max=1.0, value=alpha),
            button(name=f"top_{name}", label="top", w=54),
            layout="row", h=26,
        ),
        layout="col", h=52,
    )


win = view(
    panel(
        spectrogram(buffer=take.bufnum, sample_rate=RATE, name="stack",
                    layers=stack(), window_size=512, freq_scale="linear",
                    label="one body, one time axis, one selection"),
        panel(
            label("layers, back to front", name="order", h=20),
            *(row(name, what, alpha, on) for name, what, alpha, on in CATALOGUE),
            layout="col", w=320,
        ),
        layout="row",
    ),
    title="The layer stack", w=1080, h=560, layout="col",
).open()

# %% [markdown]
# ## The stack is re-stated, not patched
# Every control sends the whole list again -- one prop, one message. That is
# what makes the order, the weights and the claim one statement instead of
# three, and it is why raising a layer also moves the ruler.

# %%
def restate() -> None:
    """Sends the stack the panel now describes, and names the ruler's owner."""
    win["stack"].set(layers=stack())
    win["order"].set(text=f"layers, back to front - ruler: {ruler_layer() or 'none'}")


def on_alpha(name: str):
    def handler(value: float) -> None:
        state[name]["alpha"] = float(value)
        restate()
    return handler


def on_toggle(name: str):
    def handler(value: float) -> None:
        state[name]["on"] = bool(value)
        restate()
    return handler


def on_top(name: str):
    def handler() -> None:
        order.remove(name)
        order.append(name)
        restate()
    return handler


for layer, _what, _alpha, _on in CATALOGUE:
    win[f"alpha_{layer}"].on_event(on_alpha(layer))
    win[f"on_{layer}"].on_event(on_toggle(layer))
    # On the **click**, not on every event a button emits: `on_event` fires for
    # the press *and* the release, so a list rotated there would move twice.
    win[f"top_{layer}"].on_click(on_top(layer))
restate()
print("raise the wave over the texture and the left ruler goes from hertz to "
      "amplitude; the faders fade one layer over the others; unchecking a layer "
      "and checking it again puts it back where it was. Close the window to stop")

# %% [markdown]
# ## What the host refuses
# The wave measures amplitude and the texture measures hertz. Asking for both
# on the vertical axis is refused rather than resolved -- whichever lost would be
# drawn on a scale that is not its own -- so exactly one layer claims it, which
# is what the panel above is doing for you.

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
