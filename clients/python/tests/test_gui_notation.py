"""The notation client: the engraved layers and the score widget's props.

`engrave` needs the engraver, which normally ships inside the package; a source
checkout that has not built it yet skips those tests, while the `guidef.score`
builder is pure and always runs. The editing tests skip one step further — on a
verovio whose editor is dead, which the released wheel's is.

Both guards ask `notation` itself rather than the engraver directly: it is
reached through the shared ABI (`clausters._native`), built with the ``verovio``
feature on, and resolved from ``clausters/_libs`` before anything installed.
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
        notation._abi._engraver()
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
    node = score(id=11, display_list={"vb": [100, 50], "glyphs": {}, "prims": []},
                 playhead_at=48000.0, sample_rate=44100.0)
    assert node["playhead_at"] == 48000.0
    assert node["sample_rate"] == 44100.0
    # unset chrome is omitted rather than sent as a null
    assert "playhead" not in node
    assert "color" not in node


def test_score_editing_is_opt_in():
    # editing is off unless asked for: a plain view sends no `editable`, so the
    # host's read-only default holds; an editor sends editable=True.
    assert "editable" not in score(id=11, display_list={"vb": [1, 1]})
    assert score(id=11, editable=True)["editable"] is True
    assert score(id=11, editable=False)["editable"] is False   # survives _drop_none


def test_score_carries_the_selected_element():
    node = score(id=11, display_list={"vb": [100, 50], "glyphs": {}, "prims": []},
                 selected="note-1")
    assert node["selected"] == "note-1"
    # the empty string is the clear, and must survive `_drop_none`
    assert score(id=11, selected="")["selected"] == ""
    assert "selected" not in score(id=11)


def test_score_sends_the_drawing_layers_but_not_the_notes():
    node = score(id=11, display_list={"vb": [100, 50], "glyphs": {}, "prims": [],
                                   "cursors": [{"t": 0, "x": 1, "y0": 0, "y1": 2}],
                                   "notes": [{"t": 0, "dur": 1, "pitch": 60, "id": "n"}]})
    assert node["cursors"][0]["x"] == 1
    assert "notes" not in node


def test_score_sends_the_pitch_quantum_the_page_was_engraved_with():
    requires_engraver()
    dl = notation.engrave(PHRASE)
    # half the staff-line spacing: a line-to-space move is one diatonic step
    assert dl["step"] == 90.0
    assert score(id=11, display_list=dl)["step"] == 90.0
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
    built = score(id=11, display_list=dl)
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
    assert view["type"] == "plane"
    inner = view["children"][0]
    assert inner["type"] == "score"
    assert inner["sample_rate"] == 48000.0


def test_score_view_sizes_itself_from_a_page_held_as_a_source():
    """The scroll reads the page's own size, so it has to reach it through a
    source too — and the page then follows an edit into every window."""
    from clausters.gui import source

    page = source(display_list={"vb": [1000, 500], "glyphs": {}, "prims": []})
    view = notation.score_view(page, scroll_id=10, score_id=11, width=800.0)
    assert view["content_h"] == 400.0
    inner = view["children"][0]
    assert inner["vb"] == [1000, 500] and "display_list" not in inner

    page.set({"vb": [1000, 500], "glyphs": {}, "prims": [{"kind": "line"}]})
    assert inner["prims"] == [{"kind": "line"}]


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


# -- MEI generation from sequencing data ------------------------------------
# The reduction to a voice is this client's; laying it out into MEI is the
# shared encoder's, whose own unit tests (pitch spelling, tick decomposition)
# live in `clausters_core::notation`. These check the whole path.

def test_from_notes_writes_a_monophonic_melody():
    requires_engraver()
    notes = [Event(midinote=60, dur=1.0), Event(midinote=62, dur=0.5),
             Event(midinote=64, dur=1.5), rest(1.0), Event(midinote=65, dur=1.0)]
    mei = notation.from_notes(notes, meter="4/4", key="C")
    assert "<rest" in mei and 'dots="1"' in mei     # the rest and the dotted 1.5
    dl = notation.engrave(mei)
    assert [n["pitch"] for n in dl["notes"]] == [60, 62, 64, 65]


def test_a_note_crossing_a_barline_splits_and_ties():
    requires_engraver()
    # 2 beats, then 3 beats starting on beat 2 of 4/4 -> the 3-beat note spans
    # the barline and is written as two tied notes at the same pitch.
    notes = [Event(midinote=60, dur=2.0), Event(midinote=67, dur=3.0)]
    mei = notation.from_notes(notes, meter="4/4")
    assert 'tie="i"' in mei and 'tie="t"' in mei
    assert [n["pitch"] for n in notation.engrave(mei)["notes"]] == [60, 67, 67]


def test_from_timeline_puts_a_downbeat_onset_on_the_bar_start():
    requires_engraver()
    # a beat-0 onset is tick 0, not tick 1: a spurious leading rest would knock
    # the whole bar off the metric grid and verovio would re-split every note.
    tl = Timeline()
    for i, pitch in enumerate((60, 62, 64, 65)):
        tl.add(i, Event(midinote=pitch, dur=1.0))
    mei = notation.from_timeline(tl, meter="4/4")
    layer = mei[mei.index("<layer"):mei.index("</layer>")]
    assert "<rest" not in layer.split("<note")[0]     # nothing before the first note
    assert layer.count("tie=") == 0                    # four clean quarters, no ties
    assert notation.engrave(mei)["notes"][0]["t"] == 0.0


def test_from_timeline_makes_chords_and_rests():
    requires_engraver()
    tl = Timeline()
    for pitch in (60, 64, 67):                      # a C-major triad at beat 0
        tl.add(0, Event(midinote=pitch, dur=1.0))
    tl.add(2, Event(midinote=72, dur=1.0))          # a gap at beat 1..2 -> rest
    mei = notation.from_timeline(tl, meter="4/4")
    assert "<chord" in mei and "<rest" in mei
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


# -- the score model ---------------------------------------------------------
#
# The model crosses as data and so do the operations, which buys a new verb for
# no ABI at all and costs the one thing `tests/bindings.rs` used to give free:
# it sees one symbol and no verbs. So the catalog is what parity is read
# against, and these are that reading for this client.


def test_the_catalog_and_this_shell_name_the_same_verbs():
    """Every operation the core knows has a helper here, and every helper here
    names an operation the core knows.

    This is the test the binding table cannot be: operations ride inside a
    payload through one symbol, so a verb that reached only one client would
    drift silently — the same structural blindness the props manifest has.
    """
    catalogued = [spec["op"] for spec in notation.ops()]
    # The shell's helpers are named after their verb, one function each. This
    # client's spelling *is* the catalog's, so there is no mapping to get wrong
    # -- the web client needs one, since `delete` is a reserved word there.
    missing = [verb for verb in catalogued if not callable(getattr(notation, verb, None))]
    assert missing == [], f"the core knows {missing} with no helper here"
    # and the catalog is not empty, which is how this test would pass by
    # checking nothing at all
    assert len(catalogued) >= 20, f"only {len(catalogued)} verbs in the catalog"


def test_a_voice_lifts_into_the_model_and_writes_the_same_bytes():
    voice = [{"midis": [60], "ticks": 8}, {"ticks": 8}, {"midis": [64, 67], "ticks": 16}]
    sheet = notation.sheet_from_voice(voice, meter="4/4", clef="G2", key="C")
    # ticks became exact rationals, and MIDI numbers became spelled pitches
    items = sheet["staves"][0]["voices"][0]["items"]
    assert items[0]["dur"] == [1, 4] and items[0]["pitches"][0]["step"] == "c"
    assert items[1]["kind"] == "rest"
    assert len(items[2]["pitches"]) == 2
    # and writing a monophonic one out is byte for byte what `from_notes`
    # produces, because `from_notes` now travels this same road
    mono = notation.sheet_from_voice(
        [{"midis": [60], "ticks": 8}, {"ticks": 8}, {"midis": [64], "ticks": 16}])
    assert notation.to_mei(mono) == notation.from_notes(
        [Event(midinote=60, dur=1.0), rest(1.0),
         Event(midinote=64, dur=2.0)], meter="4/4")


def test_transposing_keeps_the_spelling_the_interval_implies():
    sheet = notation.sheet_from_voice([{"midis": [60], "ticks": 8}])
    # a major third up from C is E natural, not F flat
    up = notation.transpose(sheet, 4)
    assert up["staves"][0]["voices"][0]["items"][0]["pitches"][0] == {
        "step": "e", "alter": 0, "octave": 4}
    # a minor third is the same two steps with the alteration doing the work
    minor = notation.transpose(sheet, 3)
    assert minor["staves"][0]["voices"][0]["items"][0]["pitches"][0]["alter"] == -1
    # and the sheet that was sent is untouched, because it crossed by value
    assert sheet["staves"][0]["voices"][0]["items"][0]["pitches"][0]["step"] == "c"


def test_a_measure_span_is_resolved_by_the_core_against_the_grid():
    # eight quarters; in 4/4 measure 2 starts at the fifth
    voice = [{"midis": [60], "ticks": 8} for _ in range(8)]
    sheet = notation.sheet_from_voice(voice, meter="4/4")
    out = notation.transpose(sheet, 12, span=notation.measures(2, 2))
    octaves = [i["pitches"][0]["octave"] for i in out["staves"][0]["voices"][0]["items"]]
    assert octaves == [4, 4, 4, 4, 5, 5, 5, 5]
    # in 3/4 the same span names different notes -- the arithmetic no client does
    sheet = notation.sheet_from_voice(voice, meter="3/4")
    out = notation.transpose(sheet, 12, span=notation.measures(2, 2))
    octaves = [i["pitches"][0]["octave"] for i in out["staves"][0]["voices"][0]["items"]]
    assert octaves == [4, 4, 4, 5, 5, 5, 4, 4]


def test_what_is_refused_says_why_and_changes_nothing():
    sheet = notation.sheet_from_voice([{"midis": [60], "ticks": 8}])
    with pytest.raises(ValueError, match="backwards"):
        notation.transpose(sheet, 1, span=notation.measures(4, 2))
    with pytest.raises(ValueError, match="from 1"):
        notation.transpose(sheet, 1, span=notation.measures(0, 1))
    # and what the model can hold but MEI cannot yet be written for
    sheet["staves"][0]["voices"][0]["items"][0]["dur"] = [1, 12]
    with pytest.raises(ValueError, match="tuplet"):
        notation.to_mei(sheet)


def _items(sheet):
    return sheet["staves"][0]["voices"][0]["items"]


def test_the_algebra_rearranges_a_score_and_composes():
    four = notation.sheet_from_voice([{"midis": [60], "ticks": 8}] * 4)

    # one score after another
    assert len(_items(notation.concat(four, four))) == 8
    # and at the same time, as voices or as staves
    assert len(notation.stack(four, four)["staves"][0]["voices"]) == 2
    assert len(notation.stack(four, four, as_staff=True)["staves"]) == 2
    # a stretch does not move a barline: four quarters doubled is two bars
    assert _items(notation.stretch(four, (2, 1)))[0]["dur"] == [1, 2]
    assert notation.to_mei(notation.stretch(four, (2, 1))).count("<measure") == 2
    # reversing keeps the length
    assert len(_items(notation.retrograde(four))) == 4
    # and composing two operations is the operation on the composed score
    def up(s):
        return notation.transpose(s, 2)
    assert notation.to_mei(up(notation.concat(four, four))) == notation.to_mei(
        notation.concat(up(four), up(four)))


def test_the_grid_opens_and_closes_and_the_music_moves_with_it():
    eight = notation.sheet_from_voice([{"midis": [60], "ticks": 8}] * 8)
    assert notation.to_mei(notation.insert_measures(eight, 2, 1)).count("<measure") == 3
    assert notation.to_mei(notation.remove_measures(eight, 2, 2)).count("<measure") == 1
    # changing the meter rewrites no note
    remetered = notation.set_meter(eight, 2, 3, 4)
    assert [i["dur"] for i in _items(remetered)] == [i["dur"] for i in _items(eight)]


def test_an_edit_names_its_item_and_deleting_is_not_silencing():
    three = notation.sheet_from_voice([{"midis": [60], "ticks": 8}] * 3)
    id = _items(three)[1]["id"]

    assert len(_items(notation.delete(three, id))) == 2
    assert len(_items(notation.silence(three, id))) == 3
    assert _items(notation.silence(three, id))[1]["id"] == id, "silencing keeps the item"
    assert _items(notation.set_dur(three, id, (1, 2)))[1]["dur"] == [1, 2]
    assert len(_items(notation.insert(three, (1, 8), after=id))) == 4
    assert _items(notation.set_pitches(
        three, id, [notation.pitch("b", 3, 1)]))[1]["pitches"][0]["step"] == "b"
    assert _items(notation.tie(three, id))[1]["tie"] is True
    assert len(notation.to_voice(three, [id], 1)["staves"][0]["voices"]) == 2
    # and every verb refuses an item that is not there, saying which
    with pytest.raises(ValueError, match="999"):
        notation.delete(three, 999)


def test_polyphony_tuplets_and_marks_reach_the_page():
    # two voices on one staff, and two staves under a brace
    duo = notation.sheet_from_voice([{"midis": [60], "ticks": 16}] * 2)
    duo = notation.stack(duo, notation.transpose(duo, -12))
    mei = notation.to_mei(duo)
    assert mei.count("<layer") == 2
    grand = notation.stack(
        notation.sheet_from_voice([{"midis": [60], "ticks": 32}]),
        notation.sheet_from_voice([{"midis": [48], "ticks": 32}]), as_staff=True)
    assert 'symbol="brace"' in notation.to_mei(grand)

    # three in the time of two, which no grid of 32nds can hold
    triplet = {"kind": "note", "pitches": [{"step": "c", "octave": 4}], "dur": [1, 12]}
    tup = notation.sheet_from_voice([{"midis": [60], "ticks": 24}])
    tup["staves"][0]["voices"][0]["items"] = [dict(triplet, id=i + 1) for i in range(3)] + [
        {"kind": "rest", "id": 4, "dur": [3, 4]}]
    assert 'num="3" numbase="2"' in notation.to_mei(tup)

    # the marks a note carries, and what is written between two notes
    s = notation.sheet_from_voice([{"midis": [60], "ticks": 8},
                                   {"midis": [64], "ticks": 8},
                                   {"midis": [67], "ticks": 16}])
    ids = [i["id"] for i in _items(s)]
    s = notation.set_marks(s, ids[0], notation.marks(
        articulations=["stacc"], dynamic="mf", sounding=(1, 8)))
    s = notation.add_spanner(s, "crescendo", ids[0], ids[2])
    mei = notation.to_mei(s)
    assert '<artic artic="stacc"/>' in mei
    assert '<dynam' in mei and 'form="cres"' in mei
    # A sounding length stays in the score and is deliberately not written:
    # an engraver reads one as the note's real duration and advances its own
    # clock by it, which pulls every attack after it earlier.
    assert 'dur="4"' in mei and "dur.ges" not in mei
    # and a spanner naming a note that is not there is refused, not dropped
    with pytest.raises(ValueError, match="999"):
        notation.add_spanner(s, "slur", ids[0], 999)


def test_an_accidental_is_printed_only_where_it_is_needed():
    # a scale in B flat prints no flat its armature already implies
    flats = notation.sheet_from_voice([{"midis": [70], "ticks": 8}] * 2, key="Bb")
    mei = notation.to_mei(flats)
    assert '<accid accid="f"/>' not in mei
    assert 'accid.ges="f"' in mei
    # a chromatic note prints its own, and does not restate it in the same bar
    sharp = notation.sheet_from_voice([{"midis": [66], "ticks": 8}] * 2, key="C")
    mei = notation.to_mei(sharp)
    assert mei.count('<accid accid="s"/>') == 1
    assert mei.count('accid.ges="s"') == 1
    # and a natural in a key that alters that step is a *sign*, not silence --
    # written with nothing at all it would read as the altered note
    natural = notation.sheet_from_voice([{"midis": [60], "ticks": 8}], key="F#")
    assert '<accid accid="n"/>' in notation.to_mei(natural)


def test_a_rest_that_fills_a_measure_is_written_as_one():
    # MEI has an element for it and an engraver draws it centred in the bar,
    # which is where a reader looks; a decomposed whole rest hangs at the start
    # and reads as a rest on the downbeat with something after it.
    duo = notation.stack(
        notation.sheet_from_voice([{"midis": [60], "ticks": 32}] * 2),
        notation.sheet_from_voice([{"midis": [60], "ticks": 32}]))
    mei = notation.to_mei(duo)
    assert "<mRest/>" in mei
    assert '<rest dur="1"' not in mei


# -- the interpreter: what the page means, read back into sound ---------------


def _quarters(n: int, **kw) -> dict:
    return notation.sheet_from_voice([{"midis": [60], "ticks": 8}] * n, **kw)


def test_the_interpretation_is_data_and_comes_from_the_core():
    reading = notation.interpretation()
    # every number the reading depends on, in one value a caller can edit --
    # and none of them written down in this client
    assert reading["dynamics"]["mf"] > reading["dynamics"]["p"]
    assert reading["articulations"]["stacc"]["factor"] == 0.5
    assert reading["beat_unit"] == 4
    # the downbeat, and nothing else: one and three in a 4/4 is a style
    assert [a["at"] for a in reading["accents"]] == [[0, 1]]


def test_a_staccato_shortens_the_sound_and_moves_no_attack():
    sheet = _quarters(4)
    ids = [i["id"] for i in sheet["staves"][0]["voices"][0]["items"]]
    sheet = notation.set_marks(sheet, ids[1],
                               notation.marks(articulations=["stacc"]))
    notes = notation.to_notes(sheet)
    # the written value and the heard one are two numbers, and only one moved
    assert notes[1]["dur"] == 1.0
    assert notes[1]["sustain"] == 0.5
    assert [n["t"] for n in notes] == [0.0, 1.0, 2.0, 3.0]


def test_a_dynamic_governs_until_the_next_one_and_a_hairpin_shapes_a_span():
    sheet = _quarters(8)
    ids = [i["id"] for i in sheet["staves"][0]["voices"][0]["items"]]
    sheet = notation.set_marks(sheet, ids[1], notation.marks(dynamic="p"))
    sheet = notation.add_spanner(sheet, "crescendo", ids[1], ids[4])
    notes = notation.to_notes(sheet)
    amps = [n["amp"] for n in notes]
    # the mark is on one note and governs every note after it
    assert amps[1] < amps[0]
    # the hairpin rises across its span...
    assert amps[2] > amps[1] and amps[3] > amps[2] and amps[4] > amps[3]
    # ...and past its far end nothing of it is left
    assert amps[5] == amps[6]


def test_a_tie_is_one_sound_and_a_tuplet_needs_no_rule():
    tied = notation.tie(_quarters(3), 1, True)
    notes = notation.to_notes(tied)
    assert len(notes) == 2, "the second note does not attack again"
    assert notes[0]["dur"] == 2.0 and notes[1]["t"] == 2.0
    # a triplet's division is already exact in the rational the item holds
    triplet = notation.stretch(_quarters(3), (1, 3))
    onsets = [n["t"] for n in notation.to_notes(triplet)]
    assert abs(onsets[2] - 2.0 / 3.0) < 1e-12


def test_an_interpretation_is_overridden_without_editing_the_core():
    style = notation.interpretation()
    style["accents"].append({"at": [1, 2], "gain": 1.1, "meter": "4/4"})
    style["detach"] = 0.6
    notes = notation.to_notes(_quarters(4), style)
    # a stress this reader believes in and the default does not
    assert abs(notes[2]["amp"] / notes[1]["amp"] - 1.1) < 1e-12
    # and a player who detaches by habit, which the default deliberately is not
    assert notes[1]["sustain"] == 0.6
    assert notation.to_notes(_quarters(4))[1]["sustain"] == 1.0


def test_a_staff_names_itself_and_never_what_plays_it():
    duo = notation.stack(_quarters(2), _quarters(2), as_staff=True)
    notes = notation.to_notes(duo)
    assert sorted({n["staff"] for n in notes}) == [0, 1]
    # the binding is made where the score is rendered, explicitly
    timeline = notation.to_timeline(duo, instruments={0: "flute", 1: "cello"})
    assert {event["instrument"] for _, event in timeline} == {"flute", "cello"}


def test_a_timeline_carries_both_lengths_onto_the_event():
    sheet = _quarters(2)
    ids = [i["id"] for i in sheet["staves"][0]["voices"][0]["items"]]
    sheet = notation.set_marks(sheet, ids[0],
                               notation.marks(articulations=["stacc"]))
    timeline = notation.to_timeline(sheet)
    _, first = next(iter(timeline))
    assert first["dur"] == 1.0 and first.sustain() == 0.5
    assert first["midinote"] == 60


def test_a_hairpin_written_to_a_note_that_is_gone_is_refused_by_name():
    sheet = _quarters(2)
    sheet["spanners"] = [{"kind": "crescendo", "from": 1, "to": 99}]
    with pytest.raises(ValueError, match="crescendo"):
        notation.to_notes(sheet)


# -- the reader: a document back into the model -------------------------------


def test_a_score_written_read_and_written_again_is_the_same_bytes():
    sheet = notation.concat(_quarters(4), _quarters(4))
    once = notation.to_mei(sheet)
    assert notation.to_mei(notation.sheet_from_mei(once)) == once


def test_a_typed_score_opens_into_the_model_and_the_algebra_can_touch_it():
    # ABC is what a reader types; verovio normalizes whatever it loaded to MEI,
    # so there is one input format here rather than four.
    phrase = ("X:1\nT:Six bars\nC:Anon.\nM:4/4\nL:1/4\nK:G\n"
              "C D E F | G/A/G/F/ E D | [CEG] G C2 |\n")
    sheet = notation.sheet_from_mei(notation.Score(phrase).mei())
    assert sheet["header"]["title"] == "Six bars"
    assert sheet["header"]["composer"] == "Anon."
    assert sheet["key"] == "G"
    items = sheet["staves"][0]["voices"][0]["items"]
    assert len(items[10]["pitches"]) == 3, "the chord came back a chord"
    assert items[4]["dur"] == [1, 8]
    # and every verb the model has now works on it, which is the whole point
    up = notation.transpose(sheet, 2)
    assert up["staves"][0]["voices"][0]["items"][0]["pitches"][0]["step"] == "d"
    assert len(notation.Score(notation.to_mei(up)).display_list()["notes"]) > 0


def test_the_emitters_own_padding_does_not_come_back_as_music():
    # A voice is written into whole measures, so a short one is padded. Reading
    # that back would grow the score by a rest every time it was saved.
    duo = notation.stack(_quarters(8), _quarters(4), as_staff=True)
    back = notation.sheet_from_mei(notation.to_mei(duo))
    lower = back["staves"][1]["voices"][0]["items"]
    assert len(lower) == 4, f"four quarters, not eight bars of padding: {lower}"


def test_the_header_the_barlines_and_the_breaks_are_edited_and_survive():
    sheet = notation.concat(_quarters(4), _quarters(4))
    sheet = notation.set_header(sheet, notation.header(
        title="Study", composer="A. Composer"))
    sheet = notation.set_barline(sheet, 1, "rptend")
    sheet = notation.set_break(sheet, 2, "system")
    back = notation.sheet_from_mei(notation.to_mei(sheet))
    assert back["header"] == {"title": "Study", "composer": "A. Composer"}
    assert back["grid"]["barlines"] == [[0, "rptend"]]
    assert back["grid"]["breaks"] == [[1, "system"]]
    # and taking one back removes it rather than storing "ordinary"
    plain = notation.set_barline(sheet, 1, "single")
    assert plain["grid"].get("barlines", []) == []


def test_a_beam_somebody_chose_is_a_spanner_like_any_other():
    sheet = notation.sheet_from_voice([{"midis": [60], "ticks": 4}] * 4)
    ids = [i["id"] for i in sheet["staves"][0]["voices"][0]["items"]]
    sheet = notation.add_spanner(sheet, "beam", ids[0], ids[3])
    mei = notation.to_mei(sheet)
    assert "<beam>" in mei
    back = notation.sheet_from_mei(mei)
    assert {"kind": "beam", "from": ids[0], "to": ids[3]} in back["spanners"]


def test_what_is_not_a_score_says_so():
    with pytest.raises(ValueError, match="XML"):
        notation.sheet_from_mei("<not xml")
    with pytest.raises(ValueError, match="score"):
        notation.sheet_from_mei("<mei><music/></mei>")
