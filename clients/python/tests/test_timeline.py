"""The `Timeline` structure: editing it, and querying it by time.

Pure unit tests over the plan itself. What **playing** one does -- its verbs,
its children, its units -- is `test_timeline_play.py`, and what a timeline on a
server transport does is `test_timeline_transport.py`; the full multi-client
lockstep is the manual E2E in
`clients/python/examples/transport/conductor.py`.
"""

from clausters.seq import (
    Event,
    EventPattern,
    MidiItem,
    OscItem,
    Pbind,
    Pn,
    Prand,
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


# ---- what a pattern is as an item ----


def test_a_timeline_refuses_a_value_pattern():
    """A value pattern is the definition of a generator and does not play, so
    it is not an item; an event pattern is."""
    tl = Timeline()
    tl.add(0.0, Pbind(freq=Pseq([440, 550]), dur=0.5))
    try:
        tl.add(1.0, Pseq([1, 2, 3]))
    except TypeError as e:
        assert "does not play" in str(e)
    else:
        raise AssertionError("a value pattern must not be a timeline item")
    assert len(tl) == 1


def test_a_list_pattern_over_events_is_an_event_pattern():
    """A list pattern resolves its class when it is built: over event patterns
    only it plays; a list that mixes events and values is a value pattern."""
    phrase = Pbind(freq=Pseq([440, 550]), dur=0.5)
    assert isinstance(Pseq([phrase, phrase]), EventPattern)
    assert isinstance(Prand([phrase]), EventPattern)
    assert isinstance(Pn(phrase, 2), EventPattern)
    assert isinstance(Pseq([phrase, phrase]), Pseq)
    assert not isinstance(Pseq([phrase, 1]), EventPattern)
    assert not isinstance(Pseq([1, 2]), EventPattern)
    assert not hasattr(Pseq([1, 2]), "play")
    assert [e["freq"] for e in Pseq([phrase], 2)] == [440, 550, 440, 550]
