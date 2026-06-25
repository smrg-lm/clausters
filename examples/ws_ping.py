#!/usr/bin/env python3
"""Drive the server over OSC/WebSocket.

The same `Server` facade as the UDP/TCP examples, but talking **OSC over a
WebSocket** — the transport a browser can reach (it cannot open raw UDP or map
shared memory). The only change is the destination interface:
`Server(interface=OscWsInterface().start())`. The client's WebSocket lives in the
native core (`clausters-ffi`, reached by ctypes — same as the shm/embed
transports), so build that once, then start the server (WebSocket is always on,
like TCP/shm — no build feature):

    cargo build -p clausters-ffi               # the client's WebSocket lib (once)
    cargo run -- --ws                          # terminal 1 (OSC on WebSocket 57120)
    python3 examples/ws_ping.py                # terminal 2

Framing (in the native core, noted here only for reference): every OSC packet
goes out as one WebSocket *binary* message and replies arrive the same way over
the one connection — the WebSocket frame *is* the packet boundary, so there is no
length prefix (unlike TCP). The same server, the same OSC: only the carrier
changed, which is what lets a browser-hosted client speak to it. A browser doing
the same `/status` round trip is in `ws_ping.html` (it uses the browser's native
`WebSocket`, not this library).
"""

import os
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))

from clausters.base import OscWsInterface
from clausters.defs import Server, SynthDef, control, sin_osc, out


def main():
    host = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 57120

    server = Server(interface=OscWsInterface(host, port).start())
    try:
        print("status:", server.status())          # a binary-framed request/reply

        # Define a synth over WebSocket and play it for a moment.
        freq = control("freq", 440.0)
        amp = control("amp", 0.2)
        sig = sin_osc(freq) * amp
        name = server.add_synthdef(SynthDef("ws_beep", out(0.0, sig), out(1.0, sig)))
        print("added synthdef:", name)

        node = server.synth("ws_beep", {"freq": 330.0})
        server.sync()
        print("playing; synths =", server.status()[2])
        time.sleep(1.0)
        server.free(node)
        print("freed")
    finally:
        server.close()


if __name__ == "__main__":
    try:
        main()
    except (OSError, ConnectionError) as e:
        sys.exit(f"could not reach the server over WebSocket (start it with --ws): {e}")
