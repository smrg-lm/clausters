#!/usr/bin/env python3
"""Editing a score **by hand**: the score editor, over a document somebody
else typed.

The third of the notation examples, and the one that closes the loop.
``score.py`` plays an engraved phrase and drags a note; ``compose.py`` builds a
score by operating on it; this one opens a document *somebody else typed* in
the **score editor** -- the application the shared crate writes once, the same
window a page opens and a standalone host opens -- and edits it the way a score
editor does: with the mouse on the page, and one of the editor's verbs behind
every button.

What it shows, roughly in the order it does it:

* **A document becomes a model.** The phrase below is ABC. Read into the
  model (`clausters.gui.notation.Score.sheet`), everything the model can do
  applies to it -- and the editor writes the page from that model when it
  opens, so a press on a note names an item of it at once.
* **What was typed once can be operated on.** The bass staff under the typed
  line is *made here*, by transposing a copy down two octaves, re-clefing it and
  stacking the two; the title, a double bar, a system break, a slur, a
  crescendo and two dynamics are written into the model the same way. These are the script's
  edits, made through the score before the window opens.
* **`edit(score)` opens the editor over the very score the script holds.**
  It returns the editor, as `edit` does for every structure; the edited data is
  read on the score passed in, which *is* the edited one.
* **A gesture names a place; the editor names the note.** A click selects a
  note and the status line under the page says what it is. A drag moves it
  along the staff -- only a note drags; a slur or a time signature is selected
  and never displaced. A press on empty staff writes a note of the editor's
  `value` there (an eighth, below) and selects it.
* **Several notes are selected at once.** Ctrl+click adds a note to the
  selection or takes it out; Shift+click extends the selection to the note
  clicked, in time and across the staves between. With `entry` switched off
  (the ``write`` button), a press on a staff selects the measure it fell in
  instead of writing a note.
* **The menu bar holds every action the editor has**, grouped as a score
  editor's are: the paper, the layout, the value a note is written with, the
  marks, the measures (a meter, a barline, a break), the transformations.
  Each entry is one of the editor's verbs, so a pick and a method call are one
  path.
* **The toolbar holds what a hand reaches for while it writes**: the value
  the next note takes, its dot, whether it is a rest and its accidental (the
  input state, which is also `editor.value`, `editor.dotted`, `editor.rest`
  and `editor.next_accidental`); the articulations, a tie and a triplet for
  what is selected; the voice of the selection; and the layout.
* **The verbs act on the selection.** Every button calls one method of the
  editor -- `move`, `scale`, `articulation`, `dynamic`, `ornament`,
  `clear_marks`, `tie`, `silence`, `delete`, `voice`, `spanner` -- and each is
  one call into the crate, which reads the score as it stands (an articulation
  is toggled against the ones the note has) and records one entry. A slur runs
  from the first selected note to the last, so it is made by selecting its two
  ends.
* **A transformation takes the measures the selection covers.** `transform`
  hands the model's operators -- here an octave up and a retrograde -- the
  span of what is selected, or everything with nothing selected.
* **The paper is the score's, and the way of looking at it is the
  window's.** The score opens as pages of A4, fixed whatever the window's
  size: drag on blank paper to pan, turn the wheel with Ctrl to zoom. `paper`
  lays it out on the next paper (`set_page`, an edit like any other, written
  into the MEI); `layout` switches to one continuous system and back, which
  edits nothing.
* **The page has text of its own.** The title and its subtitle, the composer,
  a copyright line and a footnote are fields of the score, each written in a
  cell of the page's head or foot -- where the printed page puts it, until
  `set_text` moves it. A click on one says which field it is on the status
  line.
* **One undo order.** Ctrl+Z and Ctrl+Shift+Z over the window walk the
  editor's entries and the script's alike, and so do the undo and redo
  buttons.

The engraver is **libverovio**, which ships inside the installed package. In a
source checkout, build and stage it once (``third_party/BUILD-VEROVIO.md``)::

    third_party/build-verovio.sh
    python clients/python/build_native.py

Then, with the client importable::

    python clients/python/examples/notation/score_editor.py

**Click** a note to select it; the status line says what is selected.
**Ctrl+click** another to add it, **Shift+click** one to select everything up
to it. **Drag** a note up or down the staff to move it (which is not
transposition: it takes the key signature's alteration for the letter it lands
on). **Press on empty staff** -- between two notes, or past the last one, on
either staff -- to write an eighth there; with **write** switched off the same
press selects the measure. The buttons act on the selection; **play** plays
the score as it stands. Close the window to stop. Needs an audio device, a display and a
GPU.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention):
step through it with Shift+Enter and the window stays up between cells, or run
it as a plain script.
"""

