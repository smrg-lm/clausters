#!/usr/bin/env python3
"""Play live over UDP from the *installed* package.

The live counterpart of ``offline_render.py``: the same ``Session`` / ``Pbind``
API, but a live RT session sends OSC over UDP to a running Clausters server. The
only thing that changes between offline and live is the session factory -- the
pattern and the clock are identical.

`Session.live` boots an audio server if none is up, so in a venv where the
client is installed (``pip install ./clients/python``) this runs on its own::

    python clients/python/examples/live_udp.py

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention):
step through it with Shift+Enter, or run it as a plain script.

The synths free themselves after each note's sustain, so nothing is left behind.
"""

# %%
import sys

from clausters import Session
from clausters.seq import Pbind, Pseq, Pwhite

# %% [markdown]
# ## The phrase
# A plain event pattern -- nothing here knows whether it will be played live or
# rendered offline.

# %%
phrase = Pbind(
    instrument="default",
    degree=Pseq([0, 2, 4, 7, 4, 2], repeats=2),
    dur=0.25,
    amp=Pwhite(0.1, 0.2),
)

# %% [markdown]
# ## The live session
# Live over UDP (default 127.0.0.1:57110). `latency` schedules each note a touch
# ahead via a wall-clock timetag so the server plays it on time. Swapping
# `Session.live` for `Session.nrt` is the *only* change that makes this offline.

# %%
session = Session.live(tempo=2.0, latency=0.1)

# %%
def run(seconds: float = 3.5):
    """Play the phrase and advance the clock in real time, then stop."""
    session.play(phrase)
    session.run(seconds)
    print("played live; synths freed themselves after their sustain")


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        session.close()
else:
    print("session up - run() to play, session.close() to end")
