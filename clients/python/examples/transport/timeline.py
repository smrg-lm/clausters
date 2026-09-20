#!/usr/bin/env python3
"""DAW-style transport over a timeline: play, locate, loop, pause, position.

A `Pbind` is a forward-only generator -- you cannot seek it. A `Timeline` is the
opposite: an editable plan of timed items with random access by beat, and it
plays itself -- `play(at=...)`, `locate(beat)`, `loop(start, end)`, `pause()`,
`stop()` -- reporting a song `position`. No clock and no playhead are handled:
the timeline plays on a clock of its own, with its own tempo map.

This example writes a phrase into a timeline, edits it programmatically, then
drives it live. Random access happens at the boundaries (play/locate/loop);
between them the timeline just goes forward.

`Session.live` boots an audio server if none is up, so this runs on its own:

    python clients/python/examples/transport/timeline.py

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention).
Stepping through it is how the transport controls are meant to be met: run a
cell, hear where the timeline went.
"""

# %%
import sys
import time

from clausters import Session
from clausters.seq import Event, Timeline

# %% [markdown]
# ## A phrase in a timeline, then edit it
# Four notes half a beat apart, each at its beat: a static list of timed items,
# so it can be edited by hand. Its ``tempo`` is the timeline's own: two beats a
# second.

# %%
timeline = Timeline(
    [(0.5 * i, Event(instrument="default", degree=degree, dur=0.5, amp=0.2))
     for i, degree in enumerate([0, 2, 4, 7])],
    tempo=2.0,
)
timeline.add(0.0, Event(instrument="default", degree=7, dur=0.5, amp=0.3))  # an accent
print(f"timeline: {len(timeline)} items over {timeline.duration()} beats")

# %% [markdown]
# ## A session to play on

# %%
session = Session.live(latency=0.1).activate()

# %% [markdown]
# ## Play from the top

# %%
timeline.play(at=0.0)
time.sleep(1.2)
print(f"position after ~1.2 s: beat {timeline.position():.2f}")

# %% [markdown]
# ## Locate
# Seek to beat 1.0 and keep playing from there -- the random access a generator
# could never do.

# %%
timeline.locate(1.0)
print("located to beat 1.0")
time.sleep(1.0)

# %% [markdown]
# ## Loop the first two beats

# %%
timeline.loop(0.0, 2.0).play(at=0.0)
print("looping [0, 2)")
time.sleep(3.0)

# %% [markdown]
# ## Pause, then stop
# `pause` holds the position; `stop` goes back to where the last `play` started.

# %%
timeline.pause()
print(f"paused at beat {timeline.position():.2f}")
timeline.stop()
print(f"stopped, back at beat {timeline.position():.2f}")

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    session.close()
    print("done")
else:
    print("up - timeline.play(at=0), timeline.locate(b), session.close() to end")
