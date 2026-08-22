#!/usr/bin/env python3
"""Generate editor-vectors.json: the same composition, drawn by both clients.

The multitrack editor is one driver in two languages, and what leaves it is a
**GuiDef**: the lanes, the clips, the bodies, the ids, and every number the
beats↔timeline-samples bridge produced. So this freezes what the Python editor
draws for a handful of compositions — each exercising one branch of the mapping
rule — and `editor-parity.test.ts` draws the same ones with the TypeScript
editor and asserts the same tree.

Ids are the drawn tree's own: both editors count a host-less draw from
``base_id`` (10 000), so the registries line up too and a mismatch is a real
difference rather than an allocation order.

The JSON is committed; regenerate with:

    python3 gen-editor-vectors.py

(from clients/web/tests/, with the Python client importable — the repo's .venv
has it installed editable.)
"""

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "python"))

from clausters.defs import SynthDef, control, in_, out, sine  # noqa: E402
from clausters.defs.buffer import Buffer as ServerBuffer  # noqa: E402
from clausters.form import (  # noqa: E402
    Aggregate, Clang, Element, Generator, Segments, Track, Vector,
)
from clausters.form.aggregate import LOGICAL  # noqa: E402
from clausters.gui.editor import Editor  # noqa: E402
from clausters.seq.automation import Automation  # noqa: E402
from clausters.seq.event import Event as SeqEvent  # noqa: E402
from clausters.seq.timeline import Timeline  # noqa: E402
from clausters.defs.ugens import Env  # noqa: E402

SR = 48_000.0
TEMPO = 2.0          # beats per second (120 bpm)
BEAT = SR / TEMPO    # 24000 timeline samples per beat


def buffer(bufnum, beats=4.0, channels=1):
    return ServerBuffer(bufnum=bufnum, frames=int(beats * BEAT), channels=channels,
                        sample_rate=SR)


def a_song():
    """Two lanes: a take on one, a melody on another — the ordinary case."""
    take = Vector(buffer(7), duration=4.0)
    audio = Aggregate([(0.0, take)], name="audio")
    melody = Track(Timeline([(0.0, SeqEvent(midinote=60, dur=1.0)),
                             (1.0, SeqEvent(midinote=64, dur=1.0)),
                             (2.0, SeqEvent(midinote=67, dur=2.0))]))
    lead = Aggregate([(2.0, melody)], name="lead")
    return Aggregate([(0.0, audio), (0.0, lead)], name="song")


def a_windowed_take():
    """A trimmed, looping take: the window travels as props of the clip."""
    take = Vector(buffer(9), duration=2.0, start=12_000.0, loop=True)
    return Aggregate([(1.0, take)], name="trimmed")


def a_joined_take():
    """Several windows read as one: one clip, one body per segment."""
    joined = Segments([(buffer(3), 0.0, 1.0), (buffer(4), 6_000.0, 1.5)])
    return Aggregate([(0.0, joined)], name="joined")


def an_envelope_on_its_event():
    """A simultaneous aggregate: one clip, the members' bodies layered — the
    arrangement's answer to attaching an envelope to the event it shapes."""
    curve = Automation(Env([0.2, 0.9, 0.1], [1.0, 1.0]), None, name="cutoff")
    notes = Track(Timeline([(0.0, SeqEvent(midinote=72, dur=2.0))]), duration=2.0)
    pair = Aggregate([(0.0, 2.0, Element(curve, duration=2.0)),
                      (0.0, 2.0, notes)], name="shaped")
    return Aggregate([(0.0, pair)], name="song")


def a_curve_over_a_rendering():
    """The composer's `sweep` lane: an envelope over a note that is *not* an
    editable timeline. One clip, two layered bodies — and the two say their
    editability separately, which is the whole point of the case. Before
    ``notes_editable`` the roll's refusal was written as the clip's ``editable``
    and the curve inherited it: an envelope that drew and could not be touched.
    """
    curve = Automation.from_points([(0.0, 200.0, 1, 0.0), (2.0, 900.0, 2, 0.0),
                                    (4.0, 300.0, 1, 0.0)], None, name="freq")
    voice = Clang(SeqEvent(instrument="drone", dur=4.0, legato=1.0, amp=0.12))
    pair = Aggregate([(0.0, voice), (0.0, Element(curve, duration=4.0))],
                     name="sweep")
    return Aggregate([(0.0, pair)], name="song")


def a_nested_aggregate():
    """A nested aggregate is a labeled rectangle — its summary — until it is
    expanded into lanes of its own."""
    inner = Aggregate([(0.0, Clang(SeqEvent(midinote=60, dur=1.0))),
                       (1.0, Clang(SeqEvent(midinote=64, dur=1.0)))],
                      name="phrase")
    return Aggregate([(0.0, inner)], name="song")


def a_patch():
    """A logical aggregate draws as a directed patch, not a timeline lane."""
    src = SynthDef("gsrc", out(control("out"), sine(control("freq", 220.0))))
    sink = SynthDef("gsink", out(0, in_(control("in")) * control("amp", 0.3)))
    g = Aggregate(kind=LOGICAL, name="chain", buses=[("mix", "audio")])
    g.add(Generator(src, controls={"out": "mix"}))
    g.add(Generator(sink, controls={"in": "mix"}))
    return g


#: (name, builder, editor kwargs, expand?) — one per branch of the mapping rule.
CASES = [
    ("a_song", a_song, {"quant": 0.25}, False),
    ("a_windowed_take", a_windowed_take, {}, False),
    ("a_joined_take", a_joined_take, {}, False),
    ("an_envelope_on_its_event", an_envelope_on_its_event, {}, False),
    ("a_curve_over_a_rendering", a_curve_over_a_rendering, {}, False),
    ("a_nested_aggregate", a_nested_aggregate, {}, False),
    ("a_nested_aggregate_expanded", a_nested_aggregate, {}, True),
    ("a_patch", a_patch, {}, False),
]


def main():
    cases = {}
    for name, build, kwargs, expand in CASES:
        element = build()
        ed = Editor(element, sample_rate=SR, tempo=TEMPO, **kwargs)
        if expand:
            # The base level: resolve the nested aggregate into lanes.
            ed.expand(element.members[0][2])
        cases[name] = {
            "quant": kwargs.get("quant", 0.0),
            "expand": expand,
            "tree": ed.draw(),
            "extent": ed.extent(),
        }

    path = pathlib.Path(__file__).with_name("editor-vectors.json")
    path.write_text(json.dumps({"sample_rate": SR, "tempo": TEMPO,
                                "cases": cases}, indent=1, sort_keys=True) + "\n")
    print(f"wrote {path} ({path.stat().st_size // 1024} KB)")


if __name__ == "__main__":
    main()
