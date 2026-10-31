"""The score editor's handler: a symbolic score in the GUI host's application,
through the handle the script already holds. Run against a real host with no
window on screen (``[gui] headless``); what a gesture does on the page is the
application's and tested in its crate (``crates/clausters-apps/src/score``).
"""

import pytest

from clausters.gui import notation
from clausters.gui.editing import ScoreEditor, edit
from conftest import opened


def _engraver() -> bool:
    try:
        notation._abi._engraver()
        return True
    except RuntimeError:
        return False


@pytest.fixture
def score(gui_host):
    if not _engraver():
        pytest.skip("no engraver: build libverovio with "
                    "third_party/build-verovio.sh and stage it with build_native.py")
    return notation.Score("@clef:G-2\n@timesig:4/4\n@data:4CDEF/ 4GABc'/")


def _items(score) -> list:
    return score.sheet()["staves"][0]["voices"][0]["items"]


def test_opening_writes_the_page_from_the_model(score, gui_host):
    # a document the engraver read carries the ids it minted; the editor
    # writes it through the model, so a press names an item at once
    editor = opened(gui_host, ScoreEditor(score))
    first = _items(score)[0]["id"]
    assert f'xml:id="n{first}"' in score.mei()
    editor.close()


def test_a_verb_edits_the_holders_score_and_walks_back(score, gui_host):
    editor = opened(gui_host, ScoreEditor(score))
    first = _items(score)[0]["id"]
    editor.select([f"n{first}"])
    assert editor.selected == [first]
    assert editor.articulation("stacc")
    assert _items(score)[0]["marks"]["articulations"] == ["stacc"]
    assert editor.undo()
    assert "marks" not in _items(score)[0]
    assert editor.redo()
    assert _items(score)[0]["marks"]["articulations"] == ["stacc"]


def test_a_spanner_runs_between_the_first_and_the_last_selected(score, gui_host):
    editor = opened(gui_host, ScoreEditor(score))
    ids = [item["id"] for item in _items(score)]
    editor.select([f"n{ids[3]}", f"n{ids[0]}"])
    assert editor.spanner("slur")
    (slur,) = score.sheet()["spanners"]
    assert (slur["kind"], slur["from"], slur["to"]) == ("slur", ids[0], ids[3])


def test_a_verb_with_nothing_selected_changes_nothing(score, gui_host):
    editor = opened(gui_host, ScoreEditor(score))
    before = score.mei()
    assert editor.delete() is False
    assert score.mei() == before


def test_the_value_in_hand_is_the_crates(score, gui_host):
    editor = opened(gui_host, ScoreEditor(score, value=(1, 8)))
    assert editor.value == (1, 8)
    editor.value = (1, 2)
    assert editor.value == (1, 2)


def test_edit_opens_a_score_in_the_score_editor(score, gui_host):
    editor = edit(score, host=gui_host)
    assert isinstance(editor, ScoreEditor)
    assert editor.score is score
    assert edit(score, host=gui_host) is editor, "the one open is handed back"
    editor.close()


def test_a_transformation_runs_over_the_measures_selected(score, gui_host):
    editor = opened(gui_host, ScoreEditor(score))
    ids = [item["id"] for item in _items(score)]
    before = [item["pitches"][0]["octave"] for item in _items(score)]
    editor.select([f"n{ids[5]}"])          # a note of the second bar
    assert editor.transform("transpose", semitones=12)
    after = [item["pitches"][0]["octave"] for item in _items(score)]
    assert after[:4] == before[:4], "the first bar was not selected"
    assert after[4:] == [octave + 1 for octave in before[4:]]
    assert editor.transform("fold") is False


def test_entry_is_the_crates_switch(score, gui_host):
    editor = opened(gui_host, ScoreEditor(score))
    assert editor.entry is False, "the window opens outside note entry"
    editor.entry = True
    assert editor.entry is True


def test_a_score_is_written_to_a_file_and_read_back_as_itself(score, gui_host, tmp_path):
    editor = opened(gui_host, ScoreEditor(score))
    assert editor.set_text("title", "A title")
    with pytest.raises(ValueError, match="no file"):
        score.write()
    path = score.write(tmp_path / "a.mei")
    assert score.path == path
    again = notation.Score.read(path)
    assert again.path == path
    assert again.sheet() == score.sheet(), "notes, marks and the page's text"


