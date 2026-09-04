"""The client's event loop and the clock over it.

Pure-unit: no host, no window, no thread where a thread is not the subject. The
loop's contract is the phases and the timers, the `AppClock`'s is the unit and
the routine, and the last group is the one thing a loop makes possible to get
wrong -- a structure read by the script while the loop's thread rewrites it.
"""

import threading
import time

import pytest

from clausters.base.appclock import AppClock
from clausters.base.loop import EventLoop
from clausters.base.stream import Routine
from clausters.seq import Timeline
from clausters.seq.event import Event


@pytest.fixture
def loop():
    made = EventLoop("test")
    yield made
    made.close()


# ---- the loop ----


def test_a_timer_runs_when_it_is_due_and_not_before(loop):
    ran = []
    loop.sched(0.05, lambda: ran.append("late"))
    loop.sched(0.0, lambda: ran.append("now"))
    loop.iterate(0.0)
    assert ran == ["now"], "a timer in the future is not run by an early turn"
    _drive(loop, 0.3)
    assert ran == ["now", "late"]


def test_timers_run_in_time_order_whatever_order_they_were_queued_in(loop):
    ran = []
    for delay in (0.03, 0.01, 0.02):
        loop.sched(delay, lambda d=delay: ran.append(d))
    _drive(loop, 0.3)
    assert ran == [0.01, 0.02, 0.03]


def test_a_timer_that_returns_a_number_is_rescheduled_by_it(loop):
    """The periodic task, written without a loop of its own -- the same
    contract `TempoClock.sched` states for a callable."""
    ticks = []

    def tick():
        ticks.append(loop.now())
        return 0.01 if len(ticks) < 3 else None

    loop.sched(0.0, tick)
    _drive(loop, 0.4)
    assert len(ticks) == 3


def test_cancel_drops_a_timer_and_says_whether_it_was_still_queued(loop):
    ran = []
    handle = loop.sched(0.05, lambda: ran.append("no"))
    assert loop.cancel(handle) is True
    assert loop.cancel(handle) is False, "cancelling twice is False, not an error"
    _drive(loop, 0.2)
    assert ran == []


def test_posted_work_runs_on_the_next_turn_and_never_re_entrantly(loop):
    """The phase contract: what a handler posts is served by the turn after the
    one it ran in. It is what makes a callback that schedules more work, or
    closes the window it was called for, safe to write."""
    order = []

    def outer():
        order.append("outer")
        loop.post(lambda: order.append("inner"))
        order.append("outer done")

    loop.post(outer)
    loop.iterate(0.0)
    assert order == ["outer", "outer done"], "the posted call did not re-enter"
    loop.iterate(0.0)
    assert order == ["outer", "outer done", "inner"]


def test_a_handler_that_raises_loses_its_turn_and_nothing_else(loop, capsys):
    ran = []

    def bad():
        raise RuntimeError("boom")

    loop.post(bad)
    loop.post(lambda: ran.append("still here"))
    loop.iterate(0.0)
    assert ran == ["still here"]
    assert "boom" in capsys.readouterr().err


def test_a_source_is_drained_until_it_has_nothing_more(loop):
    """One wake empties a burst: a source that has three items hands over all
    three in the turn it became ready, not one per turn."""
    got = []
    items = [1, 2, 3]
    loop.add_source(read=lambda timeout=0.0: items.pop(0) if items else None,
                    deliver=got.append)
    loop.iterate(0.0)
    assert got == [1, 2, 3]


def test_a_removed_source_stops_being_drained(loop):
    got = []
    items = [1, 2]
    source = loop.add_source(read=lambda timeout=0.0: items.pop(0) if items else None,
                             deliver=got.append)
    loop.remove_source(source)
    loop.iterate(0.0)
    assert got == []


