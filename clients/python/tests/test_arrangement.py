"""The arrangement's client side (`clausters.arrangement`).

What the crate defines, written and read here. The crossing that proves the two
agree is `crates/clausters-document/tests/arrangement_parity.rs`, over a vector
this client generates; what this suite checks is the surface a person actually
types against, and the few rules that are the client's own to keep.
"""

from clausters.arrangement import (Arrangement, Automation, Content, Fade,
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
    # An alternate take is still part of the piece.
    assert track.end == 16.0


def test_an_active_lane_that_is_not_there_answers_nothing():
    assert Track(id=1, lanes=[Lane(id=10)], active=7).active_lane is None


def test_the_map_answers_the_entry_in_force_and_nothing_before_the_first():
    piece = Arrangement()
    piece.set_tempo(Tempo(at=8.0, bpm=90.0))
    piece.set_tempo(Tempo(at=0.0, bpm=120.0))
    piece.set_tempo(Tempo(at=16.0, bpm=60.0, ramp=True))
    assert [t.at for t in piece.tempo] == [0.0, 8.0, 16.0]
    assert piece.tempo_at(7.9).bpm == 120.0
    assert piece.tempo_at(8.0).bpm == 90.0
    assert piece.tempo_at(20.0).ramp


def test_a_piece_that_never_said_a_tempo_says_nothing():
    # No 120 invented: naming a default would decide a musical question the
    # document has no business in.
    assert Arrangement().tempo_at(0.0) is None
    assert Arrangement().meter_at(0.0) is None


def test_two_tempos_at_one_beat_is_a_state_the_map_cannot_hold():
    piece = Arrangement()
    piece.set_tempo(Tempo(at=4.0, bpm=120.0))
    piece.set_tempo(Tempo(at=4.0, bpm=90.0))
    assert len(piece.tempo) == 1 and piece.tempo_at(4.0).bpm == 90.0


def test_two_markers_may_share_a_beat_because_people_do_that():
    piece = Arrangement()
    piece.add_marker(Marker(id=1, at=16.0, name="B"))
    piece.add_marker(Marker(id=2, at=16.0, name="chorus"))
    piece.add_marker(Marker(id=3, at=0.0, name="A"))
    assert [m.name for m in piece.markers] == ["A", "B", "chorus"]


def test_a_span_that_meets_the_next_one_covers_no_beat_twice():
    first, then = Span(0.0, 8.0), Span(8.0, 16.0)
    assert first.length == 8.0 and first.end == then.start


def test_nothing_said_is_nothing_written():
    # An empty piece writes an empty object rather than zero of everything, and
    # a plain region writes no layer, no fades, no mute and no playrate.
    assert Arrangement().write() == {}
    written = region(1, 0.0, 4.0).write()
    assert set(written) == {"id", "position", "length", "content"}
    assert "playrate" not in written["content"]


def test_a_whole_piece_round_trips():
    piece = Arrangement()
    piece.set_tempo(Tempo(at=0.0, bpm=96.0))
    piece.set_meter(Meter(at=0.0, beats=7, unit=8))
    piece.loop_span = Span(0.0, 12.0)
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
    piece.tracks.append(track)

    back = Arrangement.read(piece.write())
    assert back == piece
    assert back.end == 32.0
    assert back.tracks[0].lanes[0].regions[1].content.playrate == 1.5
    assert second.overlaps(first)


def test_a_composite_region_carries_the_general_tree_unchanged():
    node = {"id": 50, "kind": "aggregate", "grouping": "concrete", "members": []}
    r = Region(id=1, position=0.0, length=8.0, content=Content.composite(node))
    assert Region.read(r.write()).content.node == node


def test_a_field_a_newer_writer_added_survives_a_load_and_a_save():
    # Everything here carries what it has no name for. Dropping it would lose a
    # piece the next version of this client wrote.
    written = {
        "tracks": [{"id": 1, "lanes": [{"id": 2, "regions": [{
            "id": 3, "position": 0.0, "length": 4.0,
            "content": {"fill": "window", "window": window(1)},
            "warp": {"mode": "beats"}}]}]}],
        "tempo": [{"at": 0.0, "bpm": 120.0, "swing": 0.62}],
        "groove": {"name": "mpc60"},
    }
    piece = Arrangement.read(written)
    assert piece.write() == written


def test_a_fill_this_build_does_not_know_is_carried_whole():
    written = {"fill": "video", "clip": "take1.mov", "offset": 0}
    assert Content.read(written).write() == written


def test_every_lane_names_its_source_and_not_only_the_one_that_plays():
    piece = Arrangement()
    track = Track(id=1, lanes=[Lane(id=2), Lane(id=3)], active=0)
    track.lanes[0].place(region(4, 0.0, 4.0, source=700))
    track.lanes[1].place(region(5, 0.0, 4.0, source=701))
    piece.tracks.append(track)
    named = [r.content.window["source"]["source"] for r in piece.regions()]
    assert named == [700, 701]


# ---- the session: the piece, and where its samples are ----

from clausters.arrangement import FrozenSource, Session, Source  # noqa: E402


def test_a_session_round_trips_with_its_table():
    piece = Arrangement()
    piece.tracks.append(Track(id=1, lanes=[Lane(id=2)]))
    piece.tracks[0].lanes[0].place(region(3, 0.0, 4.0, source=700))
    session = Session(arrangement=piece,
                      sources={700: Source.file("take.wav").shaped(2, 480, 48_000.0)},
                      provenance={"script": "make.py"})
    written = session.write()
    assert written["sources"]["700"]["location"] == {"at": "file", "path": "take.wav"}
    assert Session.read(written) == session


def test_an_absent_arrangement_reads_as_an_empty_one_rather_than_as_nothing():
    # The crate's own rule, mirrored: a session always has a piece, possibly
    # empty, so nothing downstream has to ask whether there is one.
    session = Session.read({"format": 1})
    assert session.arrangement.tracks == []
    assert session.write() == {"format": 1}


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
    piece = Arrangement()
    track = Track(id=1, lanes=[Lane(id=2), Lane(id=3)])
    track.lanes[0].place(region(4, 0.0, 4.0, source=700))
    track.lanes[1].place(region(5, 0.0, 4.0, source=701))
    piece.tracks.append(track)
    session = Session(arrangement=piece, sources={700: Source.file("one.wav")})
    # Every lane, not only the one that plays.
    assert session.dangling() == [701]


def test_a_frozen_source_keeps_what_the_table_said():
    # A piece opened with no way to read its files must still write back every
    # location it was given -- without this it would save with every source
    # marked volatile, which is a format that loses its contents on the second
    # save.
    entry = Source.file("take.wav").shaped(2, 480, 48_000.0)
    frozen = FrozenSource(700, entry)
    assert frozen.bufnum == 700 and frozen.path == "take.wav"
    assert (frozen.channels, frozen.frames, frozen.sample_rate) == (2, 480, 48_000.0)
    assert FrozenSource(701).path is None


def test_a_session_field_a_newer_writer_added_survives():
    written = {"format": 1, "mixer": {"buses": [{"id": 1, "name": "reverb"}]}}
    assert Session.read(written).write() == written
