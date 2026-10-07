"""An event sequence: events as concrete data, held by the document, each with
an identity of its own. The handle edits the Rust structure in place, and a
script reads and writes it through objects -- a `SeqEvent` per event, an
`Automation` per curve -- that these tests read back.

The web client's `tests/sequence.test.ts` is the same suite; keep them reading
alike.
"""

import pytest

from clausters.base import TempoMap
from clausters.seq import Event, EventSequence


def _sequence():
    return EventSequence([(0.0, Event(midinote=60)), (1.0, Event(midinote=62)),
                          (2.0, Event(midinote=64))])


def test_events_are_held_in_beat_order():
    seq = EventSequence([(2.0, Event(midinote=64)), (0.0, Event(midinote=60))])
    assert len(seq) == 2
    assert [(beat, event.midinote()) for beat, event in seq] == [(0.0, 60), (2.0, 64)]
    assert [event.at for event in seq.events] == [0.0, 2.0]


def test_removing_an_event_leaves_its_neighbours_theirs():
    seq = _sequence()
    first, second, third = seq.events
    first.remove()
    second.at = 3.0
    assert second.at == 3.0 and second["midinote"] == 62
    assert third["midinote"] == 64 and list(seq.events) == [third, second]
    assert first.sequence is None
    with pytest.raises(ValueError, match="no longer holds"):
        first.at = 1.0


def test_an_add_answers_the_event_it_made():
    seq = _sequence()
    added = seq.events.add(5.0, {"midinote": 70})
    assert added is seq.events[-1] and added.at == 5.0 and added["midinote"] == 70
    again = seq.events.add(5.0, added)               # its keys, copied
    assert again is not added and seq.events.at(5.0) == [added, again]


def test_a_key_is_written_with_its_familys_coherence():
    seq = EventSequence([(0.0, Event(freq=440.0, midinote=69))])
    event, = seq.events
    event["midinote"] = 72
    assert event.event.freq() == pytest.approx(523.2511306)


def test_the_tempo_map_travels_with_the_events():
    seq = EventSequence([(0.0, {"midinote": 60, "sustain": 2.0})], tempo_map=TempoMap(2.0))
    assert seq.tempo_map.secs_at(4.0) == pytest.approx(2.0)
    seq.tempo_map = None
    assert seq.tempo_map is None
    assert seq.duration() == 2.0


def test_the_data_round_trips_ids_and_all():
    seq = _sequence()
    back = EventSequence.from_data(seq.data())
    assert back.data() == seq.data()
    assert EventSequence.from_data([{"at": 1.0, "data": {"midinote": 60}}]).events[0].at == 1.0


def test_the_events_are_objects_and_one_event_is_one_object():
    seq = EventSequence([(0.0, Event(midinote=60)), (1.0, Event(midinote=62)),
                         (1.0, Event(midinote=64)), (3.0, Event(midinote=67))])
    first = seq.events[0]
    assert first is seq.events[0] is next(iter(seq.events)), "the identity map"
    assert first.at == 0.0 and first["midinote"] == 60 and "midinote" in first
    assert first.event["midinote"] == 60 and isinstance(first.event, Event)
    assert [e["midinote"] for e in seq.events.at(1.0)] == [62, 64]
    assert [e["midinote"] for e in seq.events.range(0.5, 3.0)] == [62, 64], "half-open"
    assert seq.events[-1]["midinote"] == 67 and len(seq.events) == 4
    assert seq.events[:2] == [first, seq.events[1]] and seq.events[:2][0] is first, \
        "a slice is a list of the same objects"
    assert {first: "a key"}[seq.events[0]] == "a key"


def test_a_read_sees_what_the_sequence_holds_now():
    seq = _sequence()
    second = seq.events[1]
    seq._apply({"intent": "move", "id": 2, "at": 5.0})   # as a hand on the roll
    assert second.at == 5.0 and seq.events[-1] is second
    seq._apply({"intent": "remove", "id": 2})
    assert second.sequence is None
    with pytest.raises(ValueError, match="no longer holds"):
        second.at


