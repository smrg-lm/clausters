#!/usr/bin/env python3
"""A phasescope and a live spectrum over server audio taps.

Two of the GUI's analysis views, both fed by the server's **audio taps** (the
same path the oscilloscope uses, ``gui_scope.py``):

- a ``phasescope`` (goniometer) reads a **stereo pair** of taps and draws them
  as the 45°-rotated Lissajous figure -- mono reads as a vertical line,
  anti-phase as horizontal, a wide field fills the lozenge -- with a running
  correlation read-out (Pearson's r) beneath;
- a ``spectrum`` (spectroscope) reads one tap and draws one forward FFT per
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

Start the audio server with a shared segment (from the repo root)::

    cargo run -- --shm /dev/shm/clausters_tap

Start the windowed GUI host on the same segment (from ``clients/gui``)::

    cargo run --bin clausters-gui -- --shm /dev/shm/clausters_tap -v

Then (client importable via ``pip install ./clients/python`` or
``PYTHONPATH=clients/python``)::

    python clients/python/examples/gui_analyzer.py
"""

import math
import sys
import time

from clausters import Session
from clausters.defs import SynthDef, control, out, sin_osc
from clausters.gui import GuiHost, panel, phasescope, spectrum, window


def stereo_def() -> SynthDef:
    """A sine on bus 0 (left) plus a rotated partner on bus 1 (right).

    ``wm``/``ws`` are the mono/side weights the script sets to ``cos(theta)`` /
    ``sin(theta)``; ``side`` is a detuned (perfect-fifth) tone, decorrelated from
    the left channel, so the right sweeps mono -> wide -> anti-phase as theta
    goes 0 -> 90 -> 180 degrees."""
    freq = control("freq", 220.0)
    amp = control("amp", 0.2)
    left = sin_osc(freq)
    side = sin_osc(freq * 1.5)  # a fifth up: decorrelated from `left`
    right = left * control("wm", 1.0) + side * control("ws", 0.0)
    return SynthDef("stereo_image", out(0.0, left * amp), out(1.0, right * amp))


def scene() -> dict:
    """A phasescope on taps 0/1 beside a spectrum on tap 0."""
    return window(
        panel(2,
              phasescope(10, tap=0, tap2=1, window_ms=30.0,
                         label="goniometer (taps 0/1)"),
              spectrum(11, tap=0, fft_size=2048, log_freq=True,
                       peak_hold=True, label="spectrum (tap 0, log Hz)"),
              layout="row"),
        title="Phasescope + live spectrum", w=760, h=420,
    )


def main():
    with Session.live() as session:  # UDP to 127.0.0.1:57110
        server = session.server
        if server.query_info().taps < 2:
            sys.exit("this server needs >= 2 taps (started with --taps 0/1?)")

        server.add_synthdef(stereo_def())
        synth = server.synth("stereo_image", {"freq": 220.0})
        server.tap(0, 0)  # left  bus 0 -> tap 0
        server.tap(1, 1)  # right bus 1 -> tap 1

        with GuiHost() as gui:  # 127.0.0.1:57210 by default
            gui.define(1, scene())
            print("goniometer: vertical=mono (r=+1), lozenge=wide (r~0), "
                  "horizontal=anti-phase (r=-1); close the window to stop")

            start = time.monotonic()
            regime = None
            while time.monotonic() - start < 30.0:
                t = time.monotonic() - start
                # Sweep the stereo image over 6 s; drift the pitch slowly so the
                # spectrum peak visibly moves along the log axis.
                theta = math.pi * (0.5 - 0.5 * math.cos(2 * math.pi * t / 6.0))
                freq = 220.0 * (2.0 ** (0.5 * math.sin(2 * math.pi * t / 11.0)))
                server.set(synth, {
                    "wm": math.cos(theta), "ws": math.sin(theta), "freq": freq,
                })
                # Narrate the regime as it crosses the three landmarks.
                deg = math.degrees(theta)
                now = ("mono" if deg < 30 else "anti-phase" if deg > 150
                       else "wide" if 60 < deg < 120 else None)
                if now is not None and now != regime:
                    regime = now
                    print(f"  {regime}")
                msg = gui.poll(timeout=0.03)
                if msg is not None and msg[0] == "/gui_closed":
                    print("window closed")
                    break

        server.tap(0, -1)
        server.tap(1, -1)
        server.free(synth)


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