# %%
import sys

from clausters import Session, TempoMap
from clausters.gui import button, edit, notation, panel

# Eight bars in ABC -- a score as it usually arrives: typed by somebody else, in
# a format that is not ours. `M:` is the meter, `L:` the length a bare letter
# means, `K:` the key (E flat, so every B, E and A is flat and none of them
# carries a sign). A letter is a note, `/` halves it, a digit multiplies it and
# `|` bars it.
PHRASE = """X:1
T:Eight bars
M:4/4
L:1/4
K:Eb
G A B c | d2 c B | A G F G | E4 |
B c d e | f2 e d | c B A B | G4 |
"""

# Two beats per second, the quarter = 120 the page is timed at.
TEMPO = 2.0

# %% [markdown]
# ## Open it, and read it
# The engraver reads ABC, MusicXML and MEI through one loader and normalizes
# what it loaded, so there is one input format by the time the model sees it.

# %%
score = notation.Score(PHRASE, page_width=1100)
typed = score.sheet()          # raises if the document could not be read
print(f"read {sum(len(v['items']) for s in typed['staves'] for v in s['voices'])} "
      f"items out of a document that was only text")

# %% [markdown]
# ## A second staff, made rather than typed
# verovio's ABC importer writes one staff whatever the source says, so the
# grand staff below is not in the document: it is the model's. A copy of the
# line goes down two octaves, takes the bass clef, and `stack` puts the two on
# one system -- a brace and one barline through both.

# %%
lower = notation.transpose(typed, -24)
lower["staves"][0]["clef"] = "F4"
score.apply({"op": "stack", "sheet": lower, "as_staff": True})

# %% [markdown]
# ## The decisions that are the writer's, not the engraver's
# A title, a double bar dividing the two halves, a system break so the second
# half starts a line -- and two spans that no single note could carry: a slur
# over the opening figure and a crescendo under it. None of these changes a
# note; all of them are statements, and a statement is stored.

# %%
score.apply({"op": "set_header",
             "header": notation.header(title="Eight bars",
                                       subtitle="a document, and its model",
                                       composer="typed, then operated on",
                                       copyright="an example of clausters",
                                       notes=["* the bass staff was made, not typed"])})
score.apply({"op": "set_barline", "measure": 4, "kind": "dbl"})
score.apply({"op": "set_break", "measure": 5, "kind": "system"})

top = [item["id"] for item in score.sheet()["staves"][0]["voices"][0]["items"]]
score.apply({"op": "add_spanner", "kind": "slur", "from": top[0], "to": top[3]})
score.apply({"op": "add_spanner", "kind": "crescendo",
             "from": top[0], "to": top[7]})
score.apply({"op": "set_marks", "id": top[0],
             "marks": notation.marks(dynamic="p")})
score.apply({"op": "set_marks", "id": top[8],
             "marks": notation.marks(dynamic="f")})

# %% [markdown]
# ## The editor
# The window is the editor's: the page, in a scroll, over a status line. The
# rows of buttons are this script's own widgets (`extra`), each calling one of
# the editor's verbs.

# %%
session = Session.live()
server = session.server

buttons = [
    panel(button(name="play", label="play"),
          button(name="stop", label="stop"),
          button(name="up", label="up"),
          button(name="down", label="down"),
          button(name="longer", label="longer"),
          button(name="shorter", label="shorter"),
          layout="row", h=34.0),
    panel(button(name="stacc", label="staccato"),
          button(name="accent", label="accent"),
          button(name="tenuto", label="tenuto"),
          button(name="trill", label="trill"),
          button(name="mf", label="mf"),
          button(name="ff", label="ff"),
          button(name="plain", label="no marks"),
          layout="row", h=34.0),
    panel(button(name="slur", label="slur"),
          button(name="voice", label="other voice"),
          button(name="tie", label="tie"),
          button(name="silence", label="silence"),
          button(name="delete", label="delete"),
          button(name="undo", label="undo"),
          button(name="redo", label="redo"),
          layout="row", h=34.0),
    panel(button(name="write", label="write: on"),
          button(name="octave", label="octave up"),
          button(name="retro", label="retrograde"),
          button(name="layout", label="layout: page"),
          button(name="paper", label="paper: A4"),
          layout="row", h=34.0),
]

