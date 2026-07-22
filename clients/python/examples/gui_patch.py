#!/usr/bin/env python3
"""The patch canvas: free placement, navigation and selection on a `graph`.

The `graph` widget is the **directed** patcher view: boxes with inlets on the
top edge and outlets on the bottom, a cord per ``outlet -> inlet`` connection.
It is built from a `clausters.defs.GraphPatch` (`to_widget` renders the model
into the widget). This example shows its **canvas** behavior — no audio:

- **Free placement** — ``geometry`` places a box (canvas units); without a place
  a box auto-stacks down the left column. Here some boxes are placed, some not.
- **Dragging** — grab a box and move it; the edit flows back as
  ``/gui_event <id> "move" <index> <x> <y>`` and prints here. Moving a selected
  box moves the whole selection.
- **Selection** — click a box to select it; drag the empty canvas to sweep a
  marquee over several; click empty canvas to clear.
- **Cording** — drag an outlet's pin onto an inlet (either grab order) to draw a
  cord; a rate mismatch is refused at the gesture. The edit flows back as
  ``/gui_event <id> "wire" <src> <outlet> <dst> <inlet>`` and prints here.
- **Navigation** — the patch is a pan/zoom canvas inside a `scroll` workspace:
  **Shift+drag** the empty canvas to pan it, wheel zooms anchored at the cursor.
  Boxes, cords and text scale together.

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

from clausters.defs import GraphPatch
from clausters.gui import GuiHost, graph, label, scroll, window

PATCH = 30


def patch_window() -> dict:
    # A directed chain: osc -> filter -> verb -> dac (the terminal sink that
    # reaches the speakers itself — no OUT box, since a bus is never drawn). Built
    # as a model, then drawn — the same GraphPatch you would compile and send.
    p = GraphPatch()
    osc = p.add("osc", outlets=["out"])
    filt = p.add("filter", inlets=["in"], outlets=["out"])
    verb = p.add("verb", inlets=["in"], outlets=["out"])
    dac = p.add("dac", inlets=["in"])   # terminal: an inlet, no outlet
    p.connect(osc, "out", filt, "in")
    p.connect(filt, "out", verb, "in")
    p.connect(verb, "out", dac, "in")
    # Some boxes placed, the rest (verb, dac) left to the auto layout.
    geometry = {osc: (60.0, 40.0), filt: (60.0, 200.0)}

    the_patch = graph(
        PATCH, **p.to_widget(geometry), label="patch",
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
                index, x, y = args[2], args[3], args[4]
                print(f"moved box {index} to ({x:.0f}, {y:.0f})")
            elif len(args) >= 2 and args[1] == "wire":
                src, outlet, dst, inlet = args[2], args[3], args[4], args[5]
                print(f"corded {src}.{outlet} -> {dst}.{inlet}")
            elif len(args) >= 2 and args[1] == "view":
                print(f"view x={args[2]:.0f} y={args[3]:.0f} zoom={args[4]:.2f}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
