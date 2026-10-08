"""The arrangement's client side (`clausters.multitrack`).

What the crate defines, written and read here. The crossing that proves the two
agree is `crates/clausters-document/tests/arrangement_parity.rs`, over a vector
this client generates; what this suite checks is the surface a person actually
types against, and the few rules that are the client's own to keep.
"""

import pytest
from clausters.multitrack import (Multitrack, Automation, Content, Fade,
                                   TakeLane, Marker, Meter, Region, Span, Tempo,
                                   Track)


def window(source: int, start: float = 0.0, duration: float = 4.0) -> dict:
    return {"source": {"source": source, "lifetime": "session", "generation": 0},
            "start": start, "duration": duration}


def one_lane(**track) -> tuple:
    """A multitrack with one track, and the take lane it starts with."""
    multitrack = Multitrack()
    t = multitrack.tracks.add(**track)
    return multitrack, t, t.active_take_lane


def place(lane, at: float, length: float, source: int = 1, **fields) -> Region:
    return lane.regions.add(at, length, Content.onto(window(source)), **fields)


def test_a_region_ends_where_its_span_ends_and_not_where_its_content_does():
    # The window is four seconds of the source; the region shows one second of
    # it. Trimming moves the region and never the source.
    _, _, lane = one_lane()
    r = place(lane, 4.0, 1.0)
    assert r.end == 5.0
    assert r.content.window["duration"] == 4.0


def test_regions_that_touch_do_not_overlap_and_regions_that_share_time_do():
    _, _, lane = one_lane()
    a, b, c = place(lane, 0.0, 4.0), place(lane, 4.0, 4.0), place(lane, 3.0, 4.0)
    assert not a.overlaps(b)
    assert a.overlaps(c)


def test_one_source_under_six_regions_is_referenced_and_not_copied():
    # Six placements, six identities, one source. That is the whole of
    # non-destructive editing, and it is what a region's own identity is for.
    _, _, lane = one_lane()
    made = [place(lane, i * 4.0, 4.0, source=1) for i in range(6)]
    assert len(lane.regions) == 6
    assert {r.content.window["source"]["source"] for r in lane.regions} == {1}
    assert made[3].position == 12.0
    assert len(set(made)) == 6


def test_a_lane_keeps_its_regions_in_position_order():
    _, _, lane = one_lane()
    for at in (8.0, 0.0, 4.0):
        place(lane, at, 2.0)
    assert [r.position for r in lane.regions] == [0.0, 4.0, 8.0]
    assert lane.end == 10.0


def test_a_track_spans_every_lane_and_plays_one():
    _, track, first = one_lane()
    second = track.take_lanes.add("take 2")
    place(first, 0.0, 4.0)
    place(second, 0.0, 16.0)
    assert track.active_take_lane is first
    assert track.active_take_lane.end == 4.0
    # An alternate take is still part of the multitrack.
    assert track.end == 16.0
    track.active_take_lane = second
    assert track.active == 1 and track.active_take_lane is second


def test_one_structure_is_one_object():
    """What a script reads is the structure, not a copy of it: the same region
    read twice is the same object, and it reads what the multitrack holds now."""
    multitrack, track, lane = one_lane(name="drums")
    r = place(lane, 1.0, 2.0)
    assert multitrack.tracks[0] is track
    assert lane.regions[0] is r and r.take_lane is lane and r.track is track
    assert next(multitrack.regions()) is r
    r.position = 3.0
    assert lane.regions[0].position == 3.0
    assert {r: "a key"}[lane.regions[0]] == "a key"


def test_no_call_takes_or_answers_an_id():
    """The ids are the crate's: nothing a script builds is given one, and
    nothing a script asks for is answered with one."""
    import inspect

    for cls in (Region, Track, TakeLane, Marker):
        assert "id" not in [p for p in inspect.signature(cls.__init__).parameters
                            if p != "self"]
    multitrack = Multitrack()
    assert not hasattr(multitrack, "track")
    _, _, lane = one_lane()
    assert not hasattr(lane, "region")


