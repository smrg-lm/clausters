#!/usr/bin/env python3
"""Drive the server over OSC/TCP (client C8).

The `Server` facade talking **length-prefixed OSC over a TCP connection** — a
reliable, ordered, connection-oriented channel with no datagram-size limit.
Since C34 this is what `Server()` does by default; this example builds the
interface explicitly (`Server(interface=OscTcpInterface().start())`) to show
the seam, and `transport="udp"` is the way back to datagrams. The server
listens on TCP by default (same port as UDP; `--no-tcp` disables it):

    cargo run --release                          # terminal 1 (OSC on TCP 57110)
    python3 examples/tcp_client.py               # terminal 2

Framing (handled inside `OscTcpInterface`, shown here only for reference): every
OSC packet goes out as a 4-byte big-endian length followed by the bytes, and
replies arrive framed the same way over the one connection — identical to
scsynth's TCP. Timing still rides on bundle timetags / `/sched`, so using TCP
changes nothing about *when* scheduled commands fire.
"""

import os
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))

from clausters.base import OscTcpInterface
from clausters.defs import Server, SynthDef, control, sin_osc, out


def main():
    host = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 57110

    server = Server(interface=OscTcpInterface(host, port).start())
    try:
        print("status:", server.status())          # a framed request/reply

        # Define a synth over TCP and play it for a moment.
        freq = control("freq", 440.0)
        amp = control("amp", 0.2)
        sig = sin_osc(freq) * amp
        name = server.add_synthdef(SynthDef("tcp_beep", out(0.0, sig), out(1.0, sig)))
        print("added synthdef:", name)

        node = server.synth("tcp_beep", {"freq": 330.0})
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
        sys.exit(f"could not reach the server over TCP (is it running with --no-tcp?): {e}")
