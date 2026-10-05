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


def _page(tree: dict) -> dict:
    """The `score` widget of an editor's window: the scroll's one child."""
    scroll = next(child for child in tree["children"] if child.get("type") == "scroll")
    return scroll["children"][0]


def _items(score) -> list:
    return score.sheet()["staves"][0]["voices"][0]["items"]


def test_the_window_is_the_page_in_a_scroll_over_a_status_line(score):
    editor = ScoreEditor(score)
    tree = editor.draw()
    toolbar, scroll, status = tree["children"][:3]
    # the toolbar is a row of the crate's tools, each under an id of its own
    tools = [tool for tool in toolbar["children"] if "id" in tool]
    assert toolbar["flow"] == "row" and len({tool["id"] for tool in tools}) == 12
    # a tool is drawn with the engraver's own symbol: its label is the SMuFL
    # character, and the window carries the outline the host draws it with
    values = next(tool for tool in tools if tool.get("type") == "choice")["options"]
    assert values[2] == "\ue1d5"
    assert tree["glyphs"]["E1D5"].startswith("M")
    assert len(tree["glyphs"]) == 19
    assert scroll["type"] == "scroll"
    page = scroll["children"][0]
    assert page["type"] == "score"
    assert page["editable"] is True and page["entry"] is True
    assert "kinds" in page and "notes" not in page
    assert status["type"] == "label"
    # the window carries the menu bar, which holds every action
    assert [title["label"] for title in tree["menu"]] == [
        "File", "Edit", "View", "Notes", "Notation", "Measures", "Transform"]


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


def test_a_transformation_runs_over_the_measures_selected(score):
    editor = ScoreEditor(score)
    ids = [item["id"] for item in _items(score)]
    before = [item["pitches"][0]["octave"] for item in _items(score)]
    editor.select([f"n{ids[5]}"])          # a note of the second bar
    assert editor.transform("transpose", semitones=12)
    after = [item["pitches"][0]["octave"] for item in _items(score)]
    assert after[:4] == before[:4], "the first bar was not selected"
    assert after[4:] == [octave + 1 for octave in before[4:]]
    assert editor.transform("fold") is False


def test_entry_is_the_crates_switch(score):
    editor = ScoreEditor(score)
    assert editor.entry is True
    editor.entry = False
    assert editor.entry is False


def test_the_layout_is_the_windows_and_the_paper_is_fixed(score):
    editor = ScoreEditor(score)
    assert editor.layout == "page"
    tree = editor.draw()
    page = _page(tree)
    # the drawing is the paper nobody chose, A4, whatever the music needs
    assert page["vb"] == [21000.0, 29700.0]
    editor.layout = "continuous"
    assert editor.layout == "continuous"
    line = _page(editor.draw())
    assert len(line["systems"]) == 1, "one system, as long as the music"
    assert score.mei() and "page.width" not in score.mei(), "a layout writes nothing"


def test_the_page_setup_is_the_scores_and_walks_back(score):
    editor = ScoreEditor(score)
    assert editor.page["paper"] == "A4" and "Octavo" in editor.page["papers"]
    assert editor.set_page("Letter", landscape=True, staff=800)
    setup = editor.page
    assert (setup["paper"], setup["landscape"]) == ("Letter", True)
    assert setup["page"]["staff"] == 800
    assert score.sheet()["page"]["width"] == 2794
    assert 'page.width="279.4mm"' in score.mei(), "it travels in the document"
    assert _page(editor.draw())["vb"][0] == 27940.0
    assert editor.undo()
    assert "page" not in score.sheet()
    assert editor.set_page("foolscap") is False


def test_a_text_of_the_page_is_written_and_placed(score):
    editor = ScoreEditor(score)
    assert editor.set_text("title", "A title")
    assert editor.set_text("note", "* a footnote")
    assert editor.set_text("composer", "A. Composer", halign="left", pages="all")
    head = score.sheet()["header"]
    assert (head["title"], head["notes"]) == ("A title", ["* a footnote"])
    assert head["places"]["composer"]["halign"] == "left"
    # each is drawn under its own id, which is what a press names
    page = _page(editor.draw())
    drawn = {p["id"]: p["s"] for p in page["prims"] if p["k"] == "text" and p.get("id")}
    assert drawn["t-title"] == "A title" and drawn["t-note-1"] == "* a footnote"
    assert page["kinds"]["t-title"] == "rend"
    assert editor.set_text("motto", "x") is False
    assert editor.undo() and editor.undo() and editor.undo()
    assert "header" not in score.sheet()


def test_the_input_state_is_the_handles_and_a_press_writes_it(score):
    editor = ScoreEditor(score, value=(1, 8))
    editor.draw()
    assert (editor.dotted, editor.rest, editor.next_accidental) == (False, False, None)
    editor.dotted = True
    editor.next_accidental = 1
    assert (editor.dotted, editor.next_accidental) == (True, 1)
    last = _items(score)[-1]["id"]
    page = editor.view.widget(editor, "page", editor.structure)
    assert editor.apply("/gui_event", [page, 1, editor._version, "insert", f"n{last}", -3, 0])
    written = _items(score)[-1]
    assert written["dur"] == [3, 16]
    assert written["pitches"][0]["alter"] == 1
    assert editor.next_accidental is None, "it was for that note"


def test_a_tool_and_a_menu_pick_are_the_editors_verbs(score):
    editor = ScoreEditor(score)
    editor.draw()
    first = _items(score)[0]["id"]
    editor.select([f"n{first}"])
    # a tool that acts reports a click
    staccato = editor.view.widget(editor, "tool", editor.structure, "stacc")
    assert editor.apply("/gui_event", [staccato, 1, editor._version, "click"])
    assert _items(score)[0]["marks"]["articulations"] == ["stacc"]
    # the verbs the menu and the tools use are methods too
    assert editor.accidental(-1)
    assert _items(score)[0]["pitches"][0]["alter"] == -1
    assert editor.voice(1)
    assert len(score.sheet()["staves"][0]["voices"]) == 2
    assert not editor.voice(1), "it is there already"
    # a measure verb acts on the measures the selection covers
    editor.select([f"n{first}"])
    assert editor.set_barline("dbl")
    assert editor.insert_measures(2)
    assert editor.undo() and editor.undo()
    # and a tool that holds state reports its value: an eighth
    value = editor.view.widget(editor, "tool", editor.structure, "value")
    editor.apply("/gui_event", [value, 2, editor._version, 3])
    assert editor.value == (1, 8)
