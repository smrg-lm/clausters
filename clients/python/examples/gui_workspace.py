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
by side, and prints the ``"view" x y zoom`` events the gestures emit. The view
state is settable back with `set` (the round trip that lets a script own the
navigation), which the **reset button** next to the plane demonstrates: its
press comes in as a ``/gui_event`` and the script answers by putting the view
back at the origin, zoom 1.

The example **launches its own GUI host** (`GuiHost.boot`) and needs no audio
server at all: a workspace is pure layout and navigation. Needs a display and a
Vulkan/Metal/DX12/GL adapter (the host opens a window).

Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python   # bundles the server + GUI binaries

then::

    python clients/python/examples/gui_workspace.py

Drag and wheel over each of the three panes; every navigation prints here.
"""

import sys
import time

from clausters.gui import GuiHost, button, knob, label, panel, scroll, toggle, window

# The plane's content area, in content units (the workspace's own coordinates).
PLANE_W, PLANE_H = 1600.0, 1200.0


def plane(id: int) -> dict:
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
            panel(200 + i,
                  label(300 + i, f"node {i}"),
                  knob(400 + i, label="amount", min=0.0, max=1.0, value=i / 8),
                  layout="col", x=x, y=y, w=300.0, h=220.0))
    return scroll(id, *boxes, content_w=PLANE_W, content_h=PLANE_H)


def vertical_list(id: int) -> dict:
    """The constrained case: a plain vertical scroll view.

    Same widget, two props. A ``col`` layout stacks the children down the
    content area; ``content_h`` makes that area taller than the pane, so the
    wheel has somewhere to scroll to.
    """
    rows = [toggle(500 + i, label=f"track {i + 1}", value=False) for i in range(20)]
    return scroll(id, *rows, layout="col", axis="y", zoom=False, content_h=900.0)


def horizontal_strip(id: int) -> dict:
    """The other constrained case: a horizontal strip (a timeline-ish ribbon)."""
    cells = [label(600 + i, f"bar {i + 1}", x=i * 90.0, y=0.0, w=80.0, h=60.0)
             for i in range(24)]
    return scroll(id, *cells, axis="x", zoom=False, content_h=70.0)


def workspace() -> dict:
    return window(
        panel(2,
              panel(5,
                    label(10, "free plane — drag to pan, wheel to zoom"),
                    button(13, label="reset view", w=120.0),
                    layout="row", h=26.0, margin=0),
              plane(20),
              layout="col", weight=3),
        panel(3,
              label(11, "vertical scroll view (axis=y, zoom off)", h=20.0),
              vertical_list(21),
              layout="col", weight=2),
        panel(4,
              label(12, "horizontal strip (axis=x, zoom off)", h=20.0),
              horizontal_strip(22),
              layout="col", h=110.0),
        title="Workspace", w=900, h=760, layout="col",
    )


def main():
    # No audio server: `boot` starts a host with no client leg and owns it,
    # stopping the process on exit.
    with GuiHost.boot() as gui:
        gui.define(1, workspace())
        print("drag and wheel over each pane; navigation events print here")
        print("('reset view' puts the plane back at the origin, zoom 1)")
        print("(close the window to end, or wait ~45 s)")

        deadline = time.monotonic() + 45.0
        closed = False

        while not closed and time.monotonic() < deadline:
            msg = gui.poll(timeout=0.1)
            if msg is None:
                continue
            addr, args = msg
            if addr == "/gui_closed":
                print(f"window {args[0]} closed")
                closed = True
            elif len(args) >= 4 and args[1] == "view":
                x, y, zoom = args[2], args[3], args[4]
                print(f"widget {args[0]}: view x={x:.1f} y={y:.1f} zoom={zoom:.2f}")
            elif args[0] == 13 and args[1:] == [1]:
                # The reset button, pressed: the view is state the script owns,
                # so it answers the event by putting the plane back — the same
                # three keys the gestures emit.
                gui.set(20, view_x=0.0, view_y=0.0, view_zoom=1.0)
                print("view reset to the origin, zoom 1")
            elif args[0] != 13:
                print(f"event from widget {args[0]}: {args[1:]}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
