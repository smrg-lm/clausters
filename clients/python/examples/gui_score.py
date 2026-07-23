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

The page is also **clickable**: every primitive carries the MEI ``xml:id`` it
was engraved from, so a press reports the element under the cursor as an
``"element"`` event and the host highlights it. Because that id is the client's
own, this script looks the note up in its own engraving and sounds it -- the
round trip an editing pass builds on.

Install the optional engraver::

    pip install verovio

Then, with the client importable (``pip install ./clients/python`` or
``PYTHONPATH=clients/python``)::

    python clients/python/examples/gui_score.py

A window opens showing the engraved phrase; it sounds once through and the
cursor follows it. Click a note to hear it again. Close the window to stop.
Needs an audio device, a display and a GPU adapter.
"""

import sys

from clausters import Event, Session, play
from clausters.gui import notation, window
from clausters.seq.timeline import Playhead, Timeline

# A multi-bar phrase in Plaine & Easie -- the most compact way to type a score;
# verovio also reads MEI, MusicXML, ABC and Humdrum through the same loader. Long
# enough to wrap into several systems, so the page scrolls.
PHRASE = ("@clef:G-2\n@keysig:xF\n@timesig:4/4\n@data:"
          "4CDEF GABc'/ 4c'BAG FEDC/ 4{DE}F2 4G8AB c'4/ "
          "4c'c'BB AAGG/ 4FFEE DDC2/ (4CEG) (4c'GE) 4C2/")


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
    # a narrow page so the phrase wraps into a few systems and the view scrolls
    dl = notation.engrave(PHRASE, page_width=1500)
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
              "click a note to hear it, close the window to stop")

        # The click round trip: the widget reports the MEI id under the cursor,
        # and that id indexes this script's own engraving -- so the clicked
        # notehead sounds, with nothing shared but the id.
        by_id = {note["id"]: note for note in dl["notes"]}
        while True:
            msg = gui.poll(timeout=0.1)
            if msg is None:
                continue
            addr, args = msg
            if addr == "/gui_closed":
                print("window closed")
                break
            if addr == "/gui_event" and len(args) >= 3 and args[1] == "element":
                note = by_id.get(args[2])
                if note is None:
                    print(f"  clicked {args[2] or '(blank paper)'}")
                    continue
                print(f"  clicked note {args[2]}: MIDI {note['pitch']} "
                      f"at {note['t']:.0f} ms")
                play(Event(midinote=note["pitch"], dur=note["dur"] / 1000.0,
                           amp=0.15))


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ValueError) as e:
        sys.exit(str(e))
