"""Editing a **multitrack**: the picture, the report and the history.

`clausters.gui.editing.MultitrackEditor` is the multitrack as one of the
fundamental structures — which is what gives it the undo every other editor has.
What is checked here is the seam rather than the mapping: the mapping is the
crate's (`clausters._native.multitrack_props`/`editing_intake`, the same one
the standalone host draws and reads with), so what could still be wrong is this
client's half — the axis a box crosses to, which buffer a source was read into,
and whether a report that means several edits lands as **one** entry.
"""

import pytest

from clausters import _native

from clausters.gui.editing import MultitrackEditor, Sources, edit
from clausters.multitrack import Content, Lane, Multitrack, Region, Tempo, Track

SR = 48_000.0


def window(source: int, start: float = 0.0, duration: float = 2.0) -> Content:
    return Content.onto({"source": {"source": source, "lifetime": "session",
                                    "generation": 0},
                         "start": start, "duration": duration})


def multitrack() -> Multitrack:
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


def editor(multitrack: Multitrack, **options) -> MultitrackEditor:
    """An editor with no window: what is checked here is the seam, and opening
    one would need a host."""
    return MultitrackEditor(multitrack, sample_rate=SR, sources={1: 7}, **options)


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
    ed = editor(multitrack())
    lanes = props(ed)["lanes"]
    assert [lanes[0], lanes[7]] == ["10", "20"], "named by their track ids"
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
    time: seven rows named `10`, `one`, `96.0`, `False`, `False`, `1.0`,
    `False`.

    Found 2026-09-09 reading the two clients against each other, which is the
    only place it was visible: every test here read `view.props` and none read
    the tree that is actually published.
    """
    ed = editor(multitrack())
    drawn = next(c for c in ed.draw()["children"] if c["type"] == "multitrack")
    assert len(drawn["lanes"]) == 2 * 7, "two tracks, seven numbers each"
    assert drawn["lanes"][:3] == ["10", "one", 96.0]
    assert len(drawn["clips"]) == 3 * 7, "three regions, seven numbers each"
    assert drawn["clips"][:2] == ["12", "10"]


def test_the_piece_is_ruled_from_above_by_a_strip_of_its_own():
    """An editor is where a position is read, and the widget draws no ruler —
    so the view places one above it, on the multitrack's own axis.

    The two have to be in **one navigation group**: an unlinked widget is a
    group of one keyed by itself, so a ruler that joined nothing would pan and
    zoom away from the lanes it is ruling.
    """
    children = editor(multitrack()).draw()["children"]
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
    ed = editor(multitrack())
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
    # And a multitrack opens with the reader at the top: the cursor is stated, so
    # there is somewhere to play from before anything is clicked.
    fresh = editor(multitrack())
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

    ed = MultitrackEditor(multitrack(), sample_rate=SR, sources={1: Held()})
    assert ed.bridge.sources.bufnum(1) == 0
    assert all(b[6] == 0 for b in clips(ed)), "the boxes over it name it"
    assert ed.bridge.sources.bufnum(9) == -1, "and a source nobody loaded is none"


def test_a_source_nobody_loaded_draws_an_empty_box():
    ed = MultitrackEditor(multitrack(), sample_rate=SR, sources={})
    assert all(b[6] == -1 for b in clips(ed)), \
        "negative and not zero: buffer 0 is a buffer"


def test_a_tempo_moves_no_box():
    """The multitrack is in seconds: a box is drawn at its seconds times the
    rate, and a tempo the multitrack holds is a ruler's to read."""
    held = multitrack()
    held.set_tempo(Tempo(at=0.0, tempo=1.0))
    held.set_tempo(Tempo(at=4.0, tempo=0.5))
    boxes = clips(editor(held))
    at_four = next(b for b in boxes if b[0] == "13")
    assert at_four[2] == pytest.approx(4.0 * SR), "four seconds in"
    assert at_four[3] == pytest.approx(2.0 * SR), "two seconds long, whatever the tempo"


# ---- the report, and the history ----

