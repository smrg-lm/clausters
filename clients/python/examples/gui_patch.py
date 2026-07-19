#!/usr/bin/env python3
"""The patch canvas: free placement, navigation and selection on a `graph`.

The `graph` widget is the patcher view of a bus-wired node graph — member
boxes, bus nodes, a wire per connection. This example shows its **canvas**
behavior:

- **Free placement** — a member or bus may carry ``x``/``y`` (canvas units);
  without them it auto-places in the classic stacked columns. Here half the
  patch is placed, half is left to the auto layout.
- **Dragging** — grab a box and move it; the edit flows back as
  ``/gui_event <id> "move" <kind> <index> <x> <y>`` and prints here. Moving a
  selected box moves the whole selection.
- **Selection** — click a box to select it; drag the empty canvas to sweep a
  marquee over several; click empty canvas to clear.
- **Rewiring** — drag a port (the square on a box's right edge) onto a bus to
  rewire that control, onto empty space to unwire (``"wire"`` prints here).
- **Navigation** — the patch sits inside a `scroll` workspace: drag the space
  around it to pan, wheel to zoom anchored at the cursor. Boxes, wires and
  text scale together.

The example **launches its own GUI host** and needs no audio server. Needs a
display and a Vulkan/Metal/DX12/GL adapter.

Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python   # bundles the server + GUI binaries

then::

    python clients/python/examples/gui_patch.py
"""

import sys
import time

from clausters.gui import GuiHost, graph, label, scroll, window

PATCH = 30


def patch_window() -> dict:
    the_patch = graph(
        PATCH,
        members=[
            ("osc", ["out"], 60.0, 60.0),        # placed
            ("filter", ["in", "out"], 60.0, 220.0),
            ("verb", ["in", "out"]),             # auto-placed (left column)
        ],
        buses=[("raw", 340.0, 90.0), ("wet", 340.0, 250.0), "OUT"],
        wires=[(0, "out", "raw"), (1, "in", "raw"), (1, "out", "wet"),
               (2, "in", "wet"), (2, "out", "OUT")],
        label="patch",
        x=0.0, y=0.0, w=700.0, h=500.0,
    )
    return window(
        label(10, "drag boxes; marquee on empty canvas; drag the outer space to pan, wheel to zoom",
              h=22.0),
        scroll(20, the_patch, content_w=1200.0, content_h=900.0),
        title="Patch", w=900, h=620, layout="col",
    )


def main():
    with GuiHost.boot() as gui:
        gui.define(1, patch_window())
        print("every move / wire / view event prints here (close or wait ~60 s)")
        deadline = time.monotonic() + 60.0
        while time.monotonic() < deadline:
            msg = gui.poll(timeout=0.1)
            if msg is None:
                continue
            addr, args = msg
            if addr == "/gui_closed":
                print(f"window {args[0]} closed")
                break
            if len(args) >= 2 and args[1] == "move":
                kind, index, x, y = args[2], args[3], args[4], args[5]
                print(f"moved {kind} {index} to ({x:.0f}, {y:.0f})")
            elif len(args) >= 2 and args[1] == "wire":
                member, control, bus = args[2], args[3], args[4]
                print(f"wired member {member} '{control}' -> '{bus or '(unwired)'}'")
            elif len(args) >= 2 and args[1] == "view":
                print(f"view x={args[2]:.0f} y={args[3]:.0f} zoom={args[4]:.2f}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
