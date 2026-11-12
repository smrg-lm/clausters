"""The notes editor's handler: a roll in the GUI host's application, over the
sequence the script holds. Run against a real host with no window on screen
(``[gui] headless``); what a gesture does on the roll is the application's and
tested in its crate (``crates/clausters-apps/src/notes``), and the door in the
host's (``crates/clausters-gui/src/host/audio_editor.rs``).
"""

import pytest

from clausters.base import TempoMap
from clausters.gui.editing import HostedTransport, NotesEditor, edit
from clausters.seq import EventSequence, Timeline
from clausters.seq.event import Event
from conftest import opened


def _take() -> EventSequence:
    return EventSequence([(0.0, Event(midinote=60, dur=1.0, instrument="default", amp=0.2)),
                          (1.0, Event(midinote=64, dur=1.0))], tempo_map=TempoMap(120.0))


def test_a_timeline_opens_as_the_events_it_renders_and_is_left_as_it_was(gui_host):
    timeline = Timeline([(0.0, Event(midinote=60, dur=1.0)), (1.0, Event(midinote=64, dur=1.0))])
    editor = edit(timeline, host=gui_host)
    assert isinstance(editor, NotesEditor)
    editor.sequence.events[0]["midinote"] = 67
    assert [event.midinote() for _, event in timeline] == [60.0, 64.0]
    editor.close()
    assert editor.sequence.events[0]["midinote"] == 67


def test_the_sequence_is_the_windows_while_it_is_open(gui_host):
    take = _take()
    objects = list(take.events)
    editor = opened(gui_host, NotesEditor(take))
    # reads answer what the window holds, through the same objects
    assert take.events[0] is objects[0] and len(take) == 2
    objects[0]["midinote"] = 62
    assert editor.undo_label and editor.can_undo
    assert editor.undo() and objects[0]["midinote"] == 60
    assert editor.redo() and objects[0]["midinote"] == 62
    assert not editor.dirty, "a script's change is no hand's"
    # a block is one entry, however many changes it holds
    with take.history("both"):
        objects[0].at = 0.5
        objects[1].at = 1.5
    assert take.history.undo_label == "both"
    assert take.history.undo() and (objects[0].at, objects[1].at) == (0.0, 1.0)
    added = take.events.add(3.0, Event(midinote=72, dur=1.0))
    assert len(take) == 3 and added.sequence is take
    assert take.history.undo() and len(take) == 2
    assert take.history.redo() and take.events[2] is added, "the same event comes back"
    editor.close()
    # the window went: the sequence holds what it left, and is the script's
    assert editor.closed and take._window() is None
    assert [e["midinote"] for e in take.events] == [62, 64, 72]
    assert take.events[0] is objects[0]
    assert not take.history.can_undo, "a history of its own, from nothing"


def test_edit_hands_back_the_roll_open_over_a_sequence(gui_host):
    take = _take()
    keys = edit(take, host=gui_host)
    assert edit(take, host=gui_host) is keys
    hertz = edit(take, host=gui_host, y_axis="hz")
    assert hertz is not keys and edit(take, host=gui_host, y_axis="hz") is hertz
    keys.close()
    assert edit(take, host=gui_host, y_axis="hz") is hertz, "the other goes on"
    assert len(take) == 2, "and it still reads the window's"
    hertz.close()


def test_two_rolls_share_a_history_and_have_a_transport_each(gui_host):
    take = _take()
    keys = edit(take, host=gui_host)
    hertz = edit(take, host=gui_host, y_axis="hz")
    take.events[0]["midinote"] = 62
    assert keys.undo_label == hertz.undo_label
    assert hertz.undo() and take.events[0]["midinote"] == 60
    keys.play(0.5)
    assert keys.playing and not hertz.playing
    keys.stop()
    assert not keys.playing
    keys.close()
    hertz.close()


