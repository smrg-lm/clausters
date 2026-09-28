"""`edit(x)` over the three fundamental structures.

One verb, three editors, and no multitrack anywhere: a curve a script built,
a timeline it filled, a buffer it holds. What is checked is the acceptance the
track was opened with -- two windows over one structure share one stack, an edit
read back is the edit that was drawn, and a window composing two structures
undoes across both in the order the edits were made.
"""

import json

import pytest

from clausters import TempoMap

from clausters.gui import edit
from clausters.gui.editing import Editing, NotesEditor, PointsEditor
from clausters.seq import EventSequence, Timeline
from clausters.defs.ugens import Bpf
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
        #: What `subscribe` was handed -- an open editor's `apply`.
        self.subscribed: list = []
        self.closed: list = []

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

    def close(self, id):
        self.closed.append(id)

    def _set_closed_handler(self, id, func):
        self.on_closed = (id, func)

    def poll(self, timeout=0.0):
        return None

    def dispatch(self, *msg):
        pass

    # The event loop's end of the protocol. A double is a host, so it answers
    # the two things an editor asks of one at `open`: what to subscribe to, and
    # a loop -- which a double does not have and does not need, since nothing
    # here is delivered by one.
    def subscribe(self, func):
        self.subscribed.append(func)
        return func

    def unsubscribe(self, func):
        if func in self.subscribed:
            self.subscribed.remove(func)

    looping = False
    loop = None


def a_curve() -> Bpf:
    return Bpf([(0.0, 200.0, "exp"), (2.0, 900.0)])


def a_timeline() -> Timeline:
    return Timeline([(0.0, SeqEvent(midinote=60, dur=1.0)),
                     (1.0, SeqEvent(midinote=64, dur=1.0))])


def opened(editor):
    host = FakeHost()
    editor.open(host)
    return host, host.trees[0]["children"][0]["id"]


# ---- the verb ----

def test_the_verb_opens_the_editor_the_structure_asks_for():
    assert isinstance(edit(a_curve(), sample_rate=SR, open=False), PointsEditor)
    assert isinstance(edit(a_timeline(), sample_rate=SR, open=False), NotesEditor)


def test_the_verb_opens_the_window_and_subscribes_the_editor_to_the_host():
    """`edit(x)` is one call: it resolves a host, opens, and plugs the editor
    into what the host delivers -- so ``edit(curve)`` on its own is a complete
    program and nothing writes a drain loop. ``open=False`` is the one case the
    separate `Editor.open` was ever for: a caller composing a window out of
    several editors."""
    host = FakeHost()
    editor = edit(a_curve(), sample_rate=SR, host=host)
    assert editor.id == editor.window and editor.id is not None
    assert editor.closed is False
    assert host.subscribed == [editor.apply]

    editor.close()
    assert editor.closed and editor.id is None
    assert host.subscribed == [], "a closed window stops being delivered to"


def test_open_false_builds_the_editor_without_a_window():
    editor = edit(a_curve(), sample_rate=SR, open=False)
    assert editor.window is None and editor.closed


def test_something_none_of_the_three_reads_says_what_they_are():
    with pytest.raises(TypeError, match="edit` opens a Buffer"):
        edit(object())


# ---- a curve ----

def test_a_curve_is_drawn_edited_and_read_back_with_no_multitrack():
    curve = a_curve()
    editor = edit(curve, sample_rate=SR, open=False)
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


def test_an_edit_made_against_a_picture_an_undo_replaced_is_refused():
    # The staleness floor, on the road an editor actually travels. A host stamps
    # every event with the version it was last told, and it is told only when an
    # acknowledgement reaches it -- a round trip a hand outruns -- so an edit
    # naming an older version is the ordinary case and applies. What does not is
    # an edit made against a picture the data has moved away from by a
    # route the host never saw: here an undo.
    curve = a_curve()
    editor = edit(curve, sample_rate=SR, open=False)
    host, wid = opened(editor)

    assert editor.apply("/gui_event", [wid, 1, 0, "points",
                                       0.0, 300.0, 1, 0.0,
                                       2.0, 100.0, 1, 0.0]) is True
    #: The version the host has just been told it is drawing, read where it
    #: lives: the version is the editing **context's**, not this view's.
    against = Editing.of(curve).version

    assert editor.undo() is True
    assert curve.to_points()[0:2] == pytest.approx([0.0, 200.0])

    # The event the hand had already sent, naming the picture it was made
    # against. Refused rather than applied: an edit-back payload is absolute
    # *and* whole, so applying one made against an older picture would silently
    # drop whatever arrived in between -- here, the undo.
    assert editor.apply("/gui_event", [wid, 2, against, "points",
                                       0.0, 900.0, 1, 0.0,
                                       2.0, 900.0, 1, 0.0]) is False
    assert curve.to_points()[0:2] == pytest.approx([0.0, 200.0]), "the undo stands"
    assert host.acks[-1][2] == "the data changed since this edit"

    # And a gesture made against the picture that now holds applies.
    assert editor.apply("/gui_event", [wid, 3, Editing.of(curve).version, "points",
                                       0.0, 600.0, 1, 0.0,
                                       2.0, 600.0, 1, 0.0]) is True
    assert curve.to_points()[0:2] == pytest.approx([0.0, 600.0])


