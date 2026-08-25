#!/usr/bin/env python3
"""A conductor's play/stop/locate driving several clients' playheads in lockstep.

The server hosts a shared transport with a DAW-style rolling state (play / stop /
position). A *conductor* (any client) calls `transport_play` / `transport_stop` /
`transport_locate` on the server; the server broadcasts the new state to every
`/server_notify` client, and each client's `Playhead.follow_transport` rolls, halts or
seeks to match. The server only broadcasts transport *control* -- it never
schedules audio; each client rolls its own playhead on the shared grid.

When the followers also `lock_to` the server (as here) the alignment is
sample-exact; in plain wall-clock mode it is beat-accurate.

This runs two independent followers in one process for clarity -- the state two
separate programs would hold. Each loops a two-note figure, and it prints their
song positions while the transport rolls: the two advance together, within the
few hundredths of a beat each poll costs (the *events* are scheduled on the same
sample; a polled position is each client's own estimate of now). It
**runs out of the box**: the conductor `boot`s the shared server (by hand that
would be ``clausters``, so stop any server already on the port first) and
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

from clausters.base import IdShare, TempoClock
from clausters.defs import Server
from clausters.seq import Pbind, Playhead, Pseq, Timeline


# %% [markdown]
# ## A follower
# An independent client: its own server connection and clock, locked and joined
# to the shared transport, with a playhead following it.

# %%
def make_follower(freq, share):
    """An independent client: its own server connection and clock, locked and
    joined to the shared transport, with a playhead following it.

    ``share`` is its slice of the client id space: several clients on one server
    each take one, so two of them never hand out the same node id."""
    server = Server(share=share)
    clock = TempoClock(tempo=2.0)
    clock.lock_to(server)             # sample-exact timing
    clock.join_transport(server)      # adopt the shared grid (tempo + origin)
    timeline = Timeline.from_pattern(
        Pbind(instrument="default", freq=Pseq([freq, freq * 1.5]), dur=0.5, amp=0.2),
        dur=1.0,
    )
    head = Playhead(timeline, clock, server).loop(0.0, 1.0)   # keep rolling
    clock.start()
    head.follow_transport(server, quant=2)   # roll on the next 2-beat boundary
    return server, clock, head


# %% [markdown]
# ## The conductor
# It brings the shared server up and defines the grid once -- beat 0 at sample
# 0, 2 beats/s -- then two followers join it, each on its own id share.

# %%
conductor = Server().boot()   # the shared server; `close` stops it again
conductor.set_transport(0, 2.0)
followers = [make_follower(440.0, IdShare(0, 2)), make_follower(550.0, IdShare(1, 2))]

# %% [markdown]
# ## Press play
# The server broadcasts, both playheads start rolling together.

# %%
print("conductor: play")
conductor.transport_play(0.0)
for _ in range(3):
    time.sleep(0.7)
    positions = [f"{head.position():.2f}" for _, _, head in followers]
    print(f"  follower positions: {positions}  (advancing together)")

# %% [markdown]
# ## Seek everyone back to the top, then stop

# %%
print("conductor: locate to 0")
conductor.transport_locate(0.0)
time.sleep(1.0)
print("conductor: stop")
conductor.transport_stop()
time.sleep(0.3)


# %%
def close():
    """Unfollow and close every client."""
    for server, clock, head in followers:
        head.unfollow_transport()
        clock.close()
        server.close()
    conductor.close()
    print("done")


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    close()
else:
    print("conductor up - conductor.transport_play(0.0), close() to end")
