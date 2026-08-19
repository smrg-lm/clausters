"""The arrangement round-tripping through the document format.

O1's Python half: what the crate defines as the shape, this client writes and
reads without losing anything it is supposed to keep -- and *by reference* what
no format can keep, which is a generator's code.
"""

import json

import pytest

from clausters.form import Aggregate, Generator, Clang, Sequence, Track, Vector
from clausters.form.document import (FIRST_VERSION, ID_ATTR, from_document, from_session,
                                      to_document, to_session)
from clausters.seq import Event as SeqEvent
from clausters.seq import Timeline


class FakeBuffer:
    """A vector wraps `clausters.defs.Buffer`, of which the document keeps only
    the slot number -- so a stand-in with a `bufnum` is the whole of what this
    conversion touches."""

    def __init__(self, bufnum):
        self.bufnum = bufnum


def an_aggregate() -> Aggregate:
    """A composition with one of everything the conversion has a body for."""
    aggregate = Aggregate()
    aggregate.add(Clang(SeqEvent(midinote=60, dur=1.0)), offset=0.0, dur=1.0)
    aggregate.add(
        Track(Timeline([(0.0, SeqEvent(midinote=64)), (1.5, SeqEvent(midinote=67))])),
        offset=1.0,
    )
    aggregate.add(
        Vector(FakeBuffer(7), instrument="take", controls={"amp": 0.4}),
        offset=4.0,
        dur=2.0,
    )
    inner = Aggregate(kind="logical")
    inner.add(Generator("rlpf", controls={"cutoff": 900.0}), offset=0.0)
    aggregate.add(inner, offset=8.0)
    return aggregate


def test_a_composition_round_trips_through_the_document():
    original = an_aggregate()
    doc = to_document(original)
    back = from_document(doc)

    # The comparison is the document itself: two trees are the same composition
    # when they convert to the same document, which is exactly the property the
    # format is for.
    assert to_document(back) == doc


def test_the_document_is_json_and_says_so():
    doc = to_document(an_aggregate())
    assert json.loads(json.dumps(doc)) == doc
    assert doc["version"] == FIRST_VERSION, "an unedited document is version one, not zero"
    assert doc["root"]["kind"] == "aggregate"
    assert doc["root"]["grouping"] == "concrete"


def test_placements_and_grouping_survive():
    doc = to_document(an_aggregate())
    members = doc["root"]["members"]
    assert [m["offset"] for m in members] == [0.0, 1.0, 4.0, 8.0]
    assert members[0]["dur"] == 1.0
    assert "dur" not in members[1]  # no placement length is not a length of zero
    assert members[3]["node"]["grouping"] == "logical"


def test_a_tracks_notes_become_placed_clangs():
    # Decision A: a note is an addressable node, not an opaque blob inside a
    # track -- which is what lets an edit name it and a log invert it.
    doc = to_document(an_aggregate())
    track = doc["root"]["members"][1]["node"]
    assert track["kind"] == "aggregate"
    assert [m["offset"] for m in track["members"]] == [0.0, 1.5]
    assert track["members"][0]["node"]["kind"] == "clang"
    assert track["members"][0]["node"]["config"]["midinote"] == 64


def test_a_generator_travels_by_reference_and_comes_back_frozen():
    # An algorithm is never serialized, the way a project file never serializes
    # a plugin. Without a resolver it comes back as the reference itself, which
    # `Generator` accepts -- the frozen case, and the floor rather than a
    # failure.
    doc = to_document(an_aggregate())
    generator = doc["root"]["members"][3]["node"]["members"][0]["node"]
    assert generator["kind"] == "generator"
    assert generator["config"] == {"generator": "rlpf", "controls": {"cutoff": 900.0}}

    frozen = from_document(doc)
    rebuilt = frozen.handles[3].element.handles[0].element
    assert isinstance(rebuilt, Generator)
    assert rebuilt.wraps == "rlpf"