def test_a_segments_shape_survives_the_round_trip():
    # The crate carries a point's `data` and reads none of it, which is what
    # keeps an undo from putting the curve back straight.
    curve = a_curve()
    editor = edit(curve, sample_rate=SR, open=False)
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
    editor = edit(curve, sample_rate=SR, open=False)
    _host, wid = opened(editor)
    assert editor.apply("/gui_event", [wid, 1, 0, "points",
                                       *curve.to_points()]) is False
    assert editor.can_undo is False


# ---- an event sequence, and a timeline rendered into one ----

def _roll(tempo=TEMPO):
    """A timeline at ``tempo``, opened: its rendered sequence is what the roll
    edits."""
    timeline = a_timeline()
    timeline.map = TempoMap(tempo)
    editor = edit(timeline, sample_rate=SR, open=False)
    return timeline, editor


def test_a_timeline_opens_as_the_events_it_renders_and_is_left_as_it_was():
    timeline, editor = _roll()
    _host, wid = opened(editor)
    assert editor.apply("/gui_event", [wid, 1, 0, "notes",
                                       1, 0.0, BEAT, 67, 13, 0,
                                       2, 2 * BEAT, BEAT, 72, 13, 0]) is True
    assert [(beat, e["midinote"]) for beat, e in editor.sequence] == [(0.0, 67), (2.0, 72)]
    # The timeline is code, and the roll edited what it produced.
    assert [(beat, event.midinote()) for beat, event in timeline] == [(0.0, 60.0), (1.0, 64.0)]
    assert editor.undo() is True
    assert [(beat, e["midinote"]) for beat, e in editor.sequence] == [(0.0, 60.0), (1.0, 64.0)]


def test_a_sequence_is_edited_in_place_by_id():
    seq = EventSequence([(0.0, SeqEvent(midinote=60, dur=1.0, instrument="bell")),
                         (1.0, SeqEvent(midinote=64, dur=1.0))], tempo_map=TempoMap(TEMPO))
    editor = edit(seq, sample_rate=SR, open=False)
    _host, wid = opened(editor)
    # Note 1 is gone and note 2 moved: order is no identity, so note 2 keeps
    # its own keys and the one removed is the one named.
    editor.apply("/gui_event", [wid, 1, 0, "notes", 2, 2 * BEAT, BEAT, 65, 13, 0])
    assert [(id, beat, e["midinote"]) for id, beat, e in seq.entries()] == [(2, 2.0, 65)]


def test_a_rolls_ruler_reads_the_sequences_own_map():
    _timeline, editor = _roll()
    tree = editor.view.build(editor)
    roll = next(node for node in _walk(tree) if node.get("type") == "notes")
    assert json.loads(roll["axes"]["x"]["tempo_map"]) == json.loads(TempoMap(TEMPO).dump())
    assert roll["note_ids"] == [1, 2]
    assert roll["notes"][:5] == [0.0, BEAT * 0.8, 60.0, 13.0, 0.0]


def _walk(node):
    yield node
    for child in node.get("children", []) or []:
        yield from _walk(child)


def test_a_note_keeps_what_the_roll_cannot_draw():
    seq = EventSequence([(0.0, SeqEvent(midinote=60, dur=1.0, instrument="bell"))],
                        tempo_map=TempoMap(TEMPO))
    editor = edit(seq, sample_rate=SR, open=False)
    _host, wid = opened(editor)
    editor.apply("/gui_event", [wid, 1, 0, "notes", 1, 0.0, BEAT, 65, 13, 0])
    _beat, event = next(iter(seq))
    assert event.get("instrument") == "bell"
    assert event.midinote() == 65.0