def test_a_move_reaches_the_piece_and_undoes():
    held = multitrack()
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
    """A report is the multitrack, so one message can mean several edits — and they
    are one thing a hand did, so `Ctrl`+`Z` walks back over all of it."""
    held = multitrack()
    ed = editor(held)
    assert report(ed, [("12", "10", 2.0 * SR, 2.0 * SR),
                       ("13", "10", 6.0 * SR, 2.0 * SR),
                       ("22", "20", 0.0, 2.0 * SR)])
    # Read through the multitrack each time: an edit replaces what the multitrack holds,
    # so a reference taken before one is a reference to what it held then.
    at = lambda: [r.position for r in held.track(10).lanes[0].regions]
    assert at() == pytest.approx([2.0, 6.0])
    assert ed.undo()
    assert at() == pytest.approx([0.0, 4.0]), "both back, in one step"


def test_a_clip_that_crossed_changes_track_and_undoes():
    held = multitrack()
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
    held = multitrack()
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
    held = multitrack()
    ed = editor(held)
    assert not report(ed, [("12", "10", 0.0, 2.0 * SR),
                           ("13", "10", 4.0 * SR, 2.0 * SR),
                           ("22", "20", 0.0, 2.0 * SR)])
    assert not ed.undo(), "and nothing was recorded to undo"


def test_the_strip_is_the_pieces_and_undoes():
    held = multitrack()
    ed = editor(held)
    ed.draw()
    wid = next(iter(ed.view.widgets))
    assert ed._route([wid, "lanes",
                      "10", "", 96.0, 0, 0, 1.0, 0,
                      "20", "", 96.0, 1, 0, 0.5, 0])
    assert held.track(20).muted
    assert held.track(20).level == pytest.approx(0.5)
    assert not held.track(10).muted, "the one nobody touched is untouched"
    assert ed.undo()
    assert not held.track(20).muted


def test_edit_opens_a_piece():
    """`edit` dispatches on what the structure is, and a multitrack is one of the
    structures it opens now that its picture and its reading are the crate's."""
    ed = edit(multitrack(), sample_rate=SR, open=False, sources={1: 7})
    assert isinstance(ed, MultitrackEditor)


def test_two_windows_over_one_piece_walk_one_stack():
    held = multitrack()
    one, two = editor(held), editor(held)
    assert report(one, [("12", "10", 2.0 * SR, 2.0 * SR),
                        ("13", "10", 4.0 * SR, 2.0 * SR),
                        ("22", "20", 0.0, 2.0 * SR)])
    assert two.undo(), "the history is the data's, not the window's"
    assert held.track(10).lanes[0].regions[0].position == pytest.approx(0.0)


# ---- the curves: the light views, in the two places one lives ----

def curved() -> Multitrack:
    """The same multitrack, with a track automation on the first track and an
    envelope inside its first box."""
    from clausters.multitrack import Automation

    written = multitrack()
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
    """Which curves a person had open is part of reopening the multitrack as they
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
    and it is opened on the **multitrack's** editing context, so one undo order
    walks both."""
    take = FakeTake(7)
    written = multitrack()
    ed = MultitrackEditor(written, sample_rate=SR, sources={1: take})
    ed.draw()
    wid = next(iter(ed.view.widgets))

    assert ed._route([wid, "enter", "12"]) is False, "entering is not an edit"
    opened = ed.entered["12"]
    assert opened is not None
    assert opened._editing is ed._editing, "one undo order, and it is the multitrack's"

    # A second double click on the same box raises the one already open.
    ed._route([wid, "enter", "12"])
    assert ed.entered["12"] is opened


class TakeServer:
    """The write a mono take's editor sends, laid into a `Take`, and the
    ``/done`` it is answered with."""

    def __init__(self, take):
        self.take = take

    def _bulk_chunk(self, timeout=None) -> int:
        return 8192

    def send_msg(self, addr, *args):
        import struct

        assert addr == "/buffer_setRange", addr
        values = struct.unpack(f"<{len(args[-1]) // 4}f", args[-1])
        for i, v in enumerate(values):
            if int(args[1]) + i < len(self.take.frames):
                self.take.frames[int(args[1]) + i] = float(v)

    def request(self, addr, *args, expect=None, timeout=None):
        self.send_msg(addr, *args)
        return "/done", [addr, int(args[0])]


