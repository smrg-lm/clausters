#!/usr/bin/env python3
"""Composing a score **algorithmically**: the sheet and its operations.

``score.py`` types a phrase by hand and ``score_from_data.py`` engraves a
timeline. This is the third thing, and the one the model exists for: a piece
built by *operating on a score*. A four-note motif is stated, and everything
after it is that motif under an operation -- repeated, inverted, turned
backwards, augmented, transposed, and finally stacked against itself.

The whole piece is written in six lines of algebra, and none of the arithmetic
is in Python. A **sheet** is a plain dict a caller holds, an operation is a
payload it sends, and both cross to `clausters_core::notation`, which is the
same core the web client binds and the same one a standalone host with no
client language uses to edit a score it opened. What this file contributes is
the names, and the order it puts them in.

Two things worth watching for as you read:

* **Spelling survives the algebra.** Inverting the motif about its own first
  note gives A-flat, not G-sharp, because a pitch keeps the letter its notehead
  sits on and an interval has a diatonic size as well as a chromatic one.
* **The grid does not move when the content does.** ``stretch`` doubles the
  written values against the barlines that were already there, so the phrase
  re-bars across them and ties where a value now overruns one -- which is what
  augmentation looks like on a page.
* **What the model holds, the page may refuse.** The last cell stacks the motif
  against itself as a second voice, which the algebra says fine and the engraver
  cannot write yet: the refusal names the milestone that owes it rather than
  quietly dropping a voice.

The engraver ships inside the package (``third_party/BUILD-VEROVIO.md``); in a
source checkout build and stage it once::

    third_party/build-verovio.sh
    python clients/python/build_native.py

Then, with the client importable::

    python clients/python/examples/notation/compose.py

A window shows the composed score; press **play** and the cursor follows the
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

# One beat per second, so an engraved millisecond is a beat/1000 -- score time
# and clock time are the same axis, which is what ties the cursor to the sound.
TEMPO = 1.0


# %% [markdown]
# ## The motif
# A sheet comes from a voice: the flat slot stream a client reduces its own data
# to. Four quarter notes, C-E-G-E, in 4/4. Ticks are 32nds on the way in and
# become exact durations in the model, where a quarter is `1/4`.

# %%
MOTIF = [{"midis": [pitch], "ticks": 8} for pitch in (60, 64, 67, 64)]
motif = notation.sheet_from_voice(MOTIF, meter="4/4", key="C")


def steps(sheet: dict) -> str:
    """The letters of a sheet's first voice, for reading a phrase in the log."""
    return " ".join(
        "".join(p["step"].upper() + "b" * -p["alter"] + "#" * p["alter"]
                for p in item.get("pitches", [])) or "-"
        for item in sheet["staves"][0]["voices"][0]["items"]
    )


print("motif      ", steps(motif))


# %% [markdown]
# ## Six operations, one piece
# Each line is the motif under one operation, and `concat` joins them in time.
# Read the phrases printed below against the page: they are the same music.

# %%
# The motif, then the same motif turned about its own first note.
turned = notation.invert(motif)
print("inverted   ", steps(turned))

# Backwards: the durations come back mirrored, so the phrase lasts as long.
backwards = notation.retrograde(motif)
print("retrograde ", steps(backwards))

# Twice as slow, against the barlines it already had.
slow = notation.stretch(motif, (2, 1))

# Up a major third -- E, not F-flat, because the interval has a diatonic size.
up = notation.transpose(motif, 4)
print("up a third ", steps(up))

# One after another: the piece.
piece = motif
for section in (turned, backwards, slow, up):
    piece = notation.concat(piece, section)

print(f"the piece is {notation.to_mei(piece).count('<measure')} measures")


# %% [markdown]
# ## What the model holds and the page cannot yet show
# The algebra can already say *counterpoint*: `stack` puts the motif against
# itself an octave down as a second voice on the same staff. Writing two voices
# out is the emission milestone, though, so the engraver's door refuses it -- by
# name, saying which it is, because a caller reading "cannot" needs to know
# whether it is wrong or early. That refusal is the feature: the alternative is
# a page that quietly drops a voice.

# %%
coda = notation.stack(motif, notation.transpose(motif, -12))
print(f"the coda holds {len(coda['staves'][0]['voices'])} voices")
try:
    notation.to_mei(coda)
except ValueError as refusal:
    print("and writing it out says:", refusal)


# %% [markdown]
# ## Engrave it
# `to_mei` writes the sheet out; from there it is the ordinary notation path --
# the same `Score` every other example uses.

# %%
score = notation.Score(notation.to_mei(piece), page_width=1600)
dl = score.display_list()
print(f"engraved {len(dl['notes'])} notes into {len(dl['prims'])} primitives")


# %% [markdown]
# ## The window

# %%
def playback_timeline(notes: list) -> Timeline:
    """Place the **engraved** notes on a timeline to play them: their ``t`` and
    ``dur`` are the score's own timemap, so the sound runs on the clock the
    cursor reads."""
    timeline = Timeline()
    for note in notes:
        timeline.add(note["t"] / 1000.0,
                     Event(midinote=note["pitch"], dur=note["dur"] / 1000.0,
                           amp=0.11))
    return timeline


def scene(display_list: dict, sample_rate: float) -> dict:
    """A minimal transport over the composed page. Every widget is *named*, so
    the script drives it by name and never picks an id."""
    return view(
        panel(button(name="play", label="play"),
              button(name="stop", label="stop"),
              layout="row", h=34.0),
        notation.score_view(display_list, name="score",
                            width=880.0, sample_rate=sample_rate),
        layout="col", title="A score composed by operating on one",
        w=920, h=460,
    )


# %% [markdown]
# ## Open it

# %%
session = Session.live(tempo=TEMPO)
server = session.server
sr = float(server.options.sample_rate)
gui = session.gui()
win = scene(dl, sr).open()
session.start()

# %%
playhead = Playhead(playback_timeline(dl["notes"]), session.clock, server)


def play():
    playhead.play(at=0.0)
    # anchor the cursor: the clock now, plus the play latency, is score 0
    _, args = server.request("/clock_query", expect=("/clock_query.reply",))
    now = float(args[0]) + server.latency * sr
    win["score"].set(playhead_at=now)


def stop():
    playhead.stop()
    win["score"].set(playhead_at=-1.0, playhead=0.0)


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