editor = edit(score, title="Score editor (a document, and its model)",
              width=960, height=760, extra=buttons)
editor.value = (1, 8)          # a press on empty staff writes an eighth
win = editor.window

# %% [markdown]
# ## Writing, or selecting a measure
# A press on empty staff means one of two things, and `entry` says which:
# write a note there, or select the measure the press fell in.

# %%
def toggle_entry() -> None:
    editor.entry = not editor.entry
    win["write"].set(label=f"write: {'on' if editor.entry else 'off'}")


# %% [markdown]
# ## The paper, and the way of looking at it
# Two switches that look alike and are not: the **layout** is this window's
# (pages, or one system that never breaks) and edits nothing; the **paper** is
# the score's, so changing it is an edit -- it is undone with the rest, and it
# is written into the MEI.

# %%
def toggle_layout() -> None:
    editor.layout = "continuous" if editor.layout == "page" else "page"
    win["layout"].set(label=f"layout: {editor.layout}")


def next_paper() -> None:
    """The next paper of the ones there are, upright."""
    setup = editor.page
    papers = setup["papers"]
    at = papers.index(setup["paper"]) if setup["paper"] in papers else -1
    paper = papers[(at + 1) % len(papers)]
    if editor.set_page(paper, landscape=False):
        win["paper"].set(label=f"paper: {paper}")


# %% [markdown]
# ## Playing what is written
# The timeline comes out of the **model**, not out of the engraving:
# `to_timeline` reads what the symbols mean, so a staccato added a moment ago
# is honoured and a dynamic governs the notes after it.

# %%
playing: dict = {"timeline": None}


def play() -> None:
    """The score as it stands right now, from the top."""
    stop()
    timeline = notation.to_timeline(score.sheet())
    timeline.map = TempoMap(TEMPO)
    playing["timeline"] = timeline.play(at=0.0, destination=server)


def stop() -> None:
    if playing["timeline"] is not None:
        playing["timeline"].stop()
        playing["timeline"] = None


# %% [markdown]
# ## Wire it up

# %%
win["play"].on_click(play)
win["stop"].on_click(stop)
win["up"].on_click(lambda: editor.move(1))
win["down"].on_click(lambda: editor.move(-1))
win["longer"].on_click(lambda: editor.scale(2, 1))
win["shorter"].on_click(lambda: editor.scale(1, 2))
win["stacc"].on_click(lambda: editor.articulation("stacc"))
win["accent"].on_click(lambda: editor.articulation("acc"))
win["tenuto"].on_click(lambda: editor.articulation("ten"))
win["trill"].on_click(lambda: editor.ornament("trill"))
win["mf"].on_click(lambda: editor.dynamic("mf"))
win["ff"].on_click(lambda: editor.dynamic("ff"))
win["plain"].on_click(editor.clear_marks)
win["slur"].on_click(lambda: editor.spanner("slur"))
win["voice"].on_click(editor.voice)
win["tie"].on_click(editor.tie)
win["silence"].on_click(editor.silence)
win["delete"].on_click(editor.delete)
win["undo"].on_click(editor.undo)
win["redo"].on_click(editor.redo)
win["write"].on_click(toggle_entry)
win["octave"].on_click(lambda: editor.transform("transpose", semitones=12))
win["retro"].on_click(lambda: editor.transform("retrograde"))
win["layout"].on_click(toggle_layout)
win["paper"].on_click(next_paper)
editor.on_closed(lambda: print("window closed"))
print("click a note to select it (Ctrl adds, Shift extends), drag one up or "
      "down the staff, press empty staff to write an eighth; the buttons act on "
      "the selection")


# %%
def run():
    """Hold the window open until it is closed. Nothing is driven here: the
    host's event loop delivers every gesture to the editor."""
    editor.wait()


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        stop()
        session.close()
else:
    print("editor up - run() to hold the window, session.close() to end")
