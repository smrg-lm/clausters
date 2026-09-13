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


def exchange():
    """**A recorded exchange between a host and an owner**, replayed as a fold.

    The acceptance of the conversation milestone stated as data: the host
    cannot tell which client it is talking to from the message sequence, so the
    sequence is recorded once and the turns it comes to are frozen. Every case
    the protocol has is in it -- an opening edit, a second gesture inside one
    round trip (the answers lag, and that is not a conflict), a script's edit
    that raises the floor, the refusal that follows, an undo addressed to the
    window, a widget this editor never drew, and a close.
    """
    def event(tag, widget, seq, against, version, *, owns=True, window=False):
        return {"addr": "/gui_event", "argc": 5, "widget": widget, "seq": seq,
                "against": against, "tag": tag, "version": version,
                "isWindow": window, "owns": owns}

    # The third of each triple is what the editor sets ``applied`` to after
    # routing -- the version the edit left behind. It is part of the exchange
    # and not bookkeeping: it is what turns "the version moved" into "it moved
    # by someone *else*".
    return [
        ("the opening edit, against the version it was told",
         event("clips", 7, 1, 1, 1), 2),
        ("a second gesture inside one round trip still names the old version",
         event("clips", 7, 2, 1, 2), 3),
        ("a script edited the piece: the version moved and no event moved it",
         event("clips", 7, 3, 2, 9), None),
        ("and a later gesture against the picture that is gone is refused too",
         event("clips", 7, 4, 8, 9), None),
        ("an unstated version applies unchecked",
         event("clips", 7, 5, 0, 9), 9),
        ("an undo is addressed to the window, not to a widget",
         event("undo", 3, 6, 9, 9, owns=False, window=True), None),
        ("a widget this editor never drew says nothing",
         event("clips", 8, 7, 9, 9, owns=False), None),
        ("a malformed event is nothing rather than an error",
         {"addr": "/gui_event", "argc": 2}, None),
        ("somebody else's window closing is not this editor's",
         {"addr": "/gui_closed", "argc": 1, "isWindow": False}, None),
        ("and this editor's is",
         {"addr": "/gui_closed", "argc": 1, "isWindow": True}, None),
    ]


def answers():
    """What to send back, in every shape there is."""
    return [
        ("an unasked push with nothing to say is not sent", 0, 3, None, []),
        ("an answered gesture is an ack", 2, 3, None, []),
        ("a refusal rides with it", 2, 3, "the composition changed since this edit", []),
        ("corrections make it a push", 2, 3, None,
         [(7, {"points": [0.0, 0.5, 1.0, 0.0]})]),
        ("and an unasked push carries them with a stamp of zero", 0, 3, None,
         [(7, {"clips": ["a", "0", 0.0, 1.0, 0.0, "a", -1]})]),
    ]


