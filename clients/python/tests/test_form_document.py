"""The arrangement round-tripping through the document format.

O1's Python half: what the crate defines as the shape, this client writes and
reads without losing anything it is supposed to keep -- and *by reference* what
no format can keep, which is a generator's code.
"""

import json

import pytest

from clausters.form import Buffer, Event, Generator, Group, Sequence, Track
from clausters.form.document import FIRST_VERSION, ID_ATTR, from_document, to_document
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
    assert doc["version"] == FIRST_VERSION, "an unedited document is version one, not zero"
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


# ---- the session: the document plus where its material is ----

def test_a_session_carries_the_document_and_its_source_table():
    from clausters.form.document import SESSION_FORMAT, from_session, to_session

    session = to_session(
        a_group(),
        sources={
            7: {
                "location": {"at": "file", "path": "takes/vocal.wav"},
                "lifetime": "external",
                "generation": 0,
            }
        },
        provenance={"script": "song.py"},
    )
    assert session["format"] == SESSION_FORMAT
    assert session["document"]["version"] == FIRST_VERSION
    assert json.loads(json.dumps(session)) == session

    element, sources = from_session(session)
    assert sources[7]["location"]["path"] == "takes/vocal.wav"
    assert element is not None


def test_a_session_saved_mid_edit_reopens_with_the_edit_still_open():
    """A save never blocks on a confirmation, so the format has to be able to
    say *this is a working copy of that, and nobody has decided yet*."""
    from clausters.form.document import from_session, to_session

    session = to_session(
        a_group(),
        sources={
            8: {
                "location": {"at": "file", "path": "scratch/edit.wav"},
                "lifetime": "session",
                "generation": 3,
                "editing": {"from": 7, "confirmed": False},
            }
        },
    )
    _, sources = from_session(json.loads(json.dumps(session)))
    assert sources[8]["editing"] == {"from": 7, "confirmed": False}
    assert sources[8]["lifetime"] == "session", "promoted by the save"


def test_a_newer_session_format_is_refused_rather_than_half_read():
    from clausters.form.document import SESSION_FORMAT, from_session, to_session

    session = to_session(a_group())
    session["format"] = SESSION_FORMAT + 1
    with pytest.raises(ValueError, match="newer than this build"):
        from_session(session)


def test_a_generators_last_rendered_result_round_trips_as_ordinary_tree():
    """What a host with no language attached shows: a generator is code, so the
    frozen result is the whole of what such a host can draw."""
    from clausters.form import Generator, Track
    from clausters.seq import Timeline

    rendered = Track(Timeline([(0.0, SeqEvent(midinote=62))]), duration=2.0)
    generator = Generator("melody", rendered=rendered)
    group = Group()
    group.add(generator, offset=0.0, dur=2.0)

    doc = to_document(group)
    node = doc["root"]["members"][0]["node"]
    assert node["kind"] == "generator"
    assert node["rendered"]["kind"] == "set"
    assert node["rendered"]["duration"] == 2.0

    back = from_document(doc)
    reopened = back.members[0][2]
    assert reopened.rendered is not None
    assert reopened.rendered.duration == 2.0


def test_a_rendered_result_keeps_its_ids_across_two_conversions():
    """Or an edit made against one conversion would name a different node in
    the next -- the same rule the rest of the tree already follows."""
    from clausters.form import Generator, Track
    from clausters.form.document import ID_ATTR
    from clausters.seq import Timeline

    rendered = Track(Timeline([(0.0, SeqEvent(midinote=62))]), duration=2.0)
    group = Group()
    group.add(Generator("melody", rendered=rendered), offset=0.0, dur=2.0)

    first = to_document(group)
    second = to_document(group)
    assert first == second
    assert getattr(rendered, ID_ATTR) is not None


# ---- the binding: one implementation of what an edit means ----

def test_an_edit_applied_through_the_crate_is_the_crates_edit():
    """The binding's whole point: the client does not apply and then report, it
    hands the document and the intent across and takes back the new document
    plus what happened. Three clients cannot mean three different things by the
    same edit if only one of them implements it."""
    from clausters import _native

    group = a_group()
    doc = to_document(group)
    node = doc["root"]["members"][0]["node"]["id"]

    result = _native.document_apply(
        doc, {"intent": "place", "node": node, "offset": 4.3},
        against={"version": doc["version"]}, quant=1.0,
    )
    assert result["outcome"]["applied"] is True
    assert result["outcome"]["effective"]["offset"] == 4.0, "the grid snapped it"
    assert result["outcome"]["reason"] == "snapped to the grid"
    assert result["document"]["version"] == doc["version"] + 1
    assert result["document"]["root"]["members"][0]["offset"] == 4.0


def test_an_edit_against_a_superseded_version_comes_back_stale():
    from clausters import _native

    doc = to_document(a_group())
    node = doc["root"]["members"][0]["node"]["id"]
    result = _native.document_apply(
        doc, {"intent": "place", "node": node, "offset": 4.0},
        against={"version": doc["version"] + 5},
    )
    assert result["outcome"]["stale"] is True
    assert result["outcome"]["applied"] is False
    assert result["document"]["version"] == doc["version"], "and it did not move"


def test_the_edited_document_reads_back_into_the_arrangement():
    """The round trip that makes the binding a *model* rather than a service:
    what comes back out of the crate is a document this client reads with the
    same converter it wrote one with."""
    from clausters import _native

    doc = to_document(a_group())
    node = doc["root"]["members"][0]["node"]["id"]
    edited = _native.document_apply(
        doc, {"intent": "place", "node": node, "offset": 7.0}
    )["document"]

    back = from_document(edited)
    assert back.members[0][0] == 7.0


def test_a_selection_resolves_to_the_span_underneath_it_through_the_crate():
    """Placement and trim both, across the ABI -- so a client asking what is
    under a selection gets the same answer the host would."""
    from clausters import _native

    doc = {
        "version": 1,
        "root": {
            "id": 1, "kind": "set", "grouping": "concrete",
            "members": [{
                "offset": 2.0, "dur": 4.0,
                "node": {
                    "id": 2, "kind": "buffer",
                    "source": {
                        "source": 100, "lifetime": "external", "generation": 2,
                        "range": {"start": 480000, "end": 672000},
                    },
                },
            }],
        },
    }
    spans = _native.document_resolve(
        doc, {"start": 3.0 * 48000, "len": 48000}, frames_per_beat=48000.0
    )
    assert len(spans) == 1
    assert spans[0]["source"] == 100
    assert spans[0]["generation"] == 2
    assert spans[0]["range"] == {"start": 528000, "end": 576000}


def test_nothing_underneath_is_an_empty_list_and_not_a_failure():
    from clausters import _native

    spans = _native.document_resolve(
        to_document(a_group()), {"start": 0.0, "len": 1.0},
        frames_per_beat=48000.0, in_beats=True,
    )
    assert spans == []