def test_each_field_is_written_through_the_multitrack_s_own_verb():
    multitrack, track, lane = one_lane()
    other = multitrack.tracks.add("other").active_take_lane
    r = place(lane, 0.0, 4.0, name="take")
    r.length = 2.0
    r.fade_in = Fade(0.5)
    r.fade_out = Fade(0.25, shape="curve", curve=-4.0)
    r.muted = True
    r.name = "kept"
    r.place(other, position=6.0)
    assert (r.position, r.length, r.muted, r.name) == (6.0, 2.0, True, "kept")
    assert r.take_lane is other and r.track is other.track
    assert r.fade_in == Fade(0.5) and r.fade_in.shape == "wel"
    assert (r.fade_out.shape, r.fade_out.curve) == ("curve", -4.0)
    track.name, track.level, track.soloed = "drums", 0.7, True
    assert (track.name, track.level, track.soloed) == ("drums", 0.7, True)
    assert multitrack.crossfade, "on unless turned off"
    multitrack.crossfade = False
    assert not multitrack.crossfade
    assert multitrack.write()["defaults"]["crossfade"] is False
    assert multitrack.default_fade == Fade(0.010), "10 ms unless set"
    multitrack.default_fade = Fade(0.05, shape="lin")
    assert multitrack.default_fade == Fade(0.05, shape="lin")
    multitrack.default_fade = None
    assert multitrack.default_fade is None
    assert multitrack.write()["defaults"]["fade"] is None
    assert not multitrack.crossfade, "one default set leaves the other"
    assert multitrack.version > 1, "every edit moves the multitrack's version"


def test_a_removed_structure_leaves_its_object_detached():
    multitrack, track, lane = one_lane()
    r = place(lane, 0.0, 4.0)
    r.remove()
    assert not r.held and len(lane.regions) == 0
    with pytest.raises(ValueError, match="no longer holds"):
        r.position
    track.remove()
    assert not lane.held and len(multitrack.tracks) == 0


def test_a_track_and_a_region_carry_curves_of_their_own():
    """The two places a curve belongs: a track's runs the length of the track
    and is drawn in a row beside it, a region's runs the length of the region
    and is drawn inside it. One type, written through its holder."""
    _, track, lane = one_lane()
    r = place(lane, 0.0, 20.0)
    level = track.automation.add({"ctl": "level"}, [(0.0, 0.0), (4.0, 1.0)], visible=True)
    gain = r.automation.add({"ctl": "gain"}, [(0.0, 1.0)], name="gain")
    assert track.automation[0] is level and r.automation[0] is gain
    assert level.visible and gain.name == "gain"
    level.points = [(0.0, 0.5)]
    assert level.points == [{"at": 0.0, "value": 0.5}]
    free = Automation({"ctl": "pan"}, [(0.0, 0.0)])
    assert r.automation.add(free) is free and free.held
    gain.remove()
    assert not gain.held and list(r.automation) == [free]


def test_the_map_answers_the_entry_in_force_and_nothing_before_the_first():
    multitrack = Multitrack()
    multitrack.set_tempo(Tempo(at=8.0, tempo=1.5))
    multitrack.set_tempo(Tempo(at=0.0, tempo=2.0))
    multitrack.set_tempo(Tempo(at=16.0, tempo=1.0, ramp=True))
    assert [t.at for t in multitrack.tempo] == [0.0, 8.0, 16.0]
    assert multitrack.tempo_at(7.9).tempo == 2.0
    assert multitrack.tempo_at(8.0).tempo == 1.5
    assert multitrack.tempo_at(20.0).ramp


def test_the_tempo_map_is_where_the_beats_fall_over_the_seconds():
    """The multitrack is in seconds; the map it holds says where its beats and
    bars fall, so a script can put a region on a bar -- and it moves nothing."""
    multitrack = Multitrack()
    assert multitrack.tempo_map().secs_at(3.0) == pytest.approx(3.0), \
        "one beat a second where it states no tempo"
    multitrack.set_tempo(Tempo(at=0.0, tempo=2.0))
    multitrack.set_tempo(Tempo(at=4.0, tempo=1.0))
    assert multitrack.tempo_map().secs_at(6.0) == pytest.approx(4.0)


