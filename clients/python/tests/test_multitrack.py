"""The arrangement's client side (`clausters.multitrack`).

What the crate defines, written and read here. The crossing that proves the two
agree is `crates/clausters-document/tests/arrangement_parity.rs`, over a vector
this client generates; what this suite checks is the surface a person actually
types against, and the few rules that are the client's own to keep.
"""

import pytest
from clausters.multitrack import (Multitrack, Automation, Content, Fade,
                                   Lane, Marker, Meter, Region, Span, Tempo,
                                   Track)


def window(source: int, start: float = 0.0, duration: float = 4.0) -> dict:
    return {"source": {"source": source, "lifetime": "session", "generation": 0},
            "start": start, "duration": duration}


def region(id: int, at: float, length: float, source: int = 1) -> Region:
    return Region(id=id, position=at, length=length,
                  content=Content.onto(window(source)))


def test_a_region_ends_where_its_span_ends_and_not_where_its_content_does():
    # The window is four seconds of the source; the region shows one beat of
    # it. Trimming moves the region and never the source.
    r = region(1, 4.0, 1.0)
    assert r.end == 5.0
    assert r.content.window["duration"] == 4.0


def test_regions_that_touch_do_not_overlap_and_regions_that_share_time_do():
    assert not region(1, 0.0, 4.0).overlaps(region(2, 4.0, 4.0))
    assert region(1, 0.0, 4.0).overlaps(region(2, 3.0, 4.0))


def test_one_source_under_six_regions_is_referenced_and_not_copied():
    # Six placements, six identities, one source. That is the whole of
    # non-destructive editing, and it is what a region's own id is for.
    lane = Lane(id=10)
    for i in range(6):
        lane.place(region(100 + i, i * 4.0, 4.0, source=1))
    assert len(lane.regions) == 6
    assert {r.content.window["source"]["source"] for r in lane.regions} == {1}
    assert lane.region(103).position == 12.0
    assert lane.region(999) is None


def test_placing_keeps_a_lane_in_position_order():
    lane = Lane(id=10)
    for id, at in ((3, 8.0), (1, 0.0), (2, 4.0)):
        lane.place(region(id, at, 2.0))
    assert [r.position for r in lane.regions] == [0.0, 4.0, 8.0]
    assert lane.end == 10.0


def test_a_track_spans_every_lane_and_plays_one():
    track = Track(id=1, lanes=[Lane(id=10), Lane(id=11, name="take 2")], active=0)
    track.active_lane.place(region(100, 0.0, 4.0))
    track.lanes[1].place(region(200, 0.0, 16.0))
    assert track.active_lane.id == 10
    assert track.active_lane.end == 4.0
    # An alternate take is still part of the multitrack.
    assert track.end == 16.0


def test_an_active_lane_that_is_not_there_answers_nothing():
    assert Track(id=1, lanes=[Lane(id=10)], active=7).active_lane is None


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


def test_a_piece_that_never_said_a_tempo_says_nothing():
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
    multitrack.add_marker(Marker(id=1, at=16.0, name="B"))
    multitrack.add_marker(Marker(id=2, at=16.0, name="chorus"))
    multitrack.add_marker(Marker(id=3, at=0.0, name="A"))
    assert [m.name for m in multitrack.markers] == ["A", "B", "chorus"]


def test_a_span_that_meets_the_next_one_covers_no_instant_twice():
    first, then = Span(0.0, 8.0), Span(8.0, 16.0)
    assert first.length == 8.0 and first.end == then.start


def test_nothing_said_is_nothing_written():
    # An empty multitrack writes an empty object rather than zero of everything, and
    # a plain region writes no layer, no fades, no mute and no playrate.
    assert Multitrack().write() == {}
    written = region(1, 0.0, 4.0).write()
    assert set(written) == {"id", "position", "length", "content"}
    assert "playrate" not in written["content"]


def test_the_piece_carries_its_own_version_and_keeps_it_out_of_an_empty_file():
    # The counter a stale edit is stale against, and it is the multitrack's rather
    # than the document's: an editor of one is not editing the other. It stays
    # out of the file while it is the first version, so an unedited multitrack still
    # writes an empty object and a file that never named one reads back at it.
    assert Multitrack().version == 1
    assert "version" not in Multitrack().write()
    assert Multitrack.read({}).version == 1
    edited = Multitrack(version=4)
    assert edited.write()["version"] == 4
    assert Multitrack.read(edited.write()).version == 4


