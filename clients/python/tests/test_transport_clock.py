"""The clock gate: a frozen TempoClock holds its beat where it was.

This is how a server transport's pause reaches a client. The sample timebase
only decides how long to sleep, so without the gate a client whose server froze
would keep advancing beats and scheduling ahead of a transport that is not moving.
"""

import time

from clausters.base.clock import TempoClock


def test_a_frozen_clock_does_not_advance():
    clock = TempoClock(2.0)
    clock.start()
    try:
        time.sleep(0.2)
        clock.freeze()
        at_freeze = clock.beats()
        time.sleep(0.3)
        assert abs(clock.beats() - at_freeze) < 1e-9
    finally:
        clock.stop()


def test_thawing_continues_rather_than_jumping():
    clock = TempoClock(2.0)
    clock.start()
    try:
        time.sleep(0.1)
        clock.freeze()
        at_freeze = clock.beats()
        time.sleep(0.3)
        clock.thaw()
        # The 0.3 s spent frozen is not in the music: the beat picks up where it
        # stopped, not 0.6 beats later.
        assert abs(clock.beats() - at_freeze) < 0.05
    finally:
        clock.stop()


def test_frozen_reports_the_state():
    clock = TempoClock(2.0)
    assert not clock.frozen
    clock.freeze()
    assert clock.frozen
    clock.thaw()
    assert not clock.frozen


def test_freeze_is_idempotent():
    clock = TempoClock(2.0)
    clock.start()
    try:
        clock.freeze()
        at_freeze = clock.beats()
        time.sleep(0.1)
        clock.freeze()  # must not re-anchor and lose the first freeze
        time.sleep(0.1)
        clock.thaw()
        assert abs(clock.beats() - at_freeze) < 0.05
    finally:
        clock.stop()


def test_thaw_without_freeze_is_a_no_op():
    clock = TempoClock(2.0)
    clock.start()
    try:
        time.sleep(0.05)
        before = clock.beats()
        clock.thaw()
        assert clock.beats() >= before
    finally:
        clock.stop()


def test_a_frozen_clock_wakes_nothing_and_thaw_does_not_burst():
    # What a frozen clock must not do: keep waking its routines while the
    # beat is held. They would stamp their events into the server's frozen
    # queue, which releases them all at once on the resume, and the thaw's
    # shifted origin would then leave a gap as long as the pause.
    from clausters.base import Routine

    clock = TempoClock(10.0)
    woken = []

    def ticks():
        while True:
            woken.append(time.monotonic())
            yield 1.0

    clock.start()
    try:
        Routine(ticks).play(clock)
        time.sleep(0.35)
        clock.freeze()
        held = len(woken)
        wall = clock.start_time
        time.sleep(0.4)
        assert len(woken) == held, "nothing is woken while the beat is held"
        clock.thaw()
        assert abs(clock.start_time - wall - 0.4) < 0.05, "timetags shift with the pause"
        time.sleep(0.35)
        after = woken[held:]
        assert 2 <= len(after) <= 5, after
        # A beat (0.1 s) apart, as before the freeze: no burst of what was due.
        assert all(b - a > 0.06 for a, b in zip(after, after[1:])), after
    finally:
        clock.stop()
