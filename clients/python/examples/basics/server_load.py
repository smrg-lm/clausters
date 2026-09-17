#!/usr/bin/env python3
"""Reading where the server's time goes: ``Server.load`` by role.

``Server.status`` answers whether the server is keeping up -- one CPU figure
for the audio thread. ``Server.load`` answers *on what*, which is the question
a session that has grown heavy actually asks: a def compiling, a buffer being
filled and a burst of commands all cost real time, and none of them shows in
that one number.

The point of interest is the shape of the reading. The server reports seconds
**since it booted**, cumulative, and the client differences its own interval
out of them -- so the first call has no ``share`` and every call after it
reports the fraction of wall time each role was busy *since your previous
call*. Two clients polling the same server therefore do not disturb each other,
which the peak in ``status`` cannot manage.

What the roles are: ``audio`` is the callback's whole block; ``dsp N`` is one
worker thread (the stages it took off the conductor, so it only appears on a
server booted with ``workers``); ``net`` is the serving turn, never the wait
for the next packet;
``nrt`` is the job queue (soundfiles, ``/buffer_gen``, the editing verbs);
``faust`` is compiling a FaustDef -- it already carries seconds before this
example sends anything, because a server with persisted defs recompiles them
at boot, and it is the clearest case of a cost neither CPU figure in ``status``
can show.

``busy`` is time the work was in progress, **not** per cent of a core: a DSP
worker spinning for its next stage is burning a core and is idle by this
reading. That is the honest answer to how much of the block budget a stage
took; ``top -H`` answers the other question.

`Session.live` boots an audio server if none is up, so in a venv where the
client is installed (``pip install ./clients/python``) this runs on its own::

    python clients/python/examples/basics/server_load.py

Needs an audio device (it plays while it measures). This file is organized as
``# %%`` cells (the VS Code / Jupyter convention): step through it with
Shift+Enter, or run it as a plain script.
"""

# %%
import sys

from clausters import Session
from clausters.defs import Buffer, format_load
from clausters.seq import INF, Pbind, Pseq, Pwhite

# %% [markdown]
# ## The session
# `workers` gives the server DSP worker threads, which is what makes the
# `dsp 0` / `dsp 1` rows exist at all. They stay at **zero** through this run,
# and that is worth reading rather than skipping: a worker takes a stage only
# inside a parallel group whose members touch **disjoint** buses, and these
# voices all sum into the output, so the server has nothing it can spread and
# runs them where it always did -- under `audio`. `basics/group_order.py` is
# the graph where it can.

# %%
session = Session.live(latency=0.1, workers=2).activate()
session.clock.set_tempo(2.0)
server = session.server

# %% [markdown]
# ## Something to measure
# A phrase that keeps arriving, so the audio and net rows have work in them.
# Nothing here is about the meter -- it is an ordinary pattern.

# %%
phrase = Pbind(
    instrument="default",
    degree=Pseq([0, 2, 4, 7, 9, 7, 4, 2], repeats=INF),
    dur=0.125,
    amp=Pwhite(0.05, 0.12),
)

# %% [markdown]
# ## Polling it
# The first call only sets the baseline (nothing to difference against yet);
# each one after it reports the share of the interval since the previous call.
# In between, a buffer is filled through `/buffer_gen` -- that runs on the NRT
# job queue, so the `nrt` row is what pays for it and neither CPU figure in
# `status` would have shown it.

# %%
def run(seconds: float = 6.0):
    """Play while polling the load, once a second."""
    session.play(phrase)
    server.load()                      # baseline: the first reading has no share
    table = Buffer.alloc(8192)

    for second in range(int(seconds)):
        session.run(1.0)
        if second == 2:
            # A job for the NRT queue, mid-run: a wavetable of eight partials.
            table.gen("sine1", 1, *[1.0 / (n + 1) for n in range(8)])
        print(format_load(server.load()))
        print()

    print(server.status())
    table.free()


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        session.close()
else:
    print("session up - run() to play and poll, session.close() to end")
