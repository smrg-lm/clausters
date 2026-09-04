#!/usr/bin/env python3
"""Engraving music notation into the GUI host: the ``score`` widget.

A read-only view like ``plot`` and the node tree, but of a **musical score**
rather than a signal. The client engraves a score with verovio (bundled in the
package) into a semantic display list -- a SMuFL glyph-outline table plus
placed glyphs, staff lines, stems and beams in page units -- and the host
tessellates it into the same triangle mesh the rest of the chrome uses. verovio
lives entirely on the client side; the host never depends on it.

One engraving carries three layers: what is **drawn**, where the **playback
cursor** sits at each onset, and the **notes** that sound. This example places
the notes on a timeline and drives them from a **transport bar** -- play, pause,
stop, rewind and play-from-the-selected-note -- anchoring the cursor to the
engine's sample clock, so the score follows the audio with **one message per
pass** (``playhead_at``), the host reading the clock every frame from there,
exactly as the timeline views do. A stopped transport is the other half of that
one number: it goes negative and the static ``playhead`` holds the cursor where
the music was left. It is the **shared** transport (`clausters.gui.Transport`),
the same object the multitrack editor drives its lanes with: a page differs only
in the unit its static cursor is placed in, and that is all
`notation.transport` fills in.

The page is also **clickable and editable**: every primitive carries the MEI
``xml:id`` it was engraved from, so a press reports the element under the cursor
as an ``"element"`` event and the host highlights it, and dragging one up or
down the staff reports a ``"transpose"`` naming the diatonic staff position that
element **reaches** -- absolute rather than a displacement, so a resend cannot
move it twice. Because the id is the client's own, this script resolves both against its
own score: a click sounds the note, a drag transposes it, re-engraves the page
and sends it back -- the whole edit round trip, with nothing shared but the id.

The engraver is **libverovio**, which ships inside the installed package -- like
the Faust compiler, nothing to install separately. In a source checkout, build
and stage it once (``third_party/BUILD-VEROVIO.md``)::

    third_party/build-verovio.sh
    python clients/python/build_native.py

Then, with the client importable (``pip install ./clients/python`` or
``PYTHONPATH=clients/python``)::

    python clients/python/examples/notation/score.py

A window opens showing the engraved phrase, stopped at the top -- press **play**
and the cursor follows the sound. Click a note to hear it and select it, drag one
up or down to transpose it, and **from note** plays from the one selected;
**undo**/**redo** walk the edits. Close the window to stop. Needs an audio
device, a display and a GPU adapter.

The undo stack is the client's, not the host's: the host holds no score, so
every edit -- and every step back through it -- happens on this side, against
`clausters.gui.notation.Score`. The host only draws the page it is sent.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention):
step through it with Shift+Enter and the window stays up between cells, or run
it as a plain script.
"""

# %%
import sys

from clausters import Event, Session, play
from clausters.gui import button, notation, panel, source, view
from clausters.seq.timeline import Playhead, Timeline

# Six bars in ABC -- the readable way to type a score by hand; verovio reads MEI
# and MusicXML through the same loader, which is what a score usually arrives as.
# The header is the whole grammar you need here: `M:` the meter, `L:` the length
# a bare letter means (a quarter), `K:` the key (G, so every F is sharp). Then a
# letter is a note (`C` is middle C, `c` the one above), `/` halves it and a
# digit multiplies it, `[CEG]` is a chord, and `|` bars it. Each bar fills its
# 4/4 exactly -- verovio drops what overflows a measure, so an over-full bar
# would be drawn short and sound short.
PHRASE = """X:1
T:Six bars
M:4/4
L:1/4
K:G
C D E F | G/A/G/F/ E D | C D/E/F/G/ A | G2 F E | [CEG] G C2 | C4 |
"""

# One beat per second, so the engraving's milliseconds are beats/1000: score time
# and clock time become the same axis, which is what lets one anchor tie the
# cursor to the sound.
TEMPO = 1.0

# %% [markdown]
# ## The window

