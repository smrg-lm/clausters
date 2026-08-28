#!/usr/bin/env python3
"""Send OSC to another application, on the beat — the client as an OSC source.

The counterpart of `osc_responder.py`. There the client *receives* OSC from any
application; here it *sends* to one, and — the point of the example — with the
same logical timing the audio gets. A destination is where OSC goes:
``session.destination(host, port)`` opens one onto another application, and its
``send_bundle`` stamps the routine's exact beat, exactly as a note does.

What travels is standard OSC: a message, or a bundle with an NTP timetag. A
destination adds nothing of ours to it — not the server's ``latency`` (that is
our audio pipeline's property, not another program's), not ``/sched_at``, not
``/server_sync``. If another application needs to run ahead, ask for it as an explicit
``delay_beats``.

There is no second program to run here: the example stands in for one with an
`OscReceiver` bound in this same process, which prints every packet it gets
with the time it was stamped for. Point ``--port`` at something else and the
same routine drives that instead.

`Session.live` boots an audio server if none is up, so this runs on its own::

    python clients/python/examples/io/osc_destination.py [beats] [--port N]

Listen to it and watch the printout together: every lamp line lands on a beat,
and the gap between consecutive stamps is exactly the tempo's, however late the
wake-up was.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention):
step through it with Shift+Enter, or run it as a plain script.
"""

# %%
import sys
import time

from clausters import Session
from clausters.base import OscReceiver
from clausters.base.stream import Routine
from clausters.seq import Event

TEMPO = 2.0                 # beats per second
COLOURS = ("red", "amber", "green", "blue")


# %% [markdown]
# ## The other application
# A receiver playing its part. Any OSC program would do -- this one just prints
# what arrives and when it was *stamped for*, which is what the example is about.

# %%
def stand_in_for_another_app(port):
    """Any OSC program would do -- this one just prints what arrives and when it
    was *stamped for*, which is what the example is about.
    """
    receiver = OscReceiver(port=port).start()
    start = time.time()

    def show(addr, args, when, src):
        # `when` is the bundle's timetag in Unix seconds (None for a bare
        # message, which carries no time at all).
        stamp = "-- (untimed)" if when is None else f"{when - start:+.3f}s"
        print(f"    {addr} {list(args)}  stamped for {stamp}")

    receiver.add(show)
    return receiver


# %% [markdown]
# ## The session and the destination
# A bare message has no time: it means "now". Use it for what has no place in a
# timeline -- here, telling the other app we are starting.

# %%
PORT = 57123
listener = stand_in_for_another_app(PORT)
print(f"standing in for an external app on UDP {listener.port}")

session = Session.live(tempo=TEMPO, latency=0.1).activate()
lights = session.destination("127.0.0.1", listener.port)
lights.send_msg("/lamps/reset")

# %% [markdown]
# ## One routine drives the sound and the other application at once
# The note and the lamp read the *same* moment: the beat the routine has
# accumulated by yielding, not what time it is when each line runs. So they stay
# locked to each other whatever the scheduler did in between.

# %%
def cue(clock, beats):
    for i in range(int(beats)):
        colour = COLOURS[i % len(COLOURS)]
        Event(instrument="default", freq=220.0 * (1 + i % 4), dur=0.5).play()
        lights.send_bundle(("/lamp", colour, 1.0))
        # A lamp that goes out half a beat later: same call, a delay in beats,
        # still on the clock's grid.
        lights.send_bundle(("/lamp", colour, 0.0), delay_beats=0.5)
        yield 1.0


# %%
def run(beats: float = 8.0):
    """Cue ``beats`` beats of notes and lamps together."""
    Routine(lambda clock: cue(clock, beats)).play()
    session.run(beats / TEMPO + 1.0)
    print("done")


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run(float(sys.argv[1]) if len(sys.argv) > 1 else 8.0)
    finally:
        listener.stop()
        session.close()
else:
    print("destination up - run(8) to cue, listener.stop(); session.close() to end")
