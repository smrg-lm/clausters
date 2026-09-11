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


def test_buffer_zero_is_a_buffer():
    """The first buffer an allocator hands out is a buffer, and a box over it
    draws — `x or -1` said it did not, so the first take a script loaded was
    the one take its boxes could not draw.

    Found by use 2026-09-10, on the box the example loads first.
    """
    class Held:
        bufnum = 0

    ed = MultitrackEditor(piece(), sample_rate=SR, sources={1: Held()})
    assert ed.bridge.sources.bufnum(1) == 0
    assert all(b[6] == 0 for b in clips(ed)), "the boxes over it name it"
    assert ed.bridge.sources.bufnum(9) == -1, "and a source nobody loaded is none"


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
    assert held.track(20).level == pytest.approx(0.5)
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


class Take(FakeTake):
    """A take the samples editor can write a stroke into — `FakeTake` plus the
    read-back the domain uses to splice one."""

    def __init__(self, bufnum: int = 7, n: int = 16):
        super().__init__(bufnum)
        self.frames = [0.0] * n

    def get_samples(self, start: int = 0, n: "int | None" = None) -> list:
        end = len(self.frames) if n is None else start + n
        return self.frames[start:end]

    def set_samples(self, values, start: int = 0):  # type: ignore[override]
        for i, v in enumerate(values):
            if start + i < len(self.frames):
                self.frames[start + i] = float(v)


def test_a_reopened_box_undoes_its_own_edit_and_not_the_pieces():
    """**One order, and a leg belonging to the take is still the take's.**

    Closing a box's window and opening it again builds a *fresh* editor over
    the same structure; what must not change is which entry an undo in that
    window steps. The identity is the **context's**, minted per structure, so
    the new editor projects the legs the old one recorded.

    Written 2026-09-09 while diagnosing `G35.6` — an undo in a reopened box's
    window stepping the multitrack's last edit. It does not reproduce here, on
    either the samples path or a nested piece, which is what rules the client's
    registration and the samples domain's inverse out of it.
    """
    take = Take()
    written = piece()
    ed = MultitrackEditor(written, sample_rate=SR, sources={1: take})
    ed.draw()
    wid = next(iter(ed.view.widgets))
    # One edit on the piece, so the pile has something else on it.
    assert ed.apply("/gui_event", [wid, 0, 0, "clips",
                                   "12", "10", 1.0 * SR, 2.0 * SR, 0.0, "", 7])
    moved = _region_at(written, 12).position

    # A stroke inside the box.
    ed._route([wid, "enter", "12"])
    box = ed.entered["12"]
    box.draw()
    bwid = next(iter(box.view.widgets))
    assert box.apply("/gui_event", [bwid, 0, 0, "draw", 0, 2, [1.0, 1.0], [0.0, 0.0]])
    assert take.frames[2:4] == [1.0, 1.0]

    # Closed and entered again: a different editor over the same take.
    box.close()
    ed.entered.pop("12", None)
    ed._route([wid, "enter", "12"])
    again = ed.entered["12"]
    assert again is not box
    again.draw()

    assert again.undo(), "the stroke is what the pile has on top"
    assert take.frames[2:4] == [0.0, 0.0], "and it is the stroke that is undone"
    assert _region_at(written, 12).position == moved, "the piece did not move"


def test_an_undo_nobody_can_apply_is_not_a_step():
    """**A step nobody could apply is not a step.**

    The walk moves the pile's cursor before anything is projected, so an entry
    naming a structure no participant holds — a box whose window was closed —
    was stepped *over*: the edit stayed and the order lost it. Closing the box,
    undoing in the piece's window and opening it again left the samples edited
    and unreachable, which is the one thing a history may not do.

    Found by use 2026-09-10.
    """
    take = Take()
    written = piece()
    ed = MultitrackEditor(written, sample_rate=SR, sources={1: take})
    ed.draw()
    wid = next(iter(ed.view.widgets))
    assert ed.apply("/gui_event", [wid, 0, 0, "clips",
                                   "12", "10", 1.0 * SR, 2.0 * SR, 0.0, "", 7])
    ed._route([wid, "enter", "12"])
    box = ed.entered["12"]
    box.draw()
    bwid = next(iter(box.view.widgets))
    assert box.apply("/gui_event", [bwid, 0, 0, "draw", 0, 2, [1.0, 1.0], [0.0, 0.0]])

    # The box's window goes, and its editor with it.
    box.close()
    ed.entered.pop("12", None)

    # An undo in the piece's window now names the stroke, which nothing here
    # can write: the pile does not move, and it says why.
    assert ed.undo() is False, "nothing could apply it"
    assert take.frames[2:4] == [1.0, 1.0], "and the edit is still there"
    assert ed.app.unreachable == "draw the samples"

    # Which is what makes it recoverable: the entry is still on top, waiting
    # for the window that can perform it.
    ed._route([wid, "enter", "12"])
    assert ed.entered["12"].undo(), "the stroke is still the top of the pile"
    assert take.frames[2:4] == [0.0, 0.0]


