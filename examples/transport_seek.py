#!/usr/bin/env python3
"""Seeking and looping a buffer by moving the **transport**, not the reader.

A buffer player normally carries its own position: it starts at frame 0 and
runs. This shows the other shape — a reader whose phase *is* the transport's
position in the piece (`TransportPos` -> `BufRd`), so the three things an
editor wants belong to the transport and not to the def:

- **seek** is `/transport_locateSample`, which moves where the piece is;
- **loop** is `/transport_loop`, a half-open span the engine wraps inside, so
  nothing is sent once a pass completes and there is no seam to hear;
- **pause** is `/transport_stop` over a governed group (`/transport_group`),
  which freezes the subtree *and* the position, so playing again continues
  instead of restarting.

That is what makes a multitrack possible: many readers, one time. Nothing here
keeps a position in step with anything, because there is only one position and
it is the server's.

The material is four one-second tones at different pitches, so every move is
audible immediately: locate to the third second and the third pitch is what
comes out. The script narrates what it is doing and prints the position it
reads back from `/transport_query` beside it — the two should agree.

Needs an audio device (it boots its own server and plays through the sound
card). Run it:

    python3 examples/transport_seek.py

`docs/sample-clock.md` explains the transport's two quantities: the clock,
which counts elapsed samples and never jumps, and the position, which is where
the piece is and does nothing but.
"""

import math
import os
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, os.path.join(ROOT, "clients", "python"))

from clausters import Session  # noqa: E402
from clausters.defs import Buffer, Group, Synth, SynthDef  # noqa: E402
from clausters.defs import buf_rd, control, out, transport_pos  # noqa: E402

SR = 48000
SECONDS = 4
TONES = [220.0, 277.183, 329.628, 440.0]  # one per second: A, C#, E, A


def material():
    """One second of each tone, with a short fade at every seam so the joins
    are not clicks — the seam this example is about is the loop's, and a
    buffer full of steps would put a click at every one."""
    samples = []
    fade = int(0.01 * SR)
    for hz in TONES:
        for i in range(SR):
            env = min(1.0, i / fade, (SR - i) / fade)
            samples.append(0.25 * env * math.sin(2 * math.pi * hz * i / SR))
    return samples


def follower_def():
    """The reader: its phase is the transport's position, so it plays wherever
    the piece is standing.

    `offset` is where this material starts in the piece — 0 here, since the
    take *is* the piece — and it is subtracted inside `transport_pos` rather
    than after it, which is what keeps the position exact in a long piece (a
    signal is 32-bit, and beyond about six minutes at 48 kHz it can no longer
    count single frames).

    `loop` on the reader stays off: the wrapping is the transport's, and a
    reader that also wrapped would be a second opinion about where the piece
    is.
    """
    bufnum = control("bufnum", 0.0)
    offset = control("offset", 0.0)
    amp = control("amp", 1.0)
    take = buf_rd(bufnum, chan=0, phase=transport_pos(offset))
    return SynthDef("transport-follower", out(0, take * amp))


def report(server, what):
    """Where the server says the piece is, in seconds. It is read from the
    engine as of its last completed block, so it is the truth about what is
    coming out of the speaker rather than about what was last asked for."""
    state = server.transport_state()
    where = state["position_sample"] / SR
    loop = state["loop"]
    span = "" if loop is None else f"  loop {loop[0] / SR:.2f}-{loop[1] / SR:.2f}s"
    print(f"  {what:<34} position {where:6.2f}s{span}")


def main():
    with Session.live() as session:
        server = session.server

        buf = Buffer.alloc(SECONDS * SR, 1)
        buf.set_samples(material())
        follower_def().send(server)

        # No `set_transport` anywhere below: this piece is measured in frames,
        # and the transport needs a beat grid only for the commands that speak
        # beats. An editor has no tempo to declare and does not declare one.

        # The reader goes in a group of its own, and *that* is what the
        # transport governs. Binding the root group would freeze every sound
        # on the server, which is the whole session.
        monitor = Group()
        Synth("transport-follower", {"bufnum": buf.bufnum, "amp": 0.8}, target=monitor)
        server.transport_group(monitor.id)

        print("four one-second tones; every move below should be audible at once")

        server.transport_locate_sample(0)
        server.transport_play()
        time.sleep(1.2)
        report(server, "playing from the start")

        # Into the third tone, with material still ahead of it -- so the stop
        # and the resume below are heard, rather than falling past the end.
        server.transport_locate_sample(2 * SR)
        time.sleep(0.1)
        report(server, "seeked to the third tone")
        time.sleep(0.7)

        server.transport_stop()
        time.sleep(0.1)
        report(server, "stopped (silence, position held)")
        time.sleep(0.8)

        server.transport_play()
        time.sleep(0.1)
        report(server, "playing again: continues, not restarts")
        time.sleep(0.7)

        # A half-open span: the second tone, exactly once per pass, joined to
        # its own start with no repeated frame.
        server.transport_loop((1 * SR, 2 * SR))
        server.transport_locate_sample(1 * SR)
        for _ in range(3):
            time.sleep(1.0)
            report(server, "looping the second tone")

        server.transport_loop(None)
        time.sleep(1.5)
        report(server, "loop off: it played on past the seam")

        server.transport_stop()
        monitor.free()
        print("done")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
