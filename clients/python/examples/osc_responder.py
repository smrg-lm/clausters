#!/usr/bin/env python3
"""Receive OSC and relay it to the server — the client as an OSC hub.

Until now the client was output-only. `OscFunc` adds the **input** path: it
listens for OSC from *any* application and dispatches matching messages to a
callback, which here plays a synth on the Clausters server. This is the
client-side counterpart of driving the server directly — the server can be
played by OSC it receives itself, or by a client that listens to OSC from
elsewhere and forwards `/s_new`.

It also shows the **transport push** (the shared grid reacting live): the
receiver registers `/notify` on its own socket, so when any client sets the
server's `/transport`, the server pushes the new grid back and an
`OscFunc('/transport.reply')` re-aligns this client — no polling.

Start a server in one terminal::

    cargo run --release                 # or: clausters   (the installed binary)

then run this (it self-sends a few `/note` messages and one transport change to
demonstrate, but it will relay anything sent to its port from another app too)::

    python clients/python/examples/osc_responder.py

An external controller would simply send, e.g. ``/note 60 0.5`` to UDP port
57121 on this host.
"""

import sys
import time

from clausters import Session
from clausters.base import OscReceiver
from clausters.base import _osclib as osc
from clausters.responders import OscFunc

LISTEN_PORT = 57121


def main() -> None:
    # A live session to play into, and a clock to time note releases on (the
    # built-in "default" def gate-releases after each note's dur, ramping out).
    session = Session.live(tempo=1.0, latency=0.1)
    server = session.server
    session.start()

    recv = OscReceiver(port=LISTEN_PORT).start()
    # Register /notify *from the receiver's own socket*, so the server's
    # /transport.reply pushes land here and reach the responder below.
    recv.send(server.target.addr(), "/notify", 1)

    def play_note(msg, when, src):
        # msg == ["/note", midinote, dur]
        midinote, dur = msg[1], msg[2]
        freq = 440.0 * 2 ** ((midinote - 69) / 12)
        synth = server.synth("default", {"freq": freq, "amp": 0.2})
        session.clock.sched(dur, lambda: server.free(synth))
        print(f"  /note {midinote} -> freq {freq:.1f} Hz for {dur:.2f}s")

    def on_transport(msg, when, src):
        # msg == ["/transport.reply", origin_sample, tempo, defined]
        origin, tempo, defined = msg[1], msg[2], msg[3]
        if defined:
            print(f"  transport changed: origin={origin} tempo={tempo} -> re-aligning")
            session.clock.join_transport(server)

    OscFunc(play_note, "/note", recv=recv)
    OscFunc(on_transport, "/transport.reply", recv=recv)

    print(f"listening for OSC on 127.0.0.1:{LISTEN_PORT} (/note <midinote> <dur>)")

    # --- self-demo: act as an external app sending into our own port ---
    feeder = OscReceiver().start()  # any socket; only used to send
    for note in (60, 64, 67, 72):
        feeder.send(("127.0.0.1", LISTEN_PORT), "/note", note, 0.4)
        time.sleep(0.5)

    # Act as the conductor: set the server transport. The server pushes the new
    # grid to /notify clients, so on_transport above fires.
    print("setting the shared transport (conductor)...")
    server.set_transport(origin_sample=0, tempo=2.0)
    time.sleep(0.3)

    feeder.stop()
    recv.stop()
    session.stop()
    session.close()
    print("done")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
