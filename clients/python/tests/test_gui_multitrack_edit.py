"""Editing a **piece**: the picture, the report and the history.

`clausters.gui.editing.MultitrackEditor` is the multitrack as one of the
fundamental structures — which is what gives it the undo every other editor has.
What is checked here is the seam rather than the mapping: the mapping is the
crate's (`clausters._native.multitrack_picture`/`multitrack_read`, the same one
the standalone host draws and reads with), so what could still be wrong is this
client's half — the axis a box crosses to, which buffer a source was read into,
and whether a report that means several edits lands as **one** entry.
"""

import pytest

from clausters.gui.editing import MultitrackEditor, Sources, edit
from clausters.multitrack import Content, Lane, Multitrack, Region, Tempo, Track

SR = 48_000.0


def window(source: int, start: float = 0.0, duration: float = 2.0) -> Content:
    return Content.onto({"source": {"source": source, "lifetime": "session",
                                    "generation": 0},
                         "start": start, "duration": duration})


def piece() -> Multitrack:
    """Two tracks: the first holding two regions, the second one."""
    first = Track(id=10, name="one",
                  lanes=[Lane(id=11, regions=[
                      Region(id=12, position=0.0, length=2.0,
                             content=window(1)),
                      Region(id=13, position=4.0, length=2.0,
                             content=window(1))])])
    second = Track(id=20, name="two",
                   lanes=[Lane(id=21, regions=[
                       Region(id=22, position=0.0, length=2.0,
                              content=window(1))])])
    return Multitrack(tracks=[first, second])


def editor(piece: Multitrack, **options) -> MultitrackEditor:
    """An editor with no window: what is checked here is the seam, and opening
    one would need a host."""
    return MultitrackEditor(piece, sample_rate=SR, sources={1: 7}, **options)


def props(ed: MultitrackEditor) -> dict:
    """What the widget is told to draw, without opening a window."""
    ed.draw()
    wid = next(iter(ed.view.widgets))
    return ed.view.props(ed, wid)


def clips(ed: MultitrackEditor) -> list:
    flat = props(ed)["clips"]
    return [flat[i:i + 7] for i in range(0, len(flat), 7)]


def report(ed: MultitrackEditor, boxes) -> bool:
    """One `"clips"` report, as the widget would send it."""
    ed.draw()
    wid = next(iter(ed.view.widgets))
    values = []
    for name, row, at, dur in boxes:
        values += [name, row, at, dur, 0.0, "", 7]
    return ed._route([wid, "clips", *values])


# ---- the picture ----

def test_a_row_per_track_and_a_box_per_region():
    ed = editor(piece())
    lanes = props(ed)["lanes"]
    assert [lanes[0], lanes[6]] == ["10", "20"], "named by their track ids"
    assert lanes[1] == "one"
    boxes = clips(ed)
    assert [b[0] for b in boxes] == ["12", "13", "22"]
    assert [b[1] for b in boxes] == ["10", "10", "20"]
    # A beat is a second at the reader's default, so a region at beat 4 is at
    # four seconds' worth of frames.
    assert boxes[1][2] == pytest.approx(4.0 * SR)
    assert boxes[1][3] == pytest.approx(2.0 * SR)
    assert boxes[0][6] == 7, "the buffer its source was read into"


def test_the_widget_is_told_the_flat_rows_and_not_one_row_per_number():
    """The props are already the wire's, so the node is made from them rather
    than through the `multitrack` builder — whose `lanes`/`clips` are the
    *tuples* a script types, and which flattened an already-flat list a second
    time: six rows named `10`, `one`, `96.0`, `False`, `False`, `1.0`.

    Found 2026-09-09 reading the two clients against each other, which is the
    only place it was visible: every test here read `view.props` and none read
    the tree that is actually published.
    """
    ed = editor(piece())
    drawn = next(c for c in ed.draw()["children"] if c["type"] == "multitrack")
    assert len(drawn["lanes"]) == 2 * 6, "two tracks, six numbers each"
    assert drawn["lanes"][:3] == ["10", "one", 96.0]
    assert len(drawn["clips"]) == 3 * 7, "three regions, seven numbers each"
    assert drawn["clips"][:2] == ["12", "10"]


