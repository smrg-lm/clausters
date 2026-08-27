#!/usr/bin/env python3
"""Two clients phase-aligned on one server's shared transport.

A server hosts a **transport** — a beat grid `(origin_sample, tempo)` it stores
under `/transport_set`. Several independent clients can *join* that grid, so a
`quant`-ed routine on each starts on the **same** beat. When each client is also
locked to the server's sample clock (`lock_to`), that alignment is sample-exact;
in plain wall-clock mode it is beat-accurate (drift-bounded).

This runs in a single process for clarity, but the two `Server` / `TempoClock`
pairs are completely independent — exactly the state two separate programs would
hold. It prints the next-bar sample each client computes (they match: that *is*
the alignment) and plays one note on each at that bar, so the two sound together.

The transport's **second half is its rolling state**, and this shows that too:
`clausters.defs.Server.transport_play`, `transport_stop` and `transport_locate`
are broadcast to every client registered for notifications, and
`clausters.seq.Playhead.follow_transport` turns those broadcasts into a client's
own playhead — the conductor rolls, halts and seeks every follower at once. The
server broadcasts *control*, never audio.

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
from clausters.seq import Event, Playhead, Timeline


# %% [markdown]
# ## An independent client
# Its own server connection and clock, locked to the server's sample clock and
# joined to the shared transport. Two of these stand in for two programs.

# %%
def make_client(share):
    """An independent client: its own server connection and clock, locked to the
    server's sample clock and joined to the shared transport.

    ``share`` is its slice of the client id space: several clients on one server
    each take one, so two of them never hand out the same node id."""
    server = Server(share=share)      # to 127.0.0.1:57110
    clock = TempoClock(tempo=1.0)     # the transport overwrites this tempo
    clock.lock_to(server)             # sample-exact, drift-free timing
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
conductor = Server().boot()       # the shared server; `close` stops it again
conductor.set_transport(0, 2.0)
(sa, ca), (sb, cb) = make_client(IdShare(0, 2)), make_client(IdShare(1, 2))

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
# ## A playhead that obeys the conductor
# The transport's other half: its rolling state. Client A's playhead follows
# the shared transport, so it is the *conductor* that starts, seeks and halts
# it — a follower has no buttons of its own. The timeline is one bar of quarter
# notes, so a locate is audible as a different place in the same figure.

# %%
figure = Timeline([
    (0, Event(freq=220.0, amp=0.15, dur=0.4)),
    (1, Event(freq=277.2, amp=0.15, dur=0.4)),
    (2, Event(freq=330.0, amp=0.15, dur=0.4)),
    (3, Event(freq=440.0, amp=0.15, dur=0.4)),
])
playhead = Playhead(figure, ca, sa).loop(0, 4)


# %%
def conduct():
    """The conductor drives every follower: roll, seek, halt. Each call is one
    broadcast, and the playhead never hears a button of its own."""
    ca.start()
    playhead.follow_transport(sa, quant=4)
    print("client A: its playhead now follows the conductor's transport")

    conductor.transport_play()
    print("conductor: transport play - every follower rolls")
    time.sleep(2.5)

    conductor.transport_locate(4.0)
    print("conductor: locate to beat 4 - every follower seeks")
    time.sleep(2.5)

    conductor.transport_stop()
    print("conductor: transport stop - every follower halts")
    time.sleep(0.3)
    playhead.unfollow_transport()
    ca.stop()


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
        conduct()
    finally:
        for s in (sa, sb, conductor):
            s.close()
else:
    print("two clients up - run() to play the aligned notes, conduct() to hand "
          "client A's playhead to the conductor; sa.close(); sb.close(); "
          "conductor.close() to end")
