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
* **What was typed once can be operated on.** The typed phrase is a single
  treble line; the bass staff under it is *made here*, by transposing a copy
  down two octaves, re-clefing it and stacking the two -- so a grand staff, a
  slur and a crescendo all exist on a document nobody wrote them into.
* **A page is a document, and a document was decided.** The title, the double
  bar dividing the halves and the system break are written into the model,
  because each is a statement somebody made. What the engraver decides when
  nobody decided is not stored and is not loss -- it is worked out again
  identically every time.
* **A gesture names a place; the model names the note.** A click reports the
  *element* under the cursor, `clausters.gui.notation.item_id` says which model
  item it was written from, and from there every verb applies.
* **Marks accumulate.** A note is staccato *and* accented *and* under a
  dynamic. `set_marks` replaces the whole set rather than merging, which is the
  honest shape for something a caller holds whole -- so every button here reads
  the marks, changes one, and writes them back.
* **Writing a note.** With ``entry=True`` a press on blank paper reports where
  the press landed -- a place, not a note -- and `insert` puts one there, the
  pitch worked out from that staff's clef and the key.
* **Two lines on one staff.** ``2nd voice`` moves the selected note into a
  second voice of its own staff, leaving a rest where it was, so nothing around
  it moves -- which is how two lines written as one come apart. A note written
  *after* one of those joins the line it follows, since `insert` puts a new item
  in the voice of the item it comes after.
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

**Click** a note to select and hear it; the window's status line says what is
selected. **Drag** one up or down the staff to move it (which is not
transposition: it takes the key signature's alteration for the letter it lands
on). **Press on empty staff** -- between two notes, or past the last one, on
either staff -- to write a quarter there. The buttons act on the selection.
Close the window to stop. Needs an audio device, a display and a GPU.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention):
step through it with Shift+Enter and the window stays up between cells, or run
it as a plain script.
"""

# %%
import sys

from clausters import Event, Session, play
from clausters.gui import button, label, notation, panel, source, view
from clausters.seq.timeline import Playhead

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

# Two beats per second: the quarter = 120 the engraver times the page at. A
# quarter is then one beat in the model and 500 ms on the page, which is what
# ties the cursor to the sound -- the transport places its cursor in *score*
# milliseconds and reads the model in beats, so the two have to agree.
TEMPO = 2.0

# How wide the page is drawn. The height follows from the engraving's aspect
# and is kept in step with it on every edit (see `refresh`), so the drawn size
# never changes under a hand that is editing.
PAGE_W = 900.0

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
# one system -- a brace and one barline through both. The whole point of
# reading a document is that everything the model can do applies to it
# afterwards.

# %%
lower = notation.transpose(typed, -24)
lower["staves"][0]["clef"] = "F4"
score.apply({"op": "stack", "sheet": lower, "as_staff": True})

# %% [markdown]
# ## The decisions that are the writer's, not the engraver's
# A title, a double bar dividing the two halves, a system break so the second
# half starts a line -- and two spans that no single note could carry: a slur
# over the opening figure and a crescendo under it. None of these changes a
# note; all of them are statements, and a statement is stored. What the
# engraver decides when nobody decided -- where the lines would otherwise
# break, how the eighths would otherwise beam, the double bar that ends any
# piece -- is not stored and is not loss.

# %%
score.apply({"op": "set_header",
             "header": notation.header(title="Eight bars",
                                       composer="typed, then operated on")})
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
# ## The window

# %%
def scene(engraved, sample_rate: float) -> dict:
    """A page that can be written on, under three rows of verbs.

    Every widget is *named*, so the script drives each by name and never picks
    an id. ``editable=True`` opts the page into pitch editing (a drag reports
    the staff position it reaches) and ``entry=True`` into note entry (a press
    on empty staff reports the place). They are separate opt-ins because a
    press on blank paper already means something everywhere else -- it clears
    the selection -- and a page that never asked to be written on must go on
    meaning that.

    ``scroll_name`` is what lets `refresh` grow the page's box with the page
    instead of letting the engraving shrink into a fixed one."""
    return view(
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
        panel(button(name="slur", label="slur x4"),
              button(name="voice", label="2nd voice"),
              button(name="tie", label="tie"),
              button(name="silence", label="silence"),
              button(name="delete", label="delete"),
              button(name="undo", label="undo"),
              button(name="redo", label="redo"),
              layout="row", h=34.0),
        label("click a note, or press empty staff to write one",
              name="status", h=22.0),
        notation.score_view(engraved, name="score", scroll_name="page",
                            width=PAGE_W, sample_rate=sample_rate,
                            editable=True, entry=True),
        layout="col", title="Score editor (a document, and its model)",
        w=960, h=640,
    )


# %% [markdown]
# ## Open the window

# %%
session = Session.live(tempo=TEMPO)
server = session.server
# `query_info` rather than the launch options: it is the one spelling both
# clients have, so this file and its page twin ask the same question.
sr = server.query_info().nominal_sample_rate
gui = session.gui()
dl = score.display_list()
engraved = source(display_list=dl)


def page_height(page: dict) -> float:
    """The page's drawn height at `PAGE_W`, in its own aspect."""
    vb = page.get("vb") or [1.0, 1.0]
    return round(PAGE_W * vb[1] / vb[0], 1) if vb[0] else PAGE_W


