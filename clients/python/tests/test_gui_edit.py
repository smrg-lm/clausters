"""`edit(x)` over the three fundamental structures.

One verb, three editors, and no composition anywhere: a curve a script built,
a timeline it filled, a buffer it holds. What is checked is the acceptance the
track was opened with — two windows over one structure share one stack, an edit
read back is the edit that was drawn, and a window composing two structures
undoes across both in the order the edits were made.
"""

import struct

import pytest

from clausters.gui import edit
from clausters.gui.editing import (Editing, NotesEditor, PointsEditor,
                                   SamplesEditor)
from clausters.seq import Timeline
from clausters.seq.automation import Automation
from clausters.seq.event import Event as SeqEvent

SR = 48_000.0
TEMPO = 2.0
BEAT = SR / TEMPO


class FakeHost:
    """What the host is told, so an answer can be read."""

    def __init__(self):
        self.acks: list = []
        self.trees: list = []
        self.next = 20_000

    def alloc_id(self) -> int:
        self.next += 1
        return self.next

    def open(self, tree, id=None):
        self.trees.append(tree)
        return 900 + len(self.trees)

    def define(self, wid, tree):
        self.trees.append(tree)
        return wid

    def ack(self, seq, doc_version=0, reason=None):
        self.acks.append((seq, [], reason))

    def push(self, seq, *corrections, doc_version=0, reason=None):
        self.acks.append((seq, list(corrections), reason))

    def poll(self, timeout=0.0):
        return None

    def dispatch(self, *msg):
        pass


def a_curve() -> Automation:
    return Automation.from_points(
        [(0.0, 200.0, 2, 0.0), (2.0, 900.0, 1, 0.0)], None, name="cutoff")


def a_timeline() -> Timeline:
    return Timeline([(0.0, SeqEvent(midinote=60, dur=1.0)),
                     (1.0, SeqEvent(midinote=64, dur=1.0))])


class FakeBuffer:
    """A server buffer, as the samples domain touches one: a number, a shape,
    and the two calls that read and write its frames."""

    def __init__(self, frames=16, channels=1):
        self.bufnum = 7
        self.frames = frames
        self.channels = channels
        self.sample_rate = SR
        self.data = [0.0] * (frames * channels)
        self.writes = 0

    def get_samples(self, start=0, count=-1, **kwargs):
        end = len(self.data) if count < 0 else start + count
        return list(self.data[start:end])

    def set_samples(self, samples, start=0, **kwargs):
        self.writes += 1
        for i, value in enumerate(samples):
            self.data[start + i] = float(value)


def opened(editor):
    host = FakeHost()
    editor.open(host)
    return host, host.trees[0]["children"][0]["id"]


def blob(values) -> bytes:
    return struct.pack(f"<{len(values)}f", *values)


# ---- the verb ----

def test_the_verb_opens_the_editor_the_structure_asks_for():
    assert isinstance(edit(a_curve(), sample_rate=SR), PointsEditor)
    assert isinstance(edit(a_timeline(), sample_rate=SR), NotesEditor)
    assert isinstance(edit(FakeBuffer()), SamplesEditor)


def test_something_none_of_the_three_reads_says_what_they_are():
    with pytest.raises(TypeError, match="edit` opens a Buffer"):
        edit(object())


# ---- a curve ----

def test_a_curve_is_drawn_edited_and_read_back_with_no_composition():
    curve = a_curve()
    editor = edit(curve, sample_rate=SR, tempo=TEMPO)
    host, wid = opened(editor)

    assert editor.apply("/gui_event", [wid, 1, 0, "points",
                                       0.0, 300.0, 1, 0.0,
                                       1.0, 500.0, 2, 0.0,
                                       2.0, 100.0, 1, 0.0]) is True
    # Read back through the object the caller already holds: no handing back.
    assert curve.to_points()[0:2] == pytest.approx([0.0, 300.0])
    assert curve.to_points()[4:6] == pytest.approx([1.0, 500.0])
    assert editor.can_undo and editor.undo_label == "draw the curve"

    assert editor.undo() is True
    assert curve.to_points()[0:2] == pytest.approx([0.0, 200.0])


def test_a_segments_shape_survives_the_round_trip():
    # The crate carries a point's `data` and reads none of it, which is what
    # keeps an undo from putting the curve back straight.
    curve = a_curve()
    editor = edit(curve, sample_rate=SR, tempo=TEMPO)
    _host, wid = opened(editor)
    editor.apply("/gui_event", [wid, 1, 0, "points",
                                0.0, 300.0, 5, -4.0,
                                2.0, 900.0, 1, 0.0])
    assert curve.to_points()[2:4] == pytest.approx([5, -4.0]), \
        "the shape the hand drew"
    editor.undo()
    assert curve.to_points()[2:4] == pytest.approx([2, 0.0]), \
        "and the shape it had before (exponential), not a straight line"


