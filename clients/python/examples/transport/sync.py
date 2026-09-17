#!/usr/bin/env python3
"""Two clients phase-aligned on one server's shared transport.

A server hosts a **transport** — a beat grid `(origin_sample, tempo)` it stores
under `/transport_set`. Several independent clients can *join* that grid, so a
`quant`-ed routine on each starts on the **same** beat. When each client's clock
is also made on the server's sample clock (`Server.sample_timebase`), that
alignment is sample-exact; on wall-clock time it is beat-accurate
(drift-bounded).

This runs in a single process for clarity, but the two `Server` / `TempoClock`
pairs are completely independent — exactly the state two separate programs would
hold. It prints the next-bar sample each client computes (they match: that *is*
the alignment) and plays one note on each at that bar, so the two sound together.

The transport's **second half is its rolling state**, and this shows that too:
`clausters.defs.Server.transport_play`, `transport_stop` and
`transport_locate_sample` move the one position the engine holds, and a
`clausters.seq.Timeline` **on that transport** (``timeline.transport = server``)
plans its items onto it — the conductor rolls, freezes and seeks every follower
at once. The server owns the time and plays no notes of its own.

It **runs out of the box**: the conductor `boot`s the shared server (by hand
that would be ``clausters``, so stop any server already on the port first) and
`close` stops it again; the two clients only connect to it, each taking its own
`clausters.base.IdShare` of the client id space so two of them never hand out
the same node id.

Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server binary

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/transport/sync.py``.
"""

import math
# %%
import sys
import time

from clausters.base import IdShare, Routine, TempoClock
from clausters.defs import Server
from clausters.defs import Group
from clausters.seq import Event, Timeline


# %% [markdown]
# ## An independent client
# Its own server connection and clock, made on the server's sample clock and
# joined to the shared transport. Two of these stand in for two programs.

# %%
def make_client(share):
    """An independent client: its own server connection and clock, made on the
    server's sample clock and joined to the shared transport.

    ``share`` is its slice of the client id space: several clients on one server
    each take one, so two of them never hand out the same node id."""
    # `attach`: the conductor booted this server and nothing here starts one.
    # The verb also verifies -- a handle pointing where nobody answers raises
    # instead of dropping every later message into a UDP void -- and sizes the
    # allocators from the capacities the running server reports.
    server = Server(share=share).attach(adopt_default=False)   # to 127.0.0.1:57110
    # sample-exact, drift-free timing; the transport overwrites this tempo
    clock = TempoClock(tempo=1.0, timebase=server.sample_timebase())
    clock.join_transport(server)      # adopt the shared beat grid
    return server, clock


# %% [markdown]
# ## Where the next bar falls
# Computed from public state only, so it is the *same* number for every client
# on the same transport.

# %%
def next_bar_sample(server, clock, quant=4):
    """The absolute sample the clock's next `quant`-beat bar falls on — computed
    from public state only, so it is the *same* number for every client on the
    same transport."""
    origin, tempo = server.transport()
    rate = clock.timebase.sample_rate
    target = math.ceil(clock.grid_beat() / quant) * quant
    return round(origin + target * rate / tempo)


# %%
def one_note(server, freq):
    def routine():
        Event(freq=freq, amp=0.2, dur=0.5).play(server)
        yield 0.5
    return routine


# %% [markdown]
# ## The conductor and the two clients
# The conductor brings the shared server up and defines the grid once: beat 0 at
# sample 0, 2 bps. Then the two clients join it, each on its own id share.

# %%
# Three id shares, not two: the conductor allocates a node of its own (the
# governed group), so it takes a share like everybody else -- two clients and a
# conductor on one server never hand out the same id.
conductor = Server(share=IdShare(0, 3)).boot()   # `close` stops it again
conductor.set_transport(0, 2.0)
# A transport owns the nodes it plays, which is what lets it freeze them: the
# governed group is where every follower's items go.
governed = Group(server=conductor)
conductor.transport_group(governed.id)
(sa, ca), (sb, cb) = make_client(IdShare(1, 3)), make_client(IdShare(2, 3))

# %% [markdown]
# ## The alignment
# Sampled back-to-back, both clients see the same next bar.

# %%
bar_a, bar_b = next_bar_sample(sa, ca), next_bar_sample(sb, cb)
print(f"client A next bar -> sample {bar_a}")
print(f"client B next bar -> sample {bar_b}")
print("aligned to the sample" if abs(bar_a - bar_b) <= 2 else "NOT aligned")


# %% [markdown]
# ## One note each, quantized to the next bar
# The routines start on the same beat, so the notes sound together. Each clock
# is started before playing so `quant` snaps against the running,
# transport-locked grid.

# %%
def run():
    for clock, server, freq in ((ca, sa, 440.0), (cb, sb, 660.0)):
        clock.start()
        clock.play(Routine(one_note(server, freq)), quant=4)
    ca.run(3.0)        # let the bar arrive and the notes play, then wind down
    cb.stop()
    print("played; the two notes landed on the same bar")


# %% [markdown]
# ## A timeline that obeys the conductor
# The transport's other half: its rolling state. Client A's timeline is **on**
# the shared transport, so it is the *conductor* that starts, seeks and halts it
# — a follower has no buttons of its own. The figure is two bars of quarter
# notes, so a locate is audible as a different place in the same figure.

# %%
figure = Timeline([
    (beat, Event(freq=freq, amp=0.15, dur=0.4, target=governed.id))
    for beat, freq in enumerate([220.0, 277.2, 330.0, 440.0,
                                 220.0, 277.2, 330.0, 440.0])
], tempo=2.0)


# %%
def conduct():
    """The conductor drives every follower: roll, seek, halt. Each call moves
    the one position the engine holds, and the timeline never hears a button of
    its own."""
    figure.transport = sa
    print("client A: its timeline is now on the conductor's transport")

    conductor.transport_play()
    print("conductor: transport play - every follower rolls")
    time.sleep(2.0)

    conductor.transport_locate_sample(2 * 48_000)
    print("conductor: locate to the second bar - every follower seeks")
    time.sleep(2.0)

    conductor.transport_stop()
    print("conductor: transport stop - every follower freezes")
    time.sleep(0.3)
    figure.transport = None


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
        conduct()
    finally:
        for s in (sa, sb, conductor):
            s.close()
else:
    print("two clients up - run() to play the aligned notes, conduct() to put "
          "client A's timeline on the conductor's transport; sa.close(); "
          "sb.close(); conductor.close() to end")
