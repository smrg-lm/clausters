#!/usr/bin/env python3
"""Engraving **sequencing data** as a score: `notation.from_timeline`.

The companion to ``gui_score.py``, which types a phrase by hand. Here the score
is not typed at all -- it is generated from the client's own data. A `Timeline`
of `Event`s (a chord progression under a melody) is turned into MEI by
`clausters.gui.notation.from_timeline`, engraved into the `score` display list,
and shown in the window: chords stacked on the beat, the melody above them,
rests where the data leaves gaps.

This is the inverse of the usual notation flow -- the events *are* the source
and the score is the view of them (data -> score). What is then **played** is
the engraved score's own ``notes`` layer, not the source timeline: engraving
carries its own tempo, so anchoring the playback cursor to the sound means
playing what the timemap timed, exactly as ``gui_score.py`` does. The piece you
hear is the piece you see, cursor locked to it.

The engraver ships inside the package (``third_party/BUILD-VEROVIO.md``); in a
source checkout build and stage it once::

    third_party/build-verovio.sh
    python clients/python/build_native.py

Then, with the client importable::

    python clients/python/examples/gui_score_from_data.py

A window shows the engraved timeline; press **play** and the cursor follows the
sound. Close the window to stop. Needs an audio device, a display and a GPU.
"""

import sys

from clausters import Event, Session
from clausters.gui import button, notation, panel, window
from clausters.seq.timeline import Playhead, Timeline

# One beat per second, so an engraved millisecond is a beat/1000 -- score time
# and clock time are the same axis, which is what ties the cursor to the sound.
TEMPO = 1.0

# Widget ids (none is 1: that is the def's own id, taken by the window root).
BAR, PLAY, STOP = 2, 3, 4
SCROLL, SCORE = 10, 11


def build_timeline() -> Timeline:
    """A four-bar timeline built in code: a triad on each downbeat with a melody
    running above it. The melody's rests and the chord onsets are exactly what
    the engraving draws -- the data is the score."""
    tl = Timeline()
    # a chord under each bar (I - IV - V - I in C), whole-note durations
    for beat, triad in [(0, (60, 64, 67)), (4, (60, 65, 69)),
                        (8, (62, 67, 71)), (12, (60, 64, 67))]:
        for pitch in triad:
            tl.add(beat, Event(midinote=pitch, dur=4.0, amp=0.08))
    # a melody above them: quarters and eighths, with a rest left in bar 2
    melody = [(0, 72, 1.0), (1, 74, 0.5), (1.5, 76, 0.5), (2, 77, 1.0),
              (3, 76, 1.0), (4, 74, 1.0), (6, 72, 1.0), (7, 74, 1.0),
              (8, 76, 1.0), (9, 79, 1.0), (10, 77, 2.0),
              (12, 76, 1.0), (13, 74, 1.0), (14, 72, 2.0)]
    for beat, pitch, dur in melody:
        tl.add(beat, Event(midinote=pitch, dur=dur, amp=0.14))
    return tl


def playback_timeline(notes: list) -> Timeline:
    """Place the **engraved** notes on a timeline to play them (their ``t``/
    ``dur`` are the score's own timemap, in ms -> beats/1000, so the sound runs
    on the same clock the cursor reads). Chords fall on the same beat and simply
    stack, as they were engraved."""
    timeline = Timeline()
    for note in notes:
        timeline.add(note["t"] / 1000.0,
                     Event(midinote=note["pitch"], dur=note["dur"] / 1000.0,
                           amp=0.11))
    return timeline


def scene(display_list: dict, sample_rate: float) -> dict:
    """A minimal transport over the engraved page."""
    return window(
        panel(BAR,
              button(PLAY, label="play"),
              button(STOP, label="stop"),
              layout="row", h=34.0),
        notation.score_view(display_list, scroll_id=SCROLL, score_id=SCORE,
                            width=880.0, sample_rate=sample_rate),
        layout="col", title="Engraved from a Timeline (data -> score)",
        w=920, h=420,
    )


def main():
    source = build_timeline()

    # The score is generated from the timeline, not typed. `from_timeline` groups
    # the events sharing a beat into chords and fills the gaps with rests; the
    # melody's durations become the written note values (a 2-beat note a half,
    # a 0.5-beat note an eighth). One beat is a quarter (`beat_unit=4`).
    score = notation.Score.from_timeline(source, meter="4/4", key="C",
                                         beat_unit=4, page_width=1600)
    dl = score.display_list()
    print(f"engraved {len(dl['notes'])} notes into {len(dl['prims'])} primitives")

    with Session.live(tempo=TEMPO) as session:
        server = session.server
        sr = float(server.options.sample_rate)
        gui = session.gui()
        gui.define(1, scene(dl, sr))
        session.start()

        # Play the engraved score, not the source timeline: the cursor rides the
        # engraving's timemap, so the sound must run on the same time base.
        playhead = Playhead(playback_timeline(dl["notes"]), session.clock, server)

        def play():
            playhead.play(at=0.0)
            # anchor the cursor: the clock now, plus the play latency, is score 0
            _, args = server.request("/clock", expect=("/clock.reply",))
            now = float(args[0]) + server.latency * sr
            gui.set(SCORE, playhead_at=now)

        def stop():
            playhead.stop()
            gui.set(SCORE, playhead_at=-1.0, playhead=0.0)

        print("press play -- the cursor follows the sound; close the window to stop")
        buttons = {PLAY: play, STOP: stop}
        while True:
            msg = gui.poll(timeout=0.1)
            if msg is None:
                continue
            addr, args = msg
            if addr == "/gui_closed":
                print("window closed")
                break
            if addr == "/gui_event" and len(args) >= 2 and args[0] in buttons:
                if args[1] == 1:
                    buttons[args[0]]()


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ValueError) as e:
        sys.exit(str(e))
