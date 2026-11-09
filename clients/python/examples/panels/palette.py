#!/usr/bin/env python3
"""The standard elements at their defaults, in every layout.

One window holds the same set of standard controls -- a label, a button, a
toggle, a choice, a number, a knob, a slider, a text field, a progress bar and
a separator -- five times over, each time in a different container, and with
nothing asked of any of them: no ``w``, no ``h``, no ``weight``, no ``pack``.
So what is drawn is what the layout does with an element's defaults:

- **a row** as wide as the window: how the width is shared along it;
- **a tall row** (``h=140``): what each element does across a cell taller
  than it is;
- **a short row** (``h=28``): what each one gives up in a cell shorter than
  it is;
- **a grid** of four columns: an element in a cell it did not choose;
- **a column**, on the right: how the height is shared down it.

No element is drawn under its **floor**, the smallest size it still reads at:
the short row grows to its tallest control's floor rather than spilling into
the grid under it, a knob gives up its caption before its disc, and a window
is never made smaller than all of it. Filling a cell is the default -- a grid
of buttons is a step sequencer -- so the buttons are as big as their cells.

A sixth container is the other half: **design**. The same buttons in a grid,
each told the most it takes (`max_w`, `max_h`) and where it stands in a cell
larger than that (`anchor`).

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the GUI binary

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/panels/palette.py``, which holds until the
window is closed. It boots only the GUI host, no audio server. Needs a display
and a GPU adapter.
"""

# %%
import sys

from clausters.gui import (GuiHost, button, choice, knob, label, number, panel,
                           progress, separator, slider, text, toggle, view)

# %% [markdown]
# ## The set
# Every standard control, each with a caption where it takes one, and nothing
# that sizes it. A function rather than a list, because a view is a tree: each
# container gets elements of its own.

# %%


def standard():
    """The standard controls, at their defaults."""
    return (
        label("a label"),
        button(label="button"),
        toggle(label="toggle"),
        choice(["one", "two", "three"], label="choice"),
        number(label="number", min=0.0, max=10.0, value=3.0),
        knob(label="knob", value=0.5),
        slider(label="slider", value=0.5),
        text(label="text", value="typed"),
        progress(0.4, label="progress"),
        separator(),
    )


# %% [markdown]
# ## The containers
# The same set in five containers. The two rows with a height are the only
# sizes on the defaults' side, and they are on the containers, never on an
# element. The last grid is the design side: buttons bounded and anchored.

# %%
gui = GuiHost().boot()

v = view(
    panel(
        panel(*standard(), layout="row", title="row"),
        panel(*standard(), layout="row", title="row, h=140", h=140),
        panel(*standard(), layout="row", title="row, h=28", h=28),
        panel(*standard(), layout="grid", cols=4, title="grid, 4 columns"),
        panel(*(button(label=f"step {i + 1}", max_w=90, max_h=32, anchor=side)
                for i, side in enumerate(("top_left", "top", "center", "bottom_right"))),
              layout="grid", cols=4, title="grid, max_w=90 max_h=32, anchored"),
        layout="col",
    ),
    panel(*standard(), layout="col", title="col", w=260),
    title="Palette", w=1400, h=900, layout="row")

win = v.open()

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    win.wait()
else:
    print("up - change a container above and open it again; win.close() to end")