def test_the_curves_read_as_automation_with_their_holder():
    seq = _sequence()
    seq._apply({"intent": "automation", "automation": {
        "id": 0, "target": {"cc": 74}, "name": "brightness",
        "points": [{"at": 0.0, "value": 0.0}, {"at": 2.0, "value": 127.0}]}})
    seq._apply({"intent": "eventautomation", "id": 1, "automation": {
        "id": 0, "target": {"bend": True}, "points": [{"at": 0.0, "value": 1.0}]}})
    brightness, = seq.automation
    assert brightness.name == "brightness" and brightness.target == {"cc": 74}
    assert brightness.to_points()[:2] == [0.0, 0.0] and brightness.held
    bend = seq.events[0].automation[0]
    assert bend is seq.events[0].automation[0]
    assert bend.target == {"bend": True} and len(seq.events[1].automation) == 0


def test_a_history_asked_for_records_the_scripts_changes_and_brings_objects_back():
    seq = _sequence()
    first, second, third = seq.events
    history = seq.history
    first.at = 0.5
    added = seq.events.add(3.0, {"midinote": 70})
    assert history.undo_label == "add an event" and history.can_undo
    assert history.undo() and added.sequence is None and len(seq) == 3
    assert history.redo() and added.sequence is seq and seq.events[-1] is added, \
        "a redone add brings back the same event, and the same object"
    second.remove()
    assert history.undo() and second.sequence is seq and second["midinote"] == 62
    curve = seq.automation.add({"cc": 1}, [(0.0, 0.0)])
    assert history.undo() and not curve.held
    assert history.redo() and curve.held and curve is seq.automation[0]


def test_a_block_is_one_entry_and_nests():
    seq = _sequence()
    with seq.history("humanize"):
        for event in seq.events:
            event["velocity"] = 90
            with seq.history("inner"):
                event.at += 0.25
    assert seq.history.undo_label == "humanize"
    assert [event.at for event in seq.events] == [0.25, 1.25, 2.25]
    assert seq.history.undo() is True
    assert [event.at for event in seq.events] == [0.0, 1.0, 2.0]
    assert seq.events[0].get("velocity") is None and not seq.history.can_undo


def test_a_refused_edit_says_why():
    seq = _sequence()
    seq.set_midi("1.0")
    with pytest.raises(ValueError, match="MIDI 1.0"):
        seq.events[0].automation.add({"bend": True}, [(0.0, 0.0)])


def test_a_sequence_goes_to_a_midi_file_and_back():
    seq = EventSequence([(0.0, Event(midinote=60, velocity=100, sustain=1.0)),
                         (1.5, Event(midinote=64, velocity=80, sustain=0.5, channel=2))],
                        tempo_map=TempoMap(4.0))
    data = seq.to_smf(ppq=96)
    assert data[:4] == b"MThd"
    back = EventSequence.from_smf(data)
    assert [(beat, e["midinote"], e["velocity"], e["sustain"], e.get("channel"))
            for beat, e in back] == [(0.0, 60, 100, 1.0, None), (1.5, 64, 80, 0.5, 2)]
    assert back.tempo_map.secs_at(4.0) == pytest.approx(1.0)
    with pytest.raises(ValueError, match="not a MIDI file"):
        EventSequence.from_smf(b"nope")


def test_a_timeline_renders_into_the_events_it_plays():
    from clausters.seq import OscItem, Pbind, Pseq, Timeline

    child = Timeline([(0.0, Event(midinote=72, dur=1.0, legato=1.0))], tempo=4.0)
    tl = Timeline([(0.0, Event(degree=0, dur=1.0, legato=0.5)),
                   (1.0, Pbind(midinote=Pseq([62, 64]), dur=0.5)),
                   (2.0, OscItem("/cue", 1)),
                   (3.0, child)], tempo=2.0)
    seq = tl.render_events()
    got = [(beat, e.get("type", "note"), e.get("midinote"), e.get("addr")) for beat, e in seq]
    assert got == [(0.0, "note", 60.0, None), (1.0, "note", 62, None),
                   (1.5, "note", 64, None), (2.0, "osc", None, "/cue"),
                   (3.0, "note", 72, None)]
    # The child's note lasts one of its beats: half a beat of the parent's.
    assert seq.events[-1]["sustain"] == pytest.approx(0.5)
    assert seq.tempo_map.secs_at(2.0) == pytest.approx(1.0), "the timeline's map rides along"
    assert tl.render_events(until=1.2).__len__() == 2


def test_a_pattern_renders_into_the_events_it_plays():
    from clausters.seq import Pbind, Pn, Pseq

    seq = Pbind(degree=Pseq([0, 2, 4]), dur=0.5).render_events()
    assert [(beat, e["degree"]) for beat, e in seq] == [(0.0, 0), (0.5, 2), (1.0, 4)]
    assert len(Pbind(degree=Pn(0)).render_events(until=3.5)) == 4