def test_the_piece_is_ruled_from_above_by_a_strip_of_its_own():
    """An editor is where a position is read, and the widget draws no ruler —
    so the view places one above it, on the piece's own axis.

    The two have to be in **one navigation group**: an unlinked widget is a
    group of one keyed by itself, so a ruler that joined nothing would pan and
    zoom away from the lanes it is ruling.
    """
    children = editor(piece()).draw()["children"]
    ruler, piece_node = children[0], children[1]
    assert ruler["type"] == "field", "the free-standing time ruler, above"
    assert piece_node["type"] == "multitrack"
    assert ruler["axes"]["x"]["link"] == piece_node["link"], "one axis, not two"
    assert ruler["axes"]["x"]["unit"] == "beats"


def test_the_position_cursor_is_kept_and_told_and_is_not_an_edit():
    """A click on the time ruler places the **position cursor**, and what
    arrives is `"locate"` with where it landed.

    It is where a playback starts and where a paste lands, so the editor keeps
    it -- the playhead is where the *music* is and moves on its own, and an
    anchor that moved on its own would not be an anchor. It is not an edit and
    reaches no history.
    """
    ed = editor(piece())
    ed.draw()
    # **It arrives on the ruler**, which is where it is placed and nowhere else
    # — so the strip is a named widget of this picture like any other, or the
    # one gesture that places the cursor would land outside the only object
    # that could hear it.
    rid = ed.view.ruler
    assert ed._owns(rid), "the ruler is this view's"
    told = []
    ed.on_locate = told.append
    assert ed._route([rid, "locate", 4.0 * SR]) is False, "placing is not an edit"
    assert ed.cursor == pytest.approx(4.0), "in the editor's units, which are beats"
    assert told == [pytest.approx(4.0)]
    # And a piece opens with the reader at the top: the cursor is stated, so
    # there is somewhere to play from before anything is clicked.
    fresh = editor(piece())
    fresh.draw()
    assert fresh.view.props(fresh, fresh.view.ruler)["cursor"] == 0.0
    # Once placed it is reported from the editor's own copy, so a resync does
    # not drag the mark back to the start.
    assert ed.view.props(ed, ed.view.ruler)["cursor"] == pytest.approx(4.0 * SR)


def test_a_source_nobody_loaded_draws_an_empty_box():
    ed = MultitrackEditor(piece(), sample_rate=SR, sources={})
    assert all(b[6] == -1 for b in clips(ed)), \
        "negative and not zero: buffer 0 is a buffer"


def test_the_piece_is_placed_through_its_own_tempo_map():
    """Four beats are not one length: under a tempo that changes they last
    longer later than earlier, and the picture has to say so."""
    held = piece()
    held.set_tempo(Tempo(at=0.0, bpm=60.0))
    held.set_tempo(Tempo(at=4.0, bpm=30.0))
    boxes = clips(editor(held))
    at_four = next(b for b in boxes if b[0] == "13")
    assert at_four[2] == pytest.approx(4.0 * SR), "four beats at a beat a second"
    assert at_four[3] == pytest.approx(4.0 * SR), \
        "two beats at half the tempo are four seconds"


# ---- the report, and the history ----

def test_a_move_reaches_the_piece_and_undoes():
    held = piece()
    ed = editor(held)
    assert report(ed, [("12", "10", 2.0 * SR, 2.0 * SR),
                       ("13", "10", 4.0 * SR, 2.0 * SR),
                       ("22", "20", 0.0, 2.0 * SR)])
    assert held.track(10).lanes[0].regions[0].position == pytest.approx(2.0)
    assert ed.undo()
    assert held.track(10).lanes[0].regions[0].position == pytest.approx(0.0)
    assert ed.redo()
    assert held.track(10).lanes[0].regions[0].position == pytest.approx(2.0)


