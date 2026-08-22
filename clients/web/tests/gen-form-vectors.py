#!/usr/bin/env python3
"""Generate form-vectors.json from the Python client's reference arrangement.

The arrangement is one layer written twice — `clausters/form/` and
`clients/web/src/form/` — and what has to agree is not the source but the two
things that leave it: the **document** a composition is written as (a shared
format three languages read) and the **flattened timeline** it renders to (the
absolute beats, and the events at them, including what a placement's length
trims).

So this script builds a handful of compositions with the Python surface and
freezes both for each one; `tests/form-parity.test.ts` rebuilds the same
compositions with the TypeScript surface and asserts the same two results. A
rule that drifts into one client — a trim rounding differently, a config key
spelled the language's way rather than the file's — fails here instead of in a
piece that reopens wrong.

The JSON is committed; regenerate with:

    python3 gen-form-vectors.py

(from clients/web/tests/, with the Python client importable — the repo's .venv
has it installed editable).
"""

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "python"))

from clausters.form import (  # noqa: E402
    Aggregate, Clang, Element, Generator, Segments, Sequence, Track, Vector,
    flatten, to_document, to_session,
)
from clausters.seq.automation import Automation  # noqa: E402
from clausters.seq import Event as SeqEvent  # noqa: E402
from clausters.seq.timeline import Timeline  # noqa: E402


class Buffer:
    """A stand-in for a server buffer: the conversion reads a ``bufnum``."""

    def __init__(self, bufnum):
        self.bufnum = bufnum


def a_piece():
    """Every leaf kind at once, placed and nested."""
    piece = Aggregate(name="piece")
    piece.add(Clang(SeqEvent(midinote=60, dur=1.0)), offset=0.0, dur=1.0)
    piece.add(Vector(Buffer(100), instrument="take", duration=4.0), offset=2.0, dur=4.0)
    inner = Aggregate()
    inner.add(Clang(SeqEvent(midinote=67, dur=0.5)), offset=0.0, dur=0.5)
    piece.add(inner, offset=8.0, dur=2.0)
    return piece


def a_trimmed_placement():
    """A placement shorter than what it holds: the DAW rule, where the trim
    happens rather than where the element says its length is."""
    held = Aggregate()
    held.add(Clang(SeqEvent(midinote=60, dur=2.0)), offset=0.0)
    held.add(Clang(SeqEvent(midinote=64, dur=2.0)), offset=2.0)
    held.add(Clang(SeqEvent(midinote=67, dur=2.0)), offset=4.0)
    piece = Aggregate()
    piece.add(held, offset=1.0, dur=3.0)
    return piece


def a_track():
    """A set with the restrictions of a multitrack view: its items are the
    client's own events, and each is a node with an id."""
    timeline = Timeline()
    timeline.add(0.0, SeqEvent(midinote=48, dur=1.0))
    timeline.add(1.5, SeqEvent(midinote=55, dur=0.5))
    track = Track(timeline, name="bass")
    piece = Aggregate()
    piece.add(track, offset=4.0)
    return piece


def a_window():
    """A trimmed, looping take, and a join of two windows read as one thing."""
    piece = Aggregate()
    piece.add(
        Vector(Buffer(7), duration=2.0, instrument="take", start=44100.0, loop=True,
               controls={"amp": 0.5}),
        offset=0.0, dur=2.0,
    )
    piece.add(
        Segments([(Buffer(7), 0.0, 1.0), (Buffer(8), 22050.0, 1.5)],
                 instrument="take"),
        offset=2.0,
    )
    return piece


def a_frozen_generator():
    """A generator nothing in this process supplies, with what it last
    rendered: the floor a host with no language attached draws."""
    rendered = Aggregate()
    rendered.add(Clang(SeqEvent(midinote=72, dur=0.25)), offset=0.0, dur=0.25)
    piece = Aggregate()
    piece.add(
        Generator("melody", duration=4.0, name="melody", rendered=rendered),
        offset=0.0, dur=4.0,
    )
    piece.add(Sequence(None, duration=1.0, name="unheld"), offset=4.0)
    return piece


def a_curve_on_its_event():
    """An envelope attached to the note it shapes: a simultaneous aggregate of a
    `Clang` and a base `Element` wrapping an `Automation`.

    The curve is the case the writer has to get right leaf-side — a base
    `Element` is also what an *unknown* body comes back as, and telling the two
    apart is what decides whether the document carries the break-points or the
    automation's own fields.
    """
    curve = Automation.from_points([(0.0, 200.0, 1, 0.0), (2.0, 900.0, 2, 0.0),
                                    (4.0, 300.0, 1, 0.0)], None, name="freq")
    piece = Aggregate()
    piece.add(Aggregate([(0.0, Clang(SeqEvent(instrument="drone", dur=4.0))),
                         (0.0, Element(curve, duration=4.0))], name="sweep"),
              offset=0.0)
    return piece


#: (name, builder). Each is built twice — once here, once in TypeScript.
CASES = [
    ("a_piece", a_piece),
    ("a_trimmed_placement", a_trimmed_placement),
    ("a_track", a_track),
    ("a_window", a_window),
    ("a_frozen_generator", a_frozen_generator),
    ("a_curve_on_its_event", a_curve_on_its_event),
]


def flat(element):
    """The flattened timeline as data: the beat, and the event's parameters (or
    the item's class, for something that is not an event)."""
    out = []
    for beat, item in flatten(element):
        if isinstance(item, SeqEvent):
            out.append({"beat": beat, "event": dict(item)})
        else:
            out.append({"beat": beat, "item": type(item).__name__})
    return out


def main():
    cases = {}
    for name, build in CASES:
        element = build()
        cases[name] = {
            "document": to_document(element),
            "flat": flat(element),
            "relation": element.temporal_relation(),
        }

    # One session, so the table's own rule travels too: a document whose
    # sources the table does not cover is refused before it is written.
    take = Vector(Buffer(100), duration=4.0, instrument="take")
    piece = Aggregate()
    piece.add(take, offset=0.0, dur=4.0)
    session = to_session(piece, sources={
        100: {"location": "takes/one.wav", "lifetime": "session", "generation": 0},
    })

    path = pathlib.Path(__file__).with_name("form-vectors.json")
    path.write_text(json.dumps({"cases": cases, "session": session}, indent=1) + "\n")
    print(f"wrote {path}")


if __name__ == "__main__":
    main()
