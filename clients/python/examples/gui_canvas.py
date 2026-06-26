#!/usr/bin/env python3
"""A shader canvas: custom visuals driven by an OSC param and a control bus.

The G9 example. A ``canvas`` widget runs a script-supplied WGSL shader over its
area, ShaderToy-style. The host gives the shader three uniforms -- ``u.time``,
``u.resolution`` and a ``u.params`` vec4 -- and the params are driven **two
ways**, which is the whole point of the widget:

- ``u.params.x`` from the **script**: ``gui.set(id, param0=...)`` sends an OSC
  value the host writes into the uniform;
- ``u.params.y`` from a **control bus**, read straight out of the audio server's
  **shared-memory segment** every frame (zero OSC), exactly the path the meters
  use. The ``buses=[..]`` argument maps a control bus onto a param slot.

So the same shader animates from an OSC parameter and from live server audio at
once. Three processes cooperate, as in ``gui_meters.py``: the **audio server**
(holding the shared segment), the **GUI host** (which maps it with ``--shm``),
and this **script**.

Start the audio server with a shared segment (from the repo root)::

    cargo run -- --shm /dev/shm/clausters_g9

Start the windowed GUI host attached to that server and segment (from
``clients/gui``)::

    cargo run --bin clausters-gui -- --server 127.0.0.1:57110 --shm /dev/shm/clausters_g9 -v

Then, with the client importable (``pip install ./clients/python`` or
``PYTHONPATH=clients/python``)::

    python clients/python/examples/gui_canvas.py

A window opens with an animated shader: its ring pulse follows the OSC ``param0``
this script sweeps, and its green channel follows the control bus this script
writes (read by the host from shared memory). Close the window, or wait, to end.
Needs a display and a GPU adapter.
"""

import math
import sys
import time

from clausters import Session
from clausters.gui import GuiHost, canvas, window

SHADER = """
fn shade(uv: vec2<f32>, frag: vec4<f32>) -> vec4<f32> {
    let p = uv * 2.0 - vec2<f32>(1.0, 1.0);
    let d = length(p);
    // params.x is an OSC value from the script; params.y is a control bus read
    // out of shared memory each frame.
    let ring = sin(d * 14.0 - u.time * 3.0 + u.params.x * 6.2831);
    let r = 0.5 + 0.5 * ring;
    let g = 0.5 + 0.5 * u.params.y;
    let b = 0.5 + 0.5 * sin(u.time + uv.x * 3.0);
    let vignette = 1.0 - 0.5 * d;
    return vec4<f32>(r * vignette, g * vignette, b * vignette, 1.0);
}
"""


def scene(bus_index: int) -> dict:
    """A window with one shader canvas: param0 from the script, param1 from a bus."""
    return window(
        canvas(10, SHADER, buses=[-1, bus_index], label="shader"),
        title="Canvas (shader)", w=560, h=560,
    )


def main():
    with Session.live() as session:  # UDP to 127.0.0.1:57110
        server = session.server
        bus = server.control_bus()  # the bus the shader's green channel follows

        with GuiHost() as gui:  # 127.0.0.1:57210 by default
            gui.define(1, scene(bus.index))
            print("an animated shader: its ring follows the OSC param, its green "
                  "channel the control bus; close the window to stop")

            start = time.monotonic()
            while time.monotonic() - start < 30.0:
                t = time.monotonic() - start
                # An OSC param straight to the shader (no audio server involved).
                gui.set(10, param0=0.5 + 0.5 * math.sin(t * 0.7))
                # A control bus the host reads from shared memory into params.y.
                server.set_bus(bus, 0.5 + 0.5 * math.cos(t * 1.3))
                msg = gui.poll(timeout=0.03)
                if msg is not None and msg[0] == "/gui_closed":
                    print("window closed")
                    break


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
