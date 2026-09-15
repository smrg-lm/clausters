#!/usr/bin/env python3
"""Loudness: how loud a render sounds, measured the way broadcast measures it.

A peak says how close a signal came to full scale; it says nothing about how
loud it *sounds*. **Loudness** does, and it is the number a delivery
specification asks for: EBU R 128 wants a programme at **-23 LUFS** with its
true peak under **-1 dBTP**. This example renders a short piece offline and
measures it through the shared core, with the algorithms ITU-R BS.1770 and the
EBU define:

- the **integrated** loudness -- the whole take, gated so that silence and the
  quiet passages do not pull it down;
- the **loudness range** -- how far the 3 s loudness spreads, in LU;
- the loudest **momentary** (400 ms) and **short-term** (3 s) readings;
- the **true peak** of each channel, beside them, since R 128 asks for both.

The piece is a verse and a chorus of one arpeggio, eight seconds each, the
chorus about 20 dB louder than the verse. Read the numbers against that: the
integrated loudness sits near the chorus, because the verse falls under the
relative gate; the range is the distance between the two; and the difference
between the integrated loudness and -23 is the gain R 128 would ask for --
reported, not applied, since measuring is not editing.

It runs with the *installed* package and needs no server, no audio device and no
display: the render is the bundled embed renderer's::

    python -m venv .venv && . .venv/bin/activate
    pip install ./clients/python
    python clients/python/examples/buffers/loudness.py

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention):
step through it with Shift+Enter, change the chorus's ``amp`` in one cell and
measure again in the next.
"""

# %%
import math
import sys

from clausters import Session, ipc
from clausters.base import Routine
from clausters.seq import Pbind, Pn, Pseq

SR = 48000.0

#: EBU R 128's programme loudness and its true-peak ceiling.
TARGET_LUFS = -23.0
CEILING_DBTP = -1.0

# %% [markdown]
# ## The piece
# One arpeggio, a verse at ``amp`` 0.03 and a chorus at 0.3: 64 notes each, a
# quarter beat apart, so eight seconds apiece at two beats a second.

# %%
piece = Pbind(
    freq=Pseq([523.25, 659.25, 783.99, 1046.5, 783.99, 659.25], repeats=22),
    dur=0.25,
    amp=Pseq([Pn(0.03, 64), Pn(0.3, 64)], repeats=1),
)

# %% [markdown]
# ## The offline session
# The clock drives a score the embed renderer turns into samples. A render ends
# at the score's last event, so a `/node_free 0` after the release is that event
# -- without it the last note's tail is cut off.

# %%
session = Session.nrt(tempo=2.0).activate()
session.play(piece)

PIECE_BEATS = 128 * 0.25        # two sections of 64 notes, a quarter beat each
TAIL = 1.0                      # beats: the release, and room


def close():
    yield PIECE_BEATS + TAIL
    session.server.send_bundle(("/node_free", 0))


Routine(close).play()

# %% [markdown]
# ## Render, then measure
# The render keeps its samples in memory, and both measurements read them
# through the shared core -- the same functions the GUI host's meters use, so a
# number printed here and a number drawn there are one number.

# %%
def run():
    """Render the score and report its loudness against EBU R 128."""
    stats = session.render(sample_rate=SR, channels=2)
    measured = ipc.loudness(stats.samples, stats.channels, stats.sample_rate)
    peaks = ipc.true_peak(stats.samples, stats.channels)
    if measured is None:
        print("no loudness: the core library is not loadable")
        return None
    dbtp = max(20 * math.log10(p) if p > 0 else -math.inf for p in peaks)

    print(f"rendered {stats.duration:.2f} s")
    print(f"integrated  {measured.integrated:6.1f} LUFS")
    print(f"range       {measured.range:6.1f} LU")
    print(f"momentary   {measured.momentary_max:6.1f} LUFS max")
    print(f"short-term  {measured.short_term_max:6.1f} LUFS max")
    print(f"true peak   {dbtp:6.1f} dBTP")
    gain = TARGET_LUFS - measured.integrated
    print(f"to reach {TARGET_LUFS} LUFS: {gain:+.1f} dB, "
          f"which puts the true peak at {dbtp + gain:+.1f} dBTP "
          f"({'under' if dbtp + gain <= CEILING_DBTP else 'over'} the "
          f"{CEILING_DBTP} dBTP ceiling)")
    return measured


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run()
else:
    print("score ready - run() to render and measure it")