def test_a_multitrack_that_never_said_a_tempo_says_nothing():
    # No 120 invented: naming a default would decide a musical question the
    # document has no business in.
    assert Multitrack().tempo_at(0.0) is None
    assert Multitrack().meter_at(0.0) is None


def test_two_tempos_at_one_beat_is_a_state_the_map_cannot_hold():
    multitrack = Multitrack()
    multitrack.set_tempo(Tempo(at=4.0, tempo=2.0))
    multitrack.set_tempo(Tempo(at=4.0, tempo=1.5))
    assert len(multitrack.tempo) == 1 and multitrack.tempo_at(4.0).tempo == 1.5


def test_two_markers_may_share_an_instant_because_people_do_that():
    multitrack = Multitrack()
    multitrack.markers.add(16.0, "B")
    multitrack.markers.add(16.0, "chorus")
    a = multitrack.markers.add(0.0, "A")
    assert [m.name for m in multitrack.markers] == ["A", "B", "chorus"]
    a.at = 20.0
    assert [m.name for m in multitrack.markers][-1] == "A"
    a.remove()
    assert len(multitrack.markers) == 2 and not a.held


def test_a_span_that_meets_the_next_one_covers_no_instant_twice():
    first, then = Span(0.0, 8.0), Span(8.0, 16.0)
    assert first.length == 8.0 and first.end == then.start


def test_nothing_said_is_nothing_written():
    # An empty multitrack writes an empty object rather than zero of everything, and
    # a plain region writes no layer, no fades, no mute and no playrate.
    assert Multitrack().write() == {}
    _, _, lane = one_lane()
    written = place(lane, 0.0, 4.0).write()
    assert set(written) == {"id", "position", "length", "content"}
    assert "playrate" not in written["content"]


def test_the_multitrack_carries_its_own_version_and_keeps_it_out_of_an_empty_file():
    # The counter a stale edit is stale against, and it is the multitrack's rather
    # than the document's: an editor of one is not editing the other. It stays
    # out of the file while it is the first version, so an unedited multitrack still
    # writes an empty object and a file that never named one reads back at it.
    assert Multitrack().version == 1
    assert "version" not in Multitrack().write()
    assert Multitrack.read({}).version == 1
    assert Multitrack.read({"version": 4}).write()["version"] == 4


def test_a_whole_multitrack_round_trips():
    multitrack = Multitrack()
    multitrack.set_tempo(Tempo(at=0.0, tempo=1.6))
    multitrack.set_meter(Meter(at=0.0, beats=7, unit=8))
    multitrack.loop_span = Span(0.0, 12.0)
    track = multitrack.tracks.add("guitars", soloed=True)
    lane = track.active_take_lane
    first = place(lane, 0.0, 20.0, fade_out=Fade(length=4.0))
    second = lane.regions.add(
        16.0, 16.0, Content.onto(window(2), playrate=1.5, args={"seed": 7}),
        layer=1, muted=True, fade_in=Fade(length=4.0, shape="exp"))
    track.automation.add({"ctl": "level"}, [{"at": 0.0, "value": 0.0, "data": {}}],
                         visible=True)

    back = Multitrack.read(multitrack.write())
    assert back.write() == multitrack.write()
    assert back.end == 32.0
    assert back.loop_span == Span(0.0, 12.0)
    assert back.tracks[0].take_lanes[0].regions[1].content.playrate == 1.5
    assert second.overlaps(first)


def test_a_composite_is_a_fill_this_build_does_not_know():
    # A window is the one fill a region has. What the multitrack once held
    # and does not -- a composite, the general tree placed as a region -- is
    # among the fills this build does not know: read, kept, written back as it
    # came, and no window.
    written = {"fill": "composite",
               "node": {"id": 50, "kind": "aggregate", "grouping": "concrete"}}
    content = Content.read(written)
    assert content.fill == "composite" and content.window is None
    assert content.write() == written
    assert not hasattr(Content, "composite")


