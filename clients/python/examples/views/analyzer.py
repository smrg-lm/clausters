#!/usr/bin/env python3
"""A phasescope, a live spectrum and a retained waterfall over server audio buses.

Three of the GUI's analysis views, all reading **audio buses** straight from the
shared-memory segment (the same path the oscilloscope uses, ``scope.py``):

- a ``phasescope`` (goniometer) reads a **stereo pair** of buses and draws them
  as the 45°-rotated Lissajous figure -- mono reads as a vertical line,
  anti-phase as horizontal, a wide field fills the lozenge -- with a running
  correlation read-out (Pearson's r) beneath;
- a ``spectrum`` (spectroscope) reads one bus and draws one forward FFT per
  frame as a magnitude curve on a log frequency axis, with per-bin averaging
  and a decaying peak-hold so it does not flicker. This one is **navigable**,
  which over a spectrum means its *frequency* axis: drag it to pan, wheel over
  it to zoom under the cursor, ``R`` to see all of it again. That axis needs no
  retention behind it -- unlike a time axis, every bin is there every frame --
  so the window is one the view carries alone, normalized over
  ``[0, Nyquist]``, settable from the script and reported back as ``view_x``;
- a **waterfall** -- the spectrogram presentation over the same live bus, with a
  ``retention`` span. That prop is the whole point of the third view: a
  forward-only source has no addressable past, so ``navigable`` over one is a
  combination that would parse and do nothing. Retention supplies the past --
  the host keeps that many seconds of the bus and analyzes them into columns as
  they arrive -- and the time axis becomes navigable, zoomable and pannable
  exactly like a spectrogram computed from a file. The span is a policy of the
  axis and is settable live, which the script does halfway through.

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
``python clients/python/examples/views/analyzer.py``. It self-launches everything
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
from clausters.gui import panel, phasescope, signal, spectrum, view
from clausters.defs import Synth

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
    # The sweep below writes these ~30 times a second, and a control is applied
    # in steps at block boundaries: unlagged, the image jumps rather than
    # sweeps, and a stepped `freq` breaks the sine's phase, which the
    # goniometer shows as the figure snapping. `lag` is the server's own
    # smoother -- it inserts a `Lag` for a lagged control -- so what the views
    # analyze is the continuous signal the script means, not the staircase the
    # messages arrive as.
    freq = control("freq", 220.0, lag=0.05)
    amp = control("amp", 0.2)
    left = sine(freq)
    side = sine(freq * 1.5)  # a fifth up: decorrelated from `left`
    right = left * control("wm", 1.0, lag=0.05) + side * control("ws", 0.0, lag=0.05)
    return SynthDef(name, out(0.0, left * amp), out(1.0, right * amp))


stereo_def().send(server)
synth = Synth("stereo_image", {"freq": 220.0}, server=server)

# %% [markdown]
# ## The two analysis views
# A phasescope on the output pair beside a spectrum on the left bus. Both are
# *named*, not numbered -- `open` hands back a handle that resolves the names.

# %%
# The seconds of the bus the host keeps. A forward-only source has no
# addressable past -- there is nothing behind the newest window to zoom out to
# -- so a retention span is what makes `navigable` mean something over one: the
# waterfall below is a spectrogram of the last RETAIN seconds, zoomable and
# pannable exactly like one computed from a file.
RETAIN = 8.0

win = view(
# The labels are sized for the boxes they sit in: these two share the
# window's width, so a caption that reads well across a whole row comes
# back clipped with an ellipsis here -- the host truncates a line that does
# not fit rather than bleeding it into the neighbour. The waterfall below
# has the full width and can afford a longer one.
panel(phasescope(0, name="gonio", window_ms=30.0,
            label="goniometer (L/R)"),
 spectrum(0, name="spectrum", fft_size=2048, log_freq=True,
          peak_hold=True, navigable=True,
          label="spectrum (left, log Hz)"),
 layout="row", h=200),
signal(view="spectrogram", bus=0, retention=RETAIN, navigable=True,
  name="waterfall", window_size=1024, freq_scale="log",
  label="waterfall (left tap, %.0f s retained)" % RETAIN,
  axes={"x": {"ruler": "time"}, "y": {"ruler": "hz"}}),
title="Phasescope + live spectrum + waterfall", w=760, h=640, layout="col").open()
win.on_closed(lambda: globals().__setitem__("_closed", True))


def on_spectrum(tag, *vals):
    """The frequency window, reported as it is navigated -- named in hertz,
    since the normalized pair is a display coordinate and the reader is
    looking at a frequency axis."""
    if tag == "view_x" and len(vals) >= 2:
        # `query_info` rather than the launch options: it is the one spelling both
        # clients have, so this file and its page twin ask the same question.
        nyquist = server.query_info().nominal_sample_rate / 2.0
        lo = 20.0 * (nyquist / 20.0) ** vals[0]
        hi = 20.0 * (nyquist / 20.0) ** min(vals[0] + vals[1], 1.0)
        print(f"  spectrum: {lo:.0f} Hz .. {hi:.0f} Hz")


win["spectrum"].on_event(on_spectrum)
print("goniometer: vertical=mono (r=+1), lozenge=wide (r~0), "
      "horizontal=anti-phase (r=-1)")
print("spectrum: drag the curve to pan its frequency axis, wheel to zoom under "
      "the cursor, R to see the whole axis again")
print(f"waterfall: the last {RETAIN:.0f} s of the same bus, and because the "
      "host retains them the axis is navigable -- wheel to zoom the time axis, "
      "drag to pan, as on a file; close the window to stop")

# %% [markdown]
# ## Drive it
# Sweep the stereo image over 6 s and drift the pitch slowly, so the spectrum
# peak visibly moves along the log axis. The views need nothing from this loop --
# the host reads the taps from the segment on its own, and the close arrives on
# the host's own event loop; the loop here only sweeps the synth.

# %%
def run(seconds: float | None = None) -> None:
    """Sweeps the stereo image for ``seconds``, narrating the three landmarks.

    Script-run there is no bound and the window is what ends it; the
    ``seconds`` argument is for a cell run, where a notebook wants the loop to
    give the prompt back.
    """
    start = time.monotonic()
    regime = None
    while not win.closed and (seconds is None or time.monotonic() - start < seconds):
        t = time.monotonic() - start
        theta = math.pi * (0.5 - 0.5 * math.cos(2 * math.pi * t / 6.0))
        freq = 220.0 * (2.0 ** (0.5 * math.sin(2 * math.pi * t / 11.0)))
        synth.set({"wm": math.cos(theta), "ws": math.sin(theta), "freq": freq})
        deg = math.degrees(theta)
        now = ("mono" if deg < 30 else "anti-phase" if deg > 150
               else "wide" if 60 < deg < 120 else regime)
        if now != regime:
            regime = now
            print(f"  {regime}")
        time.sleep(0.03)


def focus(start: float, length: float) -> None:
    """Points the spectrum's frequency window at a slice of its axis, in
    normalized display units (``0, 1`` = all of it) -- the same window the
    pointer moves, set the way any other prop is set. The gesture and the
    script write one window, so navigating by hand from here just continues
    from where this left it."""
    win["spectrum"].set(view_start=start, view_len=length)
    print(f"  spectrum window -> {start:.2f} +{length:.2f}")


def retain(seconds: float) -> None:
    """Resizes the retained span live -- the axis's own policy, set the way
    any other prop is set. Narrowing it drops the oldest history at once
    rather than when the ring next fills, so the waterfall's time axis
    visibly shortens under the picture."""
    win["waterfall"].set(retention=seconds,
                         label=f"waterfall (left tap, {seconds:.0f} s retained)")
    print(f"  retention -> {seconds:.0f} s")


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run(15.0)
        # The span is a live prop: halve it, watch the waterfall's time axis
        # shorten under the picture, then put it back.
        retain(RETAIN / 2)
        run(8.0)
        retain(RETAIN)
        # ...and so is the spectrum's frequency window: zoom into the upper
        # half of the axis, where the swept fifth lives, then release it.
        focus(0.5, 0.5)
        run(8.0)
        focus(0.0, 1.0)
        run()
    finally:
        session.close()
else:
    print("analyzer up - run(10) to sweep the image, retain(4) to narrow the "
          "waterfall's span, focus(0.5, 0.5) to zoom the spectrum's frequency "
          "axis, session.close() to end")
