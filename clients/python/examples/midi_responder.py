#!/usr/bin/env python3
"""Drive the server from a MIDI keyboard — a MidiFunc turning notes into synths.

The MIDI counterpart of `osc_responder.py`: `MidiFunc` listens on a virtual MIDI
**input** port (other apps/devices route into it) and dispatches each message to
a callback. Here a note-on starts a synth on the Clausters server and the
matching note-off frees it — the client-side mirror of the server's own direct
MIDI path (a server can be played by MIDI it receives itself, or by a client
that listens to MIDI and forwards `/synth_new`).

Needs the live cdylib (the virtual input port):

    cargo build --release -p clausters-midi --features live

`Session.live` boots an audio server if none is up, so this runs on its own; it
opens a virtual MIDI input port named ``clausters-in``::

    python clients/python/examples/midi_responder.py [seconds]

That port is a loose cable until you wire a MIDI source into it. With the script
running, connect a keyboard (or any source) to it — list ports with ``pw-link
-o`` / ``-i`` and wire them with ``pw-link``, or visually in qpwgraph; with raw
ALSA, ``aconnect``. Play: each key sounds a synth until released.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention),
which is how a responder wants to be met: bring the port up in one cell, then
play your keyboard and watch the notes arrive.
"""

# %%
import sys
import time

from clausters import Session
from clausters.base import MidiReceiver
from clausters.responders import MidiFunc
from clausters.defs import Synth


# %% [markdown]
# ## The session and the virtual MIDI port
# `MidiReceiver` opens a virtual input any DAW or keyboard can be routed into.

# %%
session = Session.live(tempo=1.0, latency=0.05)
server = session.server

recv = MidiReceiver(port="clausters-in").start()
voices = {}  # active note number -> Synth

# %% [markdown]
# ## The responders
# One voice per held note, freed when the note-off arrives (or a note-on with
# velocity 0, which is running-status for the same thing).

# %%
def note_on(msg, src):
    if msg["velocity"] == 0:  # running-status note-off
        return note_off(msg, src)
    freq = 440.0 * 2 ** ((msg["note"] - 69) / 12)
    amp = msg["velocity"] / 127 * 0.3
    voices[msg["note"]] = Synth("default", {"freq": freq, "amp": amp},
                                    server=server)
    print(f"  note on  {msg['note']} ({freq:.1f} Hz)")


def note_off(msg, src):
    synth = voices.pop(msg["note"], None)
    if synth is not None:
        synth.free()
        print(f"  note off {msg['note']}")


MidiFunc(note_on, "note_on", recv=recv)
MidiFunc(note_off, "note_off", recv=recv)
print('listening on virtual MIDI port "clausters-in"; play your keyboard...')


# %%
def run(seconds: float = 20.0):
    """Hold the port open for ``seconds`` and free whatever is still sounding."""
    try:
        time.sleep(seconds)
    finally:
        for synth in list(voices.values()):
            synth.free()


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run(float(sys.argv[1]) if len(sys.argv) > 1 else 20.0)
    finally:
        recv.stop()
        session.close()
else:
    print("port open - play your keyboard; recv.stop(); session.close() to end")
