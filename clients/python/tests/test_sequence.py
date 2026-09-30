"""An event sequence: events as concrete data, held by the document, each with
an id. The handle edits the Rust structure in place; these tests read it back
through the same handle.

The web client's `tests/sequence.test.ts` is the same suite; keep them reading
alike.
"""

import pytest

from clausters.base import TempoMap
from clausters.seq import Event, EventSequence


def _sequence():
    return EventSequence([(0.0, Event(midinote=60)), (1.0, Event(midinote=62)),
                          (2.0, Event(midinote=64))])


def test_events_are_held_in_beat_order_each_with_an_id():
    seq = EventSequence([(2.0, Event(midinote=64)), (0.0, Event(midinote=60))])
    assert len(seq) == 2
    assert [(beat, event.midinote()) for beat, event in seq] == [(0.0, 60), (2.0, 64)]
    assert sorted(id for id, _beat, _event in seq.entries()) == [1, 2]


def test_removing_an_event_leaves_its_neighbours_theirs():
    seq = _sequence()
    first, second, third = (id for id, _b, _e in seq.entries())
    seq.remove(first)
    seq.move(second, 3.0)
    assert seq.get(second) == (3.0, seq.get(second)[1])
    assert seq.get(second)[1]["midinote"] == 62
    assert seq.get(third)[1]["midinote"] == 64
    with pytest.raises(KeyError):
        seq.get(first)


def test_an_add_answers_its_id_and_an_edit_its_inverse():
    seq = _sequence()
    answer = seq.apply({"intent": "add", "event": {"at": 5.0, "data": {"midinote": 70}}})
    assert answer["applied"] and answer["id"] == 4
    seq.apply(answer["current"])                       # undo
    assert len(seq) == 3
    assert seq.add(4.0, {"midinote": 67}) == 5, "an id is never handed out twice"


def test_a_key_is_written_with_its_familys_coherence():
    seq = EventSequence([(0.0, Event(freq=440.0, midinote=69))])
    (id, _beat, _event), = seq.entries()
    seq.set(id, "midinote", 72)
    assert seq.get(id)[1].freq() == pytest.approx(523.2511306)


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
    assert EventSequence.from_data([{"at": 1.0, "data": {"midinote": 60}}]).entries()[0][0] == 1


def test_a_refused_edit_says_why():
    with pytest.raises(ValueError, match="no event 9"):
        _sequence().remove(9)


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
    assert seq.get(seq.entries()[-1][0])[1]["sustain"] == pytest.approx(0.5)
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


def test_a_lane_and_a_notes_expression_are_curves_the_sequence_holds():
    seq = _sequence()
    first = seq.entries()[0][0]
    lane = seq.add_lane({"cc": 74}, [(0.0, 0.0), (2.0, 127.0)], name="brightness")
    bend = seq.add_expression(first, {"bend": True}, [(0.0, 0.0), (0.5, 1.0)])
    data = seq.data()
    assert [(c["id"], c["name"], len(c["points"])) for c in data["lanes"]] == [
        (lane, "brightness", 2)]
    held = next(e for e in data["events"] if e["id"] == first)
    assert [c["id"] for c in held["expression"]] == [bend]
    assert lane != bend != first, "one counter for events and curves"
    seq.remove_expression(first, bend)
    seq.remove_lane(lane)
    data = seq.data()
    assert not data.get("lanes")
    assert not next(e for e in data["events"] if e["id"] == first).get("expression")


def test_a_midi_spec_admits_the_curves_it_can_say():
    seq = _sequence()
    first = seq.entries()[0][0]
    assert seq.midi is None, "a sequence for the server"
    seq.set_midi("1.0")
    assert seq.midi == "1.0"
    seq.add_expression(first, {"pressure": True}, [(0.0, 0.5)])
    with pytest.raises(ValueError, match="MIDI 1.0"):
        seq.add_expression(first, {"bend": True}, [(0.0, 0.0)])
    seq.set_midi("mpe", members=7)
    assert seq.midi == "mpe"
    assert seq.data()["midi"] == {"mpe": {"upper": False, "members": 7}}
    seq.add_expression(first, {"bend": True}, [(0.0, 0.0)])
    with pytest.raises(ValueError, match="bend"):
        seq.set_midi("1.0")
    seq.set_midi(None)
    assert seq.midi is None