def _take():
    """A take played a little off the grid."""
    from clausters.seq import Event, EventSequence

    return EventSequence([
        (0.03, Event(midinote=60, dur=0.93, velocity=71)),
        (1.01, Event(midinote=62, dur=0.97, velocity=64)),
        (1.98, Event(midinote=64, dur=1.02, velocity=80)),
        (3.02, Event(midinote=65, dur=0.9, velocity=77)),
    ])


def test_a_sequence_is_edited_on_its_page_and_only_what_was_edited_moves(score, gui_host):
    take = _take()
    played = take.to_rows("at", "midinote", "velocity")
    editor = edit(take, view="score", host=gui_host, division=8)
    assert isinstance(editor, ScoreEditor)
    assert editor.structure is take and editor.sequence is take
    # the page is the take, read: four quarters -- and the take as it was
    assert [item["dur"] for item in _items(editor.score)] == [[1, 4]] * 4
    assert take.to_rows("at", "midinote", "velocity") == played

    third = _items(editor.score)[2]["id"]
    editor.select([f"n{third}"])
    assert editor.articulation("stacc")
    # the third note is marked in the sequence, played when it was
    assert take.to_rows("at", "articulations")[2] == (1.98, ["stacc"])
    # and the others are as they were played
    now = take.to_rows("at", "midinote", "velocity", "articulations")
    assert [row[:3] for row in now] == played
    assert [row[3] for i, row in enumerate(now) if i != 2] == [None] * 3

    # the entry is the sequence's: its history walks it back, and the page follows
    assert take.history.undo()
    assert take.to_rows("articulations")[2] == (None,)
    assert "marks" not in _items(editor.score)[2]


def test_a_roll_and_a_page_over_one_sequence_follow_each_other(score, gui_host):
    from clausters.gui.editing import NotesEditor

    take = _take()
    roll = edit(take, host=gui_host)
    page = edit(take, view="score", host=gui_host)
    assert isinstance(roll, NotesEditor) and isinstance(page, ScoreEditor)
    # a note moved through the sequence -- as a roll moves it -- is on the page
    first = take.events[0]
    first["midinote"] = 67
    page.adopt()
    assert _items(page.score)[0]["pitches"][0]["step"] == "g"
    with pytest.raises(ValueError, match="no view"):
        edit(take, view="tablature", open=False)


def test_the_window_holds_the_score_and_a_close_hands_it_back(score, gui_host):
    first = _items(score)[0]["id"]
    editor = opened(gui_host, ScoreEditor(score))
    editor.select([f"n{first}"])
    assert editor.articulation("stacc")
    # the score reads what the window holds, and a change through it is an
    # entry of the window's
    assert _items(score)[0]["marks"]["articulations"] == ["stacc"]
    assert score.apply({"op": "set_marks", "id": first, "marks": {}})
    assert "marks" not in _items(score)[0]
    assert editor.undo() and _items(score)[0]["marks"]["articulations"] == ["stacc"]
    editor.close()
    assert editor.closed
    # the handle reads its own again, as the window left it
    assert score.__dict__.get("_page") is None
    assert _items(score)[0]["marks"]["articulations"] == ["stacc"]
    assert 'artic="stacc"' in score.mei() or "stacc" in score.mei()


def test_a_saved_score_is_the_file_it_was_written_to(score, gui_host, tmp_path):
    editor = opened(gui_host, ScoreEditor(score))
    path = str(tmp_path / "saved.mei")
    assert editor.save(path) == path
    assert score.path == path
    with open(path, encoding="utf-8") as file:
        written = file.read()
    assert written.startswith("<?xml") or "<mei" in written
    assert not editor.unsaved
    # reading it back is the same score, and takes the history's place
    assert notation.Score.read(path).sheet()["staves"]
    editor.close()


