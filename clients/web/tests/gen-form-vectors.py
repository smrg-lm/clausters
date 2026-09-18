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
aggregate that reopens wrong.

The JSON is committed; regenerate with:

    python3 gen-form-vectors.py

(from clients/web/tests/, with the Python client importable — the repo's .venv
has it installed editable).
"""

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "python"))

from clausters.form import (Aggregate, Clang, Element, Generator, Segments,  # noqa: E402
                            Sequence, Track, Vector, flatten)
from clausters.seq.automation import Automation  # noqa: E402
from clausters.seq import Event as SeqEvent  # noqa: E402
from clausters.seq.timeline import Timeline  # noqa: E402


class Buffer:
    """A stand-in for a server buffer: the conversion reads a ``bufnum``."""

    def __init__(self, bufnum):
        self.bufnum = bufnum


def an_aggregate():
    """Every leaf kind at once, placed and nested."""
    aggregate = Aggregate(name="aggregate")
    aggregate.add(Clang(SeqEvent(midinote=60, dur=1.0)), offset=0.0, dur=1.0)
    aggregate.add(Vector(Buffer(100), instrument="take", duration=4.0), offset=2.0, dur=4.0)
    inner = Aggregate()
    inner.add(Clang(SeqEvent(midinote=67, dur=0.5)), offset=0.0, dur=0.5)
    aggregate.add(inner, offset=8.0, dur=2.0)
    return aggregate


def a_trimmed_placement():
    """A placement shorter than what it holds: the DAW rule, where the trim
    happens rather than where the element says its length is."""
    held = Aggregate()
    held.add(Clang(SeqEvent(midinote=60, dur=2.0)), offset=0.0)
    held.add(Clang(SeqEvent(midinote=64, dur=2.0)), offset=2.0)
    held.add(Clang(SeqEvent(midinote=67, dur=2.0)), offset=4.0)
    aggregate = Aggregate()
    aggregate.add(held, offset=1.0, dur=3.0)
    return aggregate


def a_track():
    """A set with the restrictions of a multitrack view: its items are the
    client's own events, and each is a node with an id."""
    timeline = Timeline()
    timeline.add(0.0, SeqEvent(midinote=48, dur=1.0))
    timeline.add(1.5, SeqEvent(midinote=55, dur=0.5))
    track = Track(timeline, name="bass")
    aggregate = Aggregate()
    aggregate.add(track, offset=4.0)
    return aggregate


def a_window():
    """A trimmed, looping take, and a join of two windows read as one thing."""
    aggregate = Aggregate()
    aggregate.add(
        Vector(Buffer(7), duration=2.0, instrument="take", start=44100.0, loop=True,
               controls={"amp": 0.5}),
        offset=0.0, dur=2.0,
    )
    aggregate.add(
        Segments([(Buffer(7), 0.0, 1.0), (Buffer(8), 22050.0, 1.5)],
                 instrument="take"),
        offset=2.0,
    )
    return aggregate


def a_frozen_generator():
    """A generator nothing in this process supplies, with what it last
    rendered: the floor a host with no language attached draws."""
    rendered = Aggregate()
    rendered.add(Clang(SeqEvent(midinote=72, dur=0.25)), offset=0.0, dur=0.25)
    aggregate = Aggregate()
    aggregate.add(
        Generator("melody", duration=4.0, name="melody", rendered=rendered),
        offset=0.0, dur=4.0,
    )
    aggregate.add(Sequence(None, duration=1.0, name="unheld"), offset=4.0)
    return aggregate


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
    aggregate = Aggregate()
    aggregate.add(Aggregate([(0.0, Clang(SeqEvent(instrument="drone", dur=4.0))),
                         (0.0, Element(curve, duration=4.0))], name="sweep"),
              offset=0.0)
    return aggregate


def a_mixed_aggregate():
    """The composition's own mixing: a muted lane, a soloed one, and a level.

    Both halves travel — the document (mixing rides in the node's configuration,
    and what is at its default states nothing) and the flattened timeline (one
    solo anywhere silences every branch that is not on a soloed path, and a
    level multiplies into the event's `amp`). A lane's *height* appears in
    neither, which is the other half of the same decision.
    """
    quiet = Track(Timeline([(0.0, SeqEvent(midinote=36, dur=1.0))]), name="quiet")
    quiet.mute = True
    lead = Track(Timeline([(0.0, SeqEvent(midinote=72, dur=1.0, amp=0.5))]), name="lead")
    lead.solo = True
    lead.level = 0.5
    pad = Track(Timeline([(0.0, SeqEvent(midinote=60, dur=1.0))]), name="pad")
    aggregate = Aggregate([(0.0, quiet), (0.0, lead), (0.0, pad)], name="mix")
    aggregate.level = 0.5
    return aggregate


#: (name, builder). Each is built twice — once here, once in TypeScript.
CASES = [
    ("an_aggregate", an_aggregate),
    ("a_trimmed_placement", a_trimmed_placement),
    ("a_track", a_track),
    ("a_window", a_window),
    ("a_frozen_generator", a_frozen_generator),
    ("a_curve_on_its_event", a_curve_on_its_event),
    ("a_mixed_aggregate", a_mixed_aggregate),
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
            "flat": flat(element),
            "relation": element.temporal_relation(),
        }

    path = pathlib.Path(__file__).with_name("form-vectors.json")
    path.write_text(json.dumps({"cases": cases}, indent=1) + "\n")
    print(f"wrote {path}")


if __name__ == "__main__":
    main()
