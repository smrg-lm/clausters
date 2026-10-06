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


def _page(tree: dict) -> "dict | None":
    """The `score` widget of an editor's window, wherever the chrome put it."""
    if tree.get("type") == "score":
        return tree
    for child in tree.get("children", ()):
        page = _page(child)
        if page is not None:
            return page
    return None


def _items(score) -> list:
    return score.sheet()["staves"][0]["voices"][0]["items"]


def test_the_window_is_the_page_in_a_scroll_over_a_status_line(score):
    editor = ScoreEditor(score)
    tree = editor.draw()
    toolbar, work, status = tree["children"][:3]
    # the palettes stand beside the page, a divider between them
    palettes, scroll = work["children"]
    assert work["split"] is True
    assert [group["title"] for group in palettes["children"]] == [
        "Notes", "Accidentals", "Articulations", "Ornaments", "Lines", "Text", "Dynamics",
        "Measures", "Repeats and jumps", "Keys and clefs", "Staves"]
    # the toolbar is a row of the crate's tools, each under an id of its own
    tools = [tool for tool in toolbar["children"] if "id" in tool]
    assert toolbar["flow"] == "row" and len({tool["id"] for tool in tools}) == 16
    # a tool is drawn with the engraver's own symbol: its label is the SMuFL
    # character, and the window carries the outline the host draws it with
    values = next(tool for tool in tools if tool.get("type") == "choice")["options"]
    assert values[2] == "\ue1d5"
    assert tree["glyphs"]["E1D5"].startswith("M")
    assert len(tree["glyphs"]) > 19, "the tools' symbols, and the palettes'"
    assert scroll["type"] == "plane"
    page = scroll["children"][0]
    assert page["type"] == "score"
    assert page["editable"] is True and page["entry"] is False
    assert "kinds" in page and "notes" not in page
    assert status["type"] == "label"
    # the window carries the menu bar, which holds every action
    assert [title["label"] for title in tree["menu"]] == [
        "File", "Edit", "View", "Play", "Notes", "Notation", "Measures", "Transform"]
    # ...and its close mark asks first, as the File menu's Close does
    assert tree["ask_close"] is True


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
    assert editor.entry is False, "the window opens outside note entry"
    editor.entry = True
    assert editor.entry is True


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


def test_without_chrome_the_window_is_the_page_and_the_handle_edits(score):
    editor = edit(score, open=False, chrome=False)
    assert isinstance(editor, ScoreEditor) and editor.chrome is False
    tree = editor.draw()
    # the page in its scroll over the status line, and nothing else: no
    # toolbar, no palettes, no dialogs, no menu bar, no symbol to label a tool
    # with -- and with no form to ask in, its close mark closes at once
    scroll, status = tree["children"]
    assert scroll["type"] == "plane" and scroll["children"][0]["type"] == "score"
    assert status["type"] == "label"
    assert "menu" not in tree and "glyphs" not in tree
    assert tree["ask_close"] is False
    # the keys are the window's, and stay
    assert tree["keys"] == ["score"] and tree["plays"] is True
    # the handle edits, and is told of an edit a hand made on the page
    told = []
    editor.on_change = lambda: told.append(_items(score)[0]["pitches"][0]["step"])
    first = _items(score)[0]["id"]
    editor.select([f"n{first}"])
    assert editor.move(1) and told == ["d"]
    page = editor.view.widget(editor, "page", editor.structure)
    # a drag names the staff position the note was let go on, here the G
    # over the staff (a script's edit raised the floor, so the drag states no
    # version)
    assert editor.apply("/gui_event", [page, 1, 0, "transpose", f"n{first}", 1])
    assert told == ["d", "g"]
    # what was edited is read on the score: its model, and the sequence it
    # renders into, for the code to go on from
    played = [event["midinote"] for _, event in score.render_events()]
    assert len(played) == len(_items(score)) and played[0] == 79
    # a verb refused answers so, and the status line says why
    editor.select([])
    assert not editor.tie()
    # and the whole window is what opens when nothing asks otherwise
    assert ScoreEditor(score).chrome is True


