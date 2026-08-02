#!/usr/bin/env python3
"""The simplest possible sound: boot a server, play a note.

No `Session`, no clock wiring. ``Server.boot()`` launches a server and adopts it
as the **default session**, so ``Event().play()`` and the free-standing
``play`` find it on their own. A bare event outside any clock plays immediately
and frees itself after its sustain.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) --
which is the point here more than anywhere: each cell is one line you could have
typed at a prompt. Step through it with Shift+Enter, or run it as a plain script
from a venv where the client is installed (``pip install ./clients/python``)::

    python clients/python/examples/hello_note.py
"""

# %%
import sys
import time

from clausters import Server, Event, play
from clausters.seq import Pbind, Pseq

# %% [markdown]
# ## Boot
# Launches a clausters process and becomes the default session's server. Closed
# (and the process stopped) on interpreter exit.

# %%
server = Server.boot()

# %% [markdown]
# ## One note, right now
# Resolved against the default session, with no clock in sight.

# %%
play(Event(degree=0))           # or: Event(degree=0).play()
time.sleep(1.0)

# %% [markdown]
# ## A short phrase
# With no clock in context, `play` uses the default session's clock, created and
# started for you.

# %%
play(Pbind(instrument="default", degree=Pseq([0, 2, 4, 7]), dur=0.4))
time.sleep(2.0)
print("played; synths freed themselves after their sustain")

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    server.close()
else:
    print("server up - play(Event(degree=4)) for another note, server.close() to end")
