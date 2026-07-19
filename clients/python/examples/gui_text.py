#!/usr/bin/env python3
"""Text on the light widgets: ``text_size``, ``wrap`` and ``align``.

Every text-bearing light widget — ``label``, ``button``, ``toggle``, ``text``,
``number``, ``menu`` and the control labels on ``slider``/``knob`` — takes a
``text_size``: a glyph scale over the host's embedded 5x7 bitmap font, whose
default 2.0 is exactly the size everything drew at before the prop existed.
``label`` additionally takes:

- ``wrap=True`` — word wrap on the font's fixed advance (a cheap width
  computation, no shaping); lines past the label's bottom edge are dropped;
- ``align`` — ``"start"`` (the default left edge), ``"center"`` or ``"end"``,
  applied per line.

Single-line text that overflows its rect — a long label on a narrow control, a
value read-out wider than a knob — clips with an ellipsis instead of bleeding
into its neighbor.

The example opens one window showing all of it side by side, then exercises the
props live over ``/gui_set`` (a growing title, a re-aligned paragraph). It
**launches its own GUI host** (`GuiHost.boot`) and needs no audio server: text
is pure drawing. Needs a display and a Vulkan/Metal/DX12/GL adapter.

Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python   # bundles the server + GUI binaries

then::

    python clients/python/examples/gui_text.py
"""

import sys
import time

from clausters.gui import (GuiHost, button, knob, label, menu, panel, slider,
                           toggle, window)

LOREM = ("a wrapped label lays its words out on the font's fixed advance, "
         "drops the lines that overflow its rect, and aligns each line "
         "start, center or end")


def sizes(id: int) -> dict:
    """The same label at growing ``text_size`` — 1.0 up to 4.0."""
    steps = [1.0, 1.5, 2.0, 3.0, 4.0]
    rows = [label(id + 1 + i, f"text size {s}", text_size=s) for i, s in enumerate(steps)]
    return panel(id, *rows, layout="col")


def alignments(id: int) -> dict:
    """One wrapped paragraph per alignment, side by side."""
    cols = [label(id + 1 + i, LOREM, wrap=True, align=a)
            for i, a in enumerate(("start", "center", "end"))]
    return panel(id, *cols, layout="row")


def controls(id: int) -> dict:
    """The controls at two text sizes, with labels long enough to clip."""
    return panel(
        id,
        slider(id + 1, label="a deliberately long slider label", value=0.4),
        knob(id + 2, label="cutoff", min=20.0, max=20000.0, value=800.0, text_size=3.0),
        button(id + 3, label="a very wordy button face"),
        toggle(id + 4, label="toggle at 3x", text_size=3.0),
        menu(id + 5, ["sine", "sawtooth", "square"], label="wave", text_size=3.0),
        layout="row",
    )


def text_window() -> dict:
    return window(
        label(10, "title", text_size=3.0, align="center", h=40.0),
        sizes(20),
        alignments(30),
        controls(40),
        title="Text", w=980, h=680, layout="col",
    )


def main():
    with GuiHost.boot() as gui:
        gui.define(1, text_window())
        print("one window: sizes, wrapped alignments, and clipped controls")
        print("(close the window to end, or wait ~30 s)")

        # The props are live: retitle bigger, then re-align the middle
        # paragraph — the same keys the GuiDef carried.
        changes = [
            (3.0, ("title", ), {"text": "TEXT_SIZE IS LIVE", "text_size": 4.0}),
            (6.0, ("align", ), {"align": "start"}),
            (9.0, ("align", ), {"align": "end"}),
        ]
        deadline = time.monotonic() + 30.0
        t0 = time.monotonic()
        pending = list(changes)
        while time.monotonic() < deadline:
            msg = gui.poll(timeout=0.1)
            if msg is not None and msg[0] == "/gui_closed":
                print(f"window {msg[1][0]} closed")
                break
            while pending and time.monotonic() - t0 > pending[0][0]:
                _, what, props = pending.pop(0)
                target = 10 if what[0] == "title" else 32  # the centered paragraph
                gui.set(target, **props)
                print(f"set {props} on widget {target}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