def test_a_resolver_supplies_what_the_document_only_names():
    doc = to_document(an_aggregate())
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
            "kind": "aggregate",
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
    aggregate = an_aggregate()
    first = to_document(aggregate)
    assert to_document(aggregate) == first

    # And an element added afterwards takes a fresh id rather than one already
    # in use.
    used = _ids(first["root"])
    aggregate.add(Clang(SeqEvent(midinote=72)), offset=12.0)
    second = to_document(aggregate)
    new = [i for i in _ids(second["root"]) if i not in used]
    assert len(new) == 1
    assert new[0] > max(used)


def test_the_temporal_metadata_crosses_both_ways():
    element = Clang(SeqEvent(midinote=60), onset=2.0, duration=0.5)
    element.resident = True
    doc = to_document(Aggregate(children=[element]))
    node = doc["root"]["members"][0]["node"]
    assert node["onset"] == 2.0 and node["duration"] == 0.5 and node["resident"] is True

    handle = from_document(doc).handles[0]
    back = handle.element
    assert back.onset == 2.0 and back.duration == 0.5 and back.resident is True
    # The id is the **placement's**: a clip is a window onto source, so the
    # handle is what carries the number an intent names.
    assert getattr(handle, ID_ATTR) == node["id"]


def test_a_sequence_of_elements_is_members_and_a_pattern_is_a_reference():
    of_elements = Sequence([Clang(SeqEvent(midinote=60)), Clang(SeqEvent(midinote=62))])
    body = to_document(Aggregate(children=[of_elements]))["root"]["members"][0]["node"]
    assert len(body["members"]) == 2
    assert "config" not in body

    of_a_pattern = Sequence("Pseq([1, 2, 3])")
    body = to_document(Aggregate(children=[of_a_pattern]))["root"]["members"][0]["node"]
    assert body["config"] == {"sequence": "Pseq([1, 2, 3])"}


def _ids(node) -> list:
    out = [node["id"]]
    for member in node.get("members", []):
        out.extend(_ids(member["node"]))
    return out


# ---- the session: the document plus where its source is ----

def test_a_session_carries_the_document_and_its_source_table():
    from clausters.form.document import SESSION_FORMAT, from_session, to_session

    session = to_session(
        an_aggregate(),
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
        an_aggregate(),
        sources={
            # The take the composition plays, and the working copy of it that an
            # unconfirmed edit is writing. Both are in the table because a
            # session whose table does not cover its own document is one that
            # reopens with source unresolved.
            7: {
                "location": {"at": "file", "path": "takes/vocal.wav"},
                "lifetime": "external",
                "generation": 0,
            },
            8: {
                "location": {"at": "file", "path": "scratch/edit.wav"},
                "lifetime": "session",
                "generation": 3,
                "editing": {"from": 7, "confirmed": False},
            },
        },
    )
    _, sources = from_session(json.loads(json.dumps(session)))
    assert sources[8]["editing"] == {"from": 7, "confirmed": False}
    assert sources[8]["lifetime"] == "session", "promoted by the save"


def test_a_newer_session_format_is_refused_rather_than_half_read():
    from clausters.form.document import SESSION_FORMAT, from_session, to_session

    session = to_session(an_aggregate(), sources={7: {"location": {"at": "file", "path": "t.wav"},
                                                 "lifetime": "session", "generation": 0}})
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
    aggregate = Aggregate()
    aggregate.add(generator, offset=0.0, dur=2.0)

    doc = to_document(aggregate)
    node = doc["root"]["members"][0]["node"]
    assert node["kind"] == "generator"
    assert node["rendered"]["kind"] == "aggregate"
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
    aggregate = Aggregate()
    aggregate.add(Generator("melody", rendered=rendered), offset=0.0, dur=2.0)

    first = to_document(aggregate)
    second = to_document(aggregate)
    assert first == second
    assert getattr(rendered, ID_ATTR) is not None


# ---- the binding: one implementation of what an edit means ----

