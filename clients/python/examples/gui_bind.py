#!/usr/bin/env python3
"""A bound knob drives a synth directly: the value bypasses the script.

The G6 example. It shows the low-latency interactive path: a knob *bound* to a
running synth's control (`GuiHost.bind`) sends its value **straight to the audio
server** on every turn, with no round-trip through this Python process. An
unbound knob would instead emit a ``/gui_event`` back here; binding swaps that
for a direct ``/n_set`` to the server.

The point of the binding is that it lives **in the GUI host, not in this
script**: ``/gui_bind`` registers ``knob 10 -> /n_set <node> freq`` inside the
host, and the host forwards every change to the audio server on its own. So the
control keeps working **after this script exits** — the binding (and the synth)
outlive the Python process, which is exactly the bypass-the-script promise. This
script only sets the scene; it deliberately leaves the knob bound and the synth
running when it returns, so you can keep turning the knob with no client at all.

Three processes cooperate, as in ``gui_meters.py``: the **audio server**, the
**GUI host** (which needs ``--server`` to reach the audio server), and this
**script**.

Start the audio server (built from the repo root)::

    cargo run

Start the windowed GUI host, attached to that server (from ``clients/gui``)::

    cargo run --bin clausters-gui -- --server 127.0.0.1:57110 -v

Then, with the client importable (``pip install ./clients/python`` or
``PYTHONPATH=clients/python``)::

    python clients/python/examples/gui_bind.py

A window opens with one knob over the audible range. Turn it: the pitch follows
directly and nothing prints here (proof the value never came back through
Python). After a short demo the script exits **without unbinding or freeing the
synth** — keep turning the knob and the pitch still follows, because the binding
runs in the host. Close the window when you are done; to silence the synth,
free it from another client or stop the audio server (``/quit``).

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
            # /n_set <synth.id> freq <value> straight to the audio server. The
            # binding now lives in the host, independent of this script.
            gui.bind(10, "/n_set", synth.id, "freq")
            print(f"knob bound to synth {synth.id} freq; turn it — the pitch "
                  "follows directly and nothing prints here (no script round-trip)")

            # Demo window: drain events just to show the bound knob sends none
            # back here, and bail out early if the window is closed.
            start = time.monotonic()
            while time.monotonic() - start < 12.0:
                msg = gui.poll(timeout=0.1)
                if msg is not None and msg[0] == "/gui_closed":
                    print("window closed — freeing the synth")
                    server.free(synth)
                    return

        # The script exits here, but it does NOT unbind or free the synth: the
        # binding keeps running in the host, so the knob still drives the pitch
        # on the server with no client at all. That is the whole point of
        # /gui_bind. Close the window or /quit the server to stop the sound.
        print(f"script exiting; knob 10 stays bound to synth {synth.id} freq — "
              "keep turning it, the host forwards the value to the server")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
