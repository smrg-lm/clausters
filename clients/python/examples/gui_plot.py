#!/usr/bin/env python3
"""Plotting an NRT render's output: the static ``plot`` view.

The second half of the G8 pair of read-only views. A ``plot`` is the lightweight
counterpart of the heavy ``waveform``: it draws a signal once (a line, or a
min/max envelope when there are more samples than pixels) with no zoom or pan --
"a simple static plot of an NRT-generated signal/file". Here the signal is
produced **offline**, by the bundled NRT renderer, with no server and no audio
device, then handed to the GUI host as a **mapped local file** (the G7 bulk
path: the samples never ride OSC).

So only **two** processes are involved -- the **GUI host** and this **script**;
no audio server is needed, because the audio was already rendered offline.

Build the embed library once (the renderer)::

    cargo build --release --features embed,realtime

Start the windowed GUI host (from ``clients/gui``); no ``--server`` needed::

    cargo run --bin clausters-gui -- -v

Then, with the client importable (``pip install ./clients/python`` or
``PYTHONPATH=clients/python``)::

    python clients/python/examples/gui_plot.py

A window opens plotting the rendered arpeggio. Close it to stop. Needs a display
and a GPU adapter.
"""

import os
import sys
import tempfile
import time

from clausters import Session
from clausters.gui import GuiHost, plot, samples_to_file, window
from clausters.seq import Pbind, Pseq, Pwhite

SR = 48_000.0


def phrase() -> Pbind:
    """A one-bar arpeggio walking a major scale, with a little amplitude jitter."""
    return Pbind(degree=Pseq([0, 2, 4, 7, 4, 2], repeats=2), dur=0.25,
                 amp=Pwhite(0.1, 0.2))


def render_mono() -> list:
    """Renders the phrase offline and returns channel 0 as a flat list of f32."""
    session = Session.nrt(tempo=2.0)
    session.play(phrase())
    samples, frames = session.render(sample_rate=SR, channels=2)
    print(f"rendered {frames} frames ({frames / SR:.2f} s) offline, no server")
    return list(samples[0::2])  # de-interleave channel 0 (stereo render)


def scene(path: str) -> dict:
    """A window with a single plot fed the rendered file (mapped, no OSC)."""
    return window(
        plot(10, path=path, min=-1.0, max=1.0, label="NRT render (mono)"),
        title="Plot of an NRT render", w=720, h=300,
    )


def main():
    fd, path = tempfile.mkstemp(prefix="clausters_plot_", suffix=".f32")
    os.close(fd)
    try:
        samples_to_file(render_mono(), path)
        print(f"wrote {os.path.getsize(path)} B of raw f32; the host maps it (no OSC)")
        with GuiHost() as gui:  # 127.0.0.1:57210 by default
            gui.define(1, scene(path))
            print("a window plots the rendered signal; close it to stop")
            start = time.monotonic()
            while time.monotonic() - start < 30.0:
                msg = gui.poll(timeout=0.1)
                if msg is not None and msg[0] == "/gui_closed":
                    print("window closed")
                    break
    finally:
        if os.path.exists(path):
            os.remove(path)


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError) as e:
        sys.exit(str(e))
