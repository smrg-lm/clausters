"""One physical time, the same live and offline, and `TempoClock.locate`.

Offline, a `LogicalTimebase` stands in for physical time: every clock of the
run has its origin on it, and a render wakes whatever is due next across all of
them, in seconds. So a script with several clocks renders as it plays, and a
locate is the same operation on either time.
"""

import pytest

from clausters import Session
from clausters.base import LogicalTimebase, ManualTimebase, Routine, TempoClock


def _emits(session):
    """``(seconds, address)`` of each bundle in the session's score, in order."""
    out = []
    for when, packet in session.server.interface.score.bundles:
        for addr in (b"/a", b"/b", b"/x"):
            if addr in packet:
                out.append((round(when, 9), addr.decode()))
    return sorted(out)


def _emitter(server, addr, beats):
    def routine():
        for i, beat in enumerate(beats):
            server.send_bundle((addr,))
            if i + 1 < len(beats):
                yield beats[i + 1] - beat
    return Routine(routine)


def test_two_clocks_of_one_script_render_together_in_seconds():
    s = Session.nrt(tempo=1.0)
    fast = s.adopt(TempoClock(2.0))
    assert fast.timebase is s.clock.timebase, "a session's clocks share its time"
    fast.start()
    s.clock.play(_emitter(s.server, "/a", [0, 1, 2]))
    fast.play(_emitter(s.server, "/b", [0, 1, 2, 3]))
    s.clock.render()
    assert _emits(s) == [
        (0.0, "/a"), (0.0, "/b"), (0.5, "/b"), (1.0, "/a"),
        (1.0, "/b"), (1.5, "/b"), (2.0, "/a"),
    ]


def test_a_clock_started_at_second_4_starts_at_second_4():
    s = Session.nrt(tempo=1.0)

    def later():
        yield 4
        late = TempoClock(2.0).start()
        late.play(_emitter(s.server, "/b", [0, 1]))

    s.clock.play(Routine(later))
    s.clock.render()
    assert _emits(s) == [(4.0, "/b"), (4.5, "/b")]


def test_an_offline_clock_that_is_never_started_does_not_play_as_live():
    s = Session.nrt(tempo=1.0)
    idle = s.adopt(TempoClock(2.0))
    idle.play(_emitter(s.server, "/b", [0, 1]))
    s.clock.play(_emitter(s.server, "/a", [0]))
    s.clock.render()
    assert _emits(s) == [(0.0, "/a")]
    idle.clear()  # left queued on purpose; nothing to warn about at exit


def test_a_bare_clock_still_renders_on_a_time_of_its_own():
    clock = TempoClock(1.0)
    s = Session.nrt()
    clock.play(_emitter(s.server, "/a", [0, 2]))
    clock.render()
    assert _emits(s) == [(0.0, "/a"), (2.0, "/a")]
    assert not isinstance(clock.timebase, LogicalTimebase), "its timebase comes back"


def test_locate_on_a_stopped_clock_is_where_start_resumes():
    tb = ManualTimebase()
    clock = TempoClock(2.0, timebase=tb)
    clock.locate(6.0)
    assert clock.beats() == 6.0
    clock.start()
    try:
        assert clock.beats() == pytest.approx(6.0)
        tb.advance(0.5)
        assert clock.beats() == pytest.approx(7.0)
    finally:
        clock.stop()


def test_locate_on_a_running_clock_puts_the_beat_on_now():
    tb = ManualTimebase()
    clock = TempoClock(1.0, timebase=tb).start()
    try:
        tb.advance(3.0)
        clock.locate(10.0)
        assert clock.beats() == pytest.approx(10.0)
        tb.advance(1.0)
        assert clock.beats() == pytest.approx(11.0)
        assert clock._quant_delay(4) == pytest.approx(1.0), "quant follows the new beat"
    finally:
        clock.stop()


def test_a_frozen_clock_stays_frozen_at_the_located_beat():
    tb = ManualTimebase()
    clock = TempoClock(1.0, timebase=tb).start()
    try:
        tb.advance(2.0)
        clock.freeze()
        clock.locate(5.0)
        tb.advance(3.0)
        assert clock.beats() == pytest.approx(5.0)
        clock.thaw()
        tb.advance(1.0)
        assert clock.beats() == pytest.approx(6.0)
    finally:
        clock.stop()


def test_a_locate_back_from_inside_a_routine_keeps_physical_time_going():
    # A loop by hand: at beat 2 the routine goes back to beat 0 once. Beats
    # repeat; the score's seconds do not.
    s = Session.nrt(tempo=1.0)
    seen = []

    def looping():
        looped = False
        for _ in range(5):
            seen.append(s.clock.beats())
            s.server.send_bundle(("/x",))
            if s.clock.beats() == 2.0 and not looped:
                looped = True
                s.clock.locate(0.0)
            yield 1

    s.clock.play(Routine(looping))
    s.clock.render(until_beat=10)
    assert seen == [0.0, 1.0, 2.0, 1.0, 2.0]
    assert [t for t, _ in _emits(s)] == [0.0, 1.0, 2.0, 3.0, 4.0]


def test_a_locate_forward_wakes_what_it_passed_at_once():
    s = Session.nrt(tempo=1.0)

    def jumper():
        yield 1
        s.clock.locate(10.0)

    s.clock.play(Routine(jumper))
    s.clock.sched_abs(5.0, _emitter(s.server, "/a", [0]))
    s.clock.sched_abs(12.0, _emitter(s.server, "/b", [0]))
    s.clock.render()
    # Located at second 1: beat 5 is overdue and wakes then; beat 12 is 2 s on.
    assert _emits(s) == [(1.0, "/a"), (3.0, "/b")]
