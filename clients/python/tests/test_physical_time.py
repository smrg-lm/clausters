"""One physical time, the same live and offline, and `TempoClock.locate`.

Offline, a `LogicalTimebase` stands in for physical time: every clock of the
run has its origin on it, and a render wakes whatever is due next across all of
them, in seconds. So a script with several clocks renders as it plays, and a
locate is the same operation on either time.
"""

import pytest

from clausters import Session
from clausters.base.main import main
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
    s = Session.nrt()
    with s:
        fast = TempoClock(2.0)
    assert fast.session is s
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
    s = Session.nrt()

    def later():
        yield 4
        late = TempoClock(2.0).start()
        late.play(_emitter(s.server, "/b", [0, 1]))

    s.clock.play(Routine(later))
    s.clock.render()
    assert _emits(s) == [(4.0, "/b"), (4.5, "/b")]


def test_an_offline_clock_that_is_never_started_does_not_play_as_live():
    s = Session.nrt()
    with s:
        idle = TempoClock(2.0)
    idle.play(_emitter(s.server, "/b", [0, 1]))
    s.clock.play(_emitter(s.server, "/a", [0]))
    s.clock.render()
    assert _emits(s) == [(0.0, "/a")]
    idle.clear()  # left queued on purpose; nothing to warn about at exit


def test_a_bare_clock_on_logical_time_renders_on_a_time_of_its_own():
    clock = TempoClock(1.0, timebase=LogicalTimebase())
    s = Session.nrt()
    clock.play(_emitter(s.server, "/a", [0, 2]))
    clock.render()
    assert _emits(s) == [(0.0, "/a"), (2.0, "/a")]
    assert clock.session is None and clock.timebase is not s.timebase


def test_an_offline_session_has_only_logical_time():
    """Every clock of an offline session is on its logical time: a clock made
    inside one takes it, and a clock or a timebase of another kind is
    refused."""
    s = Session.nrt()
    assert isinstance(s.timebase, LogicalTimebase)
    with s:
        assert TempoClock(3.0).timebase is s.timebase
        with pytest.raises(ValueError, match="session's timebase"):
            TempoClock(1.0, timebase=ManualTimebase())
    with pytest.raises(ValueError, match="LogicalTimebase"):
        Session.nrt(timebase=ManualTimebase())
    with pytest.raises(ValueError, match="session's timebase"):
        s.adopt(TempoClock(1.0))


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
    s = Session.nrt()
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
    s = Session.nrt()

    def jumper():
        yield 1
        s.clock.locate(10.0)

    s.clock.play(Routine(jumper))
    s.clock.sched_abs(5.0, _emitter(s.server, "/a", [0]))
    s.clock.sched_abs(12.0, _emitter(s.server, "/b", [0]))
    s.clock.render()
    # Located at second 1: beat 5 is overdue and wakes then; beat 12 is 2 s on.
    assert _emits(s) == [(1.0, "/a"), (3.0, "/b")]


# ---- a session is the context clocks are made in ----


def test_a_session_given_a_clock_takes_its_timebase_and_refuses_another():
    clock = TempoClock(2.0, timebase=LogicalTimebase())
    s = Session.nrt(clock=clock)
    assert s.clock is clock and s.timebase is clock.timebase
    with pytest.raises(ValueError, match="session's timebase"):
        Session.nrt(clock=TempoClock(1.0, timebase=LogicalTimebase()),
                    timebase=LogicalTimebase())
    with pytest.raises(ValueError, match="another session"):
        Session.nrt(clock=clock)


def test_a_session_switched_inside_a_routine_is_the_one_in_force():
    """The context a ``with`` block sets wins over the running routine's
    session: an offline session made and rendered from inside another's routine
    makes its clocks on its own time and plays on its own server."""
    outer = Session.nrt()
    inner = Session.nrt()
    made = []

    def body():
        with inner:
            made.append((TempoClock(1.0), main.resolve_clock(), main.resolve_server()))
        yield None

    outer.clock.play(Routine(body))
    outer.clock.render()
    clock, resolved, server = made[0]
    assert clock.session is inner and clock.timebase is inner.timebase
    assert resolved is inner.clock and server is inner.server


def test_a_timelines_hidden_clock_belongs_to_the_session_it_sounds_in():
    from clausters.seq import OscItem, Timeline

    tl = Timeline([(0, OscItem("/a")), (1, OscItem("/a"))], tempo=2.0)
    first = Session.nrt()
    with first:                     # a block closes its session on the way out
        tl.play()
        first.clock.render()
        assert tl._player.clock.session is first
        assert _emits(first) == [(0.0, "/a"), (0.5, "/a")]

    second = Session.nrt()
    with second:
        tl.play(0)
        assert tl._player.clock.session is second, "a new clock, in the new session"
        assert tl._player.clock.timebase is second.timebase
        second.clock.render()
        assert _emits(second) == [(0.0, "/a"), (0.5, "/a")]


def test_a_timeline_sounding_in_one_session_is_refused_in_another():
    from clausters.seq import OscItem, Timeline

    tl = Timeline([(0, OscItem("/a")), (8, OscItem("/a"))])
    first = Session.nrt().activate()
    tl.play()
    first.deactivate()
    with Session.nrt(), pytest.raises(RuntimeError, match="another session"):
        tl.play()
    tl.stop()
    first.close()


def test_an_activated_session_does_not_take_over_another_sessions_routines():
    """`activate` puts a session in force outside any routine; a routine on
    another session's clock still resolves that session, which is what keeps
    two sessions isolated while one of them is ambient."""
    ambient = Session.nrt().activate()
    other = Session.nrt()
    seen = []

    def body():
        seen.append((main.resolve_server(), main.resolve_clock(), TempoClock(1.0)))
        yield None

    try:
        other.clock.play(Routine(body))
        other.clock.render()
    finally:
        ambient.deactivate()
    server, clock, made = seen[0]
    assert server is other.server and clock is other.clock
    assert made.session is other and made.timebase is other.timebase