def test_a_note_the_hand_made_gets_an_id_and_the_roll_is_told():
    _timeline, editor = _roll()
    host, wid = opened(editor)
    editor.apply("/gui_event", [wid, 1, 0, "notes",
                                1, 0.0, BEAT * 0.8, 60, 13, 0,
                                2, BEAT, BEAT * 0.8, 64, 13, 0,
                                0, 3 * BEAT, BEAT, 67, 90, 0])
    assert [id for id, _b, _e in editor.sequence.entries()] == [1, 2, 3]
    _seq, corrections, _reason = host.acks[-1]
    assert corrections[0][1]["note_ids"] == [1, 2, 3]


def test_what_the_roll_does_not_draw_is_kept():
    from clausters.seq.timeline import OscItem

    timeline = a_timeline()
    timeline.add(3.0, OscItem("/mark"))
    timeline.map = TempoMap(TEMPO)
    editor = edit(timeline, sample_rate=SR, open=False)
    _host, wid = opened(editor)
    editor.apply("/gui_event", [wid, 1, 0, "notes", 1, 0.0, BEAT, 67, 13, 0])
    assert [e.get("type", "note") for _b, e in editor.sequence] == ["note", "osc"]


def test_a_marker_dragged_in_the_roll_moves_it():
    from clausters.seq.timeline import OscItem

    timeline = a_timeline()
    timeline.add(3.0, OscItem("/hit", 7))
    timeline.map = TempoMap(TEMPO)
    editor = edit(timeline, sample_rate=SR, open=False)
    _host, wid = opened(editor)
    assert editor.apply("/gui_event", [wid, 1, 0, "osc", 1.5 * BEAT, "/hit"])
    at = [(beat, e) for beat, e in editor.sequence if e.get("type") == "osc"]
    assert at[0][0] == 1.5
    assert at[0][1]["args"] == [7], "the message it sends is not the lane's to lose"
    assert editor.undo_label == "edit the markers"
    assert editor.undo() is True
    assert [beat for beat, e in editor.sequence if e.get("type") == "osc"] == [3.0]


def test_a_marker_removed_in_the_roll_leaves_its_neighbours_theirs():
    from clausters.seq.timeline import OscItem

    seq = EventSequence([(0.0, OscItem("/a", 1)), (1.0, OscItem("/b", 2)),
                         (2.0, OscItem("/c", 3))], tempo_map=TempoMap(TEMPO))
    editor = edit(seq, sample_rate=SR, open=False)
    _host, wid = opened(editor)
    assert editor.apply("/gui_event", [wid, 1, 0, "osc", 0.0, "/a", 2 * BEAT, "/c"])
    assert [(e["addr"], e["args"]) for _b, e in seq] == [("/a", [1]), ("/c", [3])]


def test_a_marker_added_in_the_roll_is_refused_and_says_why():
    from clausters.seq.timeline import OscItem

    seq = EventSequence([(0.0, OscItem("/a"))], tempo_map=TempoMap(TEMPO))
    editor = edit(seq, sample_rate=SR, open=False)
    host, wid = opened(editor)
    assert editor.apply("/gui_event", [wid, 1, 0, "osc", 0.0, "/a", BEAT, ""]) is False
    assert len(seq) == 1
    seq_, corrections, reason = host.acks[-1]
    assert seq_ == 1 and "a marker is the message it sends" in (reason or "")
    assert corrections and corrections[0][1]["osc"] == [0.0, "/a"]


# ---- the acceptance the track was opened with ----

def test_edit_called_twice_gives_two_windows_and_one_stack():
    curve = a_curve()
    left = edit(curve, sample_rate=SR, open=False)
    right = edit(curve, sample_rate=SR, open=False)
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
    curve = a_curve()
    seq = EventSequence([(0.0, SeqEvent(midinote=60, dur=1.0))], tempo_map=TempoMap(TEMPO))
    curve_editor = edit(curve, sample_rate=SR, context=context, open=False)
    roll = edit(seq, sample_rate=SR, context=context, open=False)
    _ch, curve_wid = opened(curve_editor)
    _rh, roll_wid = opened(roll)

    curve_editor.apply("/gui_event", [curve_wid, 1, 0, "points",
                                      0.0, 300.0, 1, 0.0, 2.0, 900.0, 1, 0.0])
    roll.apply("/gui_event", [roll_wid, 1, 0, "notes", 1, 0.0, BEAT, 67, 13, 0])
    assert context.undo_label == "edit the notes"

    # The notes go back first: one pile, walked in the order the edits landed.
    assert roll.undo() is True
    assert [e.midinote() for _b, e in seq] == [60.0]
    assert curve.to_points()[1] == pytest.approx(300.0), "the curve has not moved yet"
    assert curve_editor.undo() is True
    assert curve.to_points()[1] == pytest.approx(200.0)


