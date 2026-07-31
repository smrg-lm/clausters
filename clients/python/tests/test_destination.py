"""`Moment` and `OscDestination`: sending OSC to applications that are not ours.

Two things under test. `Moment` is the one answer to "what time is it *for this
event*" — the running routine's exact beat, a foreign clock's own now, or the
clockless wall clock outside any routine. `OscDestination` is what carries that
onto the wire for an application we do not control: standard OSC, no latency
and no server-only commands, verified end to end against a real `OscReceiver`
on loopback.
"""

import time

import pytest

from clausters.base import Moment, OscDestination, OscReceiver, Routine, TempoClock


# ---- Moment ----


def test_moment_outside_a_routine_is_the_wall_clock():
    """No clock in scope: beats read as seconds and now is now."""
    m = Moment.current()
    assert m.clock is None
    assert m.beat == 0.0
    assert m.secs() == 0.0
    assert m.instant() == pytest.approx(time.time(), abs=0.5)
    # A delay on a clockless moment is a duration in seconds.
    assert m.at(2.0).instant() == pytest.approx(time.time() + 2.0, abs=0.5)


def test_moment_inside_a_routine_is_the_exact_logical_beat():
    """The beat the clock stamped on the routine, not what time it is now."""
    seen = []
    clock = TempoClock(tempo=2.0)

    def body():
        seen.append(Moment.current())
        yield 1.5
        seen.append(Moment.current())

    clock.play(Routine(body))
    clock.render(until_beat=10.0)

    assert [m.beat for m in seen] == [0.0, 1.5]
    assert all(m.clock is clock for m in seen)
    # Seconds are the clock's own axis: 1.5 beats at 2 beats/s.
    assert seen[1].secs() == pytest.approx(0.75)


def test_moment_on_a_foreign_clock_asks_that_clock():
    """A routine's exact beat belongs to *its* clock; another one is asked for
    its own now, which is what keeps a cross-clock send on the right axis."""
    seen = []
    theirs = TempoClock(tempo=1.0)
    theirs.start()
    ours = TempoClock(tempo=1.0)

    def body():
        seen.append((Moment.current(), Moment.current(theirs)))
        yield None

    ours.play(Routine(body))
    ours.render(until_beat=1.0)
    theirs.stop()

    own, foreign = seen[0]
    assert own.clock is ours and own.beat == 0.0
    assert foreign.clock is theirs
    # `theirs` has been running, so its beat is its own elapsed one.
    assert foreign.beat > 0.0


def test_moment_at_moves_along_the_same_clock():
    clock = TempoClock(tempo=4.0)
    m = Moment(clock, 2.0)
    assert m.at(1.0) == Moment(clock, 3.0)
    assert m.at(1.0).secs() == pytest.approx(0.75)


# ---- OscDestination ----


def _wait(predicate, timeout=2.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.005)
    return False


@pytest.fixture
def receiver():
    r = OscReceiver().start()
    yield r
    r.stop()


def test_destination_sends_a_message(receiver):
    got = []
    receiver.add(lambda addr, args, when, src: got.append((addr, args)))

    dest = OscDestination("127.0.0.1", receiver.port)
    try:
        dest.send_msg("/hello", 1, 2.5, "there")
    finally:
        dest.close()

    assert _wait(lambda: got), "no datagram arrived"
    addr, args = got[0]
    assert addr == "/hello"
    assert args[0] == 1
    assert args[1] == pytest.approx(2.5)
    assert args[2] == "there"


def test_destination_bundles_at_the_routines_logical_beat(receiver):
    """The payload of the design: another application gets the same logical
    timing the server does, with no clock knowledge of its own."""
    got = []
    receiver.add(lambda addr, args, when, src: got.append((addr, args, when)))

    clock = TempoClock(tempo=1.0)
    clock.start()
    dest = OscDestination("127.0.0.1", receiver.port)

    def body():
        dest.send_bundle(("/one",), delay_beats=0.0)
        dest.send_bundle(("/two",), delay_beats=0.25)
        yield None

    try:
        clock.play(Routine(body))
        assert _wait(lambda: len(got) >= 2), f"only got {got}"
    finally:
        dest.close()
        clock.stop()

    by_addr = {addr: when for addr, _args, when in got}
    assert set(by_addr) == {"/one", "/two"}
    # Both timetags are real instants, and the second is a quarter-beat later.
    assert by_addr["/two"] - by_addr["/one"] == pytest.approx(0.25, abs=0.02)


def test_destination_carries_no_latency():
    """Latency is our audio pipeline's property, not an external app's: a
    destination never adds one, so its timetag is the moment itself."""
    sent = []

    class FakeInterface:
        time_mode = "unix"

        def send_bundle(self, target, when, *messages):
            sent.append(when)

        def send_msg(self, target, addr, *args):
            pass

    clock = TempoClock(tempo=1.0)
    clock.start()                      # places the wall-clock origin
    dest = OscDestination("127.0.0.1", 57120, interface=FakeInterface())
    at = Moment(clock, 4.0)
    try:
        dest.send_bundle(("/x",), at=at)
    finally:
        clock.stop()

    assert sent == [at.instant()] == [clock.start_time + 4.0]


def test_a_borrowed_interface_is_left_open():
    """A destination closes only what it opened."""
    closed = []

    class FakeInterface:
        time_mode = "unix"

        def close(self):
            closed.append(True)

    dest = OscDestination(interface=FakeInterface())
    dest.close()
    assert closed == []
