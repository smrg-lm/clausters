#!/usr/bin/env python3
"""The **score editor**: a document somebody else typed, operated on, edited
by hand, played, saved and rendered.

The notation example, whole. It opens a document in the score editor -- the
application the shared crate writes once, the same window a page opens and a
standalone host opens -- and goes through everything a score is here: a model
the script operates on, a page a hand edits, a sound the server plays, a file,
and a sequence a roll can take.

What it shows, in the order it does it:

* **A document becomes a model.** The phrase below is ABC. Read into the
  model (`clausters.gui.notation.Score.sheet`), everything the model can do
  applies to it -- and the editor writes the page from that model when it
  opens, so a press on a note names an item of it at once.
* **What was typed once can be operated on.** The bass staff under the typed
  line is *made here*, with the model's operators: a copy transposed down two
  octaves, re-clefed and stacked under the line. The title, a double bar, a
  system break, a slur, a crescendo and two dynamics are written into the
  model the same way. These are the script's edits, made through the score
  before the window opens; the same operators are the window's Transform
  menu.
* **`edit(score)` opens the editor over the very score the script holds.**
  It returns the editor, as `edit` does for every structure; the edited data is
  read on the score passed in, which *is* the edited one.
* **The window is the application's.** Nothing in it is this script's:
  - the **menu bar** holds every action the editor has -- the file, the
    paper, the layout, the value a note is written with, the marks, the
    measures (a meter, a barline, a break), the transformations -- and an
    entry that ends in three dots opens a dialog over the window (the page's
    text, its margins, a transformation's parameter, a file's path);
  - the **toolbar** holds what a hand reaches for while it writes: the value
    the next note takes, its dot, whether it is a rest and its accidental (the
    input state, which is also `editor.value`, `editor.dotted`, `editor.rest`
    and `editor.next_accidental`), the articulations, a tie and a triplet for
    what is selected, the voice, the layout, and at the far edge the
    transport. Each tool is drawn with the engraver's own symbol, and with
    the editor's own drawing where the engraver's face has none (a tie, a
    barline, a hairpin);
  - the **palettes**, beside the page, are what can be written: a kind of
    element to a folding group -- accidentals, articulations, ornaments,
    lines, dynamics, measures -- each entry a verb over what is selected, and
    what it is said in a sentence when the pointer rests on it.
* **A gesture names a place; the editor names the note.** A click selects a
  note and the status line under the page says what it is. A drag moves it
  along the staff -- only a note drags; a slur or a time signature is selected
  and never displaced. A press on empty staff writes what the toolbar says:
  a note or a rest of the value in hand (an eighth, below), and selects it.
* **Several notes are selected at once.** Ctrl+click adds a note to the
  selection or takes it out; Shift+click extends the selection to the note
  clicked, in time and across the staves between. With entry switched off
  (Notes, Write notes), a press on a staff selects the measure it fell in.
* **A pick and a method call are one path.** Every entry of the menu, every
  tool and every palette entry is one of the editor's verbs, and each verb is
  a method: the fermata on the last note is put there by this script
  (`select`, `ornament`), as the Ornaments palette would.
* **The score plays on the server, and the page's cursor follows it.** The
  space bar, the toolbar's transport and the Play menu play it from where the
  selection starts -- several notes selected are the stretch a loop repeats
  -- on a transport of its own (`editor.play`, `editor.stop`). An edit made
  while it plays is heard on from where the position is.
* **The paper is the score's, and the way of looking at it is the window's.**
  The score opens as pages of A4, fixed whatever the window's size: drag on
  blank paper to pan, turn the wheel with Ctrl to zoom. File, Paper lays it
  out on another (an edit like any other, written into the MEI); View switches
  to one continuous system and back, which edits nothing.
* **The page has text of its own.** The title and its subtitle, the composer,
  a copyright line and a footnote are fields of the score, each written in a
  cell of the page's head or foot. A click on one says which field it is on
  the status line, and a double click types over it where it is drawn (Enter
  writes it, Escape leaves it); File, Page text edits them all.
* **A score is saved as itself.** The script saves it once (`editor.save`),
  so the file is the score's and Ctrl+S, or File, Save, writes it again
  without asking; File, Open reads another document in its place, as one step
  of the history.
* **A score is rendered into a sequence, one way.** `score.render_events()`
  is what plays, and `roll()` opens it in the notes editor: the notes, a
  channel to a voice, and the crescendo as each channel's automation. File,
  Export writes the same sequence as a MIDI file or a clip.
* **One undo order.** Ctrl+Z and Ctrl+Shift+Z over the window walk the
  editor's entries and the script's alike.

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
either staff -- to write an eighth there. The **space bar** plays and stops.
Close the window to stop. Needs an audio device, a display and a GPU. The file
it saves goes to ``clients/python/examples/out/``.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention):
step through it with Shift+Enter and the window stays up between cells, or run
it as a plain script.
"""

# %%
import os
import sys

from clausters import Session
from clausters.gui import edit, notation

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

# Where the score is saved: the examples' own directory for what a run leaves.
SAVED = os.path.normpath(os.path.join(
    os.path.dirname(os.path.abspath(__file__)), os.pardir, "out", "eight_bars.mei"))

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
# one system -- a brace and one barline through both. `transpose` and `stack`
# are two of the model's operators; the window's Transform menu holds them all.

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
# The window is the application's, whole: the menu bar, the toolbar, the
# palettes beside the page, the status line under it. The session is made
# ambient first, so the editor plays on its server without being told which,
# and its host is opened wired to that server: the page's cursor is drawn from
# the position of the transport the score plays on, which only a host that is
# a client of the server can read.

# %%
session = Session.live().activate()
session.gui()          # the host wired to this session's server

editor = edit(score, title="Score editor (a document, and its model)",
              width=1100, height=800)
editor.value = (1, 8)          # a press on empty staff writes an eighth

# %% [markdown]
# ## A pick and a method call are one path
# The Ornaments palette puts a fermata on what is selected. So does this: the
# last note of the line is selected and given one, as one step of the history
# -- Ctrl+Z over the window takes it back.

# %%
editor.select([f"n{top[-1]}"])
editor.ornament("fermata")
editor.select([])

# %% [markdown]
# ## The score's file
# Saved once, the file is the score's: Ctrl+S and File, Save write it again
# without asking. It is the score itself, as MEI -- notes, marks, the slur and
# the crescendo, the page and its text -- and reading it back is the same page.

# %%
os.makedirs(os.path.dirname(SAVED), exist_ok=True)
print(f"saved to {editor.save(SAVED)}")


# %%
editor.on_closed(lambda: print("window closed"))
print("click a note to select it (Ctrl adds, Shift extends), drag one up or "
      "down the staff, press empty staff to write an eighth; the space bar "
      "plays, and the menus, the toolbar and the palettes act on the selection")


# %% [markdown]
# ## The score as a roll
# `render_events` renders the score into a sequence, one way: the notes, each
# on its voice's channel, and the crescendo as the automation of both staves'
# channels. It is what the space bar plays, and `roll()` opens it in the notes
# editor -- where what is done to it stays in that sequence.

# %%
def roll():
    """The score as it stands, rendered and opened as a roll."""
    return edit(score.render_events(), title="The score, rendered")


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
        session.close()
else:
    print("editor up - run() to hold the window, roll() to see the score as a "
          "roll, session.close() to end")