def test_a_field_a_newer_writer_added_survives_a_load_and_a_save():
    # Everything here carries what it has no name for. Dropping it would lose a
    # multitrack the next version of this client wrote.
    written = {
        "tracks": [{"id": 1, "take_lanes": [{"id": 2, "regions": [{
            "id": 3, "position": 0.0, "length": 4.0,
            "content": {"fill": "window", "window": window(1)},
            "warp": {"mode": "beats"}}]}]}],
        "tempo": [{"at": 0.0, "tempo": 2.0, "swing": 0.62}],
        "groove": {"name": "mpc60"},
    }
    multitrack = Multitrack.read(written)
    assert multitrack.write() == written
    assert multitrack.tracks[0].take_lanes[0].regions[0].extra == {"warp": {"mode": "beats"}}


def test_a_fill_this_build_does_not_know_is_carried_whole():
    written = {"fill": "video", "clip": "take1.mov", "offset": 0}
    assert Content.read(written).write() == written


def test_every_lane_names_its_source_and_not_only_the_one_that_plays():
    multitrack, track, first = one_lane()
    place(first, 0.0, 4.0, source=700)
    place(track.take_lanes.add(), 0.0, 4.0, source=701)
    named = [r.content.window["source"]["source"] for r in multitrack.regions()]
    assert named == [700, 701]


# ---- the session: the multitrack, and where its samples are ----

from clausters.document import SESSION_FORMAT  # noqa: E402
from clausters.multitrack import FrozenSource, Session, Source  # noqa: E402


def test_a_session_keeps_where_a_pass_ends():
    """The transport's end is the session's, in the three forms a playback
    takes it: none for a pass that rolls on -- which is not written --
    ``"contents"``, and a number of seconds for an end marker."""
    assert "end" not in Session().write()
    assert Session.read({"format": SESSION_FORMAT}).end is None
    for end in ("contents", 12.5):
        written = Session(end=end).write()
        assert written["end"] == end
        assert Session.read(written).end == end
        assert "end" not in Session.read(written).extra


def test_a_session_round_trips_with_its_table():
    multitrack, _, lane = one_lane()
    place(lane, 0.0, 4.0, source=700)
    session = Session(multitrack=multitrack,
                      sources={700: Source.file("take.wav").shaped(2, 480, 48_000.0)},
                      provenance={"script": "make.py"})
    written = session.write()
    assert written["sources"]["700"]["location"] == {"at": "file", "path": "take.wav"}
    assert Session.read(written) == session


def test_an_absent_arrangement_reads_as_an_empty_one_rather_than_as_nothing():
    # The crate's own rule, mirrored: a session always has a multitrack, possibly
    # empty, so nothing downstream has to ask whether there is one.
    session = Session.read({"format": SESSION_FORMAT})
    assert len(session.multitrack.tracks) == 0
    assert session.write() == {"format": SESSION_FORMAT}


def test_a_save_knows_what_it_cannot_promise():
    session = Session(sources={
        1: Source.file("kept.wav"),
        2: Source.volatile(),
        3: Source.file("scratch.wav", lifetime="temporary"),
    })
    session.sources[3].editing = {"from": 1, "confirmed": False}
    assert session.volatile() == [2], "samples nobody wrote down"
    assert session.open_edits() == [3], "an edit still undecided"

    # A save mid-edit promotes the working copy and leaves the edit open:
    # auto-confirming would turn a save into an edit.
    assert session.promote(3) and session.sources[3].lifetime == "session"
    assert session.open_edits() == [3], "still undecided, and that is the point"
    assert session.confirm(3) and session.open_edits() == []
    assert not session.promote(1), "nothing temporary about it"