def test_a_resend_of_the_curve_is_not_an_edit():
    curve = a_curve()
    editor = edit(curve, sample_rate=SR, tempo=TEMPO)
    _host, wid = opened(editor)
    assert editor.apply("/gui_event", [wid, 1, 0, "points",
                                       *curve.to_points()]) is False
    assert editor.can_undo is False


# ---- a timeline ----

def test_a_roll_edits_the_timeline_the_caller_holds():
    timeline = a_timeline()
    editor = edit(timeline, sample_rate=SR, tempo=TEMPO)
    _host, wid = opened(editor)

    assert editor.apply("/gui_event", [wid, 1, 0, "notes",
                                       0.0, BEAT, 67, 100, 0,
                                       2 * BEAT, BEAT, 72, 100, 0]) is True
    played = [(beat, event.midinote()) for beat, event in timeline]
    assert played == [(0.0, 67.0), (2.0, 72.0)]
    assert editor.undo() is True
    assert [(beat, event.midinote()) for beat, event in timeline] == \
        [(0.0, 60.0), (1.0, 64.0)]


def test_a_note_keeps_what_the_roll_cannot_draw():
    # Order is the only identity the payload carries, so the i-th note's own
    # event is edited rather than rebuilt from the five numbers.
    timeline = Timeline([(0.0, SeqEvent(midinote=60, dur=1.0, instrument="bell"))])
    editor = edit(timeline, sample_rate=SR, tempo=TEMPO)
    _host, wid = opened(editor)
    editor.apply("/gui_event", [wid, 1, 0, "notes", 0.0, BEAT, 65, 100, 0])
    _beat, event = next(iter(timeline))
    assert event.get("instrument") == "bell"
    assert event.midinote() == 65.0


def test_what_the_roll_does_not_draw_is_kept():
    from clausters.seq.timeline import OscItem

    timeline = a_timeline()
    marker = OscItem("/mark")
    timeline.add(3.0, marker)
    editor = edit(timeline, sample_rate=SR, tempo=TEMPO)
    _host, wid = opened(editor)
    editor.apply("/gui_event", [wid, 1, 0, "notes", 0.0, BEAT, 67, 100, 0])
    assert any(item is marker for _beat, item in timeline), \
        "a rebuilt timeline would have dropped it"


def test_a_marker_dragged_in_the_roll_moves_it_on_the_timeline():
    # The lane the roll draws and nobody answered: a marker slid in the OSC
    # lane is an edit of the timeline, with an inverse like any other.
    from clausters.seq.timeline import OscItem

    timeline = a_timeline()
    timeline.add(3.0, OscItem("/hit", 7))
    editor = edit(timeline, sample_rate=SR, tempo=TEMPO)
    _host, wid = opened(editor)
    assert editor.apply("/gui_event", [wid, 1, 0, "osc", 1.5 * BEAT, "/hit"])
    at = [(beat, item) for beat, item in timeline if isinstance(item, OscItem)]
    assert at == [(1.5, at[0][1])], "the marker moved, and it is the same item"
    assert at[0][1].args == (7,), "the message it sends is not the lane's to lose"
    assert editor.undo_label == "edit the markers"
    assert editor.undo() is True
    assert [beat for beat, item in timeline if isinstance(item, OscItem)] == [3.0]


def test_a_marker_removed_in_the_roll_leaves_its_neighbours_theirs():
    # Matched by label rather than by order, so removing one does not hand the
    # next one's message to the wrong marker.
    from clausters.seq.timeline import OscItem

    timeline = Timeline([(0.0, OscItem("/a", 1)), (1.0, OscItem("/b", 2)),
                         (2.0, OscItem("/c", 3))])
    editor = edit(timeline, sample_rate=SR, tempo=TEMPO)
    _host, wid = opened(editor)
    assert editor.apply("/gui_event", [wid, 1, 0, "osc", 0.0, "/a", 2 * BEAT, "/c"])
    assert [(item.addr, item.args) for _beat, item in timeline] == \
        [("/a", (1,)), ("/c", (3,))]


def test_a_marker_added_in_the_roll_is_refused_and_says_why():
    # A marker is the message it sends and the lane cannot type one, so the
    # gesture is answered rather than half-applied: the reason, and the markers
    # as they still are.
    from clausters.seq.timeline import OscItem

    timeline = Timeline([(0.0, OscItem("/a"))])
    editor = edit(timeline, sample_rate=SR, tempo=TEMPO)
    host, wid = opened(editor)
    assert editor.apply("/gui_event", [wid, 1, 0, "osc", 0.0, "/a", BEAT, ""]) \
        is False
    assert len(timeline) == 1
    seq, corrections, reason = host.acks[-1]
    assert seq == 1 and "OscItem" in (reason or "")
    assert corrections and corrections[0][1]["osc"] == [0.0, "/a"]