def test_a_score_reads_into_a_sequence_and_a_sequence_engraves():
    from clausters.gui import notation
    from clausters.seq import OscItem

    seq = EventSequence([(0.0, Event(midinote=60, dur=1.0)), (1.0, Event(midinote=64, dur=1.0)),
                         (1.5, OscItem("/cue"))])
    sheet = notation.sheet_from_timeline(seq)
    back = notation.to_sequence(sheet)
    assert [(beat, e["midinote"], e["dur"]) for beat, e in back] == [(0.0, 60, 1.0),
                                                                      (1.0, 64, 1.0)], \
        "the osc event has no pitch, so no note on the page"


def test_a_sequence_curve_and_a_notes_curve_are_curves_the_sequence_holds():
    seq = _sequence()
    first = seq.events[0]
    curve = seq.automation.add({"cc": 74}, [(0.0, 0.0), (2.0, 127.0)], name="brightness")
    bend = first.automation.add({"bend": True}, [(0.0, 0.0), (0.5, 1.0)])
    assert list(seq.automation) == [curve] and list(first.automation) == [bend]
    assert (curve.name, len(curve.points)) == ("brightness", 2)
    curve.points = [(0.0, 10.0), (4.0, 20.0)]               # written back whole
    assert seq.data()["automation"][0]["points"][1] == {"at": 4.0, "value": 20.0}
    assert curve is seq.automation[0], "it keeps its id, so it is the same object"
    bend.remove()
    curve.remove()
    assert not curve.held and len(seq.automation) == 0 and len(first.automation) == 0


def test_a_free_curve_becomes_the_view_once_added():
    from clausters.multitrack import Automation

    level = Automation({"control": "amp"}, [(0.0, 0.1), (4.0, 0.5)], name="level")
    assert not level.held and level.id == 0
    assert _sequence().automation.add(level) is level and level.held
    level.name = "loudness"
    assert level.write()["name"] == "loudness" and level.id != 0
    with pytest.raises(ValueError, match="held already"):
        _sequence().automation.add(level)


def test_a_midi_spec_admits_the_curves_it_can_say():
    seq = _sequence()
    first = seq.events[0]
    assert seq.midi is None, "a sequence for the server"
    seq.set_midi("1.0")
    assert seq.midi == "1.0"
    first.automation.add({"pressure": True}, [(0.0, 0.5)])
    with pytest.raises(ValueError, match="MIDI 1.0"):
        first.automation.add({"bend": True}, [(0.0, 0.0)])
    seq.set_midi("mpe", members=7)
    assert seq.midi == "mpe"
    assert seq.data()["midi"] == {"mpe": {"upper": False, "members": 7}}
    first.automation.add({"bend": True}, [(0.0, 0.0)])
    with pytest.raises(ValueError, match="bend"):
        seq.set_midi("1.0")
    seq.set_midi(None)
    assert seq.midi is None


def test_a_sequences_curves_go_to_a_midi_file_and_back():
    seq = EventSequence([(0.0, Event(midinote=60, velocity=100, sustain=1.0)),
                         (0.0, Event(midinote=64, velocity=100, sustain=1.0))])
    seq.set_midi("mpe")
    seq.automation.add({"cc": 7}, [(0.0, 100.0)])
    seq.events[0].automation.add({"bend": True}, [(0.0, 6.0)])
    messages = seq.midi_messages()
    assert messages[0] == (0.0, bytes([0xB0, 101, 0])), "the zone first"
    back = EventSequence.from_smf(seq.to_smf())
    assert back.midi == "mpe"
    assert [curve.target for curve in back.automation] == [{"cc": 7}], "the master's: the zone's"
    bent = next(e for e in back.events if len(e.automation))
    assert bent.automation[0].target == {"bend": True}
    assert bent.automation[0].points[0]["value"] == pytest.approx(6.0)


def test_a_sequence_goes_to_a_midi2_clip_and_back():
    seq = EventSequence([(0.0, Event(midinote=60, velocity=100, sustain=1.0))])
    seq.set_midi("2.0")
    first = seq.events[0]
    first.automation.add({"bend": True}, [(0.0, 12.0)])
    first.automation.add({"cc": 1}, [(0.0, 64.0)])
    data = seq.to_clip()
    assert data[:8] == b"SMF2CLIP"
    back = EventSequence.from_clip(data)
    assert back.midi == "2.0"
    values = {list(c.target)[0]: c.points[0]["value"] for c in back.events[0].automation}
    assert values["bend"] == pytest.approx(12.0)
    assert values["cc"] == pytest.approx(64.0, abs=1e-3)
    with pytest.raises(ValueError, match="SMF2CLIP"):
        EventSequence.from_clip(b"MThd")