def test_what_is_marked_is_the_windows_and_the_events_themselves(gui_host):
    take = _take()
    keys = edit(take, host=gui_host)
    hertz = edit(take, host=gui_host, y_axis="hz")
    first = take.events[0]
    keys.select([first])
    assert keys.selected == [first] and keys.selected[0] is first
    assert hertz.selected == [], "each window has its own"
    keys.unselect()
    assert keys.selected == []
    keys.close()
    assert keys.selected == [], "empty with no window open"
    hertz.close()


def test_the_grid_and_the_span_are_the_windows(gui_host):
    take = _take()
    editor = opened(gui_host, NotesEditor(take))
    assert editor.grid == 0.25
    editor.grid = 1.0
    assert editor.grid == 1.0
    assert editor.snap_to_grid is True
    editor.snap_to_grid = False
    assert editor.snap_to_grid is False and editor.grid == 1.0
    assert editor.monophonic is False
    editor.monophonic = True
    assert editor.monophonic is True and editor.snap_to_grid is False
    transport = editor.transport
    assert isinstance(transport, HostedTransport)
    assert transport.span is None
    transport.span = (0.5, 1.5)
    assert transport.span == (0.5, 1.5)
    transport.span = None
    assert transport.span is None
    transport.play(0.0, 1.0, looping=True)
    assert transport.playing
    transport.stop()
    assert not transport.playing
    editor.close()


def test_a_verb_is_asked_of_the_window_and_its_actions_read(gui_host):
    take = _take()
    editor = opened(gui_host, NotesEditor(take))
    assert editor.select_all() and editor.verb("view_all")
    assert editor.actions["undo"]["enabled"] is False
    assert editor.select_all() and editor.join(), "a line of two pitches is joined"
    assert take.events[1].get("joins") == take.events[0]._id
    assert editor.actions["undo"]["enabled"] is True, "and the join is an entry"
    assert editor.select_all() and editor.unjoin()
    assert take.events[1].get("joins") is None
    # a transposition with the selection kept: both notes, one entry each
    assert editor.select_all() and editor.step_up()
    assert [e["midinote"] for e in take.events] == [61.0, 65.0]
    assert editor.octave_down()
    assert [e["midinote"] for e in take.events] == [49.0, 53.0]
    assert editor.undo() and editor.undo()
    assert [e["midinote"] for e in take.events] == [60.0, 64.0]
    # the panel beside the roll: shown by its switch, no edit
    assert not editor.events_panel
    label = editor.undo_label
    editor.events_panel = True
    assert editor.events_panel and editor.undo_label == label
    editor.events_panel = False
    assert not editor.events_panel
    # the display beside the roll: its switch and its settings, no edit
    assert not editor.display_panel and editor.display["slurs"] == "arc"
    editor.display_panel = True
    editor.set_display(slurs="band", pitch_line=False, lanes={"velocity": False})
    shown = editor.display
    assert editor.display_panel and editor.undo_label == label
    assert (shown["slurs"], shown["pitch_line"], shown["hidden_lanes"]) == ("band", False, ["velocity"])
    with pytest.raises(ValueError):
        editor.set_display(marks="spiral")
    assert editor.display["marks"] == "row", "a refused setting changes nothing"
    editor.display_panel = False
    # a selection by pitch marks what shares it, and edits nothing
    editor.select([take.events[0]])
    assert editor.select_pitch() and editor.selected == [take.events[0]]
    assert editor.select_channel() and len(editor.selected) == 2
    editor.close()
    assert editor.verb("view_all") is False, "no window, no verb"


def test_a_voice_from_a_pattern_is_left_as_it_was_written(gui_host):
    from clausters.seq import pseq, pvoice

    voice = pvoice("default", midinote=pseq([60, 62, 64, 64, 67]), dur=0.5,
                   legato=pseq([1.0, 1.0, 1.0, 0.6, 0.8]), artic=True).render_events()
    before = voice.data()
    editor = opened(gui_host, NotesEditor(voice))
    editor.monophonic = True
    editor.close()
    after = voice.data()
    assert after == before, "opened and closed, the roll wrote nothing"
    assert not any("tie" in e["data"] for e in after["events"]) and "notation" not in after