win = scene(engraved, sr).open()
session.start()

selected: dict = {"element": None, "item": None}


def say(line: str) -> None:
    """Put a line on the window's status label and in the log."""
    print(f"  {line}")
    win["status"].set(text=line)


def refresh() -> None:
    """Re-engrave and replace the drawn page in place. Every edit ends here.

    **The page's box grows with the page**, which is the whole of keeping an
    edit from moving the picture: a score widget draws what it is sent to fit
    the box it is given, so a page that gained a system would otherwise shrink
    to stay inside, re-scaling everything a hand was working on. Setting the
    height from the new engraving's aspect keeps the drawn size fixed and lets
    the scroll do what a scroll is for -- and the reader's own zoom, which is
    the scroll's, is untouched either way."""
    page = score.display_list()
    engraved.set(page)
    height = page_height(page)
    win["score"].set(h=height)
    win["page"].set(content_h=height)


def item():
    """The selected model item, or None with a word about why."""
    if selected["item"] is None:
        say("select a note first (click one)")
    return selected["item"]


def locate(id):
    """Where the item sits and what it is written as: ``(staff, voice, item)``,
    or three Nones. An edit that depends on the current value -- a length, a
    mark it toggles, the voice it is in -- reads it rather than guessing."""
    for si, staff in enumerate(score.sheet()["staves"]):
        for vi, voice in enumerate(staff["voices"]):
            for entry in voice["items"]:
                if entry["id"] == id:
                    return si, vi, entry
    return None, None, None


def find(id) -> dict | None:
    """Just the item."""
    return locate(id)[2]


def edit(op: str, **params) -> None:
    """Apply one operation to the selected item, as one undo step."""
    id = item()
    if id is None:
        return
    if score.apply({"op": op, "id": id, **params}):
        refresh()
        say(f"{op} on item {id}")
    else:
        say(f"{op} refused on item {id}")


# %% [markdown]
# ## The marks, which accumulate
# `set_marks` **replaces** the whole set rather than merging -- the honest
# shape for something a caller holds whole, since a merge would leave no way to
# take a mark away. So a button that changes one mark reads them, changes that
# one, and writes them back; which is also why a note can be staccato and
# accented and under a dynamic at the same time.

# %%
def with_marks(**changes):
    """The selected item's marks with `changes` applied, or None if nothing is
    selected. A value of None takes a mark away."""
    id = item()
    if id is None:
        return None, None
    entry = find(id)
    if entry is None:
        return None, None
    marks = dict(entry.get("marks") or {})
    for key, value in changes.items():
        if value is None:
            marks.pop(key, None)
        else:
            marks[key] = value
    return id, marks


