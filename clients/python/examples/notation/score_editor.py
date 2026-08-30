#!/usr/bin/env python3
"""Editing a score **by hand**: the page, the model, and every verb between them.

The third of the notation examples, and the one that closes the loop.
``score.py`` plays an engraved phrase and drags a note; ``compose.py`` builds a
piece by operating on it; this one opens a document *somebody else typed* and
edits it the way a score editor does -- with the mouse on the page, and one of
the model's verbs behind every gesture.

What it shows, roughly in the order it does it:

* **A document becomes a model.** The phrase below is ABC. Until it is *read*
  (`clausters.gui.notation.Score.sheet`) it is a picture: it draws, it plays,
  and not one verb applies to it. Reading is the whole difference between a
  score you can look at and a score you can edit.
* **A page is a document, and a document was decided.** The title, the closing
  double bar and the system break are written into the model rather than left to
  the engraver -- because each is a statement somebody made, and what the
  engraver works out when nobody said anything is not the same kind of thing.
* **A gesture names a place; the model names the note.** A click reports the
  *element* under the cursor, `clausters.gui.notation.item_id` says which model
  item it was written from, and from there every verb applies: transpose,
  lengthen, shorten, articulate, tie, silence, delete.
* **Writing a note.** With ``entry=True`` a press on blank paper reports where
  the press landed -- a place, not a note -- and `insert` puts one there, the
  pitch worked out from that staff's clef and the key.
* **What is written is what is heard.** Play at any point: the piece is read
  out of the *model*, so a staccato you just added shortens the sound and moves
  no attack, and a dynamic governs the notes after it.
* **One undo stack, and it is the model's.** Every edit is one
  `clausters.gui.notation.Score.apply` -- one operation, one step back.

The engraver is **libverovio**, which ships inside the installed package. In a
source checkout, build and stage it once (``third_party/BUILD-VEROVIO.md``)::

    third_party/build-verovio.sh
    python clients/python/build_native.py

Then, with the client importable::

    python clients/python/examples/notation/score_editor.py

**Click** a note to select and hear it. **Drag** one up or down the staff to
move it (which is not transposition: it takes the key signature's alteration for
the letter it lands on). **Press on blank paper** inside the staff to write a
quarter there. The buttons act on the selected note. Close the window to stop.
Needs an audio device, a display and a GPU.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention):
step through it with Shift+Enter and the window stays up between cells, or run
it as a plain script.
"""

# %%
import sys

from clausters import Event, Session, play
from clausters.gui import button, notation, panel, source, view
from clausters.seq.timeline import Playhead

# Eight bars in ABC -- a score as it usually arrives: typed by somebody else, in
# a format that is not ours. `M:` is the meter, `L:` the length a bare letter
# means, `K:` the key (E flat, so every B, E and A is flat and none of them
# carries a sign). A letter is a note, `/` halves it, a digit multiplies it,
# `[CEG]` is a chord and `|` bars it.
PHRASE = """X:1
T:Eight bars
M:4/4
L:1/4
K:Eb
G A B c | d2 c B | A G F G | E4 |
B c d e | f2 e d | c B A B | G4 |
"""

# One beat per second, so the engraving's milliseconds are the model's beats
# times 1000: score time and clock time are the same axis, which is what lets a
# single anchor tie the cursor to the sound.
TEMPO = 1.0

# %% [markdown]
# ## Open it, and read it
# The engraver reads ABC, MusicXML and MEI through one loader and normalizes
# what it loaded, so there is one input format by the time the model sees it.

# %%
score = notation.Score(PHRASE, page_width=1100)
sheet = score.sheet()          # raises if the document could not be read
print(f"read {sum(len(v['items']) for s in sheet['staves'] for v in s['voices'])} "
      f"items out of a document that was only text")

# %% [markdown]
# ## The decisions that are the writer's, not the engraver's
# A title, a double bar dividing the two halves, and a system break so the
# second half starts a line. None of these changes a note; all three are
# statements, and a statement is stored. What the engraver decides when nobody
# decided -- where the lines would otherwise break, how the eighths would
# otherwise beam, the double bar that ends any piece -- is not stored and is not
# loss, because it is worked out again identically every time. (Ask for that
# last one and nothing is stored: `set_barline(8, "end")` on the final measure
# says what the page already says.)

# %%
score.apply({"op": "set_header",
             "header": notation.header(title="Eight bars",
                                       composer="typed by hand")})
score.apply({"op": "set_barline", "measure": 4, "kind": "dbl"})
score.apply({"op": "set_break", "measure": 5, "kind": "system"})

# %% [markdown]
# ## The window

