#!/usr/bin/env python3
"""A triggered audio-rate oscilloscope over a server audio tap.

The GUI's ``scope`` widget has two rates. Its control-rate form (see
``gui_meters.py``) plots a control bus's history one sample per frame; this
example uses the audio-rate form -- a real **oscilloscope** showing the actual
samples of a live signal, with a level trigger that holds the trace still.

The data path is the server's **audio taps**: pre-allocated sample rings inside
the shared-memory segment. ``Server.tap(tap, bus)`` routes an audio bus into a
ring; from then on the engine appends that bus's samples every block, and the
GUI host reads the newest window straight out of shared memory each frame --
zero per-frame OSC. (A browser host cannot map the segment; it subscribes
``/tap_stream`` instead and receives the same windows as ``/tap_data``
messages. ``Server.stream_taps`` exposes that path to Python too, for headless
capture of a live signal.)

Start the audio server with a shared segment (from the repo root)::

    cargo run -- --shm /dev/shm/clausters_tap

Start the windowed GUI host on the same segment (from ``clients/gui``)::

    cargo run --bin clausters-gui -- --shm /dev/shm/clausters_tap -v

Then, with the client importable (``pip install ./clients/python`` or
``PYTHONPATH=clients/python``)::

    python clients/python/examples/gui_scope.py

A window opens with two oscilloscopes on the same tap: a **triggered** one,
whose sine stays locked in place while its frequency sweeps (each redraw
aligns to a rising zero crossing), and a **free-running** one (trigger far
above the signal, so the alignment never fires) that shows why triggering
exists -- the same signal drifting through the window. Close the window, or
wait, to end.
"""

import math
import sys
import time

from clausters import Session
from clausters.defs import SynthDef, control, out, sine
from clausters.gui import GuiHost, panel, scope, window


def scene() -> dict:
    """Two audio-rate scopes reading tap 0: triggered vs free-running."""
    return window(
        panel(2,
              scope(10, tap=0, window_ms=15.0, trigger=0.0,
                    label="triggered (level 0.0)"),
              # A trigger level the signal never reaches: no rising crossing is
              # ever found, so the scope free-runs on the newest window.
              scope(11, tap=0, window_ms=15.0, trigger=9.0,
                    label="free-running"),
              layout="col"),
        title="Audio-rate oscilloscope", w=560, h=420,
    )


def main():
    with Session.live() as session:  # UDP to 127.0.0.1:57110
        server = session.server
        info = server.query_info()
        if info.taps == 0:
            sys.exit("this server has no tap region (started with --taps 0?)")

        # A sine on audio bus 0 (the hardware out), and bus 0 routed into
        # audio tap 0. The tap is what the oscilloscopes read.
        server.add_synthdef(SynthDef(
            "tone", out(0.0, sine(control("freq", 220.0)) * control("amp", 0.2))))
        synth = server.synth("tone", {"freq": 220.0})
        server.tap(0, 0)

        with GuiHost() as gui:  # 127.0.0.1:57210 by default
            gui.define(1, scene())
            print("the top trace stays locked while the pitch sweeps; "
                  "the bottom one drifts; close the window to stop")

            start = time.monotonic()
            while time.monotonic() - start < 20.0:
                # Sweep the frequency so the triggered trace visibly re-locks:
                # 220..440 Hz and back, once per 8 s.
                phase = (time.monotonic() - start) / 8.0
                freq = 330.0 + 110.0 * math.sin(2 * math.pi * phase)
                server.set(synth, {"freq": freq})
                msg = gui.poll(timeout=0.03)
                if msg is not None and msg[0] == "/gui_closed":
                    print("window closed")
                    break

        server.tap(0, -1)  # stop the tap; the ring goes quiet
        server.free(synth)


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