class Take(FakeTake):
    """A take the samples editor can write a stroke into — `FakeTake` plus the
    server its writes go to."""

    def __init__(self, bufnum: int = 7, n: int = 16):
        super().__init__(bufnum)
        self.frames = [0.0] * n
        self.server = TakeServer(self)

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
    either the samples path or a nested multitrack, which is what rules the client's
    registration and the samples domain's inverse out of it.
    """
    take = Take()
    written = multitrack()
    ed = MultitrackEditor(written, sample_rate=SR, sources={1: take})
    ed.draw()
    wid = next(iter(ed.view.widgets))
    # One edit on the multitrack, so the pile has something else on it.
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
    assert _region_at(written, 12).position == moved, "the multitrack did not move"


def test_a_box_closed_does_not_block_the_piece_s_undo():
    """**The pile's scope is the context's, not a window's.**

    An entry names a structure, and what puts an edit back onto one is its
    *vocabulary* — neither of which is on screen. When the applier was a **view**
    instead, a box entered from a multitrack and then closed left an entry nobody
    could apply: the step was refused, and since a refused step puts the cursor
    back, the very next undo hit the same entry. The pile was not missing one
    step, it was **blocked** — every edit the multitrack had made behind that entry
    was unreachable until the box was opened again.

    Found by use 2026-09-12, by hand, in `examples/editors/edit_multitrack.py`.
    The earlier reading of it (2026-09-10) is the one this replaces: the refusal
    was correct given a view-shaped participant, and the participant was wrong.
    """
    take = Take()
    written = multitrack()
    ed = MultitrackEditor(written, sample_rate=SR, sources={1: take})
    ed.draw()
    wid = next(iter(ed.view.widgets))
    assert ed.apply("/gui_event", [wid, 0, 0, "clips",
                                   "12", "10", 1.0 * SR, 2.0 * SR, 0.0, "", 7])
    moved = _region_at(written, 12).position
    ed._route([wid, "enter", "12"])
    box = ed.entered["12"]
    box.draw()
    bwid = next(iter(box.view.widgets))
    assert box.apply("/gui_event", [bwid, 0, 0, "draw", 0, 2, [1.0, 1.0], [0.0, 0.0]])

    # The box's window goes, and its editor with it.
    box.close()
    ed.entered.pop("12", None)

    # The stroke is still the top of the pile, and an undo in the **multitrack's**
    # window performs it: the take and its vocabulary are registered here, and
    # neither went with the window.
    assert ed.undo() is True, "the multitrack can put back an edit made inside a box"
    assert take.frames[2:4] == [0.0, 0.0], "and it is the stroke that was undone"
    assert ed.app.refusal is None

    # ...and the order keeps going, which is the half that was actually broken:
    # a refused step put the cursor back, so everything behind it was walled off.
    assert ed.undo() is True, "the entry behind it is reachable"
    assert _region_at(written, 12).position != moved, "the box went back"

    # Both come forward again, in order.
    assert ed.redo() is True and _region_at(written, 12).position == moved
    assert ed.redo() is True and take.frames[2:4] == [1.0, 1.0]


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
    ed = editor(multitrack())
    assert ed.enter("12") is None, "the source is a bare buffer number"
    assert ed.enter("nowhere") is None


def test_the_windows_entered_from_a_piece_close_with_it():
    """A window entered *from* the multitrack is part of looking at the multitrack. What
    outlives both is the history, which is the data's and was never a
    window's."""
    take = FakeTake(7)
    ed = MultitrackEditor(multitrack(), sample_rate=SR, sources={1: take})
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
    ed = editor(multitrack())
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

    written = multitrack()
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
    # box's start, not from the multitrack's.
    ed.draw()
    wid = next(iter(ed.view.widgets))
    edited = list(flat)
    for i in range(0, len(edited), 5):
        if edited[i] == "40" and edited[i + 1] == 0.0:
            edited[i + 2] = 0.25
    assert ed._route([wid, "points", *edited])
    # The multitrack is re-read on an edit, so the region is looked up again.
    fade = written.tracks[1].lanes[0].regions[0].automation[0]
    assert fade.points[0]["at"] == pytest.approx(0.0)
    assert fade.points[0]["value"] == pytest.approx(0.25)
    assert fade.points[1]["at"] == pytest.approx(2.0)


# ---- what the multitrack is heard as ----