def _region_at(held, region_id: int):
    for track in held.tracks:
        for lane in track.lanes:
            for region in lane.regions:
                if region.id == region_id:
                    return region
    raise AssertionError(f"no region {region_id}")


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


# ---- what the piece is heard as ----

def _plan(ed) -> dict:
    """The instance plan for an editor's piece, the way `Playback` asks for it.

    The plan itself is the crate's and is tested there; what these check is the
    **crossing** — that this client hands it the piece, the axis and the source
    table it actually holds, which is the half a client can get wrong on its
    own.
    """
    from clausters import _native

    return _native.multitrack_plan(ed.structure.write(), ed.bridge.rate,
                                   ed.bridge.bpm, ed.bridge.sources.table())


def test_a_box_is_planned_in_frames_from_where_its_window_opens():
    """The crossing from the piece to the readers: a box is placed in beats and
    read in frames, and a trimmed one reads on rather than restarting."""
    ed = editor(piece())
    region = ed.structure.tracks[0].lanes[0].regions[1]
    region.content = window(1, start=0.5, duration=2.0)
    reader = _plan(ed)["tracks"][0]["clips"][1]["readers"][0]
    assert reader["buffer"] == 7, "the buffer the source was read into"
    assert reader["at"] == pytest.approx(4.0 * SR), "a beat is a second here"
    assert reader["span"] == pytest.approx(2.0 * SR)
    assert reader["start"] == pytest.approx(0.5 * SR), "where the window opens"
    assert reader["looping"] is False


def test_a_muted_box_and_an_unloaded_source_are_not_read():
    """Two different answers: a muted box is planned at nothing, and a box whose
    source nobody loaded is not planned at all — the second is a piece that
    arrived without its takes, which is not the same as a silent one."""
    ed = editor(piece())
    region = ed.structure.tracks[0].lanes[0].regions[0]
    region.muted = True
    assert _plan(ed)["tracks"][0]["clips"][0]["mute"] == 1.0

    region.muted = False
    region.content = window(9)          # a source the table has no buffer for
    planned = [c["region"] for c in _plan(ed)["tracks"][0]["clips"]]
    assert region.id not in planned


def test_the_source_table_carries_the_width_that_picks_the_wiring():
    """A mono take is panned into its track and a stereo one is balanced, so
    which clip def a box goes in follows from the source's width — and the width
    is the client's to report, since only it loaded the samples."""
    ed = editor(piece())
    table = ed.bridge.sources.table()
    assert table[1]["buffer"] == 7
    assert table[1]["channels"] >= 1
    assert 9 not in table, "a source nobody loaded is not in it"
    assert _plan(ed)["tracks"][0]["clips"][0]["slot"].startswith("clips.")


def test_the_mixer_rules_reach_the_plan_and_a_solo_silences_the_rest():
    """The document holds the flags and never reads them: what a track
    contributes is the mixer's rule, and the mixer is in the crate — so both
    clients get the same answer instead of each writing one."""
    ed = editor(piece())
    p = ed.structure
    one, two = p.tracks
    assert _plan(ed)["tracks"][0]["gain"] == 1.0, "a track that said nothing is at full"
    assert _plan(ed)["tracks"][0]["mute"] == 0.0

    one.config = {"level": 0.25}
    assert _plan(ed)["tracks"][0]["gain"] == pytest.approx(0.25)

    one.muted = True
    assert _plan(ed)["tracks"][0]["mute"] == 1.0
    one.muted = False

    two.soloed = True
    assert _plan(ed)["tracks"][0]["mute"] == 1.0, "another track is soloed"
    assert _plan(ed)["tracks"][1]["mute"] == 0.0
