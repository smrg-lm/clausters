#!/usr/bin/env python3
"""A `Voices` node: one instrument in the tree, played by MIDI, by a pattern
and by the client, and a note never makes a node.

**An instrument is a node that reads an event bus.** `Voices` holds eight
voices of the built-in ``default``, all built with the node, and its
``notes`` control names the event bus it reads -- as a node's ``in`` names
the audio bus it reads. Whatever writes that bus plays it: a note takes a
free voice, resets it and starts it; its end closes the voice's gate.

Three writers reach the same eight voices, one after the other:

- **MIDI.** `Server.midi_bind` routes channel 0 to the bus. The chord here
  arrives as MIDI 2.0 packets (`Server.midi_ump`), the path a keyboard on the
  server's MIDI input takes -- boot the server with ``--midi`` and a keyboard
  on channel 0 plays the same voices.
- **A pattern.** An `clausters.seq.Event` whose ``instrument`` is the node is
  one of its notes, so ``pbind(instrument=keys, ...)`` plays it; a ``pvoice``
  line on it is **one held voice**, each joined note moving that voice to
  its pitch instead of re-attacking.
- **The client.** `Server.bus_event` writes notes on the bus from a routine,
  at its logical beat.

The node count is printed at the end: one node, however many notes played.

`Session.live` boots an audio server if none is up, so in a venv where the
client is installed (``pip install ./clients/python``) this runs on its own::

    python clients/python/examples/basics/voices.py

Needs an audio device. This file is organized as ``# %%`` cells (the VS Code /
Jupyter convention): step through it with Shift+Enter, or run it as a plain
script.
"""

# %%
import sys
import time

from clausters import Routine, Session, Voices
from clausters.seq import pbind, pseq, pvoice

session = Session.live().activate()
session.start()
server = session.server

# %% [markdown]
# ## The instrument and its bus
# The bus is allocated like any other; the node reads it, and channel 0 of
# the MIDI input writes it.

# %%
bus = server.event_buses.alloc()
keys = Voices("default", 8, {"notes": bus})
server.midi_bind(0, bus)
server.sync()


# %% [markdown]
# ## Three writers, one instrument

# %%
def midi_chord():
    """An A major chord as MIDI 2.0 note-ons on channel 0, then their note-offs:
    the server reads them as it reads its MIDI input."""
    for key in (69, 73, 76):
        server.midi_ump(0x4090_0000 | key << 8, 0x2000_0000)
    time.sleep(1.5)
    for key in (69, 73, 76):
        server.midi_ump(0x4080_0000 | key << 8, 0)


def from_the_client():
    """Notes written on the bus from a routine, each at its beat."""
    for i, key in enumerate((81, 76, 72)):
        server.bus_event(bus, {"kind": "note_on", "key": key, "velocity": 0.15,
                               "note_id": i + 1})
        yield 0.5
    server.bus_event(bus, *({"kind": "note_off", "note_id": i + 1} for i in range(3)))


def run():
    print("MIDI on channel 0: a chord")
    midi_chord()
    time.sleep(1.0)

    print("a pattern: each note a voice of the node")
    pbind(instrument=keys, midinote=pseq([72, 76, 79, 84, 79, 76]), dur=0.25,
          amp=0.15).play()
    time.sleep(2.0)

    print("a pvoice line: one held voice moving through its notes")
    pvoice(keys, midinote=pseq([72, 74, 76, 77, 79]), dur=0.4, amp=0.15).play()
    time.sleep(2.5)

    print("the client writes the bus")
    Routine(from_the_client).play()
    time.sleep(2.5)

    print(f"nodes on the server: {server.status().synths} -- the Voices node alone")


# %%
def finish():
    server.midi_unbind(0)
    keys.free()
    server.event_buses.free(bus)
    session.close()


if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        finish()
else:
    print("session up - run() to hear it, finish() to end")
