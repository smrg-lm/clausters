#!/usr/bin/env python3
"""Stopping everything at once: ``panic``, and what it leaves standing.

Three things sound here, each kept going by something different: a **drone**,
a node that plays until it is freed; an **arpeggio**, a routine on the
session's clock that makes a new note every quarter beat; and a **responder**,
an `OscFunc` that would answer a message from another program. Freeing the
nodes alone does not stop that: the routine makes the next note a moment
later, and a bundle already queued on the server makes one too.

`panic` is the one verb for all of it -- sclang's ``CmdPeriod``. It empties the
session's clocks, halts its timelines, silences its server (every transport
stopped, every queued bundle dropped, every node freed) and frees every
responder not marked ``permanent``. It closes nothing: the clock keeps its beat
and goes on running, and the note played after it sounds.

The free-standing ``panic()`` stops the session in force, the one an ambient
``play`` reaches; ``session.panic()`` names one. In clausters-editor, ``Ctrl+.``
does the same for every open session after it interrupts the code -- step to
the arpeggio's cell and press it instead of running the panic cell.

Needs an audio device. Run it as a script (``python panic.py``) or step through
it cell by cell (``# %%``).
"""

# %%
import sys
import time

from clausters import Session, panic, play
from clausters.base import Routine
from clausters.defs import sine
from clausters.responders import OscFunc
from clausters.seq.event import Event

# %% [markdown]
# ## The session
# `Session.live` boots a server if none answers and stops the one it started.
# Activating it makes it the session in force: what `play` reaches, and what
# the free-standing `panic` stops.

# %%
session = Session.live().activate()

# %% [markdown]
# ## Three things that sound
# A drone, which is a node and nothing else; an arpeggio, a routine making a
# note every quarter beat; and two responders -- one ordinary, one `permanent`,
# the kind a controller mapping would be.

# %%
drone = play(sine(660) * 0.04)


def arpeggio():
    while True:
        for freq in (880.0, 1100.0, 1320.0, 1760.0):
            Event(freq=freq, legato=0.4, amp=0.05).play()
            yield 0.25


Routine(arpeggio).play()

ping = OscFunc(lambda msg, time, src: print("ping", msg), "/ping")
fader = OscFunc(lambda msg, time, src: print("fader", msg), "/fader",
                permanent=True)

# %% [markdown]
# ## Panic
# The drone and the arpeggio stop together, and the arpeggio does not come
# back: its routine left the clock. The ordinary responder is freed and the
# permanent one still answers.

# %%
def stop_everything():
    panic()
    time.sleep(0.3)
    print(f"after the panic: {session.server.status().synths} synths, "
          f"{session.clock.queued} routines queued, "
          f"/ping answered: {ping.enabled}, /fader answered: {fader.enabled}")


# %% [markdown]
# ## And it plays again
# Nothing was closed, so the next note sounds on the same clock and server.

# %%
def play_again():
    Event(freq=1320.0, sustain=1.0, amp=0.08).play()


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        time.sleep(4.0)             # the drone and the arpeggio, for a while
        stop_everything()
        time.sleep(1.0)             # silence
        play_again()
        time.sleep(1.5)
    finally:
        session.close()
else:
    print("sounding - stop_everything() (or Ctrl+. in clausters-editor), "
          "then play_again(); session.close() to end")