def editor_exchange():
    """**The multitrack editor, turn by turn**, as the host and the server see
    it.

    The acceptance of the multitrack application stated as data: the clients
    hold a handle over the applications crate and carry out what it answers, so
    a hand's gestures come to **the same messages to the host, the same calls
    on the playback and the same piece** whichever client carries them. The
    gestures run through the Python client's own `MultitrackEditor.apply`, and
    `editing-parity.test.ts` replays them through the web client's.

    Widgets are named by role rather than by number -- `piece`, `ruler`, the
    window, the transport row -- because which id a client's allocator hands
    out is its own, and what is compared is what the editor decided.
    """
    from clausters.gui.editing import MultitrackEditor
    from clausters.multitrack import Multitrack

    SR = 48_000.0

    def box(id, at):
        return {"id": id, "position": at, "length": 2.0,
                "content": {"fill": "window",
                            "window": {"source": {"source": 1, "lifetime": "session"},
                                       "start": 0.0, "duration": 2.0}}}

    written = {"version": 1, "tracks": [
        {"id": 10, "name": "one",
         "lanes": [{"id": 11, "regions": [box(12, 0.0), box(13, 4.0)]}]},
        {"id": 20, "name": "two", "lanes": [{"id": 21, "regions": [box(22, 0.0)]}]},
    ]}
    sources = {"1": 7}
    controls = {"rewind": 50, "play": 51, "stop": 52, "clock": 53}

    class Recorder:
        """A host that writes down what it is told."""

        def __init__(self):
            self.messages = []

        def ack(self, seq, doc_version=0, reason=None):
            self.messages.append(["ack", seq, doc_version, reason, []])

        def push(self, seq, *corrections, doc_version=0, reason=None):
            self.messages.append(["push", seq, doc_version, reason,
                                  [[w, p] for w, p in corrections]])

        def set(self, *args, **props):
            pass

    class Playback:
        """A playback that writes down what it is asked to do."""

        def __init__(self):
            self.calls = []
            self.playing = False
            self.meters = {}

        def play(self):
            self.calls.append(["play"])
            self.playing = True

        def pause(self):
            self.calls.append(["pause"])
            self.playing = False

        def stop(self):
            self.calls.append(["stop"])
            self.playing = False

        def cue(self, beat):
            self.calls.append(["cue", beat])

        def sync(self):
            self.calls.append(["sync"])

    piece = Multitrack.read(json.loads(json.dumps(written)))
    ed = MultitrackEditor(piece, sample_rate=SR, sources={1: 7})
    host, playback = Recorder(), Playback()
    ed._host = host
    ed._window = 1
    ed.playback = playback
    ed.draw()
    ed._controls = dict(controls)
    ids = {"piece": ed.view.piece, "ruler": ed.view.ruler, "window": 1, **controls}
    roles = {ed.view.piece: "piece", ed.view.ruler: "ruler"}

    def septuples():
        flat = ed.view.props(ed, ed.view.piece)["clips"]
        return [list(flat[i:i + 7]) for i in range(0, len(flat), 7)]

    def moved(name, at):
        out = []
        for row in septuples():
            if row[0] == name:
                row[2] = at * SR
            out += row
        return out

    def split(name):
        out = []
        for row in septuples():
            if row[0] == name:
                first = list(row)
                first[3] = 1.0 * SR
                out += first
                out += ["white 2", row[1], row[2] + 1.0 * SR, 1.0 * SR, 1.0 * SR,
                        "", row[6]]
            else:
                out += row
        return out

    script = [
        ("a box moved on its lane", "piece", "clips", lambda: moved("12", 2.0), "now"),
        ("a box split under a name the host minted", "piece", "clips",
         lambda: split("13"), "now"),
        ("the cursor placed on the ruler", "ruler", "locate", lambda: [4.0 * SR], "now"),
        ("play from the transport row", "play", "click", list, "now"),
        ("stop from the transport row", "stop", "click", list, "now"),
        ("rewind from the transport row", "rewind", "click", list, "now"),
        ("the space bar", "window", "play", list, "now"),
        ("an undo", "window", "undo", list, "now"),
        ("an edit made against a picture that is gone", "piece", "clips",
         lambda: moved("22", 6.0), 1),
    ]
    turns = []
    for seq, (name, target, tag, values, against) in enumerate(script, start=1):
        host.messages.clear()
        playback.calls.clear()
        args = values()
        stated = int(ed._version) if against == "now" else against
        changed = ed.apply("/gui_event", [ids[target], seq, stated, tag, *args])
        turns.append({
            "name": name,
            "target": target,
            "seq": seq,
            "against": against,
            "tag": tag,
            "values": args,
            "changed": bool(changed),
            "messages": [
                [kind, s, version, reason,
                 [[roles.get(w, w), props] for w, props in corrections]]
                for kind, s, version, reason, corrections in host.messages],
            "playback": list(playback.calls),
            "regions": [[t.id, r.id, r.position, r.length]
                        for t in piece.tracks for lane in t.lanes
                        for r in lane.regions],
        })
    return {
        "name": "the multitrack editor answers a recorded exchange",
        "kind": "editor_exchange",
        "piece": written,
        "rate": SR,
        "sources": sources,
        "controls": controls,
        "turns": turns,
    }


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
    state = {"floor": 1, "applied": 1}
    for name, message, applied in exchange():
        answer = _native.conversation_read(state, message)
        state = answer["state"]
        if applied is not None:
            state = dict(state, applied=applied)
        vectors.append({
            "name": name,
            "kind": "conversation",
            "message": message,
            "turn": answer["turn"],
            "applied": applied,
            "after": state,
        })
    for name, seq, version, reason, corrections in answers():
        vectors.append({
            "name": name,
            "kind": "answer",
            "seq": seq,
            "docVersion": version,
            "reason": reason,
            "corrections": [{"widget": w, "props": p} for w, p in corrections],
            "answer": _native.conversation_answer(seq, version, reason, corrections),
        })
    for name, piece, _sources in pieces():
        vectors.append({
            "name": f"the names of {name}",
            "kind": "names",
            "piece": piece,
            "names": _native.multitrack_names(piece),
        })
    vectors.append(editor_exchange())
    out = pathlib.Path(__file__).with_name("editing-vectors.json")
    out.write_text(json.dumps(vectors, indent=2) + "\n")
    print(f"wrote {out.name}: {len(vectors)} vectors")


if __name__ == "__main__":
    main()