def test_the_roll_edits_in_its_format_s_mode_and_converts_into_copies(gui_host):
    take = EventSequence([(0.03, Event(midinote=60, dur=0.97, instrument="default")),
                          (1.01, Event(midinote=64, dur=1.0, instrument="default"))])
    editor = opened(gui_host, NotesEditor(take))
    assert editor.mode == "midi" and take.format == "events"
    editor.channel = 1
    assert editor.channel == 1, "the MIDI mode edits a channel"
    editor.channel = None
    assert editor.tool == "select", "the pointer, at first"
    editor.tool = "draw"
    assert editor.tool == "draw"
    with pytest.raises(ValueError):
        editor.tool = "brush"
    editor.tool = "select"
    page = editor.to_notation()
    assert [e.at for e in take.events] == [0.03, 1.01], "the take is as it was"
    assert page.format == "notation" and [e.at for e in page.events] == [0.0, 1.0]
    assert page.events[0]["value"] == [1, 4]
    assert editor.actions["undo"]["enabled"] is False, "a copy is no edit"
    editor.close()
    written = opened(gui_host, NotesEditor(page))
    assert written.mode == "notation", "a page is edited as one"
    with pytest.raises(ValueError):
        written.channel = 1  # a page is edited a voice at a time
    written.voice = (0, 1)
    assert written.voice == (0, 1)
    written.scope = ("staff", 0)
    assert written.scope == ("staff", 0)
    written.scope = "system"
    written.voice = (0, 0)
    assert written.select_all() and written.staccato()
    assert all(e.get("articulations") == ["stacc"] for e in page.events)
    assert page.events[0]["sustain"] < 1.0, "heard short at once"
    written.writing = "ties"
    assert written.writing == "ties" and page.parts[0]["writing"]["polyphony"] == "ties"
    midi = written.to_midi("2.0")
    assert midi.format == "2.0" and midi.events[0].get("value") is None
    assert page.format == "notation" and page.events[0]["value"] == [1, 4], "the page stays"
    written.close()


def test_a_pedal_written_on_a_file_s_notes_takes_ids_nothing_else_has(gui_host):
    """**What the page derives is numbered by the sequence**: an MPE file read
    back holds notes with curves of their own, and the pedal written over them
    is a curve of the sequence under an id no note and no note's curve has --
    the roll names a curve by its id, so a pedal sharing one with a bend was
    drawn as one curve with both and edited into both."""
    chord = EventSequence([(0.0, Event(midinote=60, dur=4.0)),
                           (0.0, Event(midinote=64, dur=4.0)),
                           (0.0, Event(midinote=67, dur=4.0))])
    chord.set_midi("mpe")
    low, middle, _ = chord.events
    low.automation.add({"bend": True}, [(0.0, 0.0), (2.0, 1.0)])
    middle.automation.add({"pressure": True}, [(0.0, 0.2), (3.0, 1.0)])
    chord.automation.add({"cc": 7}, [(0.0, 90.0), (4.0, 120.0)], name="volume")
    take = EventSequence.from_smf(chord.to_smf()).notated()
    editor = opened(gui_host, NotesEditor(take))
    assert editor.mode == "notation"
    assert editor.select_all() and editor.pedal()

    def pedalled():
        return any(curve["target"].get("cc") == 64 for curve in take.data()["automation"])

    # the verb is the window's, performed through the host's loop
    assert gui_host._wait_while(lambda: not pedalled(), 5.0), "the pedal"
    data = take.data()
    ids = [event["id"] for event in data["events"]]
    ids += [curve["id"] for curve in data["automation"]]
    ids += [curve["id"] for event in data["events"] for curve in event.get("automation", [])]
    assert len(ids) == len(set(ids)), f"an id named two things: {sorted(ids)}"
    editor.close()
