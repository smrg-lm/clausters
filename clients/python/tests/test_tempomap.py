"""A tempo map as a value: shared between clocks, written out, read back.

The map is not a field of anything -- it is a value on the beat axis, the peer
of a `Timeline`, and a clock is the process that moves over it. These are the
three facts that follow, and `clients/web/tests/tempomap.test.ts` asserts the
same ones in the same order.
"""

import pytest

from clausters._native import TempoMap
from clausters.base.clock import TempoClock


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
