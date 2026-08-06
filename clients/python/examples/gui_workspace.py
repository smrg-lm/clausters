#!/usr/bin/env python3
"""The 2D workspace: one `scroll` container, three views of it.

`scroll` shows its children through a window onto a **virtual content area**
larger than the widget. It is deliberately *general first*: the default is the
full 2D plane — drag the empty background to pan both axes, wheel to zoom
anchored at the cursor — and the familiar constrained scroll views are that
same widget configured down, not separate widgets:

- ``axis="y", zoom=False`` — a plain vertical scroll view (the wheel scrolls),
- ``axis="x", zoom=False`` — a horizontal strip,
- the default — the free plane.

This example puts all three in one window so the degeneration is visible side
by side, and prints the ``"view" x y zoom`` events the gestures emit. Every pane
and the reset button are *named*, so the script wires each by name and never
matches a widget id. The view state is settable back with `set` (the round trip
that lets a script own the navigation), which the **reset button** next to the
plane demonstrates: its press comes in as a ``/gui_event`` and the handle answers
by putting the view back at the origin and **clearing** the zoom (``view_zoom=0``),
which returns the plane to its default scale rather than pinning it to 1.

The example **launches its own GUI host** (`GuiHost.boot`) and needs no audio
server at all: a workspace is pure layout and navigation. Needs a display and a
Vulkan/Metal/DX12/GL adapter (the host opens a window).

Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python   # bundles the server + GUI binaries

then::

    python clients/python/examples/gui_workspace.py

Drag and wheel over each of the three panes; every navigation prints here.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention):
step through it with Shift+Enter and the window stays up between cells, or run
it as a plain script.
"""

# %%
import sys
import time

from clausters.gui import GuiHost, button, knob, label, panel, scroll, toggle, window

# The plane's content area, in content units (the workspace's own coordinates).
PLANE_W, PLANE_H = 1600.0, 1200.0


# %% [markdown]
# ## The panes

# %%
def plane() -> dict:
    """The general case: a free 2D plane holding a scattered set of widgets.

    The children carry ``x``/``y``/``w``/``h`` in *content* units. With no
    ``content_w``/``content_h`` the content area would size itself from those
    placement extents; naming it explicitly gives the plane room to roam past
    its contents, which is what a patch canvas wants.
    """
    boxes = []
    for i in range(9):
        col, row = i % 3, i // 3
        x, y = 60.0 + col * 480.0, 60.0 + row * 380.0
        boxes.append(
            panel(label(f"node {i}"),
                  knob(label="amount", min=0.0, max=1.0, value=i / 8),
                  layout="col", x=x, y=y, w=300.0, h=220.0))
    return scroll(*boxes, name="plane", content_w=PLANE_W, content_h=PLANE_H)


def vertical_list() -> dict:
    """The constrained case: a plain vertical scroll view.

    Same widget, two props. A ``col`` layout stacks the children down the
    content area; ``content_h`` makes that area taller than the pane, so the
    wheel has somewhere to scroll to.
    """
    rows = [toggle(label=f"track {i + 1}", value=False) for i in range(20)]
    return scroll(*rows, name="vlist", layout="col", axis="y", zoom=False,
                  content_h=900.0)


def horizontal_strip() -> dict:
    """The other constrained case: a horizontal strip (a timeline-ish ribbon)."""
    cells = [label(f"bar {i + 1}", x=i * 90.0, y=0.0, w=80.0, h=60.0)
             for i in range(24)]
    return scroll(*cells, name="hstrip", axis="x", zoom=False, content_h=70.0)


# %% [markdown]
# ## The workspace

# %%
def workspace() -> dict:
    return window(
        panel(panel(label("free plane — drag to pan, wheel to zoom"),
                    button(name="reset", label="reset view", w=120.0),
                    layout="row", h=26.0, margin=0),
              plane(),
              layout="col", weight=3),
        panel(label("vertical scroll view (axis=y, zoom off)", h=20.0),
              vertical_list(),
              layout="col", weight=2),
        panel(label("horizontal strip (axis=x, zoom off)", h=20.0),
              horizontal_strip(),
              layout="col", h=110.0),
        title="Workspace", w=900, h=760, layout="col",
    )


# %% [markdown]
# ## Reporting what the gestures did

# %%
def on_view(name: str):
    """A pane's ``"view"`` edit-back, wired by name — the same three keys the
    reset button sets back."""
    def handler(tag, *vals):
        if tag == "view" and len(vals) >= 3:
            print(f"{name}: view x={vals[0]:.1f} y={vals[1]:.1f} zoom={vals[2]:.2f}")
    return handler


# %% [markdown]
# ## Open it
# No audio server: `boot` starts a host with no client leg and owns it, stopping
# the process on exit.

# %%
gui = GuiHost().boot()
win = gui.open(workspace())
print("drag and wheel over each pane; navigation events print here")
print("('reset view' puts the plane back at the origin, at its default zoom)")

closed = [False]
win.on_closed(lambda: closed.__setitem__(0, True))


# %% [markdown]
# ## The reset button
# The view is state the script owns, so the handle answers by putting the plane
# back -- the same three keys the gestures emit. ``view_zoom=0`` **clears** the
# zoom instead of naming one, so the plane returns to its default (the display's
# own scale). Naming ``1.0`` here would pin it to one physical pixel per content
# unit, which on a 2x screen is half the size the plane opened at.

# %%
def reset(value):
    if value == 1:
        win["plane"].set(view_x=0.0, view_y=0.0, view_zoom=0)
        print("view reset to the origin, zoom 1")


win["plane"].on_event(on_view("plane"))
win["vlist"].on_event(on_view("vlist"))
win["hstrip"].on_event(on_view("hstrip"))
win["reset"].on_event(reset)


# %%
def run(seconds: float = 45.0):
    """Pump the host until the window closes or ``seconds`` elapse."""
    deadline = time.monotonic() + seconds
    while not closed[0] and time.monotonic() < deadline:
        gui.pump(timeout=0.1)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        gui.stop()
else:
    print("workspace up - run(20) to drive it, gui.stop() to end")