def test_a_save_that_fails_leaves_the_score_and_its_file(score, gui_host, tmp_path):
    editor = opened(gui_host, ScoreEditor(score))
    path = str(tmp_path / "kept.mei")
    editor.save(path)
    with pytest.raises(RuntimeError):
        editor.save(str(tmp_path / "no" / "such" / "folder.mei"))
    assert score.path == path
    assert not editor.unsaved
    editor.close()


def test_a_document_that_does_not_read_leaves_the_window_as_it_was(score, gui_host, tmp_path):
    editor = opened(gui_host, ScoreEditor(score))
    path = str(tmp_path / "kept.mei")
    editor.save(path)
    before = score.mei()
    bad = tmp_path / "bad.mei"
    bad.write_text("this is no score")
    with pytest.raises(RuntimeError):
        editor.load(str(bad))
    assert score.mei() == before
    assert score.path == path
    assert not editor.unsaved
    editor.close()


def test_new_file_replaces_the_document_as_one_entry(score, gui_host, tmp_path):
    editor = opened(gui_host, ScoreEditor(score))
    path = str(tmp_path / "kept.mei")
    editor.save(path)
    before = score.mei()
    editor.new_file()
    assert score.mei() != before, "the same score holds it"
    assert editor.has_document
    assert editor.undo()
    assert score.mei() == before
    assert not editor.unsaved, "the file came back with the document"
    assert editor.save() == path
    editor.close()


def test_close_file_lets_the_score_go_and_the_window_stays(score, gui_host, tmp_path):
    editor = opened(gui_host, ScoreEditor(score))
    path = str(tmp_path / "kept.mei")
    editor.save(path)
    assert editor.set_text("title", "After the save")
    left = score.mei()
    window = int(editor.window)
    told = []
    editor.on_closed(lambda: told.append(True))
    editor.close_file()
    gui_host.pump(0.2)
    assert editor.closed and told == [True]
    assert score.path == path
    assert score.mei() == left, "the score holds what the window left it as"
    assert gui_host.call_app(window, {"query": "file"})["empty"] is True, "the window stays"
    gui_host.call_app(window, {"do": {"verb": "close"}})


def test_a_window_over_no_score_has_no_document_until_one_is_made(gui_host, tmp_path):
    if not _engraver():
        pytest.skip("no engraver")
    editor = opened(gui_host, ScoreEditor())
    assert editor.score is None
    assert not editor.has_document
    assert not editor.unsaved
    with pytest.raises(ValueError, match="no file is open"):
        editor.save(str(tmp_path / "nothing.mei"))
    assert not (tmp_path / "nothing.mei").exists()
    editor.new_file()
    assert editor.has_document
    path = str(tmp_path / "made.mei")
    assert editor.save(path) == path
    assert "<mei" in open(path, encoding="utf-8").read()
    assert editor.load(path) in (True, False)
    assert editor.has_document and not editor.unsaved
    editor.close()


def test_a_score_plays_in_the_application(score, gui_host):
    editor = opened(gui_host, ScoreEditor(score))
    assert editor.playing is False
    editor.play(1.0)
    assert editor.playing is True
    editor.stop()
    assert editor.playing is False
    assert editor.transport.playing is False
    editor.close()


def test_a_page_and_a_roll_over_one_sequence_are_one_order(gui_host):
    if not _engraver():
        pytest.skip("no engraver")
    from clausters.seq import EventSequence
    from clausters.seq.event import Event

    take = EventSequence([(0.0, Event(midinote=60, dur=1.0)),
                          (1.0, Event(midinote=62, dur=1.0)),
                          (2.0, Event(midinote=64, dur=1.0)),
                          (3.0, Event(midinote=65, dur=1.0))])
    roll = opened(gui_host, edit(take, host=gui_host, open=False))
    page = edit(take, host=gui_host, view="score", division=8)
    assert page.sequence is take and page.structure is take
    assert edit(take, host=gui_host, view="score") is page
    first = take.events[0]
    # a script's change reaches both, and either walks it back
    first["midinote"] = 67
    assert roll.undo_label == page.undo_label
    assert page.undo() and first["midinote"] == 60
    assert roll.redo() and first["midinote"] == 67
    # an edit on the page is an edit of the sequence
    items = page.score.sheet()["staves"][0]["voices"][0]["items"]
    page.select([f"n{items[1]['id']}"])
    assert page.move(1)
    assert take.events[1]["midinote"] != 62
    assert roll.undo() and take.events[1]["midinote"] == 62
    # a transport each
    roll.play(0.5)
    assert roll.playing and not page.playing
    roll.stop()
    page.close()
    assert take.events[0]["midinote"] == 67, "the roll still has it"
    roll.close()
    assert take.events[0]["midinote"] == 67, "the last window left it so"
    assert take._window() is None


