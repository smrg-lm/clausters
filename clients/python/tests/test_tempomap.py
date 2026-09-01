"""A tempo map as a value: shared between clocks, written out, read back.

The map is not a field of anything -- it is a value on the beat axis, the peer
of a `Timeline`, and a clock is the process that moves over it. These are the
three facts that follow, and `clients/web/tests/tempomap.test.ts` asserts the
same ones in the same order.
"""

import json

import pytest

from clausters._native import TempoMap
from clausters.base.clock import TempoClock
from clausters.base.stream import Routine


def test_every_clock_builds_its_own_map():
    a, b = TempoClock(2.0), TempoClock(2.0)
    a.set_tempo(4.0)
    assert a.tempo == 4.0
    assert b.tempo == 2.0


def test_two_clocks_handed_one_map_are_reading_one_piece():
    piece = TempoMap(1.0)
    piece.push(4.0, 2.0)  # written ahead of any clock: the NRT half
    lead = TempoClock(tempo_map=piece)
    second = TempoClock(tempo_map=piece)
    assert lead.beats2secs(8.0) == 6.0
    assert second.beats2secs(8.0) == 6.0

    # ...and a live gesture on one is on both: the RT half, on the same map.
    lead.set_tempo(3.0, over=4.0, unit="seconds", curve="exponential")
    assert lead.map.version == second.map.version
    assert lead.beats2secs(20.0) == second.beats2secs(20.0)
    assert lead.beats2secs(0.0) == 0.0  # the past is untouched


def test_a_fork_stops_the_two_being_one():
    piece = TempoMap(1.0)
    own = TempoClock(tempo_map=piece.copy())
    own.set_tempo(9.0)
    assert own.tempo == 9.0
    assert piece.tempo_at(0.0) == 1.0


def test_a_live_gesture_lands_on_a_map_written_ahead_of_the_clock():
    # The append-only rule is the map's and stays: push refuses to go
    # backwards. Saying "from here on" is the gesture's job.
    piece = TempoMap(1.0)
    piece.push(4.0, 2.0)
    with pytest.raises(ValueError):
        piece.push(1.0, 3.0)
    clock = TempoClock(tempo_map=piece)
    clock.set_tempo(3.0)  # at beat 0, under the breakpoint at 4
    assert clock.tempo == 3.0
    assert clock.beats2secs(8.0) == 8.0 / 3.0  # the plan after it is gone


def test_a_map_round_trips_through_its_breakpoints():
    m = TempoMap(1.0)
    m.shaped(2.0, 6.0, 1.0, 2.0, curve="exponential")
    json = m.dumps()
    assert "secs" not in json  # the integral is derived
    back = TempoMap.loads(json)
    for b in (-1.0, 0.0, 2.0, 4.5, 9.0):
        assert back.secs_at(b) == m.secs_at(b)
    assert back.version == 1  # a loaded map has had no edits


def test_a_stored_map_is_checked_by_the_door_that_reads_it():
    for json in ("[]", '[{"beats":0.0,"tempo":0.0}]', "not json"):
        with pytest.raises(ValueError):
            TempoMap.loads(json)


def test_a_gesture_says_where_it_is_written():
    # A piece's tempo, written before any clock has run: `at` is the whole of
    # what makes that possible from the clock's own verb.
    clock = TempoClock(1.0)
    clock.set_tempo(2.0, at=8.0)
    clock.set_tempo(4.0, over=4.0, at=16.0, curve="exponential")
    assert clock.beats2secs(8.0) == 8.0
    assert clock.beats2secs(12.0) == 10.0
    assert [p["beats"] for p in json.loads(clock.map.dumps())] == [0.0, 8.0, 16.0, 20.0]


def test_a_gesture_inside_a_routine_is_written_at_the_routines_own_beat():
    # `beats()` is the paced beat and the routine's is the yield-exact one; a
    # breakpoint at 3.00034 is inaudible and stays in the map forever. So a
    # gesture made from inside a routine on this clock is written where the
    # routine is -- exactly where its notes are.
    clock = TempoClock(100.0)  # fast, so the run is short and the pacing drifts
    written = []

    def melody():
        yield 3.0
        clock.set_tempo(200.0)
        written.append(json.loads(clock.map.dumps())[-1]["beats"])
        yield 1.0

    Routine(melody).play(clock)
    clock.run(0.2)
    assert written == [3.0]  # not 3.0004, which is where `beats()` would be


def test_a_clock_is_saved_as_a_name_and_a_map():
    # What of a clock belongs to the piece: the tempo, and the name a lane
    # refers to it by. Not its position, not its queue, not its timebase.
    clock = TempoClock(2.0, name="lead")
    clock.set_tempo(4.0, over=8.0, at=4.0, curve="exponential")
    back = TempoClock.loads(clock.dumps())
    assert back.name == "lead"
    assert back.beats2secs(12.0) == clock.beats2secs(12.0)
    assert "timebase" not in clock.dumps()
    with pytest.raises(ValueError):
        TempoClock.loads("{}")


def test_polytempo_is_several_named_clocks():
    # The reason the saved unit is a clock and not "the" tempo: a canon at
    # three tempi is three of them, and a lane says which one it runs on.
    canon = [TempoClock(t, name=f"voice {i}") for i, t in enumerate((1.0, 1.5, 2.0))]
    written = [c.dumps() for c in canon]
    read = [TempoClock.loads(j) for j in written]
    assert [c.name for c in read] == ["voice 0", "voice 1", "voice 2"]
    assert [c.tempo for c in read] == [1.0, 1.5, 2.0]
