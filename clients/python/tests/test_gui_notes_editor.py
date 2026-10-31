"""The notes editor's handler: a roll in the GUI host's application, over the
sequence the script holds. Run against a real host with no window on screen
(``[gui] headless``); what a gesture does on the roll is the application's and
tested in its crate (``crates/clausters-apps/src/notes``), and the door in the
host's (``clients/gui/src/host/audio_editor.rs``).
"""

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
    editor.close()
    assert editor.verb("view_all") is False, "no window, no verb"
