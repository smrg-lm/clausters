#!/usr/bin/env python3
"""Play live from an *embedded* server running inside this process.

The third session flavour, next to ``offline_render.py`` (NRT) and
``live_udp.py`` (a separate server over UDP). ``Session.embed()`` opens the
whole Clausters server -- audio device and engine -- *in this process*, through
the native library bundled in the wheel. There is no socket and no separate
server process: OSC is delivered by function call. Yet the code above the
session is identical to the live/offline cases, because only the session factory
(the `Server`'s communication interface) changes -- the pattern and the clock do
not.

Just run it; nothing else needs to be started::

    python clients/python/examples/embedded.py

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention):
step through it with Shift+Enter, or run it as a plain script.

Contrast with the *standalone* server, which the wheel also ships as the
``clausters`` command (a separate process you point UDP/TCP clients, ``ShmClient``
or other machines at)::

    clausters            # start the standalone server, then use Session.live(...)

The embedded server is the batteries-included path: import it and make sound.
The synths free themselves after each note's sustain, so nothing is left behind.
"""

# %%
import sys

from clausters import Session
from clausters.seq import Pbind, Pseq, Pwhite

# %% [markdown]
# ## The phrase
# Identical to the one in `live_udp.py` and `offline_render.py` -- the pattern
# never knows which server flavour will play it.

# %%
phrase = Pbind(
    instrument="default",
    degree=Pseq([0, 2, 4, 7, 4, 2], repeats=2),
    dur=0.25,
    amp=Pwhite(0.1, 0.2),
)

# %% [markdown]
# ## The embedded session
# The server runs in-process; `latency` still schedules each note a touch ahead
# (via a wall-clock timetag the in-process server reads against the same clock)
# so it sounds on time rather than late. The embedded server is reachable for
# direct queries too -- the same OSC request/reply, only in-process.
# `interface.server` is the handle.

# %%
session = Session.embed(tempo=2.0, latency=0.1)
print("embedded server:", session.server.interface.server.sample_rate, "Hz")

# %%
def run(seconds: float = 3.5):
    """Play the phrase and advance the clock in real time, then stop."""
    session.play(phrase)
    session.run(seconds)
    print("played from the embedded server; synths freed after their sustain")


# %%
# Closing the session shuts the embedded server (and its audio device) down.
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        session.close()
else:
    print("embedded server up - run() to play, session.close() to end")