def test_a_source_only_a_region_names_is_reported_missing():
    multitrack, track, first = one_lane()
    place(first, 0.0, 4.0, source=700)
    place(track.take_lanes.add(), 0.0, 4.0, source=701)
    session = Session(multitrack=multitrack, sources={700: Source.file("one.wav")})
    # Every lane, not only the one that plays.
    assert session.dangling() == [701]


def test_a_sequence_of_events_is_a_source_held_in_the_file():
    # The events are written into the file and read back as a sequence,
    # the handle itself held by the table, so a save writes what an editor did.
    from clausters.seq import EventSequence
    from clausters.seq.event import Event

    notes = EventSequence([(1.0, Event(midinote=60, sustain=0.5))])
    session = Session(sources={900: Source.events(notes)})
    notes.events.add(2.0, Event(midinote=64, sustain=0.5))
    written = session.write()
    location = written["sources"]["900"]["location"]
    assert location["at"] == "events" and len(location["sequence"]["events"]) == 2
    back = Session.read(written)
    assert back.sources[900].sequence.data() == notes.data()
    assert back.write() == written
    assert session.volatile() == [], "its events are in the file"
    assert session.sequences() == {900: notes}, "the handle itself"


def test_a_frozen_source_keeps_what_the_table_said():
    # A multitrack opened with no way to read its files must still write back every
    # location it was given -- without this it would save with every source
    # marked volatile, which is a format that loses its contents on the second
    # save.
    entry = Source.file("take.wav").shaped(2, 480, 48_000.0)
    frozen = FrozenSource(700, entry)
    assert frozen.bufnum == 700 and frozen.path == "take.wav"
    assert (frozen.channels, frozen.frames, frozen.sample_rate) == (2, 480, 48_000.0)
    assert FrozenSource(701).path is None


def test_a_session_field_a_newer_writer_added_survives():
    written = {"format": SESSION_FORMAT, "mixer": {"buses": [{"id": 1, "name": "reverb"}]}}
    assert Session.read(written).write() == written


def test_a_format_2_session_opens_in_seconds():
    """An older file is read through the crate's migration: its beats go to
    seconds through the tempo map it saved, and it is written back current."""
    old = {"format": 2, "multitrack": {
        "tempo": [{"at": 0.0, "bpm": 120.0}],
        "markers": [{"id": 1, "at": 8.0}],
        "tracks": [{"id": 1, "lanes": [{"id": 2, "regions": [{
            "id": 3, "position": 2.0, "length": 4.0,
            "content": {"fill": "window", "window": {
                "source": {"source": 1, "lifetime": "session"},
                "start": 0.0, "duration": 2.0}}}]}]}]}}
    session = Session.read(old)
    assert session.format == SESSION_FORMAT
    assert session.multitrack.tempo[0].tempo == 2.0
    region = session.multitrack.tracks[0].take_lanes[0].regions[0]
    assert (region.position, region.length) == (1.0, 2.0)
    assert session.multitrack.markers[0].at == 4.0


# ---- the presentation: what a window shows of a multitrack ----

from clausters.multitrack import TakeLaneView, TrackView, View  # noqa: E402


def a_multitrack() -> Multitrack:
    """Vocals (10) on take lanes 11 and 12, a region (20) on the first, and a
    second track (30) with its take lane (31) -- the ids a view is written by."""
    return Multitrack.read({"tracks": [
        {"id": 10, "take_lanes": [
            {"id": 11, "regions": [{"id": 20, "position": 0.0, "length": 4.0,
                                    "content": {"fill": "window", "window": window(700)}}]},
            {"id": 12}]},
        {"id": 30, "take_lanes": [{"id": 31}]}]})


def test_a_view_that_says_nothing_writes_an_empty_object():
    # The arrangement's rule, mirrored: a view of a multitrack nobody has touched
    # costs a file two braces.
    assert View().write() == {}


