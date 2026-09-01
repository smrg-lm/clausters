#!/usr/bin/env python3
"""Composing a score **algorithmically**: the sheet and its operations.

``score.py`` types a phrase by hand and ``score_from_data.py`` engraves a
timeline. This is the third thing, and the one the model exists for: a piece
built by *operating on a score*. A four-note motif is stated, and everything
after it is that motif under an operation -- repeated, inverted, turned
backwards, augmented, transposed, and finally stacked against itself as a coda
on two staves.

The whole piece is written in six lines of operators, and none of the arithmetic
is in Python. A **sheet** is a plain dict a caller holds, an operation is a
payload it sends, and both cross to `clausters_core::notation`, which is the
same core the web client binds and the same one a standalone host with no
client language uses to edit a score it opened. What this file contributes is
the names, and the order it puts them in.

Two things worth watching for as you read:

* **Spelling survives the operators.** Inverting the motif about its own first
  note gives A-flat, not G-sharp, because a pitch keeps the letter its notehead
  sits on and an interval has a diatonic size as well as a chromatic one.
* **The grid does not move when the content does.** ``stretch`` doubles the
  written values against the barlines that were already there, so the phrase
  re-bars across them and ties where a value now overruns one -- which is what
  augmentation looks like on a page.
* **What is written and what is heard are two things.** The coda's first note
  carries a staccato, which shortens it in performance and changes nothing about
  the page. Honouring that is the *interpreter's*, not the engraving's -- writing
  a shortened length into the document moves every attack after it -- and it is
  what `notation.to_timeline` does at the end of this file: it reads the sheet's
  symbols into events carrying both lengths.
* **A tuplet cannot be split.** Three triplet eighths are `1/12` each -- exact
  as a rational, impossible on any grid of 32nds -- and a group that would cross
  a barline is refused by name rather than written into bars nobody meant.

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

from clausters import Session
from clausters.gui import button, notation, panel, view
from clausters.seq.timeline import Playhead

# Two beats per second: the quarter = 120 the engraver times the page at. Score
# time and clock time are then the same axis, which is what ties the cursor to
# the sound -- a quarter is one beat here and half a second there.
TEMPO = 2.0


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
# ## A coda on two staves, and the marks on it
# `stack` puts the motif against itself an octave down. It goes on a **staff of
# its own** (`as_staff=True`), in the bass clef the register asks for: written as
# a second voice on the treble staff the same notes are correct and unreadable,
# a run of ledger lines under the line they answer. Two staves take a brace and
# one barline through both.
#
# The marks go on afterwards, and they are two different kinds of thing. What
# one note carries goes on the note; a slur has **two ends** and so lives beside
# the staves.
#
# What a staccato *means* for playback is deliberately not on the page: a
# shortened note is a performance decision, and writing it into the document
# makes an engraver's own clock advance by the shortened length, which pulls
# every attack after it earlier. The dot is written; honouring it is the
# interpreter's, and that is the next milestone.

# %%
lower = notation.transpose(motif, -12)
lower["staves"][0]["clef"] = "F4"          # the register asks for a bass clef
coda = notation.stack(motif, lower, as_staff=True)
print(f"the coda has {len(coda['staves'])} staves")

upper = coda["staves"][0]["voices"][0]["items"]
coda = notation.set_marks(coda, upper[0]["id"],
                          notation.marks(articulations=["stacc"], dynamic="mf"))
coda = notation.add_spanner(coda, "slur", upper[0]["id"], upper[-1]["id"])

piece = notation.concat(piece, coda)
print(f"with the coda: {notation.to_mei(piece).count('<measure')} measures, "
      f"{len(piece['spanners'])} slur")


# %% [markdown]
# ## What a page still refuses
# A tuplet cannot be split, so one that would cross a barline is refused by
# name rather than written into bars nobody meant. Three triplet eighths fill a
# quarter -- exact as `1/12` each, and impossible on any grid of 32nds.

# %%
triplet = notation.sheet_from_voice([{"midis": [72], "ticks": 8}])
triplet["staves"][0]["voices"][0]["items"] = [
    {"kind": "note", "id": i + 1, "pitches": [notation.pitch(step, 5)], "dur": [1, 12]}
    for i, step in enumerate(("c", "d", "e"))
]
print("a triplet writes as:",
      "num=\"3\" numbase=\"2\"" in notation.to_mei(triplet))

crossing = notation.concat(
    notation.sheet_from_voice([{"midis": [72], "ticks": 28}]), triplet)
try:
    notation.to_mei(crossing)
except ValueError as refusal:
    print("and one that would not fit says:", refusal)


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
# `query_info` rather than the launch options: it is the one spelling both
# clients have, so this file and its page twin ask the same question.
sr = server.query_info().nominal_sample_rate
gui = session.gui()
win = scene(dl, sr).open()
session.start()

# %%
# What plays is the **interpretation of the sheet**, not the engraved notes:
# `to_timeline` reads the page's own symbols -- the staccato shortens the sound
# and moves no attack, the downbeat is stressed, a dynamic governs the notes
# after it -- and hands back events carrying both lengths, ``dur`` written and
# ``sustain`` heard. Pass ``instruments=`` to say what plays each staff; left
# out, as here, they take the client's default, because the notation itself
# never says.
playhead = Playhead(notation.to_timeline(piece), session.clock, server)


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