def test_a_sequence_curve_goes_to_its_notes_and_a_chord_gives_it_back():
    seq = EventSequence([(1.0, Event(midinote=m, sustain=2.0)) for m in (60, 64, 67)])
    curve = seq.automation.add({"bend": True, "channel": 0}, [(0.0, 0.0), (4.0, 4.0)])
    seq.automation.to_events(curve)
    assert len(seq.automation) == 0 and not curve.held
    for event in seq.events:
        points = [(p["at"], p["value"]) for p in event.automation[0].points]
        assert points == [(0.0, 1.0), (2.0, 3.0)]
    back = seq.automation.from_events({"bend": True})
    assert list(seq.automation) == [back]
    assert back.target == {"bend": True, "channel": 0}
    first, second = seq.events[0], seq.events[1]
    first.automation.add({"pressure": True}, [(0.0, 0.0), (2.0, 1.0)])
    second.automation.add({"pressure": True}, [(0.0, 0.0), (2.0, 0.5)])
    with pytest.raises(ValueError, match="cannot say both"):
        seq.automation.from_events({"pressure": True})


def test_a_sequence_is_read_as_rows_of_the_keys_chosen():
    seq = EventSequence([
        (0.0, Event(midinote=60, dur=1.0)),
        (1.0, Event(degree=2, dur=0.5, velocity=127)),
        (1.5, {"pitches": [{"step": "e", "alter": -1, "octave": 4}], "dur": 2.0}),
    ])
    assert seq.to_rows("midinote", "dur") == [(60.0, 1.0), (64.0, 0.5), (63.0, 2.0)]
    placed = seq.to_rows("at", "amp", "pan")
    assert placed[1] == (1.0, 1.0, None), "a velocity has an amp; a key it lacks is None"


def test_a_line_of_rows_plays_back_to_back_and_reads_back():
    rows = [(None, 0.5), (60.0, 1.0), ([64.0, 67.0], 1.5), (None, 1.0), (72.0, 2.0)]
    seq = EventSequence.from_rows(rows)
    assert [e.at for e in seq.events] == [0.5, 1.5, 1.5, 4.0]
    assert seq.to_rows("midinote", "dur", line=True) == rows
    # one row per event otherwise, and rows that name their places keep them
    assert seq.to_rows("midinote") == [(60.0,), (64.0,), (67.0,), (72.0,)]
    placed = EventSequence.from_rows([(2.0, 60), (0.0, 72)], ("at", "midinote"))
    assert placed.to_rows("at", "midinote") == [(0.0, 72.0), (2.0, 60.0)]
    with pytest.raises(ValueError, match="row 0"):
        EventSequence.from_rows([(60,)])


def test_a_sequence_is_separated_into_its_lines():
    seq = EventSequence([
        (0.0, Event(midinote=72, dur=1.0, staff=0, voice=0)),
        (0.0, Event(midinote=60, dur=2.0, staff=0, voice=1, channel=1)),
        (0.0, Event(midinote=48, dur=2.0, staff=1, voice=0, channel=2)),
    ])
    seq.set_midi("1.0")
    seq.automation.add({"cc": 11, "channel": 2}, [(0.0, 50.0)])
    voices = seq.separate()
    assert [part.to_rows("midinote") for part in voices] == [[(72.0,)], [(60.0,)], [(48.0,)]]
    assert [len(part.automation) for part in voices] == [0, 0, 1]
    kept = [row for part in voices for row in part.to_rows("id")]
    assert kept == seq.to_rows("id"), "an event keeps the id it had"
    bass = voices[2]
    assert bass.to_rows("staff", "channel") == [(0, 2)], "its staff is the top one"
    assert bass.midi == "1.0"
    assert len(seq.separate("staff")) == 2 and len(seq.separate("channel")) == 3
    assert len(seq) == 3, "the whole is not changed"
    # a line whose note bends on its own is a channel to each note
    bass.set_midi("2.0")
    bass.events[0].automation.add({"bend": True}, [(0.0, 0.0), (2.0, 2.0)])
    assert bass.separate("channel")[0].midi == "mpe"
    with pytest.raises(ValueError, match="separated by"):
        seq.separate("track")
