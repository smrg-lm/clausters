#!/usr/bin/env python3
"""DAW-style transport over a static timeline: play, locate, loop, position.

A `Pbind` is a forward-only generator -- you cannot seek it. A `Timeline` is the
opposite: a static, editable list of timed items with random access by beat, so
a `Playhead` can offer real transport controls -- `play(at=…)`, `locate(beat)`,
`loop(start, end)`, `stop()` -- and report a song `position`.

This example captures a pattern into a timeline, edits it programmatically, then
drives it live with the playhead. Random access happens at the boundaries
(play/locate/loop); between them the playhead just scans forward.

`Session.live` boots an audio server if none is up, so this runs on its own:

    python clients/python/examples/transport/timeline.py

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention).
Stepping through it is how the transport controls are meant to be met: run a
cell, hear where the playhead went.
"""

# %%
import sys
import time

from clausters import Session
from clausters.seq import Event, Pbind, Playhead, Pseq, Timeline

# %% [markdown]
# ## Bounce a pattern to a clip, then edit it
# `Timeline.from_pattern` captures a forward-only generator into a static list of
# timed items -- and once static, it can be edited by hand.

# %%
timeline = Timeline.from_pattern(
    Pbind(instrument="default", degree=Pseq([0, 2, 4, 7]), dur=0.5, amp=0.2),
    dur=2.0,
)
timeline.add(0.0, Event(instrument="default", degree=7, dur=0.5, amp=0.3))  # an accent
print(f"timeline: {len(timeline)} items over {timeline.duration()} beats")

# %% [markdown]
# ## The playhead

# %%
session = Session.live(tempo=2.0, latency=0.1)
head = Playhead(timeline, session.clock, session.server)
session.start()

# %% [markdown]
# ## Play from the top

# %%
head.play(at=0.0)
time.sleep(1.2)
print(f"position after ~1.2 s: beat {head.position():.2f}")

# %% [markdown]
# ## Locate
# Seek to beat 1.0 and keep playing from there -- the random access a generator
# could never do.

# %%
head.locate(1.0)
print("located to beat 1.0")
time.sleep(1.0)

# %% [markdown]
# ## Loop the first two beats

# %%
head.loop(0.0, 2.0).play(at=0.0)
print("looping [0, 2)")
time.sleep(3.0)

# %%
head.stop()
print(f"stopped at beat {head.position():.2f}")

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    session.close()
    print("done")
else:
    print("head up - head.play(at=0), head.locate(b), session.close() to end")
