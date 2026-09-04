#!/usr/bin/env python3
"""A shader canvas: custom visuals driven by an OSC param and a control bus.

A ``canvas`` widget runs a script-supplied WGSL shader over its area,
ShaderToy-style. The host gives the shader three uniforms -- ``u.time``,
``u.resolution`` and a ``u.params`` vec4 -- and the params are driven **two
ways**, which is the whole point of the widget:

- ``u.params.x`` from the **script**: ``handle.set(param0=...)`` sends an OSC
  value the host writes into the uniform;
- ``u.params.y`` from a **control bus**, read straight out of the audio server's
  **shared-memory segment** every frame (zero OSC), exactly the path the meters
  use. The ``buses=[..]`` argument maps a control bus onto a param slot.

So the same shader animates from an OSC parameter and from live server audio at
once -- which needs the host to map the server's segment, wired by `Session.gui`.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server + GUI binaries

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/panels/canvas.py``. It self-launches the audio
server (with a shared-memory segment) and the GUI host mapping it; by hand that
is ``clausters --shm <path>`` and ``clausters-gui --server 127.0.0.1:57110 --shm
<path>``. Run this with no server already up on 57110, so the session boots its
own. Needs a display and a GPU adapter.
"""

# %%
import math
import sys
import time

from clausters import Session
from clausters.defs import Bus
from clausters.gui import canvas, view

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

# %% [markdown]
# ## Launch the server and the GUI, and a bus for the shader
# `session.gui()` maps the server's shared-memory segment, so the canvas can read
# the control bus into `u.params.y` with no per-frame messages.

# %%
session = Session.live()
server = session.server
gui = session.gui()
bus = Bus.control(server=server)  # the bus the shader's green channel follows

# %% [markdown]
# ## The canvas
# One shader canvas: `param0` from the script, `param1` from the bus (the `-1`
# slot stays script-driven). Named, so `open` resolves it.

# %%
win = view(
    canvas(name="shader", shader=SHADER, buses=[-1, bus.index], label="shader"),
    title="Canvas (shader)", w=560, h=560).open()
print("an animated shader: its ring follows the OSC param, its green "
      "channel the control bus; close the window to stop")

# %% [markdown]
# ## Drive it
# Sweep the OSC param straight to the shader and write the control bus (which the
# host reads from shared memory into `params.y`).

# %%
def run(seconds: float | None = None) -> None:
    """Sweeps the OSC param and the control bus for ``seconds``.

    Script-run there is no bound and the window is what ends it; the
    ``seconds`` argument is for a cell run, where a notebook wants the loop to
    give the prompt back. Nothing here drains the host — the close arrives on
    its own event loop — so this only writes the two values.
    """
    start = time.monotonic()
    while not win.closed and (seconds is None or time.monotonic() - start < seconds):
        t = time.monotonic() - start
        win["shader"].set(param0=0.5 + 0.5 * math.sin(t * 0.7))
        bus.set(0.5 + 0.5 * math.cos(t * 1.3))
        time.sleep(0.03)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        session.close()
else:
    print("canvas up - run(10) to sweep the params, session.close() to end")
