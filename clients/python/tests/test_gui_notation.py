"""The notation client: the engraved layers and the score widget's props.

`engrave` needs the engraver, which normally ships inside the package; a source
checkout that has not built it yet skips those tests, while the `guidef.score`
builder is pure and always runs. The editing tests skip one step further — on a
verovio whose editor is dead, which the released wheel's is.

Both guards ask `notation` itself rather than importing ``verovio`` at top
level: the engraver is resolved from ``clausters/_libs`` before anything
installed, so a plain import is not what these tests depend on.
"""

import functools
import json

import pytest

from clausters import Event
from clausters.gui import notation, score
from clausters.seq.event import rest
from clausters.seq.timeline import Timeline


@functools.cache
def _engraver() -> bool:
    """Whether this checkout can engrave at all."""
    try:
        notation._verovio()
        return True
    except RuntimeError:
        return False


def requires_engraver():
    if not _engraver():
        pytest.skip("no engraver: build libverovio with "
                    "third_party/build-verovio.sh and stage it with build_native.py")


# A two-bar phrase in Plaine & Easie, small enough to engrave in milliseconds.
PHRASE = "@clef:G-2\n@timesig:4/4\n@data:4CDEF/ 4GABc'/"


def test_score_carries_the_playhead_anchor_and_rate():
    node = score(11, display_list={"vb": [100, 50], "glyphs": {}, "prims": []},
                 playhead_at=48000.0, sample_rate=44100.0)
    assert node["playhead_at"] == 48000.0
    assert node["sample_rate"] == 44100.0
    # unset chrome is omitted rather than sent as a null
    assert "playhead" not in node
    assert "color" not in node


def test_score_editing_is_opt_in():
    # editing is off unless asked for: a plain view sends no `editable`, so the
    # host's read-only default holds; an editor sends editable=True.
    assert "editable" not in score(11, display_list={"vb": [1, 1]})
    assert score(11, editable=True)["editable"] is True
    assert score(11, editable=False)["editable"] is False   # survives _drop_none


def test_score_carries_the_selected_element():
    node = score(11, display_list={"vb": [100, 50], "glyphs": {}, "prims": []},
                 selected="note-1")
    assert node["selected"] == "note-1"
    # the empty string is the clear, and must survive `_drop_none`
    assert score(11, selected="")["selected"] == ""
    assert "selected" not in score(11)


def test_score_sends_the_drawing_layers_but_not_the_notes():
    node = score(11, display_list={"vb": [100, 50], "glyphs": {}, "prims": [],
                                   "cursors": [{"t": 0, "x": 1, "y0": 0, "y1": 2}],
                                   "notes": [{"t": 0, "dur": 1, "pitch": 60, "id": "n"}]})
    assert node["cursors"][0]["x"] == 1
    assert "notes" not in node


def test_score_sends_the_pitch_quantum_the_page_was_engraved_with():
    requires_engraver()
    dl = notation.engrave(PHRASE)
    # half the staff-line spacing: a line-to-space move is one diatonic step
    assert dl["step"] == 90.0
    assert score(11, display_list=dl)["step"] == 90.0
    # it follows verovio's `unit`, not the staff scale — which is why the host
    # is told rather than left to assume it
    assert notation.engrave(PHRASE, scale=100)["step"] == 90.0
    assert notation.engrave(PHRASE, options={"unit": 6})["step"] == 60.0


def test_the_page_json_carries_the_drawing_layers_only():
    requires_engraver()
    dl = notation.engrave(PHRASE)
    page = json.loads(notation.page_json(dl))
    assert sorted(page) == ["cursors", "glyphs", "prims", "step", "vb"]
    # what the builder sends when it defines the widget, so a re-engraved page
    # replaces it exactly
    built = score(11, display_list=dl)
    assert all(built[k] == page[k] for k in page)


def test_a_note_owns_everything_drawn_inside_it():
    requires_engraver()
    # eighths, so every note has a notehead, a stem and a flag -- three
    # primitives verovio gives ids of their own, which would otherwise scatter
    # one note across three elements to select and drag
    dl = notation.engrave("@clef:G-2\n@timesig:4/4\n@data:8CDEF8GABc'/")
    first = dl["notes"][0]["id"]
    parts = [p["k"] for p in dl["prims"] if p.get("id") == first]
    assert parts == ["glyph", "line", "glyph"]  # notehead, stem, flag
    # a chord is not an element in that sense: its notes nest inside it and
    # each keeps its own id, or one of them could not be transposed alone
    chord = notation.engrave("@clef:G-2\n@timesig:4/4\n@data:4C^E^G4G2C/")
    heads = [n["id"] for n in chord["notes"][:3]]
    assert len(set(heads)) == 3
    assert all(len([p for p in chord["prims"] if p.get("id") == i]) == 1
               for i in heads)