def test_the_wait_is_bounded_by_the_nearest_timer(loop):
    """Not by the timeout it was asked for: a loop with something due in 20 ms
    does not sleep for a second first."""
    loop.sched(0.02, lambda: None)
    start = time.monotonic()
    loop.iterate(1.0)
    assert time.monotonic() - start < 0.5


def test_a_thread_of_its_own_runs_it_and_stop_ends_it(loop):
    ran = threading.Event()
    loop.sched(0.0, ran.set)
    loop.start()
    assert ran.wait(1.0), "the loop's thread ran the timer"
    assert loop.running
    loop.stop()
    assert not loop.running


def test_posting_from_another_thread_wakes_the_wait(loop):
    """The wake channel: a post does not have to sit out the current wait --
    which is what a routine on the clock thread depends on."""
    loop.start()
    landed = threading.Event()
    threading.Thread(target=lambda: loop.post(landed.set)).start()
    assert landed.wait(1.0)


# ---- the clock ----


def test_the_clock_schedules_in_seconds(loop):
    clock = AppClock(loop)
    at = []
    clock.sched(0.05, lambda: at.append(clock.elapsed()))
    _drive(loop, 0.4)
    assert at and 0.04 <= at[0] < 0.4


def test_a_routine_is_resumed_by_the_seconds_it_yields(loop):
    """The animation, and the reason the loop's timers and the clock are one
    object: it is a routine that waits, not a second scheduling vocabulary."""
    clock = AppClock(loop)
    frames = []

    def blink():
        for i in range(3):
            frames.append(i)
            yield 0.01

    clock.play(Routine(blink))
    _drive(loop, 0.5)
    assert frames == [0, 1, 2]


def test_a_routine_carries_the_clock_it_runs_on(loop):
    clock = AppClock(loop)
    seen = []

    def routine():
        from clausters.base.main import main

        seen.append(main.current_routine.clock)
        yield 0.0

    clock.play(Routine(routine))
    loop.iterate(0.0)
    assert seen == [clock]


def test_unsched_cancels_what_was_queued_for_an_item(loop):
    clock = AppClock(loop)
    ran = []
    routine = Routine(lambda: (yield 0.0))
    clock.sched(0.05, lambda: ran.append("no"))
    handle = clock.sched(0.05, routine)
    assert clock.unsched(routine) is True
    assert loop.cancel(handle) is False, "unsched took it out of the loop's queue"


def test_defer_runs_on_the_loop_and_returns_at_once(loop):
    """The door a routine on the musical clock reaches a window through: its own
    thread must never block, so what it defers runs where the windows are."""
    clock = AppClock(loop)
    ran = []
    clock.defer(lambda: ran.append("there"))
    assert ran == [], "defer does not run it here"
    loop.iterate(0.0)
    assert ran == ["there"]


# ---- what a loop makes possible to get wrong ----


def test_a_timeline_is_never_read_half_rewritten():
    """The measured one. A projection that clears and re-adds leaves the
    timeline empty in between, and a script reading it from its own thread sees
    that: 87.7% of reads at 4000 notes, and none at all at 8 -- CPython's switch
    interval hides a short rebuild, which is what makes it invisible in a small
    test and systematic in a real piece. `Timeline.replace` binds the new order
    in one step instead.
    """
    n = 2000
    timeline = Timeline([(float(i), Event(midinote=60, dur=1.0)) for i in range(n)])
    stop = threading.Event()

    def writer():
        while not stop.is_set():
            timeline.replace([(float(i) + 0.25, Event(midinote=60, dur=1.0))
                              for i in range(n)])

    thread = threading.Thread(target=writer, daemon=True)
    thread.start()
    try:
        short = sum(1 for _ in range(2000) if len(list(timeline)) != n)
    finally:
        stop.set()
        thread.join(timeout=2.0)
    assert short == 0


def _drive(loop, seconds: float):
    """Turn the loop by hand until ``seconds`` are up -- the `iterate` face, so
    a test needs no thread to be about timers."""
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        loop.iterate(0.01)