def test_the_page_setup_is_the_scores_and_walks_back(score, gui_host):
    editor = opened(gui_host, ScoreEditor(score))
    assert editor.page["paper"] == "A4" and "Octavo" in editor.page["papers"]
    assert editor.set_page("Letter", landscape=True, staff=800)
    setup = editor.page
    assert (setup["paper"], setup["landscape"]) == ("Letter", True)
    assert setup["page"]["staff"] == 800
    assert score.sheet()["page"]["width"] == 2794
    assert 'page.width="279.4mm"' in score.mei(), "it travels in the document"
    assert editor.undo()
    assert "page" not in score.sheet()
    assert editor.set_page("foolscap") is False
    editor.close()


def test_a_text_of_the_page_is_written_and_placed(score, gui_host):
    editor = opened(gui_host, ScoreEditor(score))
    assert editor.set_text("title", "A title")
    assert editor.set_text("note", "* a footnote")
    assert editor.set_text("composer", "A. Composer", halign="left", pages="all")
    head = score.sheet()["header"]
    assert (head["title"], head["notes"]) == ("A title", ["* a footnote"])
    assert head["places"]["composer"]["halign"] == "left"
    assert editor.set_text("motto", "x") is False
    assert editor.undo() and editor.undo() and editor.undo()
    assert "header" not in score.sheet()
    editor.close()


def test_a_score_is_exported_as_the_sequence_it_renders(score, gui_host, tmp_path):
    from clausters.seq import EventSequence

    editor = opened(gui_host, ScoreEditor(score))
    notes = len(_items(score))
    # a MIDI file, by its extension: the notes, at the score's own tempo
    path = editor.export(tmp_path / "a.mid")
    read = EventSequence.from_smf(open(path, "rb").read())
    assert len(read) == notes
    assert read.tempo_map.secs_at(2.0) == pytest.approx(1.0)
    # a clip, named or by its extension
    clip = editor.export(tmp_path / "a.midi2")
    assert len(EventSequence.from_clip(open(clip, "rb").read())) == notes
    assert editor.export(tmp_path / "b.bin", "clip") == str(tmp_path / "b.bin")
    with pytest.raises(ValueError, match="smf"):
        editor.export(tmp_path / "c.mid", "wav")
    editor.close()


def test_the_marks_signs_and_staves_are_methods_too(score, gui_host):
    editor = opened(gui_host, ScoreEditor(score))
    first = _items(score)[0]["id"]
    editor.select([f"n{first}"])
    assert editor.mark("tremolo", 2)
    assert _items(score)[0]["marks"]["tremolo"] == 2
    assert editor.lyric("la")
    assert editor.control("tempo", "Lento", bpm=60)
    assert editor.set_key("D")
    assert editor.navigation("segno")
    assert editor.multirests()
    assert editor.set_staff(label="Flute")
    sheet = score.sheet()
    assert sheet["controls"][0]["bpm"] == 60
    assert sheet["key"] == "D"
    assert sheet["grid"]["marks"] == [[0, "segno"]]
    assert sheet["staves"][0]["label"] == "Flute"
    editor.close()


def test_the_windows_actions_are_read_from_the_application(score, gui_host):
    editor = opened(gui_host, ScoreEditor(score))
    actions = editor.actions
    assert actions["delete"]["enabled"] is False
    assert actions["delete"]["reason"] == "nothing is selected"
    assert actions["entry"]["checked"] is False
    editor.entry = True
    assert editor.actions["entry"]["checked"] is True
    editor.close()
