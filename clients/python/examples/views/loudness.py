#!/usr/bin/env python3
"""Loudness, drawn over the take it measures -- and over a bus, live.

A peak says how close a signal came to full scale; it says nothing about how
loud it **sounds**. Loudness does, and it is the number a delivery
specification asks for: EBU R 128 wants a programme at -23 LUFS. This example
draws it where it belongs -- **over the waveform**, on a scale of its own:

- ``measure="peak momentary"`` puts the loudness curve over the samples. It is
  what a meter would have read at each point of the take (the 400 ms window up
  to it), so a quiet passage reads low and a loud one high, with the transition
  taking the window's own length to happen (and the first 400 ms rising out
  of the silence before the take, which is the window filling, as it does on a
  meter just reset);
- the curve is read against the **target line** (-23 LUFS) and the **LU ruler**
  on the right of the body, because a loudness is not an amplitude and has no
  business on the amplitude axis the samples are drawn against;
- the **numbers** over the picture are the whole of R 128: the integrated
  loudness, the loudness range, the true peak and the ratio between the last
  two. Sweep a selection over either half of the take and they re-measure it,
  marked ``SEL``;
- the same ``measure`` over a **bus** is the live meter's own curve, fed from
  the bus as it plays. No extra API: the same word means the same thing over a
  take and over a signal that has not happened yet.

The take is two halves, three seconds each: a **stereo 1 kHz tone at -30 dBFS**
followed by the same tone at **-17 dBFS**, built at 8 kHz (a take carries its own
rate, and the example sends it over the wire every run). A stereo tone's peak level in dBFS
*is* its loudness in LUFS -- that is the calibration BS.1770's -0.691 exists for
-- so the curve should sit on -30 and then on -17, and the integrated figure
between them, nearer the loud half because the quiet one falls under the
relative gate.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server + GUI binaries

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/views/loudness.py``. It self-launches the audio
server and the GUI host; by hand that is ``clausters`` and ``clausters-gui
--server 127.0.0.1:57110``. Needs a display, a GPU adapter and an audio output:
the live half makes sound.
"""

# %%
import math
import sys
from array import array

from clausters import Session
from clausters.defs import Buffer, Synth, SynthDef, control, out, sine
from clausters.gui import scope, view, waveform

#: The take's own rate. It is **not** the server's: a take carries its rate with
#: it, and this one is built at 8 kHz because the example sends it over the wire
#: every run and a 1 kHz tone needs nothing faster. The measurement is the same
#: one -- BS.1770's filters are redesigned for whatever rate they are handed,
#: and its windows are seconds rather than samples.
RATE = 8_000.0

# %% [markdown]
# ## Launch the server and the GUI
# The host fetches the take over its client leg, so it needs `--server`, which
# `Session.gui()` wires up.

# %%
session = Session.live()
server = session.server
gui = session.gui()

# %% [markdown]
# ## The take: two halves, sixteen decibels apart
# A stereo 1 kHz tone at -30 dBFS for three seconds, then the same tone at
# -17 dBFS -- six LU over the target, so the curve sits inside its scale
# rather than on the ceiling. Both halves are longer than the short-term
# window, so either one can be selected and measured on its own.

# %%
def tone(seconds: float, db: float) -> list:
    """A stereo 1 kHz sine at `db` dBFS per channel, interleaved."""
    amp = 10 ** (db / 20)
    out_ = []
    for i in range(int(seconds * RATE)):
        v = amp * math.sin(2 * math.pi * 1000.0 * i / RATE)
        out_.extend((v, v))
    return out_


samples = array("f", tone(3.0, -30.0) + tone(3.0, -17.0))
take = Buffer.from_samples(samples, 2, server=server)
server.sync()

# %% [markdown]
# ## A tone whose level breathes, for the live half
# The gain is modulated by a slow sine, so the live curve rises and falls
# without anything scheduling it: what the meter reads is what the engine is
# playing.

# %%
voice = sine(control("freq", 440.0)) * (0.02 + 0.22 * (sine(0.12) * 0.5 + 0.5))
SynthDef("breathe", out(0.0, [voice, voice])).send(server)
server.sync()

# %% [markdown]
# ## The window: the take, and the bus
# Both lanes measure the same two things — the samples (`peak`) and the
# loudness over them (`momentary`). The stored lane draws the curve over the
# whole take; the live one draws the readings as they arrive.

# %%
win = view(
    waveform(buffer=take.bufnum, sample_rate=RATE, measure="peak momentary",
             label="the take: -30 dBFS, then -17 dBFS", ruler_y=True,
             overlay=True, loudness_stats=True),
    scope(0, channels=2, measure="peak momentary", window_ms=80.0,
          overlay=True, label="the bus, live: the meter's own curve"),
    title="Loudness: the curve over the samples", w=860, h=620, layout="col",
).open()
playing = Synth("breathe", {"freq": 440.0}, server=server)
print("the curve over the take sits on -30 and then on -17 LUFS, against the "
      "-23 target line; the numbers are the take's until you sweep a "
      "selection over one half. The lower lane is the same measure on the bus. "
      "Close the window to stop")

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        win.wait()
    finally:
        playing.free()
        session.close()
else:
    print("up - playing.free() and session.close() to end")