def _plan(ed) -> dict:
    """The instance plan for an editor's multitrack, the way `Playback` asks for it.

    The plan itself is the crate's and is tested there; what these check is the
    **crossing** — that this client hands it the multitrack, the axis and the source
    table it actually holds, which is the half a client can get wrong on its
    own.
    """
    from clausters import _native

    return _native.multitrack_plan(ed.structure.write(), ed.bridge.rate,
                                   ed.bridge.sources.table())


def test_a_box_is_planned_in_frames_from_where_its_window_opens():
    """The crossing from the multitrack to the readers: a box is placed in seconds and
    read in frames, and a trimmed one reads on rather than restarting."""
    ed = editor(multitrack())
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
    source nobody loaded is not planned at all — the second is a multitrack that
    arrived without its takes, which is not the same as a silent one."""
    ed = editor(multitrack())
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
    ed = editor(multitrack())
    table = ed.bridge.sources.table()
    assert table[1]["buffer"] == 7
    assert table[1]["channels"] >= 1
    assert 9 not in table, "a source nobody loaded is not in it"
    assert _plan(ed)["tracks"][0]["clips"][0]["slot"].startswith("clips.")


def test_the_mixer_rules_reach_the_plan_and_a_solo_silences_the_rest():
    """The document holds the flags and never reads them: what a track
    contributes is the mixer's rule, and the mixer is in the crate — so both
    clients get the same answer instead of each writing one."""
    ed = editor(multitrack())
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


def test_rewind_puts_the_cursor_back_at_the_top():
    """The cursor's own verb. Stop goes back to the **mark** -- which is what
    tells it from pause -- so with nothing else the way back to the top is
    finding beat zero on screen and clicking it."""
    ed = editor(multitrack())
    ed.cursor = 12.0
    ed.rewind()
    assert ed.cursor == 0.0


class _AdoptingHost:
    """A host that adopts what it is told, the way the real one does: it keeps
    the names of the last picture pushed to it."""

    def __init__(self):
        self.names = None
        self.rows = None
        self.pushes = 0

    def push(self, seq, *corrections, doc_version=0, reason=None):
        self.pushes += 1
        for _wid, props in corrections:
            if "clips" in props:
                self.names = list(props["clips"][::7])
            if "lanes" in props:
                self.rows = list(props["lanes"][::7])

    def ack(self, seq, doc_version=0, reason=None):
        pass

    def set(self, wid, **props):
        pass


def _wired(ed) -> _AdoptingHost:
    """An editor answering a host, without opening a window."""
    host = _AdoptingHost()
    # Through the application, which is what an `open` would do: it is the
    # window set that adopts a host, and it hands it to each editor's echo.
    ed._host = host
    ed._window = 1
    ed.draw()
    return host


def test_a_name_the_host_minted_is_answered_with_the_one_the_piece_kept():
    """The host makes a **track** from a double click and a **box** from a
    split, and in both it mints the word while the document mints the id.

    Until the picture goes back the two are naming the same thing differently,
    and a name the multitrack does not know is not ignored -- it is read as
    something *new*. So the next report about that box minted it again, and
    again after that: a split box took a fresh id on every drag, losing
    whatever was hung on it, and a box dropped on a track the host had just
    made landed on a track nobody had.
    """
    held = multitrack()
    ed = editor(held)
    host = _wired(ed)
    wid = next(iter(ed.view.widgets))

    def ids():
        return [r.id for t in held.tracks for lane in t.lanes for r in lane.regions]

    def boxes():
        flat = props(ed)["clips"]
        return [flat[i:i + 7] for i in range(0, len(flat), 7)]

    # A split, reported as the host reports one: the second half under a name
    # the host minted and the client never said.
    payload = []
    for box in boxes():
        if box[0] == "12":
            first = list(box)
            first[3] = 1.0 * SR
            payload += first
            payload += ["white 2", box[1], 1.0 * SR, 1.0 * SR, 1.0 * SR, "", box[6]]
        else:
            payload += list(box)
    ed.apply("/gui_event", [wid, 1, ed._version, "clips", *payload])
    split = ids()
    assert len(split) == 4, "the split landed"
    assert host.pushes == 1, "and the picture went back with it"
    assert host.names == [str(i) for i in split], "under the ids the multitrack kept"

    # The next gesture, reported with the names the host was just given: the
    # boxes are the same boxes.
    payload = []
    for name, box in zip(host.names, boxes()):
        row = list(box)
        row[0] = name
        row[2] = float(row[2]) + 1000.0
        payload += row
    ed.apply("/gui_event", [wid, 2, ed._version, "clips", *payload])
    assert ids() == split, "nothing was minted a second time"

    # And a track made in the host: the same rule, and the `meters` prop rides
    # with it -- a track that reached the server has buses to read.
    rows = list(props(ed)["lanes"]) + ["track 1", "three", 96.0, 0, 0, 1.0, 0]
    ed.apply("/gui_event", [wid, 3, ed._version, "lanes", *rows])
    assert host.rows == [str(t.id) for t in held.tracks]
    assert len(held.tracks) == 3


def test_a_track_made_in_the_host_is_a_track_in_the_plan():
    """A double click on a header makes a track, and what the host sends back
    is the rows as they now stand. The claim here is the other half: what the
    host adds is added on the **server** too, empty or not.

    A track with nothing on it is still a strip, a fader and a meter -- it is
    where the next box will land, and a multitrack that only instantiated the tracks
    that happened to have boxes would build them at the moment a box was
    dropped, which is the one moment it must not.
    """
    ed = editor(multitrack())
    ed.draw()
    wid = next(iter(ed.view.widgets))
    rows = list(props(ed)["lanes"]) + ["0", "three", 96.0, 0, 0, 1.0, 0]
    assert ed._route([wid, "lanes", *rows])
    planned = _plan(ed)["tracks"]
    assert [t["track"] for t in planned] == [10, 20, 23]
    assert planned[2]["clips"] == [], "and it is planned empty rather than left out"


def test_a_metered_track_names_the_buses_the_host_reads():
    """The meters are the playback's and the strip is the host's, so what the
    widget carries is *where to look*: a lane, the level run and the mark run,
    and how many channels each is.

    The host reads those buses itself every frame, which is why a level that
    moves every block costs no message at all.
    """
    class FakePlayback:
        meters = {10: (40, 2)}

    ed = editor(multitrack())
    assert props(ed)["meters"] == [], "a multitrack nobody plays has no meters"
    ed.playback = FakePlayback()
    assert props(ed)["meters"] == ["10", 40, 42, 2]


def test_the_playback_sends_the_crate_s_steps_and_waits_where_they_say():
    """**What is left in a client is a socket, and waiting on it.**

    What a multitrack needs, the messages that carry it out and how it is played are
    the crate's (`clausters._native.MultitrackPlayback`), and so is which reply
    releases what (`clausters._native.StepRunner`), tested there because they
    are one implementation for every endpoint. This is the other half: the
    message a step waits on goes out as the request whose reply is handed back,
    a barrier included, and a 64-bit sample goes out as one.
    """
    from clausters.base import _osclib
    from clausters.gui.editing.playback import Playback

    class Server:
        """Answers as a server does: a barrier with its id, a command with its
        own name and the index it was sent with."""

        def __init__(self):
            self.log = []

        def send_msg(self, addr, *args):
            self.log.append(("send", addr) + args)

        def request(self, addr, *args, expect=None, timeout=None):
            self.log.append(("request", addr) + args)
            if addr == "/server_sync":
                return "/server_sync.reply", list(args)
            return "/done", [addr, *args[:1]]

    playback = object.__new__(Playback)
    playback.server = Server()
    playback._runner = _native.StepRunner()
    playback._run([
        {"send": {"addr": "/buffer_alloc", "args": [{"i": 3}, {"i": 2}, {"i": 1}]}},
        {"await": {"command": "/buffer_alloc", "index": 3}},
        {"send": {"addr": "/buffer_setRange",
                  "args": [{"i": 3}, {"i": 0}, {"b": [0.5, 1.0]}]}},
        {"sync": 1},
    ])
    kinds = [entry[:2] for entry in playback.server.log]
    assert kinds == [("request", "/buffer_alloc"), ("send", "/buffer_setRange"),
                     ("request", "/server_sync")], "the fill waits for the allocation"

    multitrack = _native.MultitrackPlayback()
    playback.server.log.clear()
    playback._run(multitrack.locate(2.0))
    (entry,) = playback.server.log
    assert entry[:2] == ("request", "/transport_locateSample")
    assert isinstance(entry[2], _osclib.Int64), "a sample rides as 64 bits"
    assert entry[2].value == multitrack.secs_to_samples(2.0)