def toggle_articulation(name: str):
    """Add or take away one articulation, leaving the others -- and the
    dynamic, and the ornament -- where they are."""
    def act():
        id, marks = with_marks()
        if id is None:
            return
        articulations = list(marks.get("articulations") or [])
        if name in articulations:
            articulations.remove(name)
        else:
            articulations.append(name)
        marks["articulations"] = articulations
        edit("set_marks", marks=marks)
    return act


def set_mark(**changes):
    """A dynamic or an ornament, put on or taken off without touching the rest."""
    def act():
        id, marks = with_marks(**changes)
        if id is None:
            return
        edit("set_marks", marks=marks)
    return act


def scale_dur(factor: int):
    """Twice as long, or half: the written value, against the barlines that
    were already there. A value that now overruns a bar is split and tied when
    the page is written, which is what augmentation looks like."""
    def act():
        id = item()
        entry = find(id) if id is not None else None
        if entry is None:
            return
        num, den = entry["dur"]
        edit("set_dur", dur=[num * factor, den] if factor > 1 else [num, den * 2])
    return act


def to_second_voice() -> None:
    """Move the selected note into a second voice of its own staff.

    **Two lines written as one come apart here**: the item keeps its id and its
    place in time, and a rest holds the gap open where it was, so nothing around
    either line moves. The voice is made if the staff has only one. A note
    written *after* one of these joins the second line, because `insert` puts a
    new item in the voice of the item it follows."""
    id = item()
    if id is None:
        return
    _, voice, _ = locate(id)
    if voice is None:
        return
    target = 1 if voice == 0 else 0
    if score.apply({"op": "to_voice", "ids": [id], "voice": target}):
        refresh()
        say(f"item {id} moved to voice {target + 1}, a rest left where it was")
    else:
        say(f"item {id} is already the only thing in voice {target + 1}")


def slur_four() -> None:
    """A slur from the selected note over the next three. A span has **two
    ends**, so it cannot ride a note the way an articulation does: it lives
    beside the staves and names the two items it runs between."""
    id = item()
    if id is None:
        return
    for staff in score.sheet()["staves"]:
        for voice in staff["voices"]:
            ids = [entry["id"] for entry in voice["items"]]
            if id in ids:
                at = ids.index(id)
                last = ids[min(at + 3, len(ids) - 1)]
                if last == id:
                    return say("nothing after it to slur to")
                if score.apply({"op": "add_spanner", "kind": "slur",
                                "from": id, "to": last}):
                    refresh()
                    say(f"slur from item {id} to item {last}")
                return


