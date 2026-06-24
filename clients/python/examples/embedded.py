#!/usr/bin/env python3
"""Play live from an *embedded* server running inside this process.

The third session flavour, next to ``offline_render.py`` (NRT) and
``live_udp.py`` (a separate server over UDP). ``Session.embed()`` opens the
whole Clausters server -- audio device and engine -- *in this process*, through
the native library bundled in the wheel. There is no socket and no separate
server process: OSC is delivered by function call. Yet the code above the
session is identical to the live/offline cases, because only the session factory
(the `Server`'s communication interface) changes -- the pattern and the clock do
not.

Just run it; nothing else needs to be started::

    python clients/python/examples/embedded.py

Contrast with the *standalone* server, which the wheel also ships as the
``clausters`` command (a separate process you point UDP/TCP clients, ``ShmClient``
or other machines at)::

    clausters            # start the standalone server, then use Session.live(...)

The embedded server is the batteries-included path: import it and make sound.
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
        amp=Pwhite(0.1, 0.2, seed=1),
    )


def main():
    # The embedded server runs in-process; `latency` still schedules each note a
    # touch ahead (via a wall-clock timetag the in-process server reads against
    # the same clock) so it sounds on time rather than late.
    with Session.embed(tempo=2.0, latency=0.1) as session:
        # The embedded server is reachable for direct queries too -- the same OSC
        # request/reply, only in-process. `interface.server` is the handle.
        print("embedded server:", session.server.interface.server.sample_rate, "Hz")
        session.play(phrase())
        session.run(3.5)  # advance the clock in real time, then stop
        print("played from the embedded server; synths freed after their sustain")
    # Leaving the `with` block closes the session, which shuts the embedded
    # server (and its audio device) down.


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError) as e:
        sys.exit(str(e))
