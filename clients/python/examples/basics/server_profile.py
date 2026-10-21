#!/usr/bin/env python3
"""Reading what the server spent its time on: ``Server.profile``.

``Server.load`` says which **role** is busy: the audio block, the serving
turn, the NRT job queue. ``Server.profile`` says what inside it, over a window
it measures on its own: which phase of the block, which synth and which def,
which command the serving turn was handling, how many bytes every reply
carried, and how long every NRT job ran and waited in the queue. It is the
reading taken when a session is slow and the question is *why*.

The point of interest is that nobody has to watch the server while it
happens. Start what should be measured, call ``profile(seconds)``, and read
the result: the call asks the server to time every synth for the window,
takes a report, waits, takes another and gives the timing back, and the
profile is the difference between the two reports. The timing is the one
part the server only does while asked, because it costs the audio thread two
clock reads per synth per block; the rest it counts all along.

What to look for in the report: under ``phases``, ``audio nodes`` is the tree
and ``audio publish`` the levels and clocks the block hands out; under
``synths`` and ``defs`` the ``default`` voices of the phrase, the short notes
already freed counted under their def; under ``commands`` the ``/synth_new``
and ``/node_set`` the pattern sends; under ``jobs`` the ``/buffer_gen`` that
fills a wavetable mid-window, with how long it ran and how long it waited.

`Session.live` boots an audio server if none is up, so in a venv where the
client is installed (``pip install ./clients/python``) this runs on its own::

    python clients/python/examples/basics/server_profile.py

Needs an audio device (it plays while it measures). This file is organized as
``# %%`` cells (the VS Code / Jupyter convention): step through it with
Shift+Enter, or run it as a plain script.
"""

# %%
import sys
import threading

from clausters import Session
from clausters.defs import Buffer, format_profile
from clausters.seq import INF, pbind, pseq, pwhite

# %% [markdown]
# ## The session

# %%
session = Session.live(latency=0.1).activate()
session.clock.set_tempo(2.0)
server = session.server

# %% [markdown]
# ## Something to measure
# A phrase that keeps arriving, so the block, the tree and the serving turn
# all have work in them. Nothing here is about the profile -- it is an
# ordinary pattern.

# %%
phrase = pbind(
    instrument="default",
    degree=pseq([0, 2, 4, 7, 9, 7, 4, 2], repeats=INF),
    dur=0.125,
    amp=pwhite(0.05, 0.12),
)

# %% [markdown]
# ## Profiling it
# `profile` blocks for its window, so the clock runs on its own (`start`
# rather than `run`, which would hold this thread) and the wavetable that
# lands in the middle of the window is filled from a timer: a `/buffer_gen` runs on the NRT job queue,
# and the report's `jobs` section is where it shows. It is sent without
# waiting for its `/done`, since the reply would arrive while `profile` is the
# one reading replies.

# %%
def run(seconds: float = 4.0):
    """Play, profile the next `seconds`, and print the report."""
    table = Buffer.alloc(8192)
    session.play(phrase)
    session.start()

    def fill():
        table.gen("sine1", 1, *[1.0 / (n + 1) for n in range(8)], wait=False)

    threading.Timer(seconds / 2, fill).start()
    profile = server.profile(seconds)
    session.stop()
    print(format_profile(profile))
    table.free()


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        session.close()
else:
    print("session up - run() to play and profile, session.close() to end")