def test_a_block_move_is_one_entry():
    """A report is the piece, so one message can mean several edits — and they
    are one thing a hand did, so `Ctrl`+`Z` walks back over all of it."""
    held = piece()
    ed = editor(held)
    assert report(ed, [("12", "10", 2.0 * SR, 2.0 * SR),
                       ("13", "10", 6.0 * SR, 2.0 * SR),
                       ("22", "20", 0.0, 2.0 * SR)])
    # Read through the piece each time: an edit replaces what the piece holds,
    # so a reference taken before one is a reference to what it held then.
    at = lambda: [r.position for r in held.track(10).lanes[0].regions]
    assert at() == pytest.approx([2.0, 6.0])
    assert ed.undo()
    assert at() == pytest.approx([0.0, 4.0]), "both back, in one step"


def test_a_clip_that_crossed_changes_track_and_undoes():
    held = piece()
    ed = editor(held)
    assert report(ed, [("12", "20", 0.0, 2.0 * SR),
                       ("13", "10", 4.0 * SR, 2.0 * SR),
                       ("22", "20", 0.0, 2.0 * SR)])
    assert any(r.id == 12 for r in held.track(20).lanes[0].regions)
    assert ed.undo()
    assert any(r.id == 12 for r in held.track(10).lanes[0].regions)


def test_a_box_the_piece_does_not_know_becomes_a_region():
    """A split names its halves after the box they came from, which is no
    region id — and that is how a new box is told from a moved one."""
    held = piece()
    ed = editor(held)
    assert report(ed, [("12", "10", 0.0, 1.0 * SR),
                       ("12 2", "10", 1.0 * SR, 1.0 * SR),
                       ("13", "10", 4.0 * SR, 2.0 * SR),
                       ("22", "20", 0.0, 2.0 * SR)])
    lane = held.track(10).lanes[0]
    assert len(lane.regions) == 3, "the two that stayed and the new one"
    assert {r.id for r in lane.regions} > {12, 13}, "it took an unused id"
    del lane
    assert ed.undo()
    assert len(held.track(10).lanes[0].regions) == 2


def test_a_report_of_what_holds_is_not_an_edit():
    held = piece()
    ed = editor(held)
    assert not report(ed, [("12", "10", 0.0, 2.0 * SR),
                           ("13", "10", 4.0 * SR, 2.0 * SR),
                           ("22", "20", 0.0, 2.0 * SR)])
    assert not ed.undo(), "and nothing was recorded to undo"


def test_the_strip_is_the_pieces_and_undoes():
    held = piece()
    ed = editor(held)
    ed.draw()
    wid = next(iter(ed.view.widgets))
    assert ed._route([wid, "lanes",
                      "10", "", 96.0, 0, 0, 1.0,
                      "20", "", 96.0, 1, 0, 0.5])
    assert held.track(20).muted
    assert held.track(20).config["level"] == pytest.approx(0.5)
    assert not held.track(10).muted, "the one nobody touched is untouched"
    assert ed.undo()
    assert not held.track(20).muted


def test_edit_opens_a_piece():
    """`edit` dispatches on what the structure is, and a piece is one of the
    structures it opens now that its picture and its reading are the crate's."""
    ed = edit(piece(), sample_rate=SR, open=False, sources={1: 7})
    assert isinstance(ed, MultitrackEditor)


def test_two_windows_over_one_piece_walk_one_stack():
    held = piece()
    one, two = editor(held), editor(held)
    assert report(one, [("12", "10", 2.0 * SR, 2.0 * SR),
                        ("13", "10", 4.0 * SR, 2.0 * SR),
                        ("22", "20", 0.0, 2.0 * SR)])
    assert two.undo(), "the history is the data's, not the window's"
    assert held.track(10).lanes[0].regions[0].position == pytest.approx(0.0)


# ---- the curves: the light views, in the two places one lives ----

def curved() -> Multitrack:
    """The same piece, with a track automation on the first track and an
    envelope inside its first box."""
    from clausters.multitrack import Automation

    written = piece()
    written.tracks[0].automation.append(
        Automation(id=30, name="gain", target={"ctl": "gain", "max": 2.0},
                   points=[{"at": 0.0, "value": 1.0},
                           {"at": 4.0, "value": 0.0,
                            "data": {"shape": 5, "curve": 4.0}}],
                   visible=True))
    written.tracks[0].lanes[0].regions[0].automation.append(
        Automation(id=31, name="env", target={"ctl": "amp"},
                   points=[{"at": 0.0, "value": 0.0}], visible=True))
    return written


