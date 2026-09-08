"""The crate's client surface, reached the way a reader would reach it
(`clausters.document`).

The surface itself is the crate's and is tested where the edits are. What this
checks is the **door**: that the public module answers, that it carries what the
web client's `document.ts` carries, and that the two spellings of the same
question -- the free function and the method -- give the same answer.

It exists because the door did not. The surface lived in `clausters._native`,
whose leading underscore says not to import it, so the only public way to a
document was through `clausters.form`'s converter -- a frozen module the
arrangement no longer goes through.
"""

import clausters
from clausters.multitrack import Multitrack, Content, Lane, Region, Track
from clausters.document import (MULTITRACK, EVENTS, POINTS, SAMPLES, TREE,
                                Document, History, Log, apply_intent,
                                domain_coalesce_key, domain_edit,
                                resolve_selection)


def test_the_module_is_reachable_as_a_public_name():
    assert clausters.document.Document is Document


def test_the_door_carries_what_the_web_client_carries():
    # `clients/web/src/index.ts` exports History, Log, applyIntent,
    # domainCoalesceKey, domainEdit and resolveSelection from document.ts, plus
    # the module itself. One client with a public door and one without is the
    # asymmetry this module closed; a name added on one side and not the other
    # is how it would open again.
    assert {name for name in dir(clausters.document) if not name.startswith("_")} >= {
        "Document", "History", "Log", "apply_intent", "domain_coalesce_key",
        "domain_edit", "resolve_selection", "TREE", "MULTITRACK", "POINTS",
        "SAMPLES", "EVENTS",
    }
    assert (TREE, MULTITRACK, POINTS, SAMPLES, EVENTS) == (
        "tree", "multitrack", "points", "samples", "events")


def test_an_empty_document_opens_and_says_what_it_holds():
    with Document() as doc:
        assert doc.version == 1
        assert doc.snapshot()["version"] == 1


def test_the_method_and_the_free_function_answer_the_same_thing():
    # `Document.coalesceKey` and `doc.inverse` are the web client's shapes; the
    # free functions are this one's. Two spellings, one answer -- which is what
    # `idiom` means and why it is not a divergence.
    from clausters._native import document_coalesce_key, document_inverse

    intent = {"intent": "place", "node": 1, "offset": 0.0}
    assert Document.coalesce_key(intent) == document_coalesce_key(intent) == "place:1"
    with Document() as doc:
        assert doc.inverse(intent) == document_inverse(doc, intent)


def test_the_one_shot_forms_need_no_document_object():
    # What a caller that holds JSON and nothing else uses.
    written = {"version": 1, "root": {"id": 1, "kind": "clang"}}
    outcome = apply_intent(written, {"intent": "place", "node": 1, "offset": 2.0})
    assert isinstance(outcome, dict)
    resolved = resolve_selection(
        written, {"start": 0.0, "len": 1.0, "nodes": [1]},
        frames_per_beat=48_000.0, frames_per_second=48_000.0
    )
    assert isinstance(resolved, list)


def test_a_domain_that_is_not_a_document_answers_here_too():
    # A curve is not a document and has no node; the same two questions are
    # asked of it, which is what keeps a second spelling of the rule out of
    # every client.
    state = [{"at": 0.0, "value": 0.0, "data": {}}]
    payload = {"intent": "setpoints",
               "points": [{"at": 0.0, "value": 1.0, "data": {}}]}
    edited = domain_edit(POINTS, state, payload)
    assert edited is not None and edited["applied"]
    assert isinstance(domain_coalesce_key(POINTS, payload), str)


# ---- the piece: a whole multitrack state across the seam ----

def a_piece() -> dict:
    """Two tracks, two lanes on the first, a region on each — the smallest
    piece a move between tracks has somewhere to move to."""
    vocals = Track(id=10, name="vocals", lanes=[Lane(id=11), Lane(id=12)])
    vocals.lanes[0].place(Region(id=100, position=0.0, length=4.0,
                                 content=Content.composite(
                                     {"id": 1, "kind": "aggregate",
                                      "grouping": "concrete", "members": []})))
    guitar = Track(id=20, name="guitar", lanes=[Lane(id=21)])
    return Multitrack(tracks=[vocals, guitar]).write()


def test_a_region_moves_between_tracks_in_one_edit_and_comes_back_in_one():
    # The piece is a vocabulary of its own, reached through the door every
    # other domain is reached through -- no new binding, in either language.
    # One intent moves the region, because where a region is means track, lane
    # and beat and an absolute edit states all three; the inverse the crate
    # hands back with it is what puts it on the lane it came from.
    move = {"intent": "placeregion", "region": 100, "track": 20, "lane": 21,
            "position": 16.0, "layer": 1}
    edited = domain_edit(MULTITRACK, a_piece(), move)
    assert edited is not None and edited["applied"]

    moved = Multitrack.read(edited["state"])
    assert moved.track(20).lanes[0].regions[0].id == 100
    assert moved.track(10).lanes[0].regions == []
    assert moved.version == 2, "and the piece carries its own counter"

    back = domain_edit(MULTITRACK, edited["state"], edited["current"])
    assert back is not None
    restored = Multitrack.read(back["state"])
    assert restored.track(10).lanes[0].regions[0].position == 0.0
    assert restored.track(20).lanes[0].regions == []


def test_a_refusal_says_why_rather_than_failing():
    edited = domain_edit(MULTITRACK, a_piece(),
                         {"intent": "placeregion", "region": 999, "track": 20,
                          "lane": 21, "position": 0.0})
    assert edited is not None
    assert not edited["applied"]
    assert edited["reason"] == "no such region"


def test_the_pieces_coalesce_key_is_asked_here_and_not_spelled_again():
    assert domain_coalesce_key(
        MULTITRACK, {"intent": "trimregion", "region": 100, "position": 0.0,
                      "length": 2.0}) == "trimregion:100"
    assert domain_coalesce_key(MULTITRACK, {"intent": "setpoints",
                                             "points": []}) == ""


def test_an_unedited_piece_writes_nothing_and_reads_back_at_the_first_version():
    # The counter stays out of the file while it is the first version, so a
    # piece that says nothing still writes an empty object.
    assert Multitrack().write() == {}
    assert Multitrack.read({}).version == 1