def test_score_view_pans_both_axes_only_when_it_can_zoom():
    page = {"vb": [1000, 500], "glyphs": {}, "prims": []}
    # zoomed in the page outgrows the view's width, so x has to pan too
    zoomable = notation.score_view(page, scroll_id=10, score_id=11)
    assert zoomable["axis"] == "both"
    assert zoomable["zoom"] == 1
    # without zoom the page always fits the width: a plain vertical scroll view
    fixed = notation.score_view(page, scroll_id=10, score_id=11, zoom=False)
    assert fixed["axis"] == "y"
    assert fixed["zoom"] == 0


def test_score_view_places_the_rate_on_the_inner_score():
    view = notation.score_view({"vb": [1000, 500], "glyphs": {}, "prims": []},
                               scroll_id=10, score_id=11, width=800.0,
                               sample_rate=48000.0)
    assert view["type"] == "scroll"
    inner = view["children"][0]
    assert inner["type"] == "score"
    assert inner["sample_rate"] == 48000.0


@functools.cache
def _editor_alive() -> bool:
    """Whether this verovio can edit at all.

    In 6.2.1 — the released wheel — a guard typo leaves the editor unreachable
    and every action returns False (see ``third_party/BUILD-VEROVIO.md``), so
    the editing tests below would fail for a reason that is not ours.
    """
    probe = notation.Score(PHRASE)
    return probe.transpose(probe.display_list()["notes"][0]["id"], 1)


def _edited_score():
    """A live score to edit, and the id of its first (sounding) note."""
    requires_engraver()
    if not _editor_alive():
        pytest.skip("this verovio's editor refuses every action; build one past "
                    "the fixed guard (see third_party/BUILD-VEROVIO.md)")
    s = notation.Score(PHRASE)
    return s, s.display_list()["notes"][0]["id"]


def _pitches(score):
    return [n["pitch"] for n in score.display_list()["notes"]]


def test_transposing_moves_the_pitch_by_diatonic_steps():
    s, nid = _edited_score()
    assert _pitches(s)[0] == 60           # middle C
    assert s.transpose(nid, -2) is True
    assert _pitches(s)[0] == 57           # two steps down the staff: A3
    assert s.transpose(nid, 3) is True
    assert _pitches(s)[0] == 62           # three back up: D4
    assert s.transpose(nid, 0) is False   # nothing to do, and no undo step


def test_editing_keeps_the_ids_the_host_selects_by():
    s, nid = _edited_score()
    before = [n["id"] for n in s.display_list()["notes"]]
    s.transpose(nid, -1)
    dl = s.display_list()
    assert [n["id"] for n in dl["notes"]] == before
    # the edited note is still drawn, under the same id
    assert nid in {p.get("id") for p in dl["prims"]}


def test_undo_and_redo_walk_the_edits():
    s, nid = _edited_score()
    s.transpose(nid, -2)
    s.transpose(nid, -1)
    assert (s.can_undo, s.can_redo) == (True, False)
    assert s.undo() is True
    assert _pitches(s)[0] == 57
    assert s.undo() is True
    assert _pitches(s)[0] == 60
    assert s.can_undo is False
    assert s.redo() is True
    assert _pitches(s)[0] == 57
    assert s.can_redo is True


def test_undo_on_a_fresh_score_answers_instead_of_crashing():
    # verovio's own undo dereferences an empty stack and takes the process
    # down, so the stack is ours -- this test is the guard for that.
    s, _ = _edited_score()
    assert s.can_undo is False
    assert s.undo() is False
    assert s.redo() is False
    assert _pitches(s)[0] == 60


def test_a_rejected_edit_leaves_the_score_untouched():
    s, nid = _edited_score()
    before = _pitches(s)
    assert s.edit("nonsense", elementId=nid) is False
    assert _pitches(s) == before
    assert s.can_undo is False


def test_the_score_round_trips_through_mei():
    s, nid = _edited_score()
    s.transpose(nid, -2)
    mei = s.mei()
    assert "<note" in mei and nid in mei
    # re-engraving the exported MEI reproduces the edited score, ids included
    assert [n["pitch"] for n in notation.engrave(mei)["notes"]] == _pitches(s)