def test_a_whole_piece_round_trips():
    multitrack = Multitrack()
    multitrack.set_tempo(Tempo(at=0.0, tempo=1.6))
    multitrack.set_meter(Meter(at=0.0, beats=7, unit=8))
    multitrack.loop_span = Span(0.0, 12.0)
    track = Track(id=1, name="guitars", soloed=True, lanes=[Lane(id=2)])
    first = track.lanes[0].place(region(3, 0.0, 20.0))
    first.fade_out = Fade(length=4.0)
    second = track.lanes[0].place(Region(
        id=4, position=16.0, length=16.0, layer=1, muted=True,
        content=Content.onto(window(2), playrate=1.5, args={"seed": 7}),
        fade_in=Fade(length=4.0, shape={"curve": "exp"})))
    track.automation.append(Automation(
        id=5, target={"ctl": "level"}, visible=True,
        points=[{"at": 0.0, "value": 0.0, "data": {}}]))
    multitrack.tracks.append(track)

    back = Multitrack.read(multitrack.write())
    assert back == multitrack
    assert back.end == 32.0
    assert back.tracks[0].lanes[0].regions[1].content.playrate == 1.5
    assert second.overlaps(first)


def test_a_region_carries_curves_of_its_own_and_they_are_not_its_tracks():
    """The two places a curve belongs: a track's runs the length of the track
    and is drawn in a lane beside it, a region's runs the length of the region
    and is drawn inside it. One type, so one reader — which is what a round trip
    checks."""
    r = region(3, 0.0, 20.0)
    r.automation.append(Automation(
        id=9, name="gain", target={"ctl": "gain"},
        points=[{"at": 0.0, "value": 1.0, "data": {}}]))
    back = Region.read(r.write())
    assert back == r
    assert back.automation[0].target == {"ctl": "gain"}
    # A region with none writes none: nothing said is nothing written.
    assert "automation" not in region(4, 0.0, 4.0).write()


def test_a_composite_region_carries_the_general_tree_unchanged():
    node = {"id": 50, "kind": "aggregate", "grouping": "concrete", "members": []}
    r = Region(id=1, position=0.0, length=8.0, content=Content.composite(node))
    assert Region.read(r.write()).content.node == node


def test_a_field_a_newer_writer_added_survives_a_load_and_a_save():
    # Everything here carries what it has no name for. Dropping it would lose a
    # multitrack the next version of this client wrote.
    written = {
        "tracks": [{"id": 1, "lanes": [{"id": 2, "regions": [{
            "id": 3, "position": 0.0, "length": 4.0,
            "content": {"fill": "window", "window": window(1)},
            "warp": {"mode": "beats"}}]}]}],
        "tempo": [{"at": 0.0, "tempo": 2.0, "swing": 0.62}],
        "groove": {"name": "mpc60"},
    }
    multitrack = Multitrack.read(written)
    assert multitrack.write() == written


def test_a_fill_this_build_does_not_know_is_carried_whole():
    written = {"fill": "video", "clip": "take1.mov", "offset": 0}
    assert Content.read(written).write() == written


def test_every_lane_names_its_source_and_not_only_the_one_that_plays():
    multitrack = Multitrack()
    track = Track(id=1, lanes=[Lane(id=2), Lane(id=3)], active=0)
    track.lanes[0].place(region(4, 0.0, 4.0, source=700))
    track.lanes[1].place(region(5, 0.0, 4.0, source=701))
    multitrack.tracks.append(track)
    named = [r.content.window["source"]["source"] for r in multitrack.regions()]
    assert named == [700, 701]


# ---- the session: the multitrack, and where its samples are ----

from clausters.multitrack import FrozenSource, Session, Source  # noqa: E402


def test_a_session_round_trips_with_its_table():
    multitrack = Multitrack()
    multitrack.tracks.append(Track(id=1, lanes=[Lane(id=2)]))
    multitrack.tracks[0].lanes[0].place(region(3, 0.0, 4.0, source=700))
    session = Session(multitrack=multitrack,
                      sources={700: Source.file("take.wav").shaped(2, 480, 48_000.0)},
                      provenance={"script": "make.py"})
    written = session.write()
    assert written["sources"]["700"]["location"] == {"at": "file", "path": "take.wav"}
    assert Session.read(written) == session


