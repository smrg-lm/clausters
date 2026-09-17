#!/usr/bin/env python3
"""A conductor's play/stop/locate driving several clients' timelines in lockstep.

The server hosts a transport: a governed group, a clock that freezes with it and
a position a locate moves. A *conductor* (any client) calls `transport_play` /
`transport_stop` / `transport_locate_sample` on the server; every client whose
timeline is **on that transport** (``timeline.transport = server``) rolls,
freezes and re-cues to match. The server broadcasts transport *control* and owns
the time -- it plays no notes of its own: each client plans its own items onto
the transport's clock, which is what makes a freeze hold them and a locate move
them.

This runs two independent followers in one process for clarity -- the state two
separate programs would hold. Each plays a two-note figure over four bars, and
it prints their positions while the transport rolls: they advance together
because it is **one** position, the engine's, rather than two estimates of it.
It **runs out of the box**: the conductor `boot`s the shared server (by hand
that would be ``clausters``, so stop any server already on the port first) and
`close` stops it again; the followers only connect to it, each taking its own
`clausters.base.IdShare` of the client id space so two of them never hand out
the same node id.

Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server binary

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/transport/conductor.py``.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention),
which suits a conductor: bring the followers up once, then drive them cell by
cell -- play, watch the positions, locate, stop.
"""

# %%
import sys
import time

from clausters.base import IdShare
from clausters.defs import Group, Server
from clausters.seq import Event, Timeline

#: Two beats a second, and the figure is four bars of two notes.
TEMPO = 2.0

# %% [markdown]
# ## The conductor
# It brings the shared server up and binds the **governed group**: a transport
# owns the nodes it plays, which is what lets it freeze them on a pause, and it
# is what every follower's items are placed under.

# %%
# Three id shares, not two: the conductor allocates a node of its own (the
# governed group), so it takes a share like everybody else -- two followers and
# a conductor on one server never hand out the same id.
conductor = Server(share=IdShare(0, 3)).boot()   # `close` stops it again
governed = Group(server=conductor)
conductor.transport_group(governed.id)
rate = conductor.query_info().nominal_sample_rate


# %% [markdown]
# ## A follower
# An independent client: its own server connection, and a timeline **on the
# conductor's transport**. No clock and no playhead: the transport is the time,
# and the timeline plans its items onto it.

# %%
def make_follower(freq, share):
    """An independent client: its own connection to the shared server, and a
    timeline on its transport.

    ``share`` is its slice of the client id space: several clients on one server
    each take one, so two of them never hand out the same node id."""
    # `attach`, not a bare handle: the conductor booted this server, nothing
    # here starts one, and the verb verifies it is up and sizes the allocators
    # from what it reports.
    server = Server(share=share).attach(adopt_default=False)
    timeline = Timeline(tempo=TEMPO)
    for bar in range(4):
        for beat, ratio in ((0.0, 1.0), (0.5, 1.5)):
            timeline.add(bar + beat,
                         Event(instrument="default", freq=freq * ratio, dur=0.4,
                               amp=0.2, target=governed.id))
    # The mode is the following: from here the conductor's verbs drive it.
    timeline.transport = server
    return server, timeline


followers = [make_follower(440.0, IdShare(1, 3)), make_follower(550.0, IdShare(2, 3))]

# %% [markdown]
# ## Press play
# One broadcast, and both timelines are rolling on the one position.

# %%
print("conductor: play")
conductor.transport_play()
for _ in range(3):
    time.sleep(0.7)
    places = [f"{tl.refresh().position():.2f}" for _, tl in followers]
    print(f"  follower positions: {places}  (one position, not two estimates)")

# %% [markdown]
# ## Seek everyone back to the top, then stop
# A locate is the engine's: it moves the position, and each follower hears the
# broadcast and re-cues its plan from there.

# %%
print("conductor: locate to the top")
conductor.transport_locate_sample(0)
time.sleep(1.5)
print("conductor: stop")
conductor.transport_stop()
time.sleep(0.3)


# %%
def close():
    """Take every timeline off the transport and close every client."""
    for server, timeline in followers:
        timeline.transport = None
        server.close()
    governed.free()
    conductor.close()
    print("done")


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    close()
else:
    print("conductor up - conductor.transport_play(), "
          "conductor.transport_locate_sample(0), close() to end")
