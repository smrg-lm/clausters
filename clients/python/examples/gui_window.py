#!/usr/bin/env python3
"""Open a real window from one declarative GuiDef: a navigable waveform.

The "first pixels" example. It builds a ``window`` containing a ``label`` and the
heavy ``waveform`` view, fed a generated signal as a binary blob carried in the
same ``/gui_def`` message, and sends it to a running ``clausters-gui`` host. The
host opens an actual window and renders the waveform; the wheel zooms toward the
pointer, left-drag pans, ``R`` resets, ``Esc`` (or the close button) closes it.

Needs a display and a Vulkan/Metal/DX12/GL adapter (the host opens a window).

Start the windowed host in one terminal (built from ``clients/gui``)::

    cd clients/gui && cargo run --bin clausters-gui -- -v

then, with the client importable (``pip install ./clients/python`` or
``PYTHONPATH=clients/python``)::

    python clients/python/examples/gui_window.py

The window stays open after this script exits — the host owns it. Close it from
the window itself.

The signal here is kept small enough that the whole def (JSON + blob) fits one
UDP datagram (~64 KB); moving large buffers without re-sending them is a later
milestone (a shared/streamed bulk path).
"""

import math
import sys

from clausters.gui import GuiHost, label, samples_to_blob, waveform, window


def decaying_sine(n: int, cycles: float) -> list[float]:
    """A short signal with visible structure: a sine that decays across the
    buffer, so the waveform shows both the cycles and the envelope."""
    return [math.sin(2 * math.pi * cycles * i / n) * math.exp(-3.0 * i / n) for i in range(n)]


def main():
    # ~8000 f32 (~32 KB) keeps the def (JSON + blob) inside one UDP datagram.
    signal = decaying_sine(8_000, cycles=120.0)
    blob = samples_to_blob(signal)

    # The waveform reads blob index 0; the blob rides beside the JSON in the
    # same /gui_def message.
    tree = window(
        label(20, "Decaying sine (wheel: zoom, drag: pan, R: reset)"),
        waveform(12, blob=0),
        title="clausters-gui - waveform", w=720, h=360, layout="col",
    )

    with GuiHost() as gui:  # 127.0.0.1:57210 by default
        gui.define(1, tree, blob)
        print("sent the window def; the host opened a window - close it when done")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
