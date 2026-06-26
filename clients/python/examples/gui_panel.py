#!/usr/bin/env python3
"""A scripted instrument panel: controls that round-trip values and events.

The G4 example. It builds a `window` of standard controls — knobs, sliders, a
number, a toggle, a button and a menu — sends it as one GuiDef, then both *drives*
a widget live with `/gui_set` and *listens* for the `/gui_event`s your
interactions emit (turn a knob, click the button) and the `/gui_closed` the host
sends when you close the window.

Needs a display and a Vulkan/Metal/DX12/GL adapter (the host opens a window).

Start the windowed host in one terminal (built from ``clients/gui``)::

    cd clients/gui && cargo run --bin clausters-gui -- -v

then, with the client importable (``pip install ./clients/python`` or
``PYTHONPATH=clients/python``)::

    python clients/python/examples/gui_panel.py

Interact with the window for a few seconds: every change prints here. Closing the
window prints a close event and ends the script early.
"""

import sys

from clausters.gui import GuiHost, button, knob, menu, number, panel, slider, toggle, window


def instrument() -> dict:
    """A filter panel: a row of knobs over a row of mixed controls."""
    return window(
        panel(2,
              knob(10, label="cutoff", min=20.0, max=20000.0, value=800.0),
              knob(11, label="res", min=0.0, max=1.0, value=0.3),
              number(12, label="gain", min=-24.0, max=24.0, value=0.0),
              layout="row"),
        panel(3,
              slider(20, label="mix", min=0.0, max=1.0, value=0.5),
              toggle(21, label="bypass", value=False),
              button(22, label="reset"),
              menu(23, ["sine", "saw", "square"], index=1, label="wave"),
              layout="row"),
        title="Filter", w=560, h=300, layout="col",
    )


def main():
    with GuiHost() as gui:  # 127.0.0.1:57210 by default
        gui.define(1, instrument())

        # Drive a widget live from the script (the /gui_set path): nudge the
        # cutoff knob a moment after the window opens.
        import time
        time.sleep(0.5)
        gui.set(10, value=2000.0)
        print("set cutoff to 2000; now interact with the window...")

        # Listen for interaction events for a while. /gui_event carries the
        # widget id and its new value; /gui_closed carries the window id.
        closed = False

        def on_event(addr, args):
            nonlocal closed
            if addr == "/gui_closed":
                print(f"window {args[0]} closed")
                closed = True
            else:
                print(f"event from widget {args[0]}: {args[1:]}")

        for _ in range(80):  # ~8 seconds, or until the window closes
            msg = gui.poll(timeout=0.1)
            if msg is not None:
                on_event(*msg)
            if closed:
                break


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