def test_the_input_state_is_the_handles_and_a_press_writes_it(score):
    editor = ScoreEditor(score, value=(1, 8))
    editor.draw()
    assert (editor.dotted, editor.rest, editor.next_accidental) == (False, False, None)
    editor.dotted = True
    editor.next_accidental = 1
    assert (editor.dotted, editor.next_accidental) == (True, 1)
    # in note entry, a press on a rest's column writes over it
    last = len(_items(score)) - 1
    editor.select([f"n{_items(score)[last]['id']}"])
    assert editor.silence()
    editor.entry = True
    page = editor.view.widget(editor, "page", editor.structure)
    rest = _items(score)[last]["id"]
    # (a script's edit raised the floor, so the press states no version)
    assert editor.apply("/gui_event", [page, 1, 0, "enter", f"n{rest}", -3, 0])
    written = _items(score)[last]
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


def test_a_menu_entry_opens_a_form_and_ok_writes_one_entry(score):
    editor = ScoreEditor(score)
    tree = editor.draw()
    stack = tree["children"][3]
    assert (stack["flow"], stack["index"]) == ("stack", 0)
    widget = lambda name: editor.view.widget(editor, "dialog", editor.structure, name)
    window = editor._window if editor._window is not None else 0
    editor._window = window
    # the bar's entry opens the form; its fields are typed; OK writes them
    editor.apply("/gui_event", [window, 1, editor._version, "menu", "dialog:text"])
    editor.apply("/gui_event", [widget("text:title"), 2, editor._version, "A title"])
    editor.apply("/gui_event", [widget("text:notes"), 3, editor._version, "* one\n* two"])
    assert "header" not in score.sheet(), "nothing until OK"
    assert editor.apply("/gui_event", [widget("text:ok"), 4, editor._version, "click"])
    head = score.sheet()["header"]
    assert (head["title"], head["notes"]) == ("A title", ["* one", "* two"])
    assert editor.undo()
    assert "header" not in score.sheet(), "the form was one entry"


def test_an_entry_of_a_palette_is_a_verb_over_the_selection(score):
    editor = ScoreEditor(score)
    editor.draw()
    first = _items(score)[0]["id"]
    editor.select([f"n{first}"])
    entry = editor.view.widget(editor, "palette", editor.structure, "ornaments:trill")
    assert editor.apply("/gui_event", [entry, 1, editor._version, "click"])
    assert _items(score)[0]["marks"]["ornament"] == "trill"
    # and the verb a palette brought is a method too
    assert editor.grace("acc")
    assert _items(score)[0]["marks"]["grace"] == "acc"
    assert editor.grace()
    assert "grace" not in _items(score)[0]["marks"]


def test_a_score_is_written_to_a_file_and_read_back_as_itself(score, tmp_path):
    editor = ScoreEditor(score)
    assert editor.set_text("title", "A title")
    with pytest.raises(ValueError, match="no file"):
        score.write()
    path = score.write(tmp_path / "a.mei")
    assert score.path == path
    again = notation.Score.read(path)
    assert again.path == path
    assert again.sheet() == score.sheet(), "notes, marks and the page's text"


def test_the_file_menu_saves_and_opens_through_this_client(score, tmp_path):
    saved, other = tmp_path / "saved.mei", tmp_path / "other.mei"
    other.write_text(notation.Score("@clef:F-4\n@data:4CD/").mei(), encoding="utf-8")
    editor = ScoreEditor(score)
    editor.draw()
    editor._window = 0
    written = len(_items(score))
    widget = lambda name: editor.view.widget(editor, "dialog", editor.structure, name)
    send = lambda *args: editor.apply("/gui_event", [args[0], 1, editor._version, *args[1:]])
    # a score with no file is asked for one, and is written there
    send(0, "menu", "save")
    send(widget("file:path"), str(saved))
    send(widget("file:ok"), "click")
    assert notation.Score.read(saved).sheet() == score.sheet()
    assert score.path == str(saved)
    # from then on Ctrl+S writes it without asking
    assert editor.articulation("stacc") is False  # nothing selected: no edit
    first = _items(score)[0]["id"]
    editor.select([f"n{first}"])
    assert editor.articulation("stacc")
    send(0, "save")
    assert notation.Score.read(saved).sheet() == score.sheet()
    # Open reads another document in place of the score, as one entry
    send(0, "menu", "dialog:open")
    send(widget("file:path"), str(other))
    send(widget("file:ok"), "click")
    assert len(_items(score)) == 2
    assert score.sheet()["staves"][0]["clef"] == "F4"
    assert editor.undo()
    assert len(_items(score)) == written
    # and the two are methods
    assert editor.load(other) and len(_items(score)) == 2
    assert editor.save(tmp_path / "again.mei") == str(tmp_path / "again.mei")


