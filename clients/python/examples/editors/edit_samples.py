#!/usr/bin/env python3
"""``edit(buffer)``: a take's samples, drawn over and written back.

The third structure, and the one whose state is **not in this process**: the
frames are in a server buffer, so what the picture draws and what a stroke
writes are the same buffer, with no copy in between. Play the take after
drawing on it and you hear the stroke.

What to do in the window: **wheel** to zoom in until each sample is a disc,
then **Alt+drag** to draw over them. **Ctrl+Z** puts the samples back — and
nothing is read from the server to do it: a stroke's event carries the run it
wrote *and* the run it replaced, which is what the protocol carries `previous`
for.

**The one domain the crate does not hold.** A curve's points and a timeline's
events are values this process owns, so the shared crate can be handed one and
asked what an edit makes of it. A span of samples is a borrowed view over
frames somewhere else, so what is shared here is the payload's shape and its
coalesce key, and where the state lives is the client's.

Run it as a script, or step through the cells::

    pip install -e clients/python

    python clients/python/examples/editors/edit_samples.py

It self-launches the audio server and the GUI host, and writes its own take —
nothing has to be found on disk.
"""

# %%
import math
import sys
import time

from clausters import Session
from clausters.defs import Buffer
from clausters.gui import edit

SECONDS = 2.0

# %% [markdown]
# ## A take, made here
#
# Two seconds of a decaying tone, written into a server buffer with
# `clausters.defs.Buffer.from_samples`. A shape worth recognizing, so a stroke
# over it is visibly a stroke over *this*.

# %%
session = Session.live()
server = session.server
rate = server.sample_rate
frames = int(SECONDS * rate)
samples = [
    math.exp(-3.0 * i / frames) * 0.7 * math.sin(2 * math.pi * 220.0 * i / rate)
    for i in range(frames)
]
take = Buffer.from_samples(samples, 1, rate, server=server)

# %% [markdown]
# ## One verb
#
# A `clausters.defs.Buffer` opens as a
# `clausters.gui.editing.SamplesEditor` — one `waveform` widget the host draws
# straight from the server's buffer, and the crate's ``samples`` vocabulary.

# %%
gui = session.gui()
editor = edit(take, title="take")
editor.open(gui)

# %% [markdown]
# ## Hear the stroke
#
# A buffer is data: something has to read it, and `clausters.Session.play`
# provides the stock reader — so playing after an edit is what makes the stroke
# audible rather than only visible.

# %%
def play():
    """Play the take as it now stands — the buffer the picture is drawing."""
    session.play(take)


# %%
def run():
    """Keep the window open until it is closed."""
    print("zoom in (wheel) until the samples are discs, then Alt+drag to draw.")
    print("Ctrl+Z puts them back. Call play() to hear what you drew.")
    while editor.window is not None:
        editor.poll(0.05)
        time.sleep(0.01)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run()
else:
    print("up — draw on the take, play() to hear it")