def test_an_absent_arrangement_reads_as_an_empty_one_rather_than_as_nothing():
    # The crate's own rule, mirrored: a session always has a multitrack, possibly
    # empty, so nothing downstream has to ask whether there is one.
    session = Session.read({"format": 3})
    assert session.multitrack.tracks == []
    assert session.write() == {"format": 3}


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
    multitrack = Multitrack()
    track = Track(id=1, lanes=[Lane(id=2), Lane(id=3)])
    track.lanes[0].place(region(4, 0.0, 4.0, source=700))
    track.lanes[1].place(region(5, 0.0, 4.0, source=701))
    multitrack.tracks.append(track)
    session = Session(multitrack=multitrack, sources={700: Source.file("one.wav")})
    # Every lane, not only the one that plays.
    assert session.dangling() == [701]


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
    written = {"format": 3, "mixer": {"buses": [{"id": 1, "name": "reverb"}]}}
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
    assert session.format == 3
    assert session.multitrack.tempo[0].tempo == 2.0
    region = session.multitrack.tracks[0].lanes[0].regions[0]
    assert (region.position, region.length) == (1.0, 2.0)
    assert session.multitrack.markers[0].at == 4.0


# ---- the presentation: what a window shows of a multitrack ----

from clausters.multitrack import LaneView, TrackView, View  # noqa: E402


def a_piece() -> Multitrack:
    multitrack = Multitrack()
    vocals = Track(id=10, lanes=[Lane(id=11), Lane(id=12)])
    vocals.lanes[0].place(region(20, 0.0, 4.0, source=700))
    multitrack.tracks.extend([vocals, Track(id=30, lanes=[Lane(id=31)])])
    return multitrack


def test_a_view_that_says_nothing_writes_an_empty_object():
    # The arrangement's rule, mirrored: a view of a multitrack nobody has touched
    # costs a file two braces.
    assert View().write() == {}


def test_a_view_says_nothing_about_what_plays():
    # The whole argument for parallel rather than a field on the model: drop
    # every view and the multitrack is the same multitrack.
    multitrack = a_piece()
    written = multitrack.write()
    view = View(name="arranger", visible=Span(0.0, 32.0))
    view.track_view(10).height = 96.0
    session = Session(multitrack=multitrack, views=[view])
    back = Session.read(session.write())
    assert back.multitrack.write() == written
    assert back.views[0].track(10).height == 96.0


def test_two_windows_over_one_piece_are_two_views_and_disagree_on_purpose():
    arranger = View(name="arranger", visible=Span(0.0, 64.0), quant=4.0)
    editor = View(name="editor", visible=Span(8.0, 20.0), quant=0.25,
                  autofit=False)
    editor.detail = 20
    session = Session(multitrack=a_piece(), views=[arranger, editor])
    back = Session.read(session.write())
    assert len(back.views) == 2
    assert back.views[0].quant == 4.0
    assert back.views[1].quant == 0.25
    assert back.views[1].autofit is False
    assert back.views[1].detail == 20


def test_a_track_nobody_touched_reads_as_the_default_and_costs_nothing():
    view = View()
    assert view.track(10) == TrackView()
    assert view.lane(11) == LaneView()
    assert view.tracks == {}, "asking is not touching"
    view.track_view(10).lanes_shown = True
    assert len(view.tracks) == 1


def test_state_goes_when_the_thing_goes():
    # The rule the client's screen-state tables were fixed to obey, in this
    # structure's terms: a height kept for a track that is not the same track is
    # a defect that looks like a feature.
    view = View()
    view.track_view(10).height = 96.0
    view.track_view(999).height = 48.0
    view.lane_view(11).height = 24.0
    view.selected = [20, 777]
    view.focused = 777
    view.detail = 20

    assert view.prune(a_piece()) is True
    assert list(view.tracks) == [10]
    assert list(view.lanes) == [11]
    assert view.selected == [20]
    assert view.focused is None
    assert view.detail == 20
    assert view.prune(a_piece()) is False, "and pruning twice finds nothing to do"


def test_a_field_a_newer_window_wrote_survives_a_load_and_a_save():
    written = {"name": "arranger", "fold": "tracks",
               "tracks": {"10": {"height": 96.0, "waveform": "rectified"}}}
    view = View.read(written)
    assert view.extra["fold"] == "tracks"
    assert view.tracks[10].extra["waveform"] == "rectified"
    assert view.write() == written


def test_a_session_written_without_views_reads_back_without_them():
    session = Session(multitrack=a_piece())
    assert "views" not in session.write()
    assert Session.read(session.write()).views == []
