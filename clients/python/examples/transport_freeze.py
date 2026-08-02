#!/usr/bin/env python3
"""Freeze a generative piece and hear that it *continued* rather than restarted.

A def running a seeded stochastic process is the case no DAW transport covers.
A DAW's material exists before you press play, so a position is an index into
it; here the material is produced as it sounds, so the piece's position **is**
the def's internal state and no number moves it. The only thing a transport can
honestly do to such a piece is stop it and let it carry on.

That is what this shows. `transport_group` binds a group to the server's
transport, and from then on `transport_stop` does three things at the same
sample: it freezes that subtree with every node's state intact, it stops the
transport clock, and it freezes the queue of anything scheduled against that
clock. `transport_play` undoes all three.

What to listen for: at the pause the texture stops **dead**, mid-gesture, and on
the resume it picks up from exactly there -- not from the top, and not from a
new random draw. Run it twice: the two takes are the same piece interrupted,
never two different pieces.

Start a server first:

    cargo run --release                 # or the installed `clausters` binary

then:

    python clients/python/examples/transport_freeze.py
"""

import time

from clausters import Session
from clausters.defs import Group, SynthDef, control, out
from clausters.defs.ugens import Env, env_gen, pan2, rlpf, white_noise


def cloud() -> SynthDef:
    """A grain of band-passed noise under a percussive envelope.

    Its randomness is seeded per instance, so the stream is reproducible --
    which is what makes "it continued" a claim you can check by ear rather than
    a feeling."""
    freq = control("freq", 800.0)
    amp = control("amp", 0.2)
    dur = control("dur", 0.4)
    env = env_gen(Env.perc(0.01, 1.0), time_scale=dur, done_action=2)
    grain = rlpf(white_noise(), freq, 0.2) * env * amp
    return SynthDef("cloud", out(0, pan2(grain, 0.0)))


def main() -> None:
    with Session.live(tempo=2.0, latency=0.1) as session:
        server = session.server
        cloud().send(server)
        server.sync()

        # One group holds the whole piece, and the transport governs it.
        piece = Group(server=server)
        server.set_transport(0, 2.0)
        server.transport_group(piece.id)
        server.transport_play()
        print(f"governing group {piece.id}; rolling")

        def grains(n, gap):
            for i in range(n):
                server.send_msg(
                    "/synth_new", "cloud", -1, 1, piece.id,
                    "freq", 400 + 90 * (i % 7), "amp", 0.18, "dur", 0.5,
                )
                time.sleep(gap)

        print("playing ~4 s -- listen to the texture")
        grains(28, 0.14)

        print("FREEZE (3 s of silence) -- the state is held, not discarded")
        server.transport_stop()
        st = server.transport_state()
        print(f"  transport clock held at {st['transport_sample']} samples")
        time.sleep(3.0)
        held = server.transport_state()
        print(f"  still {held['transport_sample']} -- it did not advance")

        print("RESUME -- the same texture carries on mid-gesture")
        server.transport_play()
        grains(28, 0.14)

        server.transport_group(None)
        print("unbound; done")


if __name__ == "__main__":
    main()