def test_a_track_curve_is_a_row_and_a_box_curve_is_a_layer():
    """The same curve in two places, and the place is the whole difference: a
    track's runs the timeline under its row, a region's is drawn inside its
    box. So they reach the widget as two props, not one with a flag."""
    ed = editor(curved())
    drawn = props(ed)
    assert drawn["curves"][:3] == ["30", "10", "gain"], "the track it is under"
    assert drawn["curves"][4] == 2.0, "the domain, read out of the target"
    assert len(drawn["curves"]) == 6, "one row, six numbers"
    assert drawn["layers"][:3] == ["31", "12", "env"], "the box it is inside"
    assert len(drawn["layers"]) == 5, "a layer states no height"


def test_every_curve_s_points_travel_in_one_list_on_this_window_s_axis():
    ed = editor(curved())
    flat = props(ed)["points"]
    points = [flat[i:i + 5] for i in range(0, len(flat), 5)]
    assert [p[0] for p in points] == ["30", "30", "31"]
    # A beat is a second at the reader's default, so the second break-point of
    # `gain` is at four seconds' worth of frames.
    assert points[1][1] == pytest.approx(4.0 * SR)
    assert points[1][3] == 5.0 and points[1][4] == 4.0, "the shape is carried"


def test_a_point_dragged_is_one_edit_and_the_curve_that_did_not_move_is_not():
    """The report is every curve there is, so what it means is the difference —
    and an undo puts the shape back, since the crate carries a point's data
    without reading it."""
    written = curved()
    ed = editor(written)
    ed.draw()
    wid = next(iter(ed.view.widgets))
    flat = list(props(ed)["points"])
    flat[2] = 0.25   # the first point of `gain`, moved
    assert ed._route([wid, "points", *flat])
    gain = next(a for a in written.tracks[0].automation if a.id == 30)
    assert gain.points[0]["value"] == pytest.approx(0.25)
    assert gain.points[1]["data"] == {"shape": 5, "curve": 4.0}
    env = written.tracks[0].lanes[0].regions[0].automation[0]
    assert env.points[0]["value"] == 0.0, "the curve nobody touched"

    assert ed.undo()
    gain = next(a for a in written.tracks[0].automation if a.id == 30)
    assert gain.points[0]["value"] == pytest.approx(1.0)


def test_a_curve_the_piece_hid_is_drawn_nowhere():
    """Which curves a person had open is part of reopening the piece as they
    left it, so it is read out of the document rather than kept in the view."""
    written = curved()
    written.tracks[0].automation[0].visible = False
    assert props(editor(written))["hidden"] == "30"


# ---- entering a box ----

class FakeTake:
    """The smallest thing `edit` opens as a take: a buffer number and samples
    it can write back."""

    def __init__(self, bufnum: int):
        self.bufnum = bufnum
        self.frames = [0.0, 0.5, 1.0]
        self.rate = SR
        self.channels = 1
        self.frames_count = len(self.frames)
        self.name = "take"

    def to_samples(self):
        return list(self.frames)

    def set_samples(self, samples):
        self.frames = list(samples)


def test_entering_a_box_opens_its_contents_on_the_piece_s_history():
    """The multitrack places; a box is entered to edit. What a box holds is a
    structure like any other, so entering one is `edit` over that structure —
    and it is opened on the **piece's** editing context, so one undo order
    walks both."""
    take = FakeTake(7)
    written = piece()
    ed = MultitrackEditor(written, sample_rate=SR, sources={1: take})
    ed.draw()
    wid = next(iter(ed.view.widgets))

    assert ed._route([wid, "enter", "12"]) is False, "entering is not an edit"
    opened = ed.entered["12"]
    assert opened is not None
    assert opened._editing is ed._editing, "one undo order, and it is the piece's"

    # A second double click on the same box raises the one already open.
    ed._route([wid, "enter", "12"])
    assert ed.entered["12"] is opened


