#!/usr/bin/env python3
"""Generate editing-vectors.json: a structure's projections, frozen.

A projection is what an editable structure owes an endpoint -- the props a host
draws it with, the payloads an edit becomes, the operations that make a server
sound it -- and the whole point of `clausters-editing` is that there is **one**
of each. That is the design; this is what proves it rather than assuming it.
The Python client asks the crate over the C ABI and the answers are frozen
here; `editing-parity.test.ts` asks the same questions over wasm and compares.

What it catches is precisely what nothing else does. Two clients drawing one
curve two ways is not a build failure, not a type error and not a lint: it is
two payloads nobody put side by side. Here they are side by side.

The JSON is committed; regenerate with:

    python3 gen-editing-vectors.py
"""

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "python"))
from clausters import _native  # noqa: E402


def curves():
    """The break-point curves, and what each one is asked with.

    Every case a view actually meets: a curve drawn for the first time, one
    drawn against an axis already in hand, one whose data outgrew that axis,
    one that spans no time at all, one with a ragged tail, and an empty one.
    """
    return [
        ("a curve drawn for the first time",
         [0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0, 2.0, 0.25, 1.0, 0.0],
         None, 0.0),
        ("an axis in hand is held, not recomputed",
         [0.0, 0.0, 1.0, 0.0, 1.0, 0.5, 1.0, 0.0],
         (-4.0, 4.0), 9.0),
        ("and widened where the data left it",
         [0.0, 0.0, 1.0, 0.0, 1.0, 40.0, 1.0, 0.0],
         (-1.0, 1.0), 0.5),
        ("a curve that spans nothing states no duration",
         [0.0, 0.5, 1.0, 0.0],
         None, 0.0),
        ("a trailing partial quad is not a point",
         [0.0, 0.5, 1.0, 0.0, 3.0, 0.5, 1.0],
         None, 0.0),
        ("shapes and curvature ride through untouched",
         [0.0, 0.0, 5.0, -3.5, 4.0, 1.0, 2.0, 0.0],
         None, 0.0),
        ("an empty curve", [], None, 0.0),
    ]


def pieces():
    """The pieces, and the table of what their sources were read into.

    One track with a box on it, an automation over the timeline and an envelope
    inside the box -- the smallest piece that exercises both curve kinds, the
    two time bases they are measured from, and a box over a source that was
    read against one that was not.
    """
    # `visible` is off unless the piece says otherwise, so one of the two is
    # open and the other folded away: `hidden` names exactly the second.
    # Every point carries the segment the wire states, so a curve reported back
    # untouched is byte-for-byte what went out -- which is what makes the
    # "nothing moved" and "only this one moved" cases below say anything.
    linear = {"shape": 1, "curve": 0.0}
    curve = {"id": 4, "visible": True, "target": {"min": -1.0, "max": 1.0},
             "points": [{"at": 0.0, "value": 0.0, "data": dict(linear)},
                        {"at": 2.0, "value": 1.0,
                         "data": {"shape": 5, "curve": -3.5}}]}
    layer = {"id": 5, "points": [{"at": 0.0, "value": 1.0, "data": dict(linear)},
                                 {"at": 1.0, "value": 0.0, "data": dict(linear)}]}
    region = {
        "id": 3, "position": 4.0, "length": 4.0,
        "content": {"fill": "window",
                    "window": {"source": {"source": 77, "lifetime": "session"},
                               "start": 0.5, "duration": 4.0}},
        "automation": [layer],
    }
    track = {"id": 1, "name": "drums", "level": 0.5, "muted": True,
             "lanes": [{"id": 2, "regions": [region]}], "automation": [curve]}
    piece = {"version": 1, "tracks": [track]}
    ramped = json.loads(json.dumps(piece))
    ramped["tempo"] = [{"at": 0.0, "bpm": 60.0},
                       {"at": 4.0, "bpm": 120.0, "ramp": True}]
    return [
        ("a piece at one beat a second", piece,
         {"77": {"buffer": 12, "channels": 1}}),
        ("the same piece over a source nobody read", piece, {}),
        ("and over a tempo that moves, where a length is not a ratio",
         ramped, {"77": {"buffer": 12, "channels": 1}}),
    ]