def test_an_edit_applied_through_the_crate_is_the_crates_edit():
    """The binding's whole point: the client does not apply and then report, it
    hands the document and the intent across and takes back the new document
    plus what happened. Three clients cannot mean three different things by the
    same edit if only one of them implements it."""
    from clausters import _native

    aggregate = an_aggregate()
    doc = to_document(aggregate)
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

    doc = to_document(an_aggregate())
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

    doc = to_document(an_aggregate())
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
            "id": 1, "kind": "aggregate", "grouping": "concrete",
            "members": [{
                "offset": 2.0, "dur": 4.0,
                "node": {
                    "id": 2, "kind": "vector",
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
        to_document(an_aggregate()), {"start": 0.0, "len": 1.0},
        frames_per_beat=48000.0, in_beats=True,
    )
    assert spans == []


# ---- the log: undo through the crate, not through a history we keep ----

def test_a_run_of_gestures_inverts_back_to_where_it_started():
    """O11's acceptance. The log lives in the crate with its spill store; what
    crosses is the document and a handle. A history this client kept would see
    only the edits it made, which is what O5 exists to prevent."""
    from clausters._native import Document, Log

    start = to_document(an_aggregate())
    with Log() as log, Document(start) as doc:
        for node, offset in [(2, 1.0), (2, 5.0), (2, 2.5)]:
            log.apply(doc, {"intent": "place", "node": node, "offset": offset},
                      label="move")
        assert len(log) == 3
        assert log.undo_label == "move"
        assert doc.snapshot()["root"] != start["root"]

        while log.can_undo:
            log.undo(doc)
        assert doc.snapshot()["root"] == start["root"], "exactly, not approximately"
        assert log.can_redo


def test_a_redo_puts_back_exactly_what_the_undo_took():
    from clausters._native import Document, Log

    document = to_document(an_aggregate())
    with Log() as log, Document(document) as doc:
        log.apply(doc, {"intent": "place", "node": 2, "offset": 3.0}, label="move")
        edited = doc.snapshot()
        assert log.undo(doc) is not None
        assert doc.snapshot()["root"] == document["root"]
        redone = log.redo(doc)
        assert doc.snapshot()["root"] == edited["root"]
        assert redone["remaining"] == [], "nothing for the owner to re-run"


def test_what_the_grid_did_is_what_gets_replayed():
    """The forward half records the *effective* edit, so a redo does not snap a
    second time — harmless with a grid, wrong the moment a rule is not
    idempotent."""
    from clausters._native import Document, Log

    with Log() as log, Document(to_document(an_aggregate())) as doc:
        outcome = log.apply(
            doc, {"intent": "place", "node": 2, "offset": 4.3},
            quant=1.0, label="move",
        )
        assert outcome["effective"]["offset"] == 4.0
        log.undo(doc)
        log.redo(doc)
        assert doc.snapshot()["root"]["members"][0]["offset"] == 4.0


def test_a_refused_edit_leaves_nothing_to_undo():
    from clausters._native import Document, Log

    with Log() as log, Document(to_document(an_aggregate())) as doc:
        log.apply(doc, {"intent": "place", "node": 999, "offset": 1.0})
        assert len(log) == 0
        assert not log.can_undo
        assert log.undo(doc) is None, "and says so rather than failing"


def test_a_destructive_inverse_is_recorded_by_the_caller():
    """The one edit the document cannot supply the inverse for: its samples are
    not in the tree, so the caller reads the span it is about to overwrite."""
    from clausters._native import Document, Log

    document = {
        "version": 1,
        "root": {
            "id": 1, "kind": "vector",
            "source": {"source": 7, "lifetime": "temporary", "generation": 4},
        },
    }
    with Log() as log, Document(document) as doc:
        log.record(
            {"edit": {"intent": "writesamples", "node": 1, "start": 10,
                      "values": [0.5, 0.5]}},
            {"intent": "writesamples", "node": 1, "start": 10,
             "values": [0.125, 0.25]},
            label="draw",
        )
        assert len(log) == 1
        undone = log.undo(doc)
        assert undone["undone"][0]["values"] == [0.125, 0.25]


def test_a_deterministic_operation_comes_back_for_the_owner_to_re_run():
    """Going back is data; going forward may be a recipe. The crate holds no
    algorithms, so it hands the recipe out rather than replaying a span."""
    from clausters._native import Document, Log

    document = {
        "version": 1,
        "root": {
            "id": 1, "kind": "vector",
            "source": {"source": 7, "lifetime": "temporary", "generation": 4},
        },
    }
    with Log() as log, Document(document) as doc:
        log.record(
            {"recompute": {"op": "normalize", "peak": 1.0}},
            {"intent": "writesamples", "node": 1, "start": 0, "values": [0.25]},
            label="normalize",
        )
        assert log.undo(doc) is not None
        redone = log.redo(doc)
        assert len(redone["remaining"]) == 1
        assert redone["remaining"][0]["recompute"]["op"] == "normalize"


def test_a_continuing_run_of_adjustments_is_one_undo():
    """A hundred small moves of the same clip are one thing the person did. The
    caller decides where the hand stopped, because only the caller knows."""
    from clausters._native import Document, Log

    with Log() as log, Document(to_document(an_aggregate())) as doc:
        for i, offset in enumerate([1.0, 1.5, 2.0]):
            previous = 0.0 if i == 0 else offset - 0.5
            log.record(
                {"edit": {"intent": "place", "node": 2, "offset": offset}},
                {"intent": "place", "node": 2, "offset": previous},
                label="move", coalesce=i > 0,
            )
        assert len(log) == 1
        log.undo(doc)
        assert doc.snapshot()["root"]["members"][0]["offset"] == 0.0


def test_a_closed_log_is_closed_twice_without_complaint():
    from clausters._native import Log

    log = Log()
    log.close()
    log.close()


def test_the_document_handle_edits_in_place_and_hands_back_only_the_outcome():
    """The shape since the tree stopped crossing on every call: a handle holds
    the composition, an intent goes over, and what comes back is what happened.
    `snapshot` is how the JSON leaves — asked for rather than paid per edit."""
    from clausters import _native

    doc_json = to_document(an_aggregate())
    node = doc_json["root"]["members"][0]["node"]["id"]
    with _native.Document(doc_json) as doc:
        assert doc.version == doc_json["version"]
        outcome = doc.apply(
            {"intent": "place", "node": node, "offset": 4.3},
            against={"version": doc.version}, quant=1.0,
        )
        # The outcome alone -- no document in it.
        assert set(outcome) == {"effective", "applied", "reason", "stale"}
        assert outcome["applied"] is True
        assert outcome["effective"]["offset"] == 4.0, "the grid snapped it"
        assert doc.version == doc_json["version"] + 1
        assert doc.snapshot()["root"]["members"][0]["offset"] == 4.0


def test_the_handle_and_the_by_value_form_agree():
    """`document_apply` is a wrapper over the handle now, so the convenience a
    script keeps and the surface an editor uses cannot drift."""
    from clausters import _native

    doc_json = to_document(an_aggregate())
    node = doc_json["root"]["members"][0]["node"]["id"]
    intent = {"intent": "place", "node": node, "offset": 2.5}

    by_value = _native.document_apply(doc_json, intent, quant=0.0)
    with _native.Document(doc_json) as doc:
        outcome = doc.apply(intent, quant=0.0)
        assert outcome == by_value["outcome"]
        assert doc.snapshot() == by_value["document"]


def test_a_refused_edit_leaves_the_document_where_it_was():
    from clausters import _native

    doc_json = to_document(an_aggregate())
    with _native.Document(doc_json) as doc:
        outcome = doc.apply({"intent": "place", "node": 9999, "offset": 1.0})
        assert outcome["applied"] is False
        assert doc.version == doc_json["version"], "and the version did not move"


def test_opening_something_that_is_not_a_document_is_an_error_not_an_empty_one():
    """The distinction worth having: a handle that opened nothing would look
    exactly like a handle onto an empty composition."""
    from clausters import _native

    with pytest.raises(ValueError):
        _native.Document({"not": "a document"})
    with _native.Document() as empty:
        assert empty.version == FIRST_VERSION
        assert empty.snapshot()["root"]["kind"] == "aggregate"


def test_an_element_used_in_two_compositions_does_not_carry_a_number_the_second_one_holds():
    # Ids are stamped on the element object and numbering starts at 1 for every
    # root, so two arrangements built in one script both hold 1, 2, 3 -- and
    # source authored in one and used in the other arrived carrying a number a
    # different element here already had. An intent naming it then reached
    # whichever node the crate found first while the editor's index kept the
    # last: one gesture, two destinations.
    shared = Clang(SeqEvent(midinote=60))
    first = Aggregate([(0.0, shared), (1.0, Clang(SeqEvent(midinote=62)))])
    to_document(first)

    second = Aggregate([(0.0, Clang(SeqEvent(midinote=64))), (1.0, Clang(SeqEvent(midinote=67)))])
    to_document(second)
    second.add(shared, 2.0)

    ids = _ids(to_document(second)["root"])
    assert len(ids) == len(set(ids)), ids
    # And a second conversion of the same tree numbers it the same way, which is
    # what the whole history rests on.
    assert _ids(to_document(second)["root"]) == ids


def test_a_leaf_that_references_its_source_may_be_placed_twice():
    # O14: a clip is a window onto a source and the identity is the source, so
    # two placements are two nodes naming one source -- which is the multitrack's
    # own semantics and what the defect at the foot of the crate's plan was about.
    take = Vector(FakeBuffer(7))
    doc = to_document(Aggregate([(0.0, take), (4.0, take)]))
    windows = [m["node"] for m in doc["root"]["members"]]
    assert windows[0]["id"] != windows[1]["id"], "two windows, two names"
    assert windows[0]["source"] == windows[1]["source"], "one source behind them"


def test_an_element_whose_data_is_in_the_node_is_not_placed_twice():
    # The other half of the rule: a clang carries its source *inside* the
    # node, so two placements would be two copies that diverge on the first
    # edit -- which is the answer the decision rejected, so it is refused rather
    # than made silently.
    twice = Clang(SeqEvent(midinote=60))
    with pytest.raises(ValueError, match="placed more than once"):
        to_document(Aggregate([(0.0, twice), (4.0, twice)]))


def test_a_tree_converted_on_its_own_is_numbered_as_it_always_was():
    aggregate = Aggregate([(0.0, Clang(SeqEvent(midinote=60))),
                           (1.0, Clang(SeqEvent(midinote=62)))])
    assert _ids(to_document(aggregate)["root"]) == [1, 2, 3]


def test_the_crate_refuses_a_document_whose_id_names_two_different_nodes():
    # The other end of the same rule: what this client can no longer produce, a
    # file or another writer still can, and the crate is the door every writer
    # passes through. The C ABI has no channel for the crate's message, so the
    # client names the collision itself once the handle comes back null.
    pytest.importorskip("clausters._native")
    from clausters import _native

    try:
        _native.lib()
    except OSError as e:
        pytest.skip(f"clausters-ffi not built: {e}")

    document = to_document(an_aggregate())
    root = document["root"]
    root["members"][0]["node"]["id"] = root["members"][1]["node"]["id"]
    with pytest.raises(ValueError, match="names two different nodes"):
        _native.Document(document)


def test_an_aggregate_keeps_its_name_through_the_document():
    # The multitrack labels its lanes from the name, so a piece that loses it
    # reopens anonymous -- and the document had no field for one at all.
    song = Aggregate([(0.0, Clang(SeqEvent(midinote=60)))], name="melody")
    back = from_document(to_document(song))
    assert back.name == "melody"


def test_a_track_comes_back_a_track_and_not_an_aggregate_of_clangs():
    # It went out as an aggregate because there is one aggregate kind; what says
    # it was a track is the body's own config, which the document carries and
    # never reads. Rebuilding it as an aggregate is what grew a level of
    # nesting nobody wrote, and left the editor drawing clips where there had
    # been a roll.
    track = Track(Timeline([(0.0, SeqEvent(midinote=60)), (1.0, SeqEvent(midinote=64))]))
    back = from_document(to_document(Aggregate([(0.0, track)])))
    inner = back.members[0][2]
    assert isinstance(inner, Track)
    assert [beat for beat, _ in inner.wraps] == [0.0, 1.0]


def test_the_song_survives_a_session_round_trip_structurally_identical():
    # The acceptance, and the thing the example's "open it again" step was
    # really testing: the same composition by identity -- same kinds, same
    # nesting, same names -- compared tree against tree rather than by eye.
    song = an_aggregate()
    song.name = "song"
    song.add(Track(Timeline([(0.0, SeqEvent(midinote=67))])), offset=16.0)

    written = to_session(song, sources={7: {"location": {"at": "file", "path": "t.wav"},
                                            "lifetime": "session", "generation": 0}})
    back, sources = from_session(json.loads(json.dumps(written)))

    assert to_document(back) == written["document"]
    assert _kinds(back) == _kinds(song)
    assert sources[7]["location"]["path"] == "t.wav"


def _kinds(element) -> list:
    """The shape of a tree as class names, nesting included."""
    out = [type(element).__name__]
    for child in getattr(element, "members", []) or []:
        out.append(_kinds(child[2]))
    return out


def test_an_unnamed_leaf_is_written_with_no_reference_rather_than_an_address():
    # It used to write `repr(obj)` -- a memory address, which is unresolvable by
    # construction and different between two runs of the same script, so it broke
    # the one property O1's acceptance asked writing to keep.
    from clausters.seq import Pbind, Pseq

    lane = Sequence(Pbind(midinote=Pseq([48, 55], 2), dur=0.5))
    node = to_document(Aggregate([(0.0, lane)]))["root"]["members"][0]["node"]
    assert "0x" not in json.dumps(node), node
    assert not (node.get("config") or {}).get("sequence")


def test_a_named_pattern_lane_is_handed_back_and_plays_again():
    # The acceptance: what the file names, the script that still holds the
    # recipe supplies -- so a reopened piece sounds that lane instead of drawing
    # it frozen.
    from clausters.seq import Pbind, Pseq

    pattern = Pbind(midinote=Pseq([48, 55], 1), dur=0.5)
    song = Aggregate([(0.0, Sequence(pattern, name="bassline"))], name="bass")

    written = json.loads(json.dumps(to_session(song)))
    back, _ = from_session(
        written,
        resolve=lambda kind, config: (
            pattern if (config or {}).get("sequence") == "bassline" else None
        ),
    )

    from clausters.form.render import flatten

    assert flatten(back), "a resolved pattern lane emits its events again"
    # And with no resolver it is frozen rather than broken: drawn, placed, silent.
    frozen, _ = from_session(written)
    assert flatten(frozen) == []


def test_the_same_script_run_twice_writes_the_same_bytes():
    # Determinism is what the reference being an address destroyed, and it can
    # only be seen across processes -- inside one, an address is stable too.
    import subprocess
    import sys

    script = (
        "import json;"
        "from clausters.form import Aggregate, Sequence;"
        "from clausters.form.document import to_session;"
        "from clausters.seq import Pbind, Pseq;"
        "lane = Sequence(Pbind(midinote=Pseq([48, 55], 2), dur=0.5), name='bassline');"
        "print(json.dumps(to_session(Aggregate([(0.0, lane)], name='bass')), sort_keys=True))"
    )
    runs = {
        subprocess.run([sys.executable, "-c", script], capture_output=True,
                       text=True, check=True).stdout
        for _ in range(2)
    }
    assert len(runs) == 1, runs


def test_a_session_whose_table_does_not_cover_its_document_is_refused():
    # The failure it replaces is invisible until two saves later: reopening
    # resolves each take into a *new* buffer, so a table built once at startup
    # stops covering the composition it is saved with, and the reopened piece
    # draws nothing where the take was -- with nothing said anywhere.
    with pytest.raises(ValueError, match="does not cover this document"):
        to_session(an_aggregate(), sources={99: {"location": {"at": "file", "path": "x.wav"},
                                            "lifetime": "session", "generation": 0}})
    # A composition with no source needs no table at all.
    assert to_session(Aggregate([(0.0, Clang(SeqEvent(midinote=60)))]))["sources"] == {}