# ---- the picture a view draws is the crate's ----


def test_a_catalogue_view_is_described_by_the_crate_and_not_by_this_client():
    # Which widget draws a take, and the three gestures it offers, were written
    # here, in the web client and in the standalone host. One answer now.
    from clausters import _native

    said = _native.view_props("waveform", {"buffer": 3, "channels": 1,
                                           "ruler": "time",
                                           "sample_rate": SR})
    assert said["type"] == "signal" and said["view"] == "trace"
    assert said["gestures"] == {"drag": "select", "alt": "draw",
                                "ctrl": "sample"}
    assert "id" not in said, "which number a widget gets is the caller's"

    # And the roll's pitch window, which is the other rule that travelled with
    # the picture: one note is its own window, padded.
    roll = _native.view_props("pianoroll", {"notes": [0.0, 1.0, 60.0, 100.0, 0.0]})
    assert roll["axes"]["y"] == {"min": 56.0, "max": 64.0}
    with pytest.raises(ValueError, match='"clip"'):
        _native.view_props("clip", {})
    with pytest.raises(ValueError, match="cannot be drawn"):
        _native.view_props("pianoroll", {"notes": "sixty"})


def test_a_sequence_with_a_marker_still_draws_its_notes():
    # The marker lane is `time label` pairs, and a label is text: typed as
    # numbers alone, one marker refused the whole roll and the window opened
    # with nothing on it.
    from clausters.seq.timeline import OscItem

    seq = EventSequence([(0.0, SeqEvent(midinote=60, dur=1.0)),
                         (3.0, OscItem("/mark", 1, "cue"))])
    editor = NotesEditor(seq, sample_rate=SR)
    roll = editor.view.build(editor)["children"][0]
    assert roll["type"] == "notes"
    assert len(roll["notes"]) == 5, "the one note is on the roll"
    assert roll["osc"][1] == "/mark", "and the marker beside it, by its label"


# ---- the axis a curve is drawn against ----


def test_a_curve_is_drawn_against_its_own_axis_and_not_the_default_one():
    """A `bpf` given no range draws against the unipolar default and fits its
    time to the last point, so a curve of any other range is pinned to the top
    of the field and the edit-back reports the *axis*'s values -- one drag
    destroys the data's range. Both ends come from the curve."""
    curve = a_curve()                       # 200 Hz to 900 Hz, ending at beat 2
    editor = edit(curve, sample_rate=SR, open=False)
    widget = editor.draw()["children"][0]

    band = widget["axes"]["y"]
    assert band["min"] < 200.0 < 900.0 < band["max"], "the curve's own band"
    assert widget["duration"] == pytest.approx(2.0), "and its own span"


def test_the_axis_is_held_rather_than_refitted_under_the_hand():
    """The defect a hand at the window notices first: an axis recomputed on
    every redraw moves the picture while a point is being dragged, so every
    *other* point visibly moves with it. It only ever grows."""
    curve = a_curve()
    editor = edit(curve, sample_rate=SR, open=False)
    first = editor.draw()["children"][0]
    band = first["axes"]["y"]

    # A point dragged down and back up must leave the drawing where it was.
    curve.set_points([0.0, 500.0, 2, 0.0, 2.0, 600.0, 1, 0.0])
    again = editor.draw()["children"][0]
    assert again["axes"]["y"] == band
    assert again["duration"] == first["duration"]

    # And a curve that no longer fits inside it widens the end that stopped
    # holding it, keeping the other.
    curve.set_points([0.0, 200.0, 2, 0.0, 4.0, 9000.0, 1, 0.0])
    wider = editor.draw()["children"][0]["axes"]["y"]
    assert wider["max"] > band["max"] and wider["min"] == band["min"]
    assert editor.draw()["children"][0]["duration"] == pytest.approx(4.0)
