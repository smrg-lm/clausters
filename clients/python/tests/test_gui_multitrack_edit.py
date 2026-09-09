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
