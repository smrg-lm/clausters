"""The crate's binding, from the Python client (`clausters.document`).

One implementation of what an edit means, reached over the C ABI: the client
hands over a document and an intent and takes back the new document plus what
happened -- it never applies and then reports, which is what would let three
clients mean three different things by one gesture. The log is the same
discipline for undo: the pile lives in the crate with its spill store, and what
crosses is the document and a handle.

These used to live in `test_form_document.py` and build their documents through
`clausters.form`'s converter. The converter is gone; the documents are written
out here, which is the honest shape anyway -- what is under test is the edit,
so what it starts from should be a fixed piece of JSON rather than the output of
something else that could itself drift.
"""

import pytest

from clausters.document import FIRST_VERSION


def a_document() -> dict:
    """A composition with the three shapes the cases name: an event at node 2,
    a take at node 3, and a nested set at node 4."""
    return {
        "version": FIRST_VERSION,
        "root": {
            "id": 1,
            "kind": "aggregate",
            "grouping": "concrete",
            "members": [
                {"offset": 0.0, "dur": 1.0,
                 "node": {"id": 2, "kind": "clang",
                          "config": {"midinote": 60, "dur": 1.0}}},
                {"offset": 4.0, "dur": 2.0,
                 "node": {"id": 3, "kind": "vector", "duration": 2.0,
                          "source": {"source": 7, "lifetime": "session",
                                     "generation": 0},
                          "config": {"instrument": "take"}}},
                {"offset": 8.0,
                 "node": {"id": 4, "kind": "aggregate", "grouping": "logical",
                          "members": [
                              {"offset": 0.0,
                               "node": {"id": 5, "kind": "generator",
                                        "config": {"name": "rlpf"}}}]}},
            ],
        },
    }


# ---- the binding: one implementation of what an edit means ----

def test_an_edit_against_a_superseded_version_comes_back_stale():
    from clausters import _native

    doc = a_document()
    node = doc["root"]["members"][0]["node"]["id"]
    result = _native.document_apply(
        doc, {"intent": "place", "node": node, "offset": 4.0},
        against={"version": doc["version"] + 5},
    )
    assert result["outcome"]["stale"] is True
    assert result["outcome"]["applied"] is False
    assert result["document"]["version"] == doc["version"], "and it did not move"


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
        doc, {"start": 3.0 * 48000, "len": 48000}, frames_per_beat=48000.0,
        frames_per_second=48000.0,
    )
    assert len(spans) == 1
    assert spans[0]["source"] == 100
    assert spans[0]["generation"] == 2
    assert spans[0]["range"] == {"start": 528000, "end": 576000}


def test_nothing_underneath_is_an_empty_list_and_not_a_failure():
    from clausters import _native

    spans = _native.document_resolve(
        a_document(), {"start": 0.0, "len": 1.0},
        frames_per_beat=48000.0, frames_per_second=48000.0, in_beats=True,
    )
    assert spans == []


# ---- the log: undo through the crate, not through a history we keep ----

def test_a_run_of_gestures_inverts_back_to_where_it_started():
    """O11's acceptance. The log lives in the crate with its spill store; what
    crosses is the document and a handle. A history this client kept would see
    only the edits it made, which is what O5 exists to prevent."""
    from clausters._native import Document, Log

    start = a_document()
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

    document = a_document()
    with Log() as log, Document(document) as doc:
        log.apply(doc, {"intent": "place", "node": 2, "offset": 3.0}, label="move")
        edited = doc.snapshot()
        assert log.undo(doc) is not None
        assert doc.snapshot()["root"] == document["root"]
        redone = log.redo(doc)
        assert doc.snapshot()["root"] == edited["root"]
        assert redone["remaining"] == [], "nothing for the owner to re-run"


def test_a_walk_is_one_door_and_comes_back_routed_per_structure():
    """Which side of an entry a direction reads, and which legs a structure
    owns, were written once per client and once per document log. Now the walk
    answers both."""
    from clausters._native import History

    with History() as history:
        curve = history.register("points")
        roll = history.register("events")
        history.record(
            [{"structure": curve, "forward": {"edit": {"points": [1]}},
              "backward": {"points": [0]}},
             {"structure": roll, "forward": {"edit": {"notes": [1]}},
              "backward": {"notes": [0]}},
             {"structure": curve, "forward": {"edit": {"points": [2]}},
              "backward": {"points": [1]}}],
            label="a gesture over both")

        walked = history.walk("undo")
        assert walked["label"] == "a gesture over both"
        assert [leg["structure"] for leg in walked["legs"]] == [curve, roll], \
            "one entry per structure, not one per leg"
        assert walked["legs"][0]["payloads"] == [{"points": [1]}, {"points": [0]}], \
            "a transaction unwinds the way it was laid down"
        assert walked["remaining"] == [], "an inverse is always an edit"
        assert history.walk("undo") is None

        back = history.walk("redo")
        assert back["legs"][0]["payloads"] == [{"points": [1]}, {"points": [2]}]

    with History() as history:
        with pytest.raises(ValueError):
            history.walk("backwards")


def test_what_the_grid_did_is_what_gets_replayed():
    """The forward half records the *effective* edit, so a redo does not snap a
    second time — harmless with a grid, wrong the moment a rule is not
    idempotent."""
    from clausters._native import Document, Log

    with Log() as log, Document(a_document()) as doc:
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

    with Log() as log, Document(a_document()) as doc:
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

    with Log() as log, Document(a_document()) as doc:
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

    doc_json = a_document()
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

    doc_json = a_document()
    node = doc_json["root"]["members"][0]["node"]["id"]
    intent = {"intent": "place", "node": node, "offset": 2.5}

    by_value = _native.document_apply(doc_json, intent, quant=0.0)
    with _native.Document(doc_json) as doc:
        outcome = doc.apply(intent, quant=0.0)
        assert outcome == by_value["outcome"]
        assert doc.snapshot() == by_value["document"]


def test_a_refused_edit_leaves_the_document_where_it_was():
    from clausters import _native

    doc_json = a_document()
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

    document = a_document()
    root = document["root"]
    root["members"][0]["node"]["id"] = root["members"][1]["node"]["id"]
    with pytest.raises(ValueError, match="names two different nodes"):
        _native.Document(document)


# ---- the source table, and reopening into a running system ----