# %%
def scene(engraved, sample_rate: float) -> dict:
    """A page that can be written on, under two rows of verbs.

    Every widget is *named*, so the script drives each by name and never picks
    an id. ``editable=True`` opts the page into pitch editing (a drag reports
    the staff position it reaches) and ``entry=True`` into note entry (a press
    on blank paper reports the place). They are separate opt-ins because a press
    on blank paper already means something everywhere else -- it clears the
    selection -- and a page that never asked to be written on must go on meaning
    that."""
    return view(
        panel(button(name="play", label="play"),
              button(name="stop", label="stop"),
              button(name="up", label="up"),
              button(name="down", label="down"),
              button(name="longer", label="longer"),
              button(name="shorter", label="shorter"),
              layout="row", h=34.0),
        panel(button(name="stacc", label="staccato"),
              button(name="dynamic", label="mf"),
              button(name="tie", label="tie"),
              button(name="silence", label="silence"),
              button(name="delete", label="delete"),
              button(name="undo", label="undo"),
              button(name="redo", label="redo"),
              layout="row", h=34.0),
        notation.score_view(engraved, name="score", width=880.0,
                            sample_rate=sample_rate,
                            editable=True, entry=True),
        layout="col", title="Score editor (a document, and its model)",
        w=920, h=520,
    )


# %% [markdown]
# ## Open the window

# %%
session = Session.live(tempo=TEMPO)
server = session.server
sr = float(server.options.sample_rate)
gui = session.gui()
engraved = source(display_list=score.display_list())
win = scene(engraved, sr).open()
session.start()

selected: dict = {"element": None, "item": None}  # what the page named, and
                                                 # the model item behind it


def refresh():
    """Re-engrave and replace the drawn page in place. Every edit ends here.

    ``engraved.set`` is the whole of it: the source rewrites the definition and
    pushes the layers to every window already showing them, so the page is the
    score as edited rather than as opened."""
    engraved.set(score.display_list())


def item():
    """The selected model item, or None with a word about why."""
    if selected["item"] is None:
        print("  select a note first (click one)")
    return selected["item"]


def find(id: int) -> dict | None:
    """The selected item as the model holds it -- what it is written as, so an
    edit that depends on the current value (longer, shorter, a mark it toggles)
    reads it rather than guessing."""
    for staff in score.sheet()["staves"]:
        for voice in staff["voices"]:
            for entry in voice["items"]:
                if entry["id"] == id:
                    return entry
    return None


def edit(op: str, **rest):
    """Apply one operation to the selected item, as one undo step."""
    id = item()
    if id is None:
        return
    if score.apply({"op": op, "id": id, **rest}):
        refresh()
        print(f"  {op} on item {id}")
    else:
        print(f"  {op} refused on item {id}")


# %% [markdown]
# ## The verbs
# Each button is one operation on the selected item. The two that read the
# item first -- the length and the articulation -- read it from the *model*,
# because an edit relative to a value has to know the value.

# %%
def scale_dur(factor: int):
    """Twice as long, or half: the written value, against the barlines that
    were already there. A value that now overruns a bar is split and tied when
    the page is written, which is what augmentation looks like."""
    entry = find(item()) if selected["item"] is not None else None
    if entry is None:
        return
    num, den = entry["dur"]
    edit("set_dur", dur=[num * factor, den] if factor > 1 else [num, den * 2])


def toggle_stacc():
    """Add or take away a staccato. A mark is *replaced* rather than merged, so
    toggling one reads the marks, changes one, and writes them back."""
    entry = find(item()) if selected["item"] is not None else None
    if entry is None:
        return
    marks = dict(entry.get("marks") or {})
    articulations = list(marks.get("articulations") or [])
    if "stacc" in articulations:
        articulations.remove("stacc")
    else:
        articulations.append("stacc")
    marks["articulations"] = articulations
    edit("set_marks", marks=marks)


# %% [markdown]
# ## Playing what is written
# The piece comes out of the **model**, not out of the engraving: `to_timeline`
# reads what the symbols mean, so a staccato added a moment ago is honoured and
# every attack after it stays where it was.

# %%
def pass_from(at: float):
    """One playback pass, read out of the model as it stands right now."""
    return Playhead(notation.to_timeline(score.sheet()),
                    session.clock, server).play(at=at)


def phrase_end() -> float:
    """Where the piece ends, in beats -- the last note's onset plus its written
    value. The transport parks the cursor there when a pass runs out."""
    notes = notation.to_notes(score.sheet())
    return max((n["t"] + n["dur"] for n in notes), default=0.0)


transport = notation.transport(gui, win["score"].id, source=pass_from,
                               tempo=TEMPO, sample_rate=sr, extent=phrase_end)
