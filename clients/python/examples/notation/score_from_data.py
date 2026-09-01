#!/usr/bin/env python3
"""Engraving **sequencing data** as a score, and hearing the score back.

The companion to ``score.py``, which types a phrase by hand. Here the score
is not typed at all -- it is generated from the client's own data. A `Timeline`
of `Event`s (a chord progression under a melody) is turned into a score by
`clausters.gui.notation.sheet_from_timeline`, engraved into the `score` display list,
and shown in the window: chords stacked on the beat, the melody above them,
rests where the data leaves gaps.

This is the inverse of the usual notation flow -- the events *are* the source
and the score is the view of them (data -> score). What is then **played** is
not the source timeline but the **score**, read back by
`clausters.gui.notation.to_timeline`: the round trip closes here, and it is a
round trip rather than a copy, because the notation carries what the events had
no way to say and the events carried what the page has no way to hold. Going
out, the exact onsets snap to written values; coming back, a written value
becomes an exact duration and a symbol becomes a decision -- which is the
interpreter's, and is data a caller can replace
(`clausters.gui.notation.interpretation`). The piece you hear is the piece you
see, cursor locked to it.

**The events say more than pitch and length.** Each one here also carries what
its note is on a *page* -- an articulation, a dynamic, an ornament, a spelling
-- and those are the keys the engraving reads and `to_timeline` puts back. They
are musical facts and not drawing instructions, which is what lets the same key
be read in both directions: the staccato in bar 1 is written as a staccato,
honoured as a shorter sound, and comes back as a staccato rather than as the
length it produced. Watch the two eighths in bar 1 (dots), the accent in bar 3,
the D flat near the end (a printed accidental, and not the C sharp a bare MIDI
number would have been spelled as), and the dynamics under the staff.

The page here is a **read-only view** -- the default: a drag on a note does
nothing, because this script does not apply edits. ``score.py`` is the other
half, an *editor* that passes ``editable=True`` and handles the ``"transpose"``
round trip. Editing is opt-in precisely so a plain plot like this one never
offers a gesture it cannot fulfil.

The engraver ships inside the package (``third_party/BUILD-VEROVIO.md``); in a
source checkout build and stage it once::

    third_party/build-verovio.sh
    python clients/python/build_native.py

Then, with the client importable::

    python clients/python/examples/notation/score_from_data.py

A window shows the engraved timeline; press **play** and the cursor follows the
sound. Close the window to stop. Needs an audio device, a display and a GPU.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention):
step through it with Shift+Enter and the window stays up between cells, or run
it as a plain script.
"""

# %%
import sys

from clausters import Event, Session
from clausters.gui import button, notation, panel, view
from clausters.seq.timeline import Playhead, Timeline

# Two beats per second: the quarter = 120 the engraver times the page at. Score
# time and clock time are then the same axis, which is what ties the cursor to
# the sound -- a quarter is one beat in the model and half a second on the page.
TEMPO = 2.0


# %% [markdown]
# ## The source timeline

# %%
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
    # A melody above them: quarters and eighths, with a rest left in bar 2.
    # The fourth field is what the note says on the *page* -- an articulation,
    # a dynamic, a spelling. None of it changes what an Event does when it is
    # played; it changes what is written when the timeline is engraved, and it
    # comes back on the event when the page is read.
    melody = [
        (0, 72, 1.0, {"dynamic": "mf"}),
        (1, 74, 0.5, {"articulations": ["stacc"]}),
        (1.5, 76, 0.5, {"articulations": ["stacc"]}),
        (2, 77, 1.0, {}),
        (3, 76, 1.0, {}),
        (4, 74, 1.0, {"dynamic": "p"}),
        # Held for half its value with nothing written to say so: the page
        # carries the length itself, since no symbol explains it.
        (6, 72, 1.0, {"sustain": 0.5}),
        (7, 74, 1.0, {}),
        (8, 76, 1.0, {"dynamic": "f", "articulations": ["marc"]}),
        (9, 79, 1.0, {}),
        (10, 77, 2.0, {"articulations": ["ten"]}),
        (12, 76, 1.0, {"dynamic": "mp"}),
        (13, 74, 0.5, {}),
        # A chromatic neighbour leaning on the C below it. In C major a bare 73
        # would be spelled as a C sharp; a D flat is what it *is*, and the sign
        # is asked for so a reader sees it.
        (13.5, 73, 0.5, {"spelling": "flat", "accidental": "written"}),
        (14, 72, 2.0, {"ornament": "fermata"}),
    ]
    for beat, pitch, dur, written in melody:
        tl.add(beat, Event(midinote=pitch, dur=dur, amp=0.14, **written))
    return tl


# %% [markdown]
# ## The window

# %%
def scene(display_list: dict, sample_rate: float) -> dict:
    """A minimal transport over the engraved page. Every widget is *named*, so
    the script drives it by name and never picks an id."""
    return view(
        panel(button(name="play", label="play"),
              button(name="stop", label="stop"),
              layout="row", h=34.0),
        notation.score_view(display_list, name="score",
                            width=880.0, sample_rate=sample_rate),
        layout="col", title="Engraved from a Timeline (data -> score)",
        w=920, h=420,
    )


# %% [markdown]
# ## Engrave it
# The score is generated from the timeline, not typed. `from_timeline` groups the
# events sharing a beat into chords and fills the gaps with rests; the melody's
# durations become the written note values (a 2-beat note a half, a 0.5-beat note
# an eighth). One beat is a quarter (``beat_unit=4``).

# %%
source = build_timeline()
# Stop at the **model** rather than at the MEI: the sheet is what is engraved
# *and* what is read back into sound, so both directions leave from one place.
sheet = notation.sheet_from_timeline(source, meter="4/4", key="C", beat_unit=4)
score = notation.Score(notation.to_mei(sheet), page_width=1600)
dl = score.display_list()
print(f"engraved {len(dl['notes'])} notes into {len(dl['prims'])} primitives")

# %% [markdown]
# ## Open it

# %%
session = Session.live(tempo=TEMPO)
server = session.server
# `query_info` rather than the launch options: it is the one spelling both
# clients have, so this file and its page twin ask the same question.
sr = server.query_info().nominal_sample_rate
gui = session.gui()
win = scene(dl, sr).open()
session.start()

# %% [markdown]
# ## Transport
# Play the **score**, not the source timeline: the cursor rides the engraving's
# timemap, so the sound must run on the same time base -- and what makes it the
# score rather than a copy of the source is that the symbols are read, not the
# events replayed.

# %%
playhead = Playhead(notation.to_timeline(sheet), session.clock, server)


def play():
    playhead.play(at=0.0)
    # anchor the cursor: the clock now, plus the play latency, is score 0
    _, args = server.request("/clock_query", expect=("/clock_query.reply",))
    now = float(args[0]) + server.latency * sr
    win["score"].set(playhead_at=now)


def stop():
    playhead.stop()
    win["score"].set(playhead_at=-1.0, playhead=0.0)


# Wire the two buttons by name: act on the press (1), ignore the release.
win["play"].on_click(play)
win["stop"].on_click(stop)
closed = [False]
win.on_closed(lambda: (closed.__setitem__(0, True), print("window closed")))
print("press play -- the cursor follows the sound; close the window to stop")


# %%
def run():
    """Pump the host until the window closes."""
    while not closed[0]:
        gui.pump(timeout=0.1)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        session.close()
else:
    print("score up - play(), stop(), run() to pump; session.close() to end")
