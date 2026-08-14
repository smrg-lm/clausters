"""The arrangement round-tripping through the document format.

O1's Python half: what the crate defines as the shape, this client writes and
reads without losing anything it is supposed to keep -- and *by reference* what
no format can keep, which is a generator's code.
"""

import json

from clausters.form import Buffer, Event, Generator, Group, Sequence, Track
from clausters.form.document import ID_ATTR, from_document, to_document
from clausters.seq import Event as SeqEvent
from clausters.seq import Timeline


class FakeBuffer:
    """A buffer element wraps `clausters.defs.Buffer`, of which the document
    keeps only the slot number -- so a stand-in with a `bufnum` is the whole of
    what this conversion touches."""

    def __init__(self, bufnum):
        self.bufnum = bufnum


def a_group() -> Group:
    """A composition with one of everything the conversion has a body for."""
    group = Group()
    group.add(Event(SeqEvent(midinote=60, dur=1.0)), offset=0.0, dur=1.0)
    group.add(
        Track(Timeline([(0.0, SeqEvent(midinote=64)), (1.5, SeqEvent(midinote=67))])),
        offset=1.0,
    )
    group.add(
        Buffer(FakeBuffer(7), instrument="take", controls={"amp": 0.4}),
        offset=4.0,
        dur=2.0,
    )
    inner = Group(kind="logical")
    inner.add(Generator("rlpf", controls={"cutoff": 900.0}), offset=0.0)
    group.add(inner, offset=8.0)
    return group


def test_a_composition_round_trips_through_the_document():
    original = a_group()
    doc = to_document(original)
    back = from_document(doc)

    # The comparison is the document itself: two trees are the same composition
    # when they convert to the same document, which is exactly the property the
    # format is for.
    assert to_document(back) == doc


def test_the_document_is_json_and_says_so():
    doc = to_document(a_group())
    assert json.loads(json.dumps(doc)) == doc
    assert doc["version"] == 0
    assert doc["root"]["kind"] == "set"
    assert doc["root"]["grouping"] == "concrete"


def test_placements_and_grouping_survive():
    doc = to_document(a_group())
    members = doc["root"]["members"]
    assert [m["offset"] for m in members] == [0.0, 1.0, 4.0, 8.0]
    assert members[0]["dur"] == 1.0
    assert "dur" not in members[1]  # no placement length is not a length of zero
    assert members[3]["node"]["grouping"] == "logical"


def test_a_tracks_notes_become_placed_events():
    # Decision A: a note is an addressable node, not an opaque blob inside a
    # track -- which is what lets an edit name it and a log invert it.
    doc = to_document(a_group())
    track = doc["root"]["members"][1]["node"]
    assert track["kind"] == "set"
    assert [m["offset"] for m in track["members"]] == [0.0, 1.5]
    assert track["members"][0]["node"]["kind"] == "event"
    assert track["members"][0]["node"]["config"]["midinote"] == 64


def test_a_generator_travels_by_reference_and_comes_back_frozen():
    # An algorithm is never serialized, the way a project file never serializes
    # a plugin. Without a resolver it comes back as the reference itself, which
    # `Generator` accepts -- the frozen case, and the floor rather than a
    # failure.
    doc = to_document(a_group())
    generator = doc["root"]["members"][3]["node"]["members"][0]["node"]
    assert generator["kind"] == "generator"
    assert generator["config"] == {"generator": "rlpf", "controls": {"cutoff": 900.0}}

    frozen = from_document(doc)
    rebuilt = frozen.handles[3].element.handles[0].element
    assert isinstance(rebuilt, Generator)
    assert rebuilt.wraps == "rlpf"


def test_a_resolver_supplies_what_the_document_only_names():
    doc = to_document(a_group())
    resolved = from_document(
        doc, resolve=lambda kind, config: "RESOLVED" if kind == "generator" else None
    )
    generator = resolved.handles[3].element.handles[0].element
    assert generator.wraps == "RESOLVED"


def test_a_body_this_client_does_not_know_is_carried_rather_than_dropped():
    # The same rule the widget protocol runs on. A document written by a newer
    # client must come back out intact, or reopening a piece in an older build
    # would quietly delete part of it.
    doc = {
        "version": 3,
        "root": {
            "id": 1,
            "kind": "set",
            "grouping": "concrete",
            "members": [
                {"offset": 0.0, "node": {"id": 2, "kind": "constellation", "seeds": [1, 2]}}
            ],
        },
    }
    back = to_document(from_document(doc), version=3)
    assert back == doc


def test_ids_are_stable_across_conversions():
    # An edit made against one conversion has to still name the right node in
    # the next, so a second conversion cannot renumber the tree.
    group = a_group()
    first = to_document(group)
    assert to_document(group) == first

    # And an element added afterwards takes a fresh id rather than one already
    # in use.
    used = _ids(first["root"])
    group.add(Event(SeqEvent(midinote=72)), offset=12.0)
    second = to_document(group)
    new = [i for i in _ids(second["root"]) if i not in used]
    assert len(new) == 1
    assert new[0] > max(used)


def test_the_temporal_metadata_crosses_both_ways():
    element = Event(SeqEvent(midinote=60), onset=2.0, duration=0.5)
    element.resident = True
    doc = to_document(Group(children=[element]))
    node = doc["root"]["members"][0]["node"]
    assert node["onset"] == 2.0 and node["duration"] == 0.5 and node["resident"] is True

    back = from_document(doc).handles[0].element
    assert back.onset == 2.0 and back.duration == 0.5 and back.resident is True
    assert getattr(back, ID_ATTR) == node["id"]


def test_a_sequence_of_elements_is_members_and_a_pattern_is_a_reference():
    of_elements = Sequence([Event(SeqEvent(midinote=60)), Event(SeqEvent(midinote=62))])
    body = to_document(Group(children=[of_elements]))["root"]["members"][0]["node"]
    assert len(body["members"]) == 2
    assert "config" not in body

    of_a_pattern = Sequence("Pseq([1, 2, 3])")
    body = to_document(Group(children=[of_a_pattern]))["root"]["members"][0]["node"]
    assert body["config"] == {"sequence": "Pseq([1, 2, 3])"}


def _ids(node) -> list:
    out = [node["id"]]
    for member in node.get("members", []):
        out.extend(_ids(member["node"]))
    return out
