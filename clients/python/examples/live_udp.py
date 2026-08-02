#!/usr/bin/env python3
"""Play live over UDP from the *installed* package.

The live counterpart of ``offline_render.py``: the same ``Session`` / ``Pbind``
API, but a live RT session sends OSC over UDP to a running Clausters server. The
only thing that changes between offline and live is the session factory -- the
pattern and the clock are identical.

`Session.live` boots an audio server if none is up, so in a venv where the
client is installed (``pip install ./clients/python``) this runs on its own::

    python clients/python/examples/live_udp.py

The synths free themselves after each note's sustain, so nothing is left behind.
"""

import sys

from clausters import Session
from clausters.seq import Pbind, Pseq, Pwhite


def phrase() -> Pbind:
    return Pbind(
        instrument="default",
        degree=Pseq([0, 2, 4, 7, 4, 2], repeats=2),
        dur=0.25,
        amp=Pwhite(0.1, 0.2),
    )


def main():
    # Live over UDP (default 127.0.0.1:57110). `latency` schedules each note a
    # touch ahead via a wall-clock timetag so the server plays it on time.
    with Session.live(tempo=2.0, latency=0.1) as session:
        session.play(phrase())
        session.run(3.5)  # advance the clock in real time, then stop
        print("played live; synths freed themselves after their sustain")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