# %% [markdown]
# ## Playing what is written
# The piece comes out of the **model**, not out of the engraving: `to_timeline`
# reads what the symbols mean, so a staccato added a moment ago is honoured, a
# dynamic governs the notes after it, and every attack stays where it was.
# `instruments` binds a staff to what plays it, since the notation never says.

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
def on_score(tag, *payload):
    """What the page reports, and what this side makes of it.

    ``"element"`` is a click: the page names the element under the cursor, and
    `clausters.gui.notation.item_id` turns it into the model item it was
    written from -- ``n7``, ``n7-2`` (a piece split across a barline) and
    ``n7-p1`` (one pitch of a chord) are all item 7, which is what lets a
    gesture anywhere on a note reach the note.

    ``"transpose"`` is a drag: it names the staff position the note **reaches**,
    absolute rather than a displacement, so an edit that arrives twice moves
    nothing the second time. Moving along the staff takes the key signature's
    alteration for the letter it lands on, so a note dragged onto a B in E flat
    is a B flat -- which is why it is not `transpose`.

    ``"insert"`` is a press on empty staff: the element the new note would
    follow, how far up the staff the press landed, and which staff. A place,
    not a note -- a staff position is a pitch only once something knows the
    clef and the key, and `insert` is what knows.

    A handler runs on the client's reply thread, where the ambient session is
    another thread's, so every `play` here names its `server`."""
    if tag == "element" and payload:
        selected["element"] = payload[0] or None
        selected["item"] = notation.item_id(payload[0]) if payload[0] else None
        entry = find(selected["item"]) if selected["item"] is not None else None
        if entry is None:
            say("nothing selected" if not payload[0]
                else f"{payload[0]} is not one of this model's items")
            return
        staff, voice, _ = locate(selected["item"])
        marks = entry.get("marks") or {}
        num, den = entry["dur"]
        say(f"item {selected['item']}: {num}/{den}"
            + f", staff {staff} voice {voice}"
            + (f", {marks}" if marks else "")
            + (", tied" if entry.get("tie") else ""))
        for note in notation.to_notes(score.sheet()):
            if note["id"] == selected["item"]:
                play(Event(midinote=note["pitch"], dur=note["sustain"],
                           amp=0.15), server=server)
                break
    elif tag == "transpose" and len(payload) >= 2:
        id = notation.item_id(payload[0])
        if id is None or find(id) is None:
            return
        if score.transpose_to(payload[0], int(payload[1])):
            refresh()
            say(f"moved item {id} to staff position {int(payload[1]):+d}")
    elif tag == "insert" and len(payload) >= 3:
        after, position, staff = payload[0], int(payload[1]), int(payload[2])
        op = {"op": "insert", "dur": [1, 4], "pitches": [],
              "position": position, "staff": staff, "voice": 0}
        id = notation.item_id(after) if after else None
        if id is not None:
            op["after"] = id
        if score.apply(op):
            refresh()
            say(f"wrote a quarter at staff position {position:+d} "
                f"on staff {staff}")
        else:
            say("the insert was refused")


# %% [markdown]
# ## Wire it up

# %%
def undo() -> None:
    """Step back one edit. The stack is the *model's*: every edit above is one
    operation, so one step back is one operation undone -- including the ones
    that came from a gesture on the page."""
    if score.undo():
        refresh()
        say("undo")
    else:
        say("nothing to undo")


def redo() -> None:
    if score.redo():
        refresh()
        say("redo")
    else:
        say("nothing to redo")


win["play"].on_click(lambda: transport.play(server))
win["stop"].on_click(transport.stop)
win["up"].on_click(lambda: edit("move_steps", steps=1))
win["down"].on_click(lambda: edit("move_steps", steps=-1))
win["longer"].on_click(scale_dur(2))
win["shorter"].on_click(scale_dur(1))
win["stacc"].on_click(toggle_articulation("stacc"))
win["accent"].on_click(toggle_articulation("acc"))
win["tenuto"].on_click(toggle_articulation("ten"))
win["trill"].on_click(set_mark(ornament="trill"))
win["mf"].on_click(set_mark(dynamic="mf"))
win["ff"].on_click(set_mark(dynamic="ff"))
win["plain"].on_click(lambda: edit("set_marks", marks={}))
win["slur"].on_click(slur_four)
win["voice"].on_click(to_second_voice)
win["tie"].on_click(lambda: edit("tie", tied=True))
win["silence"].on_click(lambda: edit("silence"))
win["delete"].on_click(lambda: edit("delete"))
win["undo"].on_click(undo)
win["redo"].on_click(redo)
win["score"].on_event(on_score)
win.on_closed(lambda: print("window closed"))
print("click a note to select and hear it, drag one up or down the staff, "
      "press empty staff to write a quarter; the buttons act on the selection")


# %%
def run():
    """Hold the window open until it is closed.

    Nothing is driven here: the host's event loop delivers every gesture, and
    the transport asks about the end of its own pass on the application clock
    over that same loop.
    """
    win.wait()


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        session.close()
else:
    print("editor up - run() to hold the window, session.close() to end")
