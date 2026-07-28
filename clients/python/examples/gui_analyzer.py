#!/usr/bin/env python3
"""A phasescope and a live spectrum over server audio buses.

Two of the GUI's analysis views, both reading **audio buses** straight from the
shared-memory segment (the same path the oscilloscope uses, ``gui_scope.py``):

- a ``phasescope`` (goniometer) reads a **stereo pair** of buses and draws them
  as the 45°-rotated Lissajous figure -- mono reads as a vertical line,
  anti-phase as horizontal, a wide field fills the lozenge -- with a running
  correlation read-out (Pearson's r) beneath;
- a ``spectrum`` (spectroscope) reads one bus and draws one forward FFT per
  frame as a magnitude curve on a log frequency axis, with per-bin averaging
  and a decaying peak-hold so it does not flicker.

The source is a sine whose **stereo image** is swept from the script: the left
channel is a fixed sine, the right is a rotation ``cos(theta)*left +
sin(theta)*detuned`` -- at ``theta = 0`` the two channels are identical (mono,
r = +1), at ``theta = 90 deg`` the right is a decorrelated detuned tone (wide,
r ~ 0), at ``theta = 180 deg`` the right is the left inverted (anti-phase,
r = -1). The base pitch also drifts, so the spectrum's peak moves along the log
axis. Watch the goniometer collapse to a vertical line, open into a lozenge,
then fall to a horizontal line as the correlation swings +1 -> 0 -> -1.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server + GUI binaries

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/gui_analyzer.py``. It self-launches everything
the analysis needs -- which is exactly the wiring you would otherwise do by
hand: `Session.live` boots the audio server with a shared-memory segment
(``shm="auto"``), and `Session.gui` boots the windowed host
mapping that **same** segment, so the two views read the buses straight from
shared memory with no per-frame messages. (By hand that is ``clausters --shm
<path>`` and ``clausters-gui --shm <path>``; run this with no server already up
on 57110, so the session boots its own.) Needs a display and a GPU adapter.
"""

# %%
import math
import sys
import time

from clausters import Session
from clausters.defs import SynthDef, control, out, sine
from clausters.gui import panel, phasescope, spectrum, window

# %% [markdown]
# ## Launch the server and the GUI
# `Session.live()` boots the server with a shared-memory segment (`shm="auto"`)
# and `session.gui()` maps the same segment -- the phasescope
# and spectrum read the buses straight from it.

# %%
session = Session.live()
server = session.server
gui = session.gui()

# %% [markdown]
# ## A sine whose stereo image the script sweeps
# `wm`/`ws` are the mono/side weights the script sets to `cos(theta)` /
# `sin(theta)`; `side` is a detuned (perfect-fifth) tone, decorrelated from the
# left channel, so the right sweeps mono -> wide -> anti-phase as theta goes
# 0 -> 90 -> 180 degrees.

# %%
def stereo_def(name: str = "stereo_image") -> SynthDef:
    freq = control("freq", 220.0)
    amp = control("amp", 0.2)
    left = sine(freq)
    side = sine(freq * 1.5)  # a fifth up: decorrelated from `left`
    right = left * control("wm", 1.0) + side * control("ws", 0.0)
    return SynthDef(name, out(0.0, left * amp), out(1.0, right * amp))


server.add_synthdef(stereo_def())
synth = server.synth("stereo_image", {"freq": 220.0})

# %% [markdown]
# ## The two analysis views
# A phasescope on the output pair beside a spectrum on the left bus. Both are
# *named*, not numbered -- `open` hands back a handle that resolves the names.

# %%
win = gui.open(window(
    panel(phasescope(0, name="gonio", window_ms=30.0,
                     label="goniometer (stereo pair)"),
          spectrum(0, name="spectrum", fft_size=2048, log_freq=True,
                   peak_hold=True, label="spectrum (left tap, log Hz)"),
          layout="row"),
    title="Phasescope + live spectrum", w=760, h=420))
win.on_closed(lambda: globals().__setitem__("_closed", True))
print("goniometer: vertical=mono (r=+1), lozenge=wide (r~0), "
      "horizontal=anti-phase (r=-1); close the window to stop")

# %% [markdown]
# ## Drive it
# Sweep the stereo image over 6 s and drift the pitch slowly, so the spectrum
# peak visibly moves along the log axis. The views need nothing from this loop --
# the host reads the taps from the segment on its own; the loop only sweeps the
# synth and pumps events so the close is seen.

# %%
_closed = False


def run(seconds: float) -> None:
    """Sweeps the stereo image for ``seconds``, narrating the three landmarks."""
    start = time.monotonic()
    regime = None
    while time.monotonic() - start < seconds and not _closed:
        t = time.monotonic() - start
        theta = math.pi * (0.5 - 0.5 * math.cos(2 * math.pi * t / 6.0))
        freq = 220.0 * (2.0 ** (0.5 * math.sin(2 * math.pi * t / 11.0)))
        server.set(synth, {"wm": math.cos(theta), "ws": math.sin(theta), "freq": freq})
        deg = math.degrees(theta)
        now = ("mono" if deg < 30 else "anti-phase" if deg > 150
               else "wide" if 60 < deg < 120 else regime)
        if now != regime:
            regime = now
            print(f"  {regime}")
        gui.pump(timeout=0.03)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run(30.0)
    finally:
        session.close()
else:
    print("analyzer up - run(10) to sweep the image, session.close() to end")