def gestures(piece, sources):
    """The gestures, and what each domain needs beside the report.

    One per vocabulary, plus the cases that are *answers* rather than edits: a
    tag a domain does not answer for (nothing to say), and a gesture a domain
    does understand and cannot write (a refusal). Those two are the pair that
    has to be told apart identically in both clients, because one leaves the
    picture alone and the other makes the host redraw it.
    """
    props = _native.multitrack_props(piece, 48000.0, 60.0,
                                     {int(k): v for k, v in sources.items()})
    clips = list(props["clips"])
    clips[2] = float(clips[2]) + 4.0 * 48_000.0
    lanes = list(props["lanes"])
    lanes[5] = 0.25
    points = list(props["points"])
    points[len(points) - 3] = 0.75
    timeline = [{"at": 0.0, "data": {"midinote": 60, "instrument": "bell"}},
                {"at": 1.0, "data": {"osc": "/cue", "args": [1]}}]
    piece_request = {"state": piece, "rate": 48000.0, "defaultBpm": 60.0,
                     "sources": sources}
    return [
        ("a curve drawn back", "points", "points",
         {"values": [0.0, 0.0, 1.0, 0.0, 2.0, 1.0, 5.0, -3.5]}),
        ("a tag no curve answers for", "points", "clips",
         {"values": [0.0, 0.0, 1.0, 0.0]}),
        ("a stroke over samples, with the run it replaced", "samples", "draw",
         {"values": [0, 4, [0.5, 0.25], [0.0, 0.0]]}),
        ("an inverse that does not cover the write", "samples", "draw",
         {"values": [0, 4, [0.5, 0.25], [0.0]]}),
        ("a note moved, keeping what the roll cannot draw", "events", "notes",
         {"values": [960.0, 480.0, 64, 100, 0], "state": timeline,
          "unitsPerBeat": 480.0, "editable": True}),
        ("a marker dragged, matched by its label", "events", "osc",
         {"values": [1440.0, "/cue"], "state": timeline,
          "unitsPerBeat": 480.0, "editable": True}),
        ("a marker the roll invented, refused out loud", "events", "osc",
         {"values": [1440.0, "/cue", 1920.0, ""], "state": timeline,
          "unitsPerBeat": 480.0, "editable": True}),
        ("a box dragged four beats along", "multitrack", "clips",
         {"values": clips, **piece_request}),
        ("a fader moved", "multitrack", "lanes",
         {"values": lanes, **piece_request}),
        ("a layer's break-point, on its box's own axis", "multitrack", "points",
         {"values": points, **piece_request}),
        ("and every curve reported back is no edit", "multitrack", "points",
         {"values": list(props["points"]), **piece_request}),
        ("a piece reported back unchanged is no edit", "multitrack", "clips",
         {"values": list(props["clips"]), **piece_request}),
    ]


def main() -> None:
    vectors = []
    for name, points, kept, held in curves():
        vectors.append({
            "name": name,
            "kind": "points_props",
            "points": points,
            "kept": list(kept) if kept is not None else None,
            "held": held,
            "props": _native.points_props(points, kept, held),
        })
    for name, piece, sources in pieces():
        vectors.append({
            "name": name,
            "kind": "multitrack_props",
            "piece": piece,
            "rate": 48000.0,
            "bpm": 60.0,
            "sources": sources,
            "props": _native.multitrack_props(piece, 48000.0, 60.0,
                                              {int(k): v for k, v in sources.items()}),
        })
    piece, sources = pieces()[0][1], pieces()[0][2]
    for name, domain, tag, request in gestures(piece, sources):
        vectors.append({
            "name": name,
            "kind": "intake",
            "domain": domain,
            "tag": tag,
            "request": request,
            "intake": _native.editing_intake(domain, tag, **request),
        })
    out = pathlib.Path(__file__).with_name("editing-vectors.json")
    out.write_text(json.dumps(vectors, indent=2) + "\n")
    print(f"wrote {out.name}: {len(vectors)} vectors")


if __name__ == "__main__":
    main()