def test_new_and_close_in_the_file_menu(score, tmp_path):
    editor = ScoreEditor(score)
    editor.draw()
    editor._window = window = 0
    widget = lambda name: editor.view.widget(editor, "dialog", editor.structure, name)
    send = lambda *args: editor.apply("/gui_event", [args[0], 1, editor._version, *args[1:]])
    # New: one staff, four empty bars, as one entry; nothing to lose yet
    send(window, "menu", "new")
    assert [i["kind"] for i in _items(score)] == ["rest"] * 4
    assert not editor.unsaved
    # a change, and Close asks: Save names a file and closes once written
    assert editor.set_text("title", "A title")
    assert editor.unsaved
    send(window, "menu", "close")
    assert not editor.closed
    send(widget("close:ok"), "click")
    send(widget("file:path"), str(tmp_path / "kept.mei"))
    send(widget("file:ok"), "click")
    assert notation.Score.read(tmp_path / "kept.mei").sheet() == score.sheet()
    assert editor.closed


def test_close_with_nothing_unsaved_closes_at_once(score, tmp_path):
    editor = ScoreEditor(score)
    editor.draw()
    editor._window = window = 0
    assert editor.set_text("title", "A title")
    editor.save(tmp_path / "a.mei")
    assert not editor.unsaved
    editor.apply("/gui_event", [window, 1, editor._version, "menu", "close"])
    assert editor.closed


def test_the_score_plays_on_a_transport_of_its_own_and_hears_an_edit(score):
    from test_gui_edit import FakeHost, _PlayingServer

    server = _PlayingServer()
    editor = ScoreEditor(score, server=server)
    host = FakeHost()
    window = editor.open(host)
    # the page's cursor is drawn from the position of the score's transport
    transport = editor.transport
    assert host.clocks == [(window, "transport", transport.id)]
    assert _page(host.trees[0])["playhead_at"] == 0

    editor.play()
    addrs = [addr for addr, _ in server.sent]
    assert "/lane_new" in addrs and addrs[-1] == "/transport_play"
    # the engraver's time, 120 quarters a minute, at 100 samples a second
    assert server.lane()[:3] == [0, 50, 100]
    assert editor.playing

    # an edit is the lane's new data, heard on from where the position is
    server.sent.clear()
    first = _items(score)[0]["id"]
    editor.select([f"n{first}"])
    assert editor.delete()
    assert [addr for addr, _ in server.sent].count("/lane_set") == 1
    assert len(server.lane()) == len(_items(score))
    # and so is a step back
    server.sent.clear()
    assert editor.undo()
    assert "/lane_set" in [addr for addr, _ in server.sent]

    # the space bar over the window stops what plays, and plays from where
    # the selection starts
    send = lambda *payload: editor.apply("/gui_event", [window, 1, editor._version, *payload])
    send("play", 0)
    assert "/transport_stop" in [addr for addr, _ in server.sent] or not editor.playing
    server.state["playing"] = False
    third = _items(score)[2]["id"]
    editor.select([f"n{third}"])
    server.sent.clear()
    send("play", 0)
    assert [addr for addr, _ in server.sent][-1] == "/transport_play"
    assert editor._playback.cursor == 2.0, "the third quarter is beat 2"


def test_a_score_is_exported_as_the_sequence_it_renders(score, tmp_path):
    from clausters.seq import EventSequence

    editor = ScoreEditor(score)
    editor.draw()
    editor._window = 0
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
    # and the File menu's Export is the same, through its form
    widget = lambda name: editor.view.widget(editor, "dialog", editor.structure, name)
    send = lambda *args: editor.apply("/gui_event", [args[0], 1, editor._version, *args[1:]])
    send(0, "menu", "dialog:export_midi")
    send(widget("file:path"), str(tmp_path / "menu.mid"))
    send(widget("file:ok"), "click")
    assert len(EventSequence.from_smf((tmp_path / "menu.mid").read_bytes())) == notes


def test_the_marks_signs_and_staves_are_methods_too(score):
    editor = ScoreEditor(score)
    editor.draw()
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
