#!/usr/bin/env python3
"""DAW-style transport over a static timeline: play, locate, loop, position.

A `Pbind` is a forward-only generator -- you cannot seek it. A `Timeline` is the
opposite: a static, editable list of timed items with random access by beat, so
a `Playhead` can offer real transport controls -- `play(at=…)`, `locate(beat)`,
`loop(start, end)`, `stop()` -- and report a song `position`.

This example captures a pattern into a timeline, edits it programmatically, then
drives it live with the playhead. Random access happens at the boundaries
(play/locate/loop); between them the playhead just scans forward.

`Session.live` boots an audio server if none is up, so this runs on its own:

    python clients/python/examples/timeline_transport.py
"""

import sys
import time

from clausters import Session
from clausters.seq import Event, Pbind, Playhead, Pseq, Timeline


def build_timeline() -> Timeline:
    # Capture a pattern into a static timeline ("bounce to a clip")...
    tl = Timeline.from_pattern(
        Pbind(instrument="default", degree=Pseq([0, 2, 4, 7]), dur=0.5, amp=0.2),
        dur=2.0,
    )
    # ...then edit it programmatically: drop an accent on the downbeat.
    tl.add(0.0, Event(instrument="default", degree=7, dur=0.5, amp=0.3))
    return tl


def main() -> None:
    timeline = build_timeline()
    print(f"timeline: {len(timeline)} items over {timeline.duration()} beats")

    with Session.live(tempo=2.0, latency=0.1) as session:
        head = Playhead(timeline, session.clock, session.server)
        session.start()

        # Play from the top.
        head.play(at=0.0)
        time.sleep(1.2)
        print(f"position after ~1.2 s: beat {head.position():.2f}")

        # Locate (seek) to beat 1.0 and keep playing from there -- the random
        # access a generator could never do.
        head.locate(1.0)
        print("located to beat 1.0")
        time.sleep(1.0)

        # Loop the first two beats a few times.
        head.loop(0.0, 2.0).play(at=0.0)
        print("looping [0, 2)")
        time.sleep(3.0)

        head.stop()
        print(f"stopped at beat {head.position():.2f}")

    print("done")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
