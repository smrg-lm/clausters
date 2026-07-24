"""The notation client: the engraved layers and the score widget's props.

`engrave` needs the optional ``verovio`` package, so those tests skip without
it; the `guidef.score` builder is pure and always runs.
"""

import json

import pytest

from clausters.gui import notation, score

# A two-bar phrase in Plaine & Easie, small enough to engrave in milliseconds.
PHRASE = "@clef:G-2\n@timesig:4/4\n@data:4CDEF/ 4GABc'/"


def test_score_carries_the_playhead_anchor_and_rate():
    node = score(11, display_list={"vb": [100, 50], "glyphs": {}, "prims": []},
                 playhead_at=48000.0, sample_rate=44100.0)
    assert node["playhead_at"] == 48000.0
    assert node["sample_rate"] == 44100.0
    # unset chrome is omitted rather than sent as a null
    assert "playhead" not in node
    assert "color" not in node


def test_score_carries_the_selected_element():
    node = score(11, display_list={"vb": [100, 50], "glyphs": {}, "prims": []},
                 selected="note-1")
    assert node["selected"] == "note-1"
    # the empty string is the clear, and must survive `_drop_none`
    assert score(11, selected="")["selected"] == ""
    assert "selected" not in score(11)


def test_score_sends_the_drawing_layers_but_not_the_notes():
    node = score(11, display_list={"vb": [100, 50], "glyphs": {}, "prims": [],
                                   "cursors": [{"t": 0, "x": 1, "y0": 0, "y1": 2}],
                                   "notes": [{"t": 0, "dur": 1, "pitch": 60, "id": "n"}]})
    assert node["cursors"][0]["x"] == 1
    assert "notes" not in node


def test_score_sends_the_pitch_quantum_the_page_was_engraved_with():
    pytest.importorskip("verovio")
    dl = notation.engrave(PHRASE)
    # half the staff-line spacing: a line-to-space move is one diatonic step
    assert dl["step"] == 90.0
    assert score(11, display_list=dl)["step"] == 90.0
    # it follows verovio's `unit`, not the staff scale — which is why the host
    # is told rather than left to assume it
    assert notation.engrave(PHRASE, scale=100)["step"] == 90.0
    assert notation.engrave(PHRASE, options={"unit": 6})["step"] == 60.0


def test_the_page_json_carries_the_drawing_layers_only():
    pytest.importorskip("verovio")
    dl = notation.engrave(PHRASE)
    page = json.loads(notation.page_json(dl))
    assert sorted(page) == ["cursors", "glyphs", "prims", "step", "vb"]
    # what the builder sends when it defines the widget, so a re-engraved page
    # replaces it exactly
    built = score(11, display_list=dl)
    assert all(built[k] == page[k] for k in page)


def test_score_view_pans_both_axes_only_when_it_can_zoom():
    page = {"vb": [1000, 500], "glyphs": {}, "prims": []}
    # zoomed in the page outgrows the view's width, so x has to pan too
    zoomable = notation.score_view(page, scroll_id=10, score_id=11)
    assert zoomable["axis"] == "both"
    assert zoomable["zoom"] == 1
    # without zoom the page always fits the width: a plain vertical scroll view
    fixed = notation.score_view(page, scroll_id=10, score_id=11, zoom=False)
    assert fixed["axis"] == "y"
    assert fixed["zoom"] == 0


def test_score_view_places_the_rate_on_the_inner_score():
    view = notation.score_view({"vb": [1000, 500], "glyphs": {}, "prims": []},
                               scroll_id=10, score_id=11, width=800.0,
                               sample_rate=48000.0)
    assert view["type"] == "scroll"
    inner = view["children"][0]
    assert inner["type"] == "score"
    assert inner["sample_rate"] == 48000.0


def _edited_score():
    """A live score to edit, and the id of its first (sounding) note."""
    pytest.importorskip("verovio")
    s = notation.Score(PHRASE)
    return s, s.display_list()["notes"][0]["id"]


def _pitches(score):
    return [n["pitch"] for n in score.display_list()["notes"]]


def test_transposing_moves_the_pitch_by_diatonic_steps():
    s, nid = _edited_score()
    assert _pitches(s)[0] == 60           # middle C
    assert s.transpose(nid, -2) is True
    assert _pitches(s)[0] == 57           # two steps down the staff: A3
    assert s.transpose(nid, 3) is True
    assert _pitches(s)[0] == 62           # three back up: D4
    assert s.transpose(nid, 0) is False   # nothing to do, and no undo step


def test_editing_keeps_the_ids_the_host_selects_by():
    s, nid = _edited_score()
    before = [n["id"] for n in s.display_list()["notes"]]
    s.transpose(nid, -1)
    dl = s.display_list()
    assert [n["id"] for n in dl["notes"]] == before
    # the edited note is still drawn, under the same id
    assert nid in {p.get("id") for p in dl["prims"]}


def test_undo_and_redo_walk_the_edits():
    s, nid = _edited_score()
    s.transpose(nid, -2)
    s.transpose(nid, -1)
    assert (s.can_undo, s.can_redo) == (True, False)
    assert s.undo() is True
    assert _pitches(s)[0] == 57
    assert s.undo() is True
    assert _pitches(s)[0] == 60
    assert s.can_undo is False
    assert s.redo() is True
    assert _pitches(s)[0] == 57
    assert s.can_redo is True


def test_undo_on_a_fresh_score_answers_instead_of_crashing():
    # verovio's own undo dereferences an empty stack and takes the process
    # down, so the stack is ours -- this test is the guard for that.
    s, _ = _edited_score()
    assert s.can_undo is False
    assert s.undo() is False
    assert s.redo() is False
    assert _pitches(s)[0] == 60


def test_a_rejected_edit_leaves_the_score_untouched():
    s, nid = _edited_score()
    before = _pitches(s)
    assert s.edit("nonsense", elementId=nid) is False
    assert _pitches(s) == before
    assert s.can_undo is False


def test_the_score_round_trips_through_mei():
    s, nid = _edited_score()
    s.transpose(nid, -2)
    mei = s.mei()
    assert "<note" in mei and nid in mei
    # re-engraving the exported MEI reproduces the edited score, ids included
    assert [n["pitch"] for n in notation.engrave(mei)["notes"]] == _pitches(s)


def test_engraving_shares_ids_and_time_between_cursors_and_notes():
    pytest.importorskip("verovio")
    dl = notation.engrave(PHRASE)
    assert dl["prims"] and dl["cursors"] and dl["notes"]
    # both layers come out of one engraving, so their onsets line up
    assert dl["notes"][0]["t"] == dl["cursors"][0]["t"] == 0.0
    assert dl["notes"][0]["pitch"] == 60  # middle C, the phrase's first note
    # the cursor sweeps left to right and the notes run forward in time
    assert [c["x"] for c in dl["cursors"]] == sorted(c["x"] for c in dl["cursors"])
    assert [n["t"] for n in dl["notes"]] == sorted(n["t"] for n in dl["notes"])
    # every note is placed on the page it was engraved from
    placed = {p.get("id") for p in dl["prims"]}
    assert all(n["id"] in placed for n in dl["notes"])