# %%
def scene(engraved, sample_rate: float) -> dict:
    """A transport bar over a scrollable, zoomable view of the engraved score.

    Every widget is *named* — the seven transport buttons and the ``score`` page
    — so the script drives each by name and never picks an id. The bar is chrome:
    a fixed height, so the page takes all the rest however the window is
    resized.

    ``engraved`` is a `clausters.gui.source` holding the display list, not the
    list itself: an edit re-engraves the score and calls ``engraved.set(...)``,
    which
    reaches this definition *and* every window drawing it. Handed the raw dict
    the view would draw the same page and the definition would go stale the
    first time a note moved."""
    return view(
        panel(button(name="rewind", label="|<"),
              button(name="play", label="play"),
              button(name="pause", label="pause"),
              button(name="stop", label="stop"),
              button(name="from_note", label="from note"),
              button(name="undo", label="undo"),
              button(name="redo", label="redo"),
              layout="row", h=34.0),
        notation.score_view(engraved, name="score",
                            width=880.0, sample_rate=sample_rate, editable=True),
        layout="col", title="Engraved score (verovio -> GPU)", w=920, h=420,
    )


# %% [markdown]
# ## What the page plays back

# %%
def phrase_timeline(notes: list) -> Timeline:
    """Place the engraved notes on a `Timeline` (see `TEMPO`: score ms are
    beats/1000). Built per play, so a transposed note is played at the pitch it
    now has."""
    timeline = Timeline()
    for note in notes:
        timeline.add(note["t"] / 1000.0,
                     Event(midinote=note["pitch"], dur=note["dur"] / 1000.0,
                           amp=0.12))
    return timeline


# %% [markdown]
# ## Engrave, open, and wire it up

# %%
# `Score` rather than `engrave`: it keeps the document open, so the page the
# window shows can be edited and re-engraved against it (a narrow page, so
# the phrase wraps into a few systems and the view scrolls).
score = notation.Score(PHRASE, page_width=1100)
dl = score.display_list()
print(f"engraved: {len(dl['glyphs'])} glyph outlines, "
      f"{len(dl['prims'])} primitives, {len(dl['cursors'])} cursor stops, "
      f"{len(dl['notes'])} notes, page {dl['vb']}")

# the session is the ambient one for the whole block, so a bare `play` below
# resolves to its server and clock
session = Session.live(tempo=TEMPO)
server = session.server
# `query_info` rather than the launch options: it is the one spelling both
# clients have, so this file and its page twin ask the same question.
sr = server.query_info().nominal_sample_rate
gui = session.gui()
engraved = source(display_list=dl)
win = scene(engraved, sr).open()

session.start()                     # the clock runs the routines

# Both round trips run off the same id: the widget reports the MEI id
# under the cursor, and that id indexes this script's own engraving.
by_id = {note["id"]: note for note in dl["notes"]}
selected = None


def pass_from(at):
    """One playback pass: the engraved notes on a fresh `Timeline`, played
    from beat `at`. The transport calls this on every play, so a note
    transposed meanwhile simply sounds at the pitch it now has."""
    return Playhead(phrase_timeline(dl["notes"]), session.clock,
                    server).play(at=at)

def phrase_end():
    """Where the piece ends, in beats: the last note's onset plus its
    length. The transport parks the cursor there when a pass runs out."""
    last = dl["notes"][-1]
    return (last["t"] + last["dur"]) / 1000.0

# The transport is the shared one (`clausters.gui.Transport`), the same
# object the multitrack editor drives its lanes with; `notation.transport`
# only fills in the page's unit -- a score cursor is placed in score
# milliseconds, not samples.
transport = notation.transport(gui, win["score"].id, source=pass_from,
                               tempo=TEMPO, sample_rate=sr, extent=phrase_end)
transport.locate(0.0)               # the cursor waits at the top
print("press play -- click a note to hear it and to select it, drag one "
      "up or down to transpose it, 'from note' plays from the selected "
      "one, undo/redo walk the edits; close the window to stop")

def from_note():
    """Play from the selected note: the click round trip put its id in
    `selected`, and this script's own engraving says when it sounds."""
    if selected not in by_id:
        print("  no note selected: click one first")
        return
    transport.play(server, at=by_id[selected]["t"] / 1000.0)

