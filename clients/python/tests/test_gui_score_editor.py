"""The score editor: a symbolic score edited in place by the crate's
application, through the handle the script already holds."""

import pytest

from clausters.gui import notation
from clausters.gui.editing import ScoreEditor, edit


def _engraver() -> bool:
    try:
        notation._abi._engraver()
        return True
    except RuntimeError:
        return False


@pytest.fixture
def score():
    if not _engraver():
        pytest.skip("no engraver: build libverovio with "
                    "third_party/build-verovio.sh and stage it with build_native.py")
    return notation.Score("@clef:G-2\n@timesig:4/4\n@data:4CDEF/ 4GABc'/")


def _items(score) -> list:
    return score.sheet()["staves"][0]["voices"][0]["items"]


def test_the_window_is_the_page_in_a_scroll_over_a_status_line(score):
    editor = ScoreEditor(score)
    tree = editor.draw()
    scroll, status = tree["children"][:2]
    assert scroll["type"] == "scroll"
    page = scroll["children"][0]
    assert page["type"] == "score"
    assert page["editable"] is True and page["entry"] is True
    assert "kinds" in page and "notes" not in page
    assert status["type"] == "label"


def test_opening_writes_the_page_from_the_model(score):
    # a document the engraver read carries the ids it minted; the editor
    # writes it through the model, so a press names an item at once
    ScoreEditor(score)
    first = _items(score)[0]["id"]
    assert f'xml:id="n{first}"' in score.mei()


def test_a_verb_edits_the_holders_score_and_walks_back(score):
    editor = ScoreEditor(score)
    first = _items(score)[0]["id"]
    editor.select([f"n{first}"])
    assert editor.selected == [first]
    assert editor.articulation("stacc")
    assert _items(score)[0]["marks"]["articulations"] == ["stacc"]
    assert editor.undo()
    assert "marks" not in _items(score)[0]
    assert editor.redo()
    assert _items(score)[0]["marks"]["articulations"] == ["stacc"]


def test_a_spanner_runs_between_the_first_and_the_last_selected(score):
    editor = ScoreEditor(score)
    ids = [item["id"] for item in _items(score)]
    editor.select([f"n{ids[3]}", f"n{ids[0]}"])
    assert editor.spanner("slur")
    (slur,) = score.sheet()["spanners"]
    assert (slur["kind"], slur["from"], slur["to"]) == ("slur", ids[0], ids[3])


def test_a_verb_with_nothing_selected_changes_nothing(score):
    editor = ScoreEditor(score)
    before = score.mei()
    assert editor.delete() is False
    assert score.mei() == before


def test_the_value_in_hand_is_the_crates(score):
    editor = ScoreEditor(score, value=(1, 8))
    assert editor.value == (1, 8)
    editor.value = (1, 2)
    assert editor.value == (1, 2)


def test_edit_opens_a_score_in_the_score_editor(score):
    editor = edit(score, open=False)
    assert isinstance(editor, ScoreEditor)
    assert editor.score is score
