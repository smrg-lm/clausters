#!/usr/bin/env python3
"""Generate notation-vectors.json: the same score, engraved by both clients.

This is the check the whole notation layer exists for. A window and a page
engrave with **one verovio** -- the same pinned sources, the same importer
options, one built natively and one by Emscripten -- and they walk the SVG with
one shared core. So the drawing must come out identical, and this is what says
so: the Python client engraves a fixture and freezes the page; the TypeScript
one engraves it in a browser-shaped stack and asserts the same page.

What is compared, and what is deliberately not: **ids are normalized away**.
verovio mints fresh `xml:id`s on every load and a client may never depend on
one across loads, so each id is replaced by the index of its first appearance --
which still checks that the *same* primitives share the *same* element, and
stops the vector from pinning strings that are meaningless between processes.

The JSON is committed; regenerate with:

    python3 gen-notation-vectors.py

(from clients/web/tests/, with the Python client importable -- the repo's .venv
has it installed editable, and its wheel carries libverovio.)
"""

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "python"))

from clausters.gui import notation  # noqa: E402
from clausters.seq import Event, rest  # noqa: E402
from clausters.seq.timeline import Timeline  # noqa: E402


def a_scale():
    """A C major scale in quarters: two bars, eight noteheads, no accidentals."""
    return [Event(midinote=m, dur=1.0) for m in (60, 62, 64, 65, 67, 69, 71, 72)]


def a_rhythm():
    """Dotted and tied values, a rest, and a bar the notes overrun -- the parts
    of the encoder that split and tie."""
    return [
        Event(midinote=67, dur=1.5),
        Event(midinote=69, dur=0.5),
        rest(1.0),
        Event(midinote=71, dur=3.0),
        Event(midinote=72, dur=2.0),
    ]


def a_chord_timeline():
    """Simultaneous events become chords, and a gap becomes a rest."""
    timeline = Timeline()
    for midi in (60, 64, 67):
        timeline.add(0.0, Event(midinote=midi, dur=1.0))
    timeline.add(2.0, Event(midinote=65, dur=1.0))
    for midi in (62, 71):
        timeline.add(3.0, Event(midinote=midi, dur=1.0))
    return timeline


#: (name, the MEI, the options the page is engraved with).
def cases():
    return [
        ("a_scale", notation.from_notes(a_scale()), {}),
        ("a_rhythm", notation.from_notes(a_rhythm()), {}),
        ("a_chord_timeline", notation.from_timeline(a_chord_timeline()), {}),
        # A different staff size and wrap width: the options travel through the
        # shared `engrave_options`, so both clients configure verovio the same.
        ("a_small_page", notation.from_notes(a_scale()),
         {"scale": 30, "page_width": 1200}),
        # A key and a clef the encoder spells with: flats, and the bass staff.
        ("in_f_on_the_bass_clef",
         notation.from_notes([Event(midinote=m, dur=1.0) for m in (53, 55, 57, 58)],
                             meter="3/4", clef="F4", key="F"),
         {}),
    ]


def normalized(page: dict) -> dict:
    """The page with every engraver-minted id replaced by the order it first
    appears in, so two processes' pages are comparable.

    ``elements`` is a list of those same ids rather than objects carrying one,
    so it is normalized by name: leaving it raw would compare two engravings'
    minted ids directly, which is the one thing this normalization exists to
    avoid."""
    ids: dict = {}

    def index(value):
        if value not in ids:
            ids[value] = len(ids)
        return ids[value]

    def walk(value):
        if isinstance(value, dict):
            return {k: (index(v) if k == "id" and isinstance(v, str)
                        else [index(e) for e in v] if k == "elements"
                        else walk(v))
                    for k, v in value.items()}
        if isinstance(value, list):
            return [walk(v) for v in value]
        return value

    return walk(page)


def main():
    out = {"engraver": None, "cases": {}}
    for name, mei, options in cases():
        score = notation.Score(mei, **options)
        page = score.display_list(1)
        out["cases"][name] = {
            "mei": mei,
            "options": options,
            "page": normalized(page),
        }
        # One transpose applied through the shared state machine, so the vector
        # also pins what an *edit* draws -- the round trip, not only the load.
        first = page["notes"][0]["id"]
        assert score.transpose(first, 1), "the engraver accepted the step"
        out["cases"][name]["edited"] = normalized(score.display_list(1))

    path = pathlib.Path(__file__).with_name("notation-vectors.json")
    path.write_text(json.dumps(out, indent=1, sort_keys=True) + "\n")
    print(f"wrote {path} ({path.stat().st_size // 1024} KB)")


if __name__ == "__main__":
    main()
