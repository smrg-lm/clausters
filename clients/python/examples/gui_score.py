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
down the staff reports a ``"transpose"`` of that element by whole diatonic
steps. Because the id is the client's own, this script resolves both against its
own score: a click sounds the note, a drag transposes it, re-engraves the page
and sends it back -- the whole edit round trip, with nothing shared but the id.

The engraver is **libverovio**, which ships inside the installed package -- like
the Faust compiler, nothing to install separately. In a source checkout, build
and stage it once (``third_party/BUILD-VEROVIO.md``)::

    third_party/build-verovio.sh
    python clients/python/build_native.py

Then, with the client importable (``pip install ./clients/python`` or
``PYTHONPATH=clients/python``)::

    python clients/python/examples/gui_score.py

A window opens showing the engraved phrase, stopped at the top -- press **play**
and the cursor follows the sound. Click a note to hear it and select it, drag one
up or down to transpose it, and **from note** plays from the one selected;
**undo**/**redo** walk the edits. Close the window to stop. Needs an audio
device, a display and a GPU adapter.

The undo stack is the client's, not the host's: the host holds no score, so
every edit -- and every step back through it -- happens on this side, against
`clausters.gui.notation.Score`. The host only draws the page it is sent.
"""

import sys

from clausters import Event, Session, play
from clausters.gui import button, notation, panel, window
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

# Widget ids: the transport bar, then the score view. None of them is 1 -- that
# is the def's own id, which the window root takes, and a widget claiming it
# would be dropped as a duplicate.
BAR, REWIND, PLAY, PAUSE, STOP, FROM_NOTE = 2, 3, 4, 5, 6, 7
UNDO, REDO = 8, 9
SCROLL, SCORE = 10, 11


def scene(display_list: dict, sample_rate: float) -> dict:
    """A transport bar over a scrollable, zoomable view of the engraved score.

    The bar is chrome: a fixed height, so the page takes all the rest however the
    window is resized."""
    return window(
        panel(BAR,
              button(REWIND, label="|<"),
              button(PLAY, label="play"),
              button(PAUSE, label="pause"),
              button(STOP, label="stop"),
              button(FROM_NOTE, label="from note"),
              button(UNDO, label="undo"),
              button(REDO, label="redo"),
              layout="row", h=34.0),
        notation.score_view(display_list, scroll_id=SCROLL, score_id=SCORE,
                            width=880.0, sample_rate=sample_rate, editable=True),
        layout="col", title="Engraved score (verovio -> GPU)", w=920, h=420,
    )


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


def main():
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
    with Session.live(tempo=TEMPO) as session:
        server = session.server
        sr = float(server.options.sample_rate)
        gui = session.gui()
        gui.define(1, scene(dl, sr))

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
        transport = notation.transport(gui, SCORE, source=pass_from, tempo=TEMPO,
                                       sample_rate=sr, extent=phrase_end)
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
            and selection across the swap."""
            nonlocal dl, by_id
            dl = score.display_list()
            gui.set(SCORE, display_list=notation.page_json(dl))
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

        # The handlers read `dl` and `by_id` when they run, so an edit made
        # meanwhile is simply played. `locate` doubles as rewind: playing, it
        # starts a fresh pass from the top instead of merely moving the cursor.
        buttons = {PLAY: lambda: transport.play(server),
                   PAUSE: transport.pause, STOP: transport.stop,
                   REWIND: lambda: transport.locate(0.0), FROM_NOTE: from_note,
                   UNDO: undo, REDO: redo}

        while True:
            # The pass ends by itself: the playhead reports its scan ran out and
            # the transport parks the cursor at `phrase_end`, rather than letting
            # it sweep off the page (rewind goes back to the top).
            transport.update()
            msg = gui.poll(timeout=0.1)
            if msg is None:
                continue
            addr, args = msg
            if addr == "/gui_closed":
                print("window closed")
                break
            if addr != "/gui_event" or len(args) < 2:
                continue
            # A button reports its press (1) and its release (0): act on the
            # press, or every click would fire the transport twice.
            if args[0] in buttons:
                if args[1] == 1:
                    buttons[args[0]]()
                continue
            if len(args) < 3:
                continue
            if args[1] == "element":
                selected = args[2] or None
                note = by_id.get(args[2])
                if note is None:
                    print(f"  clicked {args[2] or '(blank paper)'}")
                    continue
                print(f"  clicked note {args[2]}: MIDI {note['pitch']} "
                      f"at {note['t']:.0f} ms")
                play(Event(midinote=note["pitch"], dur=note["dur"] / 1000.0,
                           amp=0.15))
            elif args[1] == "transpose":
                # The edit round trip. The host drew the drag; this side makes
                # it true -- transpose by those diatonic steps, re-engrave, and
                # send the page back, which is what retires the preview. The
                # ids survive the edit, so `by_id` keeps indexing the same
                # notes (at their new pitches) and the note stays selected.
                element, steps = args[2], int(args[3])
                if not score.transpose(element, steps):
                    print(f"  refused to transpose {element}: this verovio has "
                          "no working editor")
                    continue
                refresh_page()
                note = by_id.get(element)
                print(f"  transposed {element} by {steps:+d} steps"
                      + (f" -> MIDI {note['pitch']}" if note else ""))
                if note is not None:
                    play(Event(midinote=note["pitch"],
                               dur=note["dur"] / 1000.0, amp=0.15))


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ValueError) as e:
        sys.exit(str(e))
