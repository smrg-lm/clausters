#!/usr/bin/env python3
"""Two transports on one server, each moving its own take and nothing else.

A server has several transports (`--transports`, 16 by default), and each is
independent: its own play, pause, locate, loop and governed group. This plays
two takes at once, one per transport, and moves them separately:

- the **left** channel is a take on transport 0 -- `server.transport_at(0)`,
  the one every transport method on a `Server` addresses;
- the **right** channel is the same four tones an octave up, on a transport
  taken for it -- `server.transport_new()`, since every transport above 0 is
  taken by what plays rather than picked by number. Each is a `Transport`,
  played as a timeline is: `play`, `pause`, `locate`, `loop`, in its own
  seconds.

Each reader follows its own transport's position (`TransportPos` -> `BufRd`)
without being told which one: a node reads the transport that governs its
group. So pausing the right side leaves the left one playing, a seek on one
moves only that side, and a loop on the left does not touch the right. The
script narrates each move and prints both positions as the server reports
them; the one that was not touched keeps going.

Needs an audio device (it boots its own server and plays through the sound
card), and headphones or two speakers to hear the sides apart. Run it:

    python3 examples/transports.py

`examples/transport_seek.py` is the one-transport version of the same reader;
`docs/sample-clock.md` explains what a transport's position and its clock are.
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


def samples(octave):
    """One second of each tone, `octave` octaves up, faded at every seam so a
    join is not a click."""
    samples = []
    fade = int(0.01 * SR)
    for hz in TONES:
        hz *= 2 ** octave
        for i in range(SR):
            env = min(1.0, i / fade, (SR - i) / fade)
            samples.append(0.25 * env * math.sin(2 * math.pi * hz * i / SR))
    return samples


def follower_def():
    """The reader: its phase is its transport's position. Which transport is
    not a control -- it is whichever governs the group the reader is in."""
    bufnum = control("bufnum", 0.0)
    side = control("out", 0.0)
    take = buf_rd(bufnum, chan=0, phase=transport_pos(0.0))
    return SynthDef("transports-follower", out(side, take))


def report(left, right, what):
    """Both transports' positions, in seconds, as the engine last played
    them."""
    at = [t.state()["position_sample"] / SR for t in (left, right)]
    print(f"  {what:<40} left {at[0]:5.2f}s   right {at[1]:5.2f}s")


def main():
    with Session.live() as session:
        server = session.server
        follower_def().send(server)

        # One transport per side, each an object whose verbs are its own.
        left = server.transport_at(0)
        right = server.transport_new()
        print(f"the server has {server.query_info().transports} transports")

        # A group per transport, each holding one reader, each bound to its
        # own transport: that binding is what the reader follows, and what a
        # stop freezes.
        for transport, octave, side in ((left, 0, 0), (right, 1, 1)):
            buf = Buffer.alloc(SECONDS * SR, 1)
            buf.set_samples(samples(octave))
            group = Group()
            Synth("transports-follower", {"bufnum": buf.bufnum, "out": side}, target=group)
            transport.group(group)
            transport.locate(0.0)

        print("four one-second tones on each side, the right an octave up")
        left.play()
        right.play()
        time.sleep(1.2)
        report(left, right, "both playing from the start")

        right.pause()
        time.sleep(0.8)
        report(left, right, "right stopped: the left plays on")

        right.play(at=3.0)
        time.sleep(0.5)
        report(left, right, "right seeked to its fourth tone")

        # A loop on the left's second tone; the right goes on to its end.
        left.loop(1.0, 2.0)
        left.locate(1.0)
        # Read off the loop's beat, so each report lands somewhere else in it.
        for _ in range(3):
            time.sleep(0.7)
            report(left, right, "left looping its second tone")

        left.unloop()
        left.pause()
        right.pause()
        right.free()
        print("done")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
