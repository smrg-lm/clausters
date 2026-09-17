"""The `Timeline` structure: editing it, and querying it by time.

Pure unit tests over the plan itself. What **playing** one does -- its verbs,
its children, its units -- is `test_timeline_play.py`, and what a timeline on a
server transport does is `test_timeline_transport.py`; the full multi-client
lockstep is the manual E2E in
`clients/python/examples/transport/conductor.py`.
"""

from clausters.seq import (
    INF,
    Event,
    MidiItem,
    OscItem,
    Pbind,
    Pseq,
    Timeline,
)


# ---- Timeline editing ----


def test_timeline_keeps_sorted_and_stable():
    tl = Timeline()
    tl.add(2.0, "c")
    tl.add(0.0, "a")
    tl.add(1.0, "b1")
    tl.add(1.0, "b2")           # same beat: stable, after b1
    assert [item for _, item in tl] == ["a", "b1", "b2", "c"]
    assert [b for b, _ in tl] == [0.0, 1.0, 1.0, 2.0]
    assert tl.duration() == 2.0


def test_timeline_remove_and_move_by_handle():
    tl = Timeline()
    a = tl.add(0.0, "a")
    b = tl.add(1.0, "b")
    tl.move(a, 5.0)             # a jumps past b, stays sorted
    assert [item for _, item in tl] == ["b", "a"]
    tl.remove(b)
    assert [item for _, item in tl] == ["a"]
    assert len(tl) == 1


def test_timeline_quantize_snaps_beats_and_keeps_order():
    tl = Timeline([(0.1, "a"), (0.9, "b"), (1.3, "c")])
    tl.quantize(0.5)
    assert list(tl) == [(0.0, "a"), (1.0, "b"), (1.5, "c")]
    tl.quantize(0.0)            # no grid: a no-op
    assert [b for b, _ in tl] == [0.0, 1.0, 1.5]


def test_timeline_random_access_by_time():
    tl = Timeline([(0.0, "a"), (1.0, "b"), (2.0, "c"), (3.0, "d")])
    assert tl.index_at(1.0) == 1          # first item at or after 1.0
    assert tl.index_at(1.5) == 2
    assert tl.range(1.0, 3.0) == [(1.0, "b"), (2.0, "c")]   # [t0, t1)
    assert tl.at(2.0) == ["c"]


# ---- capture a pattern into a timeline ----


def test_timeline_from_pattern_records_beats():
    tl = Timeline.from_pattern(Pbind(freq=Pseq([440, 550, 660]), dur=0.5))
    assert len(tl) == 3
    assert [b for b, _ in tl] == [0.0, 0.5, 1.0]
    assert [e["freq"] for _, e in tl] == [440, 550, 660]


def test_timeline_from_pattern_bounds_an_endless_pattern_by_beats():
    """``dur`` is the ordinary way to bounce something that never ends."""
    endless = Pbind(freq=Pseq([440, 550], INF), dur=0.25)
    tl = Timeline.from_pattern(endless, dur=1.0)
    assert [b for b, _ in tl] == [0.0, 0.25, 0.5, 0.75, 1.0]


def test_timeline_from_pattern_refuses_an_endless_pattern_with_no_bound():
    """With no ``dur`` an endless pattern would run forever, so the bounce
    counts what it has recorded and gives up. The cap is the caller's
    (``max_events``) so this does not have to record a million events to prove
    it — and it is deliberately **not** in `TempoClock.render`, where a long
    offline render of a real score is meant to take a long time."""
    endless = Pbind(freq=Pseq([440, 550], INF), dur=0.25)
    try:
        Timeline.from_pattern(endless, max_events=32)
    except RuntimeError as e:
        assert "did not end after 32 events" in str(e)
        assert "dur=" in str(e), "the message says how to bound it"
    else:
        raise AssertionError("an endless bounce with no bound must not return")
