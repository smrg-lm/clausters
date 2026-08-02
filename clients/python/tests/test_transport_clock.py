"""The clock gate: a frozen TempoClock holds its beat where it was.

This is how a server transport's pause reaches a client. The sample timebase
only decides how long to sleep, so without the gate a client whose server froze
would keep advancing beats and scheduling ahead of a piece that is not moving.
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
        # The 0.3 s spent frozen is not in the piece: the beat picks up where it
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