transport.locate(0.0)

# %% [markdown]
# ## The page's three edit-backs

# %%
def on_score(tag, *rest):
    """What the page reports, and what this side makes of it.

    ``"element"`` is a click: the page names the element under the cursor, and
    `clausters.gui.notation.item_id` turns it into the model item it was
    written from -- ``n7``, ``n7-2`` (a piece split across a barline) and
    ``n7-p1`` (one pitch of a chord) are all item 7, which is what lets a
    gesture anywhere on a note reach the note.

    ``"transpose"`` is a drag: it names the staff position the note **reaches**,
    absolute rather than a displacement, so an edit that arrives twice moves
    nothing the second time. It is `move_steps` and not `transpose` -- moving
    along the staff takes the key signature's alteration for the letter it lands
    on, so a note dragged onto a B in E flat is a B flat.

    ``"insert"`` is a press on blank paper: the element the new note would
    follow, how far up the staff the press landed, and which staff. A place, not
    a note -- a staff position is a pitch only once something knows the clef and
    the key, and `insert` is what knows.

    A handler runs on the client's reply thread, where the ambient session is
    another thread's, so every `play` here names its `server`."""
    if tag == "element" and rest:
        selected["element"] = rest[0] or None
        selected["item"] = notation.item_id(rest[0]) if rest[0] else None
        entry = find(selected["item"]) if selected["item"] is not None else None
        if entry is None:
            print(f"  clicked {rest[0] or '(blank paper)'}")
            return
        pitches = entry.get("pitches") or []
        print(f"  selected item {selected['item']} ({rest[0]}), "
              f"{len(pitches)} pitch(es), written {entry['dur']}")
        for note in notation.to_notes(score.sheet()):
            if note["id"] == selected["item"]:
                play(Event(midinote=note["pitch"], dur=note["sustain"],
                           amp=0.15), server=server)
                break
    elif tag == "transpose" and len(rest) >= 2:
        id = notation.item_id(rest[0])
        if id is None:
            print(f"  {rest[0]} was not written from this model")
            return
        # The page reports where the note landed; the model is told to move it
        # there, and the step count is worked out against the note it has.
        entry = find(id)
        if entry is None or not entry.get("pitches"):
            return
        if score.transpose_to(rest[0], int(rest[1])):
            refresh()
            print(f"  moved item {id} to staff position {int(rest[1]):+d}")
    elif tag == "insert" and len(rest) >= 3:
        after, position, staff = rest[0], int(rest[1]), int(rest[2])
        op = {"op": "insert", "dur": [1, 4], "pitches": [],
              "position": position, "staff": staff, "voice": 0}
        id = notation.item_id(after) if after else None
        if id is not None:
            op["after"] = id
        if score.apply(op):
            refresh()
            print(f"  wrote a quarter at staff position {position:+d} "
                  f"on staff {staff}")
        else:
            print("  the insert was refused")


# %% [markdown]
# ## Wire it up

# %%
def undo():
    """Step back one edit. The stack is the *model's*: every edit above is one
    operation, so one step back is one operation undone -- including the ones
    that came from a gesture on the page."""
    if score.undo():
        refresh()
        print("  undo")
    else:
        print("  nothing to undo")


def redo():
    if score.redo():
        refresh()
        print("  redo")
    else:
        print("  nothing to redo")


win["play"].on_click(lambda: transport.play(server))
win["stop"].on_click(transport.stop)
win["up"].on_click(lambda: edit("move_steps", steps=1))
win["down"].on_click(lambda: edit("move_steps", steps=-1))
win["longer"].on_click(lambda: scale_dur(2))
win["shorter"].on_click(lambda: scale_dur(1))
win["stacc"].on_click(toggle_stacc)
win["dynamic"].on_click(lambda: edit("set_marks", marks=notation.marks(dynamic="mf")))
win["tie"].on_click(lambda: edit("tie", tied=True))
win["silence"].on_click(lambda: edit("silence"))
win["delete"].on_click(lambda: edit("delete"))
win["undo"].on_click(undo)
win["redo"].on_click(redo)
win["score"].on_event(on_score)
closed = [False]
win.on_closed(lambda: (closed.__setitem__(0, True), print("window closed")))
print("click a note to select and hear it, drag one up or down the staff, "
      "press blank paper inside the staff to write a quarter; the buttons act "
      "on the selection. Close the window to stop")


# %%
def run():
    """Pump the host until the window closes."""
    while not closed[0]:
        transport.update()
        gui.pump(timeout=0.1)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        session.close()
else:
    print("editor up - run() to pump, session.close() to end")
