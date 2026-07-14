#!/usr/bin/env python3
"""The simplest possible sound: boot a server, play a note.

No `Session`, no clock wiring. ``Server.boot()`` launches a server and adopts it
as the **default session**, so ``Event().play()`` and the free-standing
``play`` find it on their own. A bare event outside any clock plays immediately
and frees itself after its sustain.

Run it from a venv where the client is installed
(``pip install ./clients/python``)::

    python clients/python/examples/hello_note.py

or interactively, line by line::

    python -i clients/python/examples/hello_note.py
"""

import sys
import time

from clausters import Server, Event, play
from clausters.seq import Pbind, Pseq


def main():
    # Launches a clausters process and becomes the default session's server.
    # Closed (and the process stopped) on interpreter exit.
    server = Server.boot()

    # One note, right now — resolved against the default session, no clock.
    play(Event(degree=0))           # or: Event(degree=0).play()
    time.sleep(1.0)

    # A short phrase. With no clock in context, `play` uses the default
    # session's clock, created and started for you.
    play(Pbind(instrument="default", degree=Pseq([0, 2, 4, 7]), dur=0.4))
    time.sleep(2.0)

    print("played; synths freed themselves after their sustain")
    server.close()


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
