#!/usr/bin/env python3
"""Freeze a generative piece and hear that it *continued* rather than restarted.

A def running a stochastic process is the case no DAW transport covers. A DAW's
audio exists before you press play, so a position is an index into it; here
the audio is produced as it sounds, so the piece's position **is** the state
of the running nodes and no number moves it. The only thing a transport can
honestly do to such a piece is stop it and let it carry on.

That is what this shows. `Server.transport_group` binds a group to the server's
transport, and from then on `Server.transport_stop` does three things at the
same sample: it freezes that subtree with every node's state intact, it stops
the transport clock, and it freezes the queue of anything scheduled against that
clock. `Server.transport_play` undoes all three.

The client has to stop too, and that is the second half of the example. A
`TempoClock`'s beats come from a sample timebase that only decides how long to
sleep, so a client whose server froze would keep advancing beats and scheduling
into a piece that is not moving. `TempoClock.freeze` holds the beat instead, and
`TempoClock.thaw` shifts the pacing origin so the frozen seconds are not part of
the piece.

What to listen for: at the pause the texture stops **dead**, mid-gesture, and on
the resume it picks up from exactly there -- not from the top. Run it twice: the
two takes are the same piece interrupted, never two different pieces.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server binary

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/transport/freeze.py``. It self-launches the
audio server (`Session.live`); by hand that would be ``clausters``.
"""

# %%
import sys
import time

from clausters import Session
from clausters.base import Routine
from clausters.defs import Group, SynthDef, control, out
from clausters.defs.ugens import Env, env_gen, pan2, rlpf, white_noise
from clausters.seq import Event

# %% [markdown]
# ## The grain
# One percussive grain of band-passed noise. `env_gen`'s ``done_action=2`` frees
# the node when the envelope ends, so the texture is a stream of short-lived
# synths -- which is exactly what makes the freeze interesting: the state that
# has to survive is spread across every grain in flight.

# %%
def cloud() -> SynthDef:
    """A grain of band-passed noise under a percussive envelope."""
    freq = control("freq", 800.0)
    amp = control("amp", 0.2)
    dur = control("dur", 0.4)
    env = env_gen(Env.perc(0.01, 1.0), time_scale=dur, done_action=2)
    grain = rlpf(white_noise(), freq, 0.2) * env * amp
    return SynthDef("cloud", out(0, pan2(grain, 0.0)))


# %% [markdown]
# ## Boot, and give the transport a group to govern
# The grid (`set_transport`) is what clients phase-align on; `transport_group` is
# what gives the transport teeth over the tree. Every grain is created **inside**
# that group (the event's ``target``), so freezing the group freezes the piece.

# %%
session = Session.live(tempo=2.0, latency=0.1).activate()
server = session.server
cloud().send(server)
server.sync()

piece = Group(server=server)
server.set_transport(0, 2.0)
server.transport_group(piece)
print(f"governing group {piece.id}")

# %% [markdown]
# ## The texture, as a routine on the session clock
# A routine yields beats; the clock resumes it at that exact logical time and the
# server stamps each grain for it. Never `time.sleep` inside a routine -- it runs
# *on* the clock thread, so sleeping there would freeze the timeline itself (and
# the wrong way: without the transport knowing).

# %%
def texture(n: int):
    """``n`` grains, one every half beat, walking a small pitch cycle."""
    for i in range(n):
        Event(instrument="cloud", target=piece.id,
              freq=400.0 + 90.0 * (i % 7), amp=0.18, dur=0.5).play(server)
        yield 0.5


session.start()
server.transport_play()
Routine(lambda: texture(64)).play()

print("playing ~4 s -- listen to the texture")
time.sleep(4.0)

# %% [markdown]
# ## Freeze
# The clock freezes with the server: the beat stops advancing, so the routine
# stops scheduling into a piece that is not moving. The look-ahead already in
# flight is not a problem -- it lands in the server's frozen queue and fires on
# the resume, in its exact relative place.

# %%
session.clock.freeze()
server.transport_stop()
print("FREEZE (3 s of silence) -- the state is held, not discarded")
print(f"  transport clock held at {server.transport_state()['transport_sample']} samples")
time.sleep(3.0)
print(f"  still {server.transport_state()['transport_sample']} -- it did not advance")

# %% [markdown]
# ## Resume
# Not *play from a position* -- continue. MIDI's continue against its start.

# %%
server.transport_play()
session.clock.thaw()
print("RESUME -- the same texture carries on mid-gesture")
time.sleep(4.0)

# %% [markdown]
# ## Unbind

# %%
def teardown():
    """Unbinding thaws whatever the transport governed, so no frozen subtree is
    left with nobody to resume it."""
    server.transport_group(None)
    piece.free()
    session.close()
    print("unbound; done")


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    teardown()
else:
    print("piece up - server.transport_stop() / transport_play(), teardown() to end")