def refresh_page():
    """Re-engrave the score and replace the drawn page in place, rebuilding
    the id index. Every edit ends here -- a drag, an undo, a redo -- since
    all the host needs is the new display list; the host keeps the playhead
    and selection across the swap.

    ``engraved.set`` is the whole of it: the source rewrites the definition and
    pushes the drawing layers to every window already showing them, so a second
    window (or a re-open of this one) shows the score as edited rather than as
    engraved."""
    global dl, by_id
    dl = score.display_list()
    engraved.set(dl)
    by_id = {note["id"]: note for note in dl["notes"]}

def undo():
    """Step back one edit. `Score` owns the undo stack (a stack of MEI
    snapshots) -- the host holds no score, so undo is the client's, and it
    answers False rather than crashing when there is nothing to undo."""
    if score.undo():
        refresh_page()
        print("  undo")
    else:
        print("  nothing to undo")

def redo():
    if score.redo():
        refresh_page()
        print("  redo")
    else:
        print("  nothing to redo")

def on_score(tag, *payload):
    """The page's two edit-backs, wired to the ``score`` handle. A click
    reports the MEI id under the cursor (``"element"``) — this side
    selects it and sounds it; a drag reports a ``"transpose"`` naming the
    staff position reached — this side makes it true, re-engraves and sends the
    page back. The handlers read `dl`/`by_id` when they run, so an edit
    made meanwhile is simply played. An event handler runs on the client's
    reply thread, and the ambient session is per-thread, so every `play`
    here names its `server` instead of letting it resolve."""
    global selected
    if tag == "element" and payload:
        selected = payload[0] or None
        note = by_id.get(payload[0])
        if note is None:
            print(f"  clicked {payload[0] or '(blank paper)'}")
            return
        print(f"  clicked note {payload[0]}: MIDI {note['pitch']} "
              f"at {note['t']:.0f} ms")
        play(Event(midinote=note["pitch"], dur=note["dur"] / 1000.0,
                   amp=0.15), server=server)
    elif tag == "transpose" and len(payload) >= 2:
        # The host drew the drag; this side makes it true -- move the note
        # **to** the staff position the payload names, re-engrave, and send
        # the page back, which is what retires the preview. The ids survive
        # the edit, so `by_id` keeps indexing the same notes (at their new
        # pitches) and the note stays selected.
        #
        # The payload is a position and not a displacement, which is what
        # makes it safe to arrive late or twice: `transpose_to` computes the
        # step count against the engraving it has right now, so a page this
        # side re-engraved meanwhile cannot make the edit land somewhere else.
        element, position = payload[0], int(payload[1])
        if not score.transpose_to(element, position):
            print(f"  refused to transpose {element}: this verovio has "
                  "no working editor")
            return
        refresh_page()
        note = by_id.get(element)
        print(f"  moved {element} to staff position {position:+d}"
              + (f" -> MIDI {note['pitch']}" if note else ""))
        if note is not None:
            play(Event(midinote=note["pitch"], dur=note["dur"] / 1000.0,
                       amp=0.15), server=server)

# Wire every widget by name: each button acts on its **click** -- the press
# completed on the button, so sliding off before letting go cancels it -- and
# `locate` doubles as rewind, so playing it starts a fresh pass from the top.
# The score page answers its own edit-backs.
win["play"].on_click(lambda: transport.play(server))
win["pause"].on_click(transport.pause)
win["stop"].on_click(transport.stop)
win["rewind"].on_click(lambda: transport.locate(0.0))
win["from_note"].on_click(from_note)
win["undo"].on_click(undo)
win["redo"].on_click(redo)
win["score"].on_event(on_score)
win.on_closed(lambda: print("window closed"))


# %%
def run():
    """Hold the window open until it is closed.

    The pass ends by itself with nothing here asking: the playhead reports its
    scan ran out and the transport parks the cursor at `phrase_end` from the
    host's application clock, rather than letting it sweep off the page (rewind
    goes back to the top).
    """
    win.wait()


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        session.close()
else:
    print("score up - run() to hold the window, session.close() to end")
