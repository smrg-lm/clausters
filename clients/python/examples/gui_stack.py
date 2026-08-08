#!/usr/bin/env python3
"""Tabs with no script in the loop: a ``stack`` and a control bound to its index.

Two things meet here. A `stack` shows **one child at a time** — the one its
``index`` names — and everything else it holds is neither laid out nor drawn
while it is away. And a widget can be **bound to another widget**
(`clausters.gui.host.GuiHost.bind_widget`), which applies its value to that
widget's property with no round-trip through this process. Put together, a menu
bound to a stack's ``index`` *is* a tab bar: the pages flip inside the host, and
nothing prints here while you click.

The pages are three views of the same take: its waveform, its spectrogram and
an envelope drawn over it. The first two are the **heavy** views, and they are
what makes the switch worth watching — a hidden one keeps its GPU slot, so
coming back to it is instant rather than a re-upload. Flip back and forth as
fast as you can and see that neither ever rebuilds.

The bottom half is the same binding aimed at an ordinary prop: a slider bound to
the envelope's ``max`` moves that curve's value axis live, so dragging it
squashes the curve toward the floor with nothing going through Python. A binding
fires an **apply, never another binding**, so widgets wired to each other settle
instead of cascading — the envelope is editable, and its edits still come back
here as events.

No audio server is involved, so this boots only the GUI host.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the GUI binary

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/gui_stack.py``. It self-launches the windowed
host with `GuiHost().boot()`; by hand that is ``clausters-gui``. Needs a display
and a GPU adapter.
"""

# %%
import math
import sys
import time

from clausters.gui import (
    GuiHost, bpf, label, menu, panel, slider, spectrogram, stack, waveform, window,
)

SR = 48_000

# %% [markdown]
# ## The take every page shows
# One sweep, in memory: the three pages are three views of the *same* samples,
# which is what makes the switch worth having.

# %%
def sweep(seconds: float = 2.0) -> list:
    """A log sine sweep, 40 Hz -> 8 kHz."""
    n = int(seconds * SR)
    phase = 0.0
    out = []
    for i in range(n):
        f = 40.0 * (8000.0 / 40.0) ** (i / n)
        phase += 2.0 * math.pi * f / SR
        out.append(math.sin(phase) * 0.8)
    return out


TAKE = sweep()

#: A shape over the take, for the third page: an envelope is the light element
#: beside the two heavy ones, and the one the reader can edit.
ENVELOPE = [(0.0, 0.0), (0.35, 1.0, "exp"), (0.7, 0.4, "sin"), (1.0, 0.0, -4.0)]

# %% [markdown]
# ## Launch the GUI host

# %%
gui = GuiHost().boot()

# %% [markdown]
# ## The window: a picker over a stack, and a slider over the envelope
# The `menu`'s options are the page names and its index *is* the stack's index,
# which is exactly why the binding is one line and no event handler.
#
# The stack has no arrangement to make — a page fills it — so it takes only a
# `margin`. Its `weight` gives it the leftover, the way any work surface takes
# the room the chrome does not.

# %%
win = gui.open(window(
    panel(label("view:", w=48.0),
          menu(["waveform", "spectrogram", "envelope"], name="picker", index=0),
          layout="row", h=32.0),
    stack(waveform(data=TAKE, sample_rate=float(SR), ruler="time"),
          spectrogram(data=TAKE, sample_rate=float(SR), ruler="time"),
          bpf(points=ENVELOPE, name="curve", min=0.0, max=1.0, duration=1.0,
              label="envelope"),
          name="pages", index=0, weight=1.0),
    panel(label("envelope max:", w=110.0),
          slider(name="scale", min=1.0, max=4.0, value=1.0),
          layout="row", h=32.0),
    title="stack: tabs with no script in the loop", w=900, h=560, layout="col"))

# %% [markdown]
# ## The two bindings
# `bind_widget` names the target widget and the property its value lands on.
# From here on the picker flips the page and the slider moves the envelope's
# value axis **inside the host** -- drive either and nothing prints in this
# process. The contrast is the envelope itself: it is *not* bound, so editing it
# still comes back as an event.

# %%
win["picker"].bind_widget(win["pages"], "index")
win["scale"].bind_widget(win["curve"], "max")
print("bound: picker -> pages.index, scale -> curve.max; "
      "flip the pages and drag the slider -- nothing prints here "
      "(no script round-trip). Editing the envelope still does.")

# %% [markdown]
# ## Drive it
# The script can still set the page itself -- a binding is another writer of the
# same prop, not an owner of it. `page(1)` from a REPL shows the spectrogram.
#
# The one thing that *does* print is an edit of the envelope (drag a point,
# Ctrl+click to add or remove one): an unbound editor sends its whole
# breakpoint list back here, which is the path the two bound widgets replaced.

# %%
_closed = False
win.on_closed(lambda: globals().__setitem__("_closed", True))
win["curve"].on_event(
    lambda tag, *flat: print(f"envelope edited: {len(flat) // 4} points")
    if tag == "points" else None)


def page(index: int) -> None:
    """Show a page from the script (what the bound picker does on its own)."""
    win["pages"].set(index=index)


def run(seconds: float) -> None:
    """Pumps events for ``seconds`` (only the envelope's edits arrive: the two
    bound widgets send none back)."""
    start = time.monotonic()
    while time.monotonic() - start < seconds and not _closed:
        gui.pump(timeout=0.1)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run(30.0)
    finally:
        gui.stop()
else:
    print("stack up - page(1) to switch from here, run(10) to pump, gui.stop() to end")
