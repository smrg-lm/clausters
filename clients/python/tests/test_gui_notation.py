"""The notation client: the engraved layers and the score widget's props.

`engrave` needs the optional ``verovio`` package, so those tests skip without
it; the `guidef.score` builder is pure and always runs.
"""

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


def test_score_view_places_the_rate_on_the_inner_score():
    view = notation.score_view({"vb": [1000, 500], "glyphs": {}, "prims": []},
                               scroll_id=10, score_id=11, width=800.0,
                               sample_rate=48000.0)
    assert view["type"] == "scroll"
    inner = view["children"][0]
    assert inner["type"] == "score"
    assert inner["sample_rate"] == 48000.0


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