def test_engraving_shares_ids_and_time_between_cursors_and_notes():
    requires_engraver()
    dl = notation.engrave(PHRASE)
    assert dl["prims"] and dl["cursors"] and dl["notes"]
    # both layers come out of one engraving, so their onsets line up
    assert dl["notes"][0]["t"] == dl["cursors"][0]["t"] == 0.0
    assert dl["notes"][0]["pitch"] == 60  # middle C, the phrase's first note
    # the cursor sweeps left to right and the notes run forward in time
    assert [c["x"] for c in dl["cursors"]] == sorted(c["x"] for c in dl["cursors"])
    assert [n["t"] for n in dl["notes"]] == sorted(n["t"] for n in dl["notes"])
    # every note is placed on the page it was engraved from
    placed = {p.get("id") for p in dl["prims"]}
    assert all(n["id"] in placed for n in dl["notes"])


# -- MEI generation from sequencing data (pure; no engraver needed) ---------

def test_midi_spells_to_scientific_pitch_with_the_accidental_world():
    assert notation._spell(60, flats=False) == ("c", 4, "")     # middle C
    assert notation._spell(61, flats=False) == ("c", 4, "s")    # C#
    assert notation._spell(66, flats=False) == ("f", 4, "s")    # F#
    assert notation._spell(61, flats=True) == ("d", 4, "f")     # spelled Db
    assert notation._spell(72, flats=False) == ("c", 5, "")     # an octave up


def test_a_duration_decomposes_into_tied_note_values():
    # ticks: whole=32, half=16, quarter=8, eighth=4 (32nd-note resolution)
    assert notation._pieces(8) == [(4, 0)]        # a quarter (one beat)
    assert notation._pieces(4) == [(8, 0)]        # an eighth
    assert notation._pieces(16) == [(2, 0)]       # a half
    assert notation._pieces(12) == [(4, 1)]       # 1.5 beats -> a dotted quarter
    assert notation._pieces(20) == [(2, 0), (8, 0)]  # 2.5 beats -> half + eighth


def test_from_notes_writes_a_monophonic_melody():
    notes = [Event(midinote=60, dur=1.0), Event(midinote=62, dur=0.5),
             Event(midinote=64, dur=1.5), rest(1.0), Event(midinote=65, dur=1.0)]
    mei = notation.from_notes(notes, meter="4/4", key="C")
    assert "<rest" in mei and 'dots="1"' in mei     # the rest and the dotted 1.5
    requires_engraver()
    dl = notation.engrave(mei)
    assert [n["pitch"] for n in dl["notes"]] == [60, 62, 64, 65]


def test_a_note_crossing_a_barline_splits_and_ties():
    # 2 beats, then 3 beats starting on beat 2 of 4/4 -> the 3-beat note spans
    # the barline and is written as two tied notes at the same pitch.
    notes = [Event(midinote=60, dur=2.0), Event(midinote=67, dur=3.0)]
    mei = notation.from_notes(notes, meter="4/4")
    assert 'tie="i"' in mei and 'tie="t"' in mei
    requires_engraver()
    assert [n["pitch"] for n in notation.engrave(mei)["notes"]] == [60, 67, 67]


def test_from_timeline_puts_a_downbeat_onset_on_the_bar_start():
    # a beat-0 onset is tick 0, not tick 1: a spurious leading rest would knock
    # the whole bar off the metric grid and verovio would re-split every note.
    tl = Timeline()
    for i, pitch in enumerate((60, 62, 64, 65)):
        tl.add(i, Event(midinote=pitch, dur=1.0))
    mei = notation.from_timeline(tl, meter="4/4")
    layer = mei[mei.index("<layer"):mei.index("</layer>")]
    assert "<rest" not in layer.split("<note")[0]     # nothing before the first note
    assert layer.count("tie=") == 0                    # four clean quarters, no ties
    requires_engraver()
    assert notation.engrave(mei)["notes"][0]["t"] == 0.0


def test_from_timeline_makes_chords_and_rests():
    tl = Timeline()
    for pitch in (60, 64, 67):                      # a C-major triad at beat 0
        tl.add(0, Event(midinote=pitch, dur=1.0))
    tl.add(2, Event(midinote=72, dur=1.0))          # a gap at beat 1..2 -> rest
    mei = notation.from_timeline(tl, meter="4/4")
    assert "<chord" in mei and "<rest" in mei
    requires_engraver()
    dl = notation.engrave(mei)
    assert [n["pitch"] for n in dl["notes"]] == [60, 64, 67, 72]


def test_a_generated_score_is_editable():
    requires_engraver()
    if not _editor_alive():
        pytest.skip("this verovio's editor refuses every action")
    s = notation.Score.from_notes([Event(midinote=60, dur=1.0),
                                   Event(midinote=62, dur=1.0)])
    nid = s.display_list()["notes"][0]["id"]
    assert s.transpose(nid, 2) is True
    assert s.display_list()["notes"][0]["pitch"] == 64   # two steps up: E4
