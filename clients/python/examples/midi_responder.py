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

Start a Clausters server in one terminal::

    cargo run --release

then run this; it opens a virtual MIDI input port named ``clausters-in``::

    python clients/python/examples/midi_responder.py [seconds]

That port is a loose cable until you wire a MIDI source into it. With the script
running, connect a keyboard (or any source) to it — list ports with ``pw-link
-o`` / ``-i`` and wire them with ``pw-link``, or visually in qpwgraph; with raw
ALSA, ``aconnect``. Play: each key sounds a synth until released.
"""

import sys
import time

from clausters import Session
from clausters.base import MidiReceiver
from clausters.responders import MidiFunc
from clausters.defs import Synth


def main() -> None:
    seconds = float(sys.argv[1]) if len(sys.argv) > 1 else 20.0

    session = Session.live(tempo=1.0, latency=0.05)
    server = session.server

    recv = MidiReceiver(port="clausters-in").start()
    voices = {}  # active note number -> Synth

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
    try:
        time.sleep(seconds)
    finally:
        for synth in list(voices.values()):
            synth.free()
        recv.stop()
        session.close()


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
