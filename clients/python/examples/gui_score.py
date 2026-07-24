#!/usr/bin/env python3
"""Engraving music notation into the GUI host: the ``score`` widget.

A read-only view like ``plot`` and the node tree, but of a **musical score**
rather than a signal. The client engraves a score with verovio (an optional
dependency) into a semantic display list -- a SMuFL glyph-outline table plus
placed glyphs, staff lines, stems and beams in page units -- and the host
tessellates it into the same triangle mesh the rest of the chrome uses. verovio
lives entirely on the client side; the host never depends on it.

One engraving carries three layers: what is **drawn**, where the **playback
cursor** sits at each onset, and the **notes** that sound. This example places
the notes on a timeline, plays them, and anchors the cursor to the engine's
sample clock -- so the score follows the audio with **one message per pass**
(``playhead_at``), the host reading the clock every frame from there, exactly
as the timeline views do.

The page is also **clickable and editable**: every primitive carries the MEI
``xml:id`` it was engraved from, so a press reports the element under the cursor
as an ``"element"`` event and the host highlights it, and dragging one up or
down the staff reports a ``"transpose"`` of that element by whole diatonic
steps. Because the id is the client's own, this script resolves both against its
own score: a click sounds the note, a drag transposes it, re-engraves the page
and sends it back -- the whole edit round trip, with nothing shared but the id.

Install the optional engraver::

    pip install verovio

Then, with the client importable (``pip install ./clients/python`` or
``PYTHONPATH=clients/python``)::

    python clients/python/examples/gui_score.py

A window opens showing the engraved phrase; it sounds once through and the
cursor follows it. Click a note to hear it again, or drag one up or down to
transpose it. Close the window to stop. Needs an audio device, a display and a
GPU adapter -- and, for the editing half, a verovio whose editor works (see
``third_party/BUILD-VEROVIO.md``; on the published 6.2.1 wheel every edit is
refused).
"""

import sys

from clausters import Event, Session, play
from clausters.gui import notation, window
from clausters.seq.timeline import Playhead, Timeline

# Six bars in Plaine & Easie -- the most compact way to type a score; verovio
# also reads MEI, MusicXML, ABC and Humdrum through the same loader. Quarters,
# eighths, a chord (``^``), a half and a whole, each bar filling its 4/4 exactly
# (verovio drops what overflows a measure, so an over-full bar would be drawn
# short and sound short).
PHRASE = ("@clef:G-2\n@keysig:xF\n@timesig:4/4\n@data:"
          "4CDEF/ 8GAGF 4ED/ 4C8DEFG4A/ 2G4FE/ 4C^E^G4G2C/ 1C/")


def scene(display_list: dict, sample_rate: float) -> dict:
    """A window filled by a scrollable, zoomable view of the engraved score."""
    return window(
        notation.score_view(display_list, scroll_id=10, score_id=11,
                            width=880.0, sample_rate=sample_rate),
        title="Engraved score (verovio -> GPU)", w=920, h=380,
    )


def phrase_timeline(notes: list) -> Timeline:
    """Place the engraved notes on a `Timeline`. The session's clock runs at one
    beat per second, so the engraving's milliseconds are beats/1000 -- score time
    and clock time are the same axis, which is what lets one anchor tie the
    cursor to the sound."""
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
    with Session.live(tempo=1.0) as session:
        server = session.server
        sr = float(server.options.sample_rate)
        gui = session.gui()
        gui.define(1, scene(dl, sr))

        session.start()
        Playhead(phrase_timeline(dl["notes"]), session.clock, server).play()
        # Anchor the cursor: `playhead_at` is the sample-clock value that score
        # time 0 maps to, so the host draws the cursor at (clock - playhead_at)
        # every frame with nothing more sent. The events sound `latency` ahead of
        # their play time, so the anchor is the clock now plus that same latency.
        _, args = server.request("/clock", expect=("/clock.reply",))
        gui.set(11, playhead_at=float(args[0]) + server.latency * sr)
        print("the phrase plays and the cursor follows the engine clock; "
              "click a note to hear it, drag one up or down to transpose it, "
              "close the window to stop")

        # Both round trips run off the same id: the widget reports the MEI id
        # under the cursor, and that id indexes this script's own engraving.
        by_id = {note["id"]: note for note in dl["notes"]}
        while True:
            msg = gui.poll(timeout=0.1)
            if msg is None:
                continue
            addr, args = msg
            if addr == "/gui_closed":
                print("window closed")
                break
            if addr != "/gui_event" or len(args) < 3:
                continue
            if args[1] == "element":
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
                dl = score.display_list()
                gui.set(11, display_list=notation.page_json(dl))
                by_id = {note["id"]: note for note in dl["notes"]}
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