def test_the_notes_gesture_does_not_move_the_markers():
    from clausters.seq.timeline import OscItem

    timeline = a_timeline()
    timeline.add(3.0, OscItem("/hit"))
    editor = edit(timeline, sample_rate=SR, tempo=TEMPO)
    _host, wid = opened(editor)
    editor.apply("/gui_event", [wid, 1, 0, "notes", 0.0, BEAT, 67, 100, 0])
    assert [(beat, type(item).__name__) for beat, item in timeline] == \
        [(0.0, "Event"), (3.0, "OscItem")]


# ---- samples ----

def test_a_stroke_writes_the_servers_buffer_and_undoes_off_the_wire():
    take = FakeBuffer(frames=8)
    editor = edit(take, tempo=TEMPO)
    _host, wid = opened(editor)

    assert editor.apply("/gui_event", [wid, 1, 0, "draw", 0, 2,
                                       blob([0.5, -0.5]),
                                       blob([0.0, 0.0])]) is True
    assert take.data[2:4] == [0.5, -0.5]
    assert editor.can_undo and editor.undo_label == "draw the samples"
    # The inverse rode on the wire: nothing was read back to invert it.
    assert editor.undo() is True
    assert take.data[2:4] == [0.0, 0.0]


def test_one_dragged_sample_is_the_same_edit_one_frame_wide():
    take = FakeBuffer(frames=8)
    editor = edit(take, tempo=TEMPO)
    _host, wid = opened(editor)
    assert editor.apply("/gui_event", [wid, 1, 0, "sample", 0, 3, 0.9, 0.0]) is True
    assert take.data[3] == pytest.approx(0.9)
    editor.undo()
    assert take.data[3] == pytest.approx(0.0)


def test_a_stroke_on_one_channel_of_a_stereo_take_leaves_the_other_alone():
    take = FakeBuffer(frames=4, channels=2)
    take.data = [0.1, 0.2] * 4
    editor = edit(take, tempo=TEMPO)
    _host, wid = opened(editor)
    editor.apply("/gui_event", [wid, 1, 0, "draw", 1, 1,
                                blob([0.7, 0.8]), blob([0.2, 0.2])])
    assert take.data == pytest.approx([0.1, 0.2, 0.1, 0.7, 0.1, 0.8, 0.1, 0.2])


# ---- the acceptance the track was opened with ----

def test_edit_called_twice_gives_two_windows_and_one_stack():
    curve = a_curve()
    left, right = edit(curve, sample_rate=SR), edit(curve, sample_rate=SR)
    left_host, wid = opened(left)
    right_host, _ = opened(right)
    right_host.acks.clear()

    left.apply("/gui_event", [wid, 1, 0, "points", 0.0, 400.0, 1, 0.0,
                              2.0, 900.0, 1, 0.0])
    assert right.can_undo, "one pile, whichever window made the edit"
    assert right_host.acks, "and the other window is told what to draw"

    # An undo in *either* updates both, which is the whole claim.
    assert right.undo() is True
    assert curve.to_points()[0:2] == pytest.approx([0.0, 200.0])


def test_a_window_over_a_curve_and_a_roll_undoes_across_both_in_order():
    # The composed case: two structures, one editing context, one order.
    context = Editing()
    curve, timeline = a_curve(), a_timeline()
    curve_editor = edit(curve, sample_rate=SR, tempo=TEMPO, context=context)
    roll = edit(timeline, sample_rate=SR, tempo=TEMPO, context=context)
    _ch, curve_wid = opened(curve_editor)
    _rh, roll_wid = opened(roll)

    curve_editor.apply("/gui_event", [curve_wid, 1, 0, "points",
                                      0.0, 300.0, 1, 0.0, 2.0, 900.0, 1, 0.0])
    roll.apply("/gui_event", [roll_wid, 1, 0, "notes", 0.0, BEAT, 67, 100, 0])
    assert context.history.undo_label == "edit the notes"

    # The notes go back first: one pile, walked in the order the edits landed.
    assert roll.undo() is True
    assert [event.midinote() for _b, event in timeline] == [60.0, 64.0]
    assert curve.to_points()[1] == pytest.approx(300.0), "the curve has not moved yet"
    assert curve_editor.undo() is True
    assert curve.to_points()[1] == pytest.approx(200.0)
