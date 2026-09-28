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