def test_a_view_says_nothing_about_what_plays():
    # The whole argument for parallel rather than a field on the model: drop
    # every view and the multitrack is the same multitrack.
    multitrack = a_multitrack()
    written = multitrack.write()
    vocals = multitrack.tracks[0]
    view = View(name="arranger", visible=Span(0.0, 32.0))
    view.track_view(vocals).height = 96.0
    session = Session(multitrack=multitrack, views=[view])
    back = Session.read(session.write())
    assert back.multitrack.write() == written
    assert back.views[0].track(back.multitrack.tracks[0]).height == 96.0


def test_two_windows_over_one_multitrack_are_two_views_and_disagree_on_purpose():
    arranger = View(name="arranger", visible=Span(0.0, 64.0), quant=4.0)
    editor = View(name="editor", visible=Span(8.0, 20.0), quant=0.25,
                  autofit=False)
    editor.detail = 20
    session = Session(multitrack=a_multitrack(), views=[arranger, editor])
    back = Session.read(session.write())
    assert len(back.views) == 2
    assert back.views[0].quant == 4.0
    assert back.views[1].quant == 0.25
    assert back.views[1].autofit is False
    assert back.views[1].detail == 20


def test_a_track_nobody_touched_reads_as_the_default_and_costs_nothing():
    vocals = a_multitrack().tracks[0]
    view = View()
    assert view.track(vocals) == TrackView()
    assert view.take_lane(vocals.take_lanes[0]) == TakeLaneView()
    assert view.tracks == {}, "asking is not touching"
    view.track_view(vocals).take_lanes_shown = True
    assert len(view.tracks) == 1


def test_state_goes_when_the_thing_goes():
    # The rule the client's screen-state tables were fixed to obey, in this
    # structure's terms: a height kept for a track that is not the same track is
    # a defect that looks like a feature.
    view = View()
    view.tracks[10] = TrackView(height=96.0)
    view.tracks[999] = TrackView(height=48.0)
    view.take_lanes[11] = TakeLaneView(height=24.0)
    view.selected = [20, 777]
    view.focused = 777
    view.detail = 20

    assert view.prune(a_multitrack()) is True
    assert list(view.tracks) == [10]
    assert list(view.take_lanes) == [11]
    assert view.selected == [20]
    assert view.focused is None
    assert view.detail == 20
    assert view.prune(a_multitrack()) is False, "and pruning twice finds nothing to do"


def test_a_field_a_newer_window_wrote_survives_a_load_and_a_save():
    written = {"name": "arranger", "fold": "tracks",
               "tracks": {"10": {"height": 96.0, "waveform": "rectified"}}}
    view = View.read(written)
    assert view.extra["fold"] == "tracks"
    assert view.tracks[10].extra["waveform"] == "rectified"
    assert view.write() == written


def test_a_session_written_without_views_reads_back_without_them():
    session = Session(multitrack=a_multitrack())
    assert "views" not in session.write()
    assert Session.read(session.write()).views == []


# ---- the history: a script's changes, recorded and walked ----

def test_a_multitrack_with_a_history_records_each_change_and_walks_it():
    multitrack, track, lane = one_lane()
    history = multitrack.history
    r = place(lane, 0.0, 2.0)
    r.position = 3.0
    assert history.undo_label == "move a region"
    assert history.undo() and r.position == 0.0
    assert history.undo() and not r.held, "the add, taken back"
    assert history.redo() and r.held and lane.regions[0] is r, "the same object"


def test_a_block_is_one_entry_undone_in_one_step():
    multitrack, track, lane = one_lane()
    history = multitrack.history
    r = place(lane, 0.0, 2.0)
    with multitrack.history("tidy"):
        r.position = 5.0
        r.length = 1.0
        track.name = "kept"
    assert history.undo_label == "tidy"
    history.undo()
    assert (r.position, r.length, track.name) == (0.0, 2.0, None)
    history.redo()
    assert (r.position, r.length, track.name) == (5.0, 1.0, "kept")


def test_a_multitrack_nobody_asked_a_history_of_records_nothing():
    multitrack, _, lane = one_lane()
    place(lane, 0.0, 2.0).position = 1.0
    from clausters.history import ATTR

    assert getattr(multitrack, ATTR, None) is None
