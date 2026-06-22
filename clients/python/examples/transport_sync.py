#!/usr/bin/env python3
"""Two clients phase-aligned on one server's shared transport.

A server hosts a **transport** — a beat grid `(origin_sample, tempo)` it stores
under `/transport`. Several independent clients can *join* that grid, so a
`quant`-ed routine on each starts on the **same** beat. When each client is also
locked to the server's sample clock (`lock_to`), that alignment is sample-exact;
in plain wall-clock mode it is beat-accurate (drift-bounded).

Start a server first:

    cargo run --release                 # or the installed `clausters` binary

then:

    python clients/python/examples/transport_sync.py

This runs in a single process for clarity, but the two `Server` / `TempoClock`
pairs are completely independent — exactly the state two separate programs would
hold. It prints the next-bar sample each client computes (they match: that *is*
the alignment) and plays one note on each at that bar, so the two sound together.
"""

import math
import sys

from clausters.base import Routine, TempoClock
from clausters.defs import Server
from clausters.seq import Event


def make_client():
    """An independent client: its own server connection and clock, locked to the
    server's sample clock and joined to the shared transport."""
    server = Server()                 # UDP to 127.0.0.1:57110
    clock = TempoClock(tempo=1.0)     # the transport overwrites this tempo
    clock.lock_to(server)             # sample-exact, drift-free timing
    clock.join_transport(server)      # adopt the shared beat grid
    return server, clock


def next_bar_sample(server, clock, quant=4):
    """The absolute sample the clock's next `quant`-beat bar falls on — computed
    from public state only, so it is the *same* number for every client on the
    same transport."""
    origin, tempo = server.transport()
    rate = clock.timebase.sample_rate
    grid_beat = (clock.timebase.current_sample() - origin) * tempo / rate
    target = math.ceil(grid_beat / quant) * quant
    return round(origin + target * rate / tempo)


def one_note(server, freq):
    def routine():
        Event(freq=freq, amp=0.2, dur=0.5).play(server)
        yield 0.5
    return routine


def main():
    # The "conductor" defines the shared grid once: beat 0 at sample 0, 2 bps.
    Server().set_transport(0, 2.0)

    (sa, ca), (sb, cb) = make_client(), make_client()

    # Sampled back-to-back, both clients see the same next bar — the alignment.
    bar_a, bar_b = next_bar_sample(sa, ca), next_bar_sample(sb, cb)
    print(f"client A next bar -> sample {bar_a}")
    print(f"client B next bar -> sample {bar_b}")
    print("aligned to the sample" if abs(bar_a - bar_b) <= 2 else "NOT aligned")

    # Play one note on each, quantized to the next bar (quant=4): the routines
    # start on the same beat, so the notes sound together. Start each clock
    # before playing so quant snaps against the running, transport-locked grid.
    for clock, server, freq in ((ca, sa, 440.0), (cb, sb, 660.0)):
        clock.start()
        clock.play(Routine(one_note(server, freq)), quant=4)

    ca.run(3.0)        # let the bar arrive and the notes play, then wind down
    cb.stop()
    for s in (sa, sb):
        s.close()
    print("played; the two notes landed on the same bar")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
