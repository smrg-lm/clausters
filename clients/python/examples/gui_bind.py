#!/usr/bin/env python3
"""A bound knob drives a synth directly: the value bypasses the script.

The G6 example. It shows the low-latency interactive path: a knob *bound* to a
running synth's control (`GuiHost.bind`) sends its value **straight to the audio
server** on every turn, with no round-trip through this Python process. An
unbound knob would instead emit a ``/gui_event`` back here; binding swaps that
for a direct ``/n_set`` to the server.

Three processes cooperate, as in ``gui_meters.py``: the **audio server**, the
**GUI host** (which needs ``--server`` to reach the audio server), and this
**script**. Here the script only sets the scene — builds a one-knob window and a
sine synth, then binds the knob to the synth's ``freq``. After that, turning the
knob changes the pitch on the server itself; the script prints *nothing* while
the binding is live (proof the value never came back). Then it ``unbind``s and
the same knob starts emitting ``/gui_event`` again.

Start the audio server (built from the repo root)::

    cargo run

Start the windowed GUI host, attached to that server (from ``clients/gui``)::

    cargo run --bin clausters-gui -- --server 127.0.0.1:57110 -v

Then, with the client importable (``pip install ./clients/python`` or
``PYTHONPATH=clients/python``)::

    python clients/python/examples/gui_bind.py

A window opens with one knob over the audible range. While it is *bound* (the
first phase) turn it: the pitch follows it directly and nothing prints here.
After ~8 s the script unbinds; now turning the knob prints ``/gui_event`` lines
and the synth holds its last frequency. Close the window to stop.

Needs a display and a Vulkan/Metal/DX12/GL adapter (the host opens a window).
"""

import sys
import time

from clausters import Session
from clausters.defs import SynthDef, control, out, sin_osc
from clausters.gui import GuiHost, knob, window


def scene() -> dict:
    """A window with a single big knob over a musical frequency range."""
    return window(
        knob(10, label="freq", min=110.0, max=880.0, value=220.0),
        title="Bound knob -> synth freq", w=420, h=260, layout="col",
    )


def beep() -> SynthDef:
    """A quiet stereo sine whose frequency is the ``freq`` control (default
    220 Hz) — the binding target ``/n_set <node> freq <value>`` drives."""
    sig = sin_osc(freq=control("freq", 220.0)) * 0.2
    return SynthDef("gui_bind_beep", out(0.0, sig), out(1.0, sig))


def main():
    with Session.live() as session:  # UDP to 127.0.0.1:57110
        server = session.server
        server.add_synthdef(beep())  # blocks until /done
        synth = server.synth("gui_bind_beep", {"freq": 220.0})

        with GuiHost() as gui:  # 127.0.0.1:57210 by default
            gui.define(1, scene())
            # Bind knob 10 to the synth's freq: turning it sends
            # /n_set <synth.id> freq <value> straight to the audio server.
            gui.bind(10, "/n_set", synth.id, "freq")
            print(f"knob bound to synth {synth.id} freq; turn it — the pitch "
                  "follows directly and nothing prints here (no script round-trip)")

            # Phase 1: bound. Drain any stray messages just to show none are
            # /gui_events from the knob.
            start = time.monotonic()
            while time.monotonic() - start < 8.0:
                msg = gui.poll(timeout=0.1)
                if msg is not None and msg[0] == "/gui_closed":
                    print("window closed")
                    server.free(synth)
                    return

            # Phase 2: unbind. The knob now emits /gui_event to this script
            # again; the synth keeps its last frequency.
            gui.unbind(10)
            print("unbound — now the knob emits events here and stops driving "
                  "the synth; turn it and watch the lines:")
            start = time.monotonic()
            while time.monotonic() - start < 10.0:
                msg = gui.poll(timeout=0.1)
                if msg is None:
                    continue
                if msg[0] == "/gui_closed":
                    print("window closed")
                    break
                print(f"event from widget {msg[1][0]}: {msg[1][1:]}")

        server.free(synth)


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