def test_a_box_with_nothing_to_open_opens_nothing():
    """A source named by number alone is a box the caller gave no structure
    for, and a name no region has is no box at all."""
    ed = editor(piece())
    assert ed.enter("12") is None, "the source is a bare buffer number"
    assert ed.enter("nowhere") is None


def test_the_windows_entered_from_a_piece_close_with_it():
    """A window entered *from* the piece is part of looking at the piece. What
    outlives both is the history, which is the data's and was never a
    window's."""
    take = FakeTake(7)
    ed = MultitrackEditor(piece(), sample_rate=SR, sources={1: take})
    ed.draw()
    wid = next(iter(ed.view.widgets))
    ed._route([wid, "enter", "12"])
    opened = ed.entered["12"]
    ed.close()
    assert opened.closed and not ed.entered


def test_a_gesture_that_changed_the_data_says_so_once():
    """The script's door onto an edit: one call per gesture however many edits
    it took, because that is what a hand did — and a window is not exempt from
    being told about its own gesture."""
    ed = editor(piece())
    told = []
    ed.on_change = lambda: told.append(1)

    def gesture(boxes) -> bool:
        """One `/gui_event`, through the door the host uses."""
        ed.draw()
        wid = next(iter(ed.view.widgets))
        values = []
        for name, row, at, dur in boxes:
            values += [name, row, at, dur, 0.0, "", 7]
        return ed.apply("/gui_event", [wid, 0, 0, "clips", *values])

    # A block move: two clips, one gesture.
    assert gesture([("12", "10", 1.0 * SR, 2.0 * SR),
                    ("13", "10", 5.0 * SR, 2.0 * SR),
                    ("22", "20", 0.0, 2.0 * SR)])
    assert len(told) == 1

    assert ed.undo()
    assert len(told) == 2, "a step of the history changed the data too"

    # A report of what already holds is not a change, so nothing is said.
    assert not gesture([("12", "10", 0.0, 2.0 * SR),
                        ("13", "10", 4.0 * SR, 2.0 * SR),
                        ("22", "20", 0.0, 2.0 * SR)])
    assert len(told) == 2


def test_a_layer_s_points_are_its_box_s_own_time():
    """A track automation runs the timeline and is measured from the origin; a
    clip envelope is drawn inside its box and is measured from where that box
    starts. It is the one thing that differs between the two on the wire."""
    from clausters.multitrack import Automation

    written = piece()
    # The box at beat 4 on the second track, with an envelope of its own.
    late = written.tracks[1].lanes[0].regions[0]
    late.position = 4.0
    late.automation.append(
        Automation(id=40, name="fade", visible=True,
                   points=[{"at": 0.0, "value": 0.0},
                           {"at": 2.0, "value": 1.0}]))
    written.tracks[0].automation.append(
        Automation(id=41, name="gain", visible=True,
                   points=[{"at": 4.0, "value": 0.5}]))

    ed = editor(written)
    flat = props(ed)["points"]
    points = {}
    for i in range(0, len(flat), 5):
        points.setdefault(flat[i], []).append(flat[i + 1])
    assert points["40"] == [0.0, pytest.approx(2.0 * SR)], "from the box's start"
    assert points["41"] == [pytest.approx(4.0 * SR)], "from the origin"

    # ...and back: a point dragged inside the box comes back as a beat from the
    # box's start, not from the piece's.
    ed.draw()
    wid = next(iter(ed.view.widgets))
    edited = list(flat)
    for i in range(0, len(edited), 5):
        if edited[i] == "40" and edited[i + 1] == 0.0:
            edited[i + 2] = 0.25
    assert ed._route([wid, "points", *edited])
    # The piece is re-read on an edit, so the region is looked up again.
    fade = written.tracks[1].lanes[0].regions[0].automation[0]
    assert fade.points[0]["at"] == pytest.approx(0.0)
    assert fade.points[0]["value"] == pytest.approx(0.25)
    assert fade.points[1]["at"] == pytest.approx(2.0)
