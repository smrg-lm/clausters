#!/usr/bin/env python3
"""A meter read the way a meter is read: decibels, a held peak and a clip lamp.

The GUI's ``meter`` widget names a **bus** and the host does the rest, once per
frame, with no message at all. What makes it a meter rather than a bar is four
rules, and this example shows all four at once:

- **the peak is the block's**, not a sample of it -- the engine walks every
  sample of every block and publishes the peak, so a reader running at a
  screen's rate still catches a transient;
- **a peak is held**: the hairline across the column is the loudest reading,
  kept ``hold`` seconds and then falling at ``decay`` decibels per second;
- **the scale is decibels**, down to a floor the reader states. The first two
  meters here are the same two buses on two floors -- the 60 dB strip a mix is
  read on, and the 144 dB dynamic range of a 24-bit render (``bits=24``) --
  which is the clearest way to see that a floor is a *question*, not a setting;
  the third carries neither ladder nor numbers (``ruler=False``,
  ``readout=False``), which is what a strip of meters in a track header is;
- **an over is latched**: the lamp above each column lights when the signal is
  flattened and **stays lit until it is clicked**. What counts as an over is a
  run of consecutive samples at full scale, which only something walking samples
  can see -- so the `clip_count` UGen counts them onto a control bus and the
  widget differences the count.

The tone is **struck** every couple of seconds rather than swelled, because a
level that rises slowly shows the scale and none of the ballistics: the attack,
the hold and the fall are only legible against a transient. The hits are far
enough apart that the whole ballistic plays out between them: the column starts
down at once, the mark waits ``hold`` seconds and only then walks down at
``decay`` decibels a second. Every fourth hit goes past full scale, so the lamps
light without anybody touching anything --
**click a meter to clear them**, and watch them come back on the fourth hit.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server + GUI binaries

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/views/vumeter.py``. It self-launches the audio
server (with a shared-memory segment) and the GUI host mapping that same
segment; by hand that is ``clausters --shm <path>`` and ``clausters-gui --shm
<path>``. Run this with no server already up on 57110, so the session boots its
own. Needs a display, a GPU adapter and an audio output -- it makes sound.
"""

# %%
import sys
import time

from clausters import Session
from clausters.defs import (AddAction, Bus, Synth, SynthDef, clip_count,
                            control, in_, out, out_ctl, sine)
from clausters.gui import meter, panel, view

# %% [markdown]
# ## Launch the server and the GUI
# `Session.live()` boots the server with a shared-memory segment;
# `session.gui()` maps the same one, which is where the levels are read.

# %%
session = Session.live()
server = session.server
gui = session.gui()

# %% [markdown]
# ## A stereo tone, and a counter watching what it produces
# The tone writes buses 0 and 1 -- the hardware outputs -- so the meter reads
# the levels the engine publishes for them and needs nothing else. The counter
# is a second synth **at the tail**, so it reads the buses after the tone has
# written them, and it writes one control bus per channel: the running count of
# overs, which is what the widget differences.

# %%
SynthDef("tone", out(0.0, [sine(control("freq", 330.0)) * control("gain", 0.2),
                           sine(control("freq", 330.0) * 1.5) * control("gain", 0.2)])
         ).send(server)
SynthDef("overs",
         out_ctl(control("out", 0.0), clip_count(in_(0.0))),
         out_ctl(control("out", 0.0) + 1.0, clip_count(in_(1.0)))).send(server)
server.sync()

tone = Synth("tone", {"freq": 330.0}, server=server)
overs = Bus.control(2, server=server)
Synth("overs", {"out": overs.index}, action=AddAction.TAIL, server=server)

# %% [markdown]
# ## Three meters over the same two buses
# Same signal, same rules. The first two differ only in their floor -- the strip
# a mix is read on, and the dynamic range of a 24-bit render -- with the numbers
# on the outside of each pair (`ruler`). The third carries **neither ladder nor
# numbers**: that is a meter and not a degraded one, and it is what a strip of
# them down the edge of a track header is. The lamps sit over the columns in all
# three, and a click on any of them puts its own lamps out.

# %%
win = view(
panel(meter(0, channels=2, name="mix", label="mix"),
 meter(0, channels=2, bits=24, ruler="right", clip=overs.index,
       name="render", label="24 bits"),
 meter(0, channels=2, ruler=False, readout=False, clip=overs.index,
       name="bare", label="bare"),
 layout="row"),
title="A meter, read as a meter", w=460, h=460).open()
print("the tone ramps past full scale about 8 s in: watch the columns, the "
      "held peak and the lamps. Click a meter to clear its lamps; close the "
      "window to stop")

# %% [markdown]
# ## Drive it
# **Hits, not a swell.** A level that rises and falls slowly shows the scale and
# none of the ballistics: the attack, the hold and the fall are only visible
# against a transient. So the tone sits at a quiet bed level and is struck every
# couple of seconds -- the column jumps at once, the mark stays where it jumped
# for a second and a half, and then both walk down at twenty decibels a second.
# Every fourth hit goes past full scale, which is what the lamps are waiting for.

# %%
BED = 0.03           # the level between hits
HIT = 0.7            # a hit that uses its headroom
OVER = 1.35          # and one that does not have any left
# Long enough for the whole ballistic to play out between hits: the mark waits
# `hold` (1.5 s by default) and only then walks down at `decay` decibels a
# second, so a hit every two seconds is a mark that never finishes falling.
PERIOD = 3.5         # seconds between hits


def run(seconds: float | None = None) -> None:
    """Strikes the tone for ``seconds``.

    Script-run there is no bound and the window is what ends it; the
    ``seconds`` argument is for a cell run, where a notebook wants the loop to
    give the prompt back. Nothing here drains the host: the meters read the
    levels from shared memory themselves.
    """
    start = time.monotonic()
    hits = 0
    next_hit = start
    while not win.closed and (seconds is None or time.monotonic() - start < seconds):
        now = time.monotonic()
        if now >= next_hit:
            # The hit is one tick long: what the meter shows for the second and
            # a half after it is the ballistics, not the signal.
            tone.set({"gain": OVER if hits % 4 == 3 else HIT})
            time.sleep(0.04)
            tone.set({"gain": BED})
            hits += 1
            next_hit = now + PERIOD
        time.sleep(0.02)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        session.close()
else:
    print("meters up - run(20) to strike the tone, session.close() to end")
