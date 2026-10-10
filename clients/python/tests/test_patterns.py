"""Patterns as functions: the form (`pattern`, the incoming value, iteration),
the two kinds, and the operators a pattern and a stream compose with."""

import itertools

import pytest

from clausters.base.stream import Routine, StopStream
from clausters.seq import (EventPattern, Pattern, embed, pattern, pbind,
                           pbinop, pn, pnarop, pseq, pseries, punop, pwhite)


def _core_or_skip():
    try:
        from clausters import _native
        _native.lib()
    except OSError as e:
        pytest.skip(f"clausters-ffi not built: {e}")


# ---- the form ----

def test_a_pattern_is_a_function_returning_a_reusable_description():
    p = pseq([1, 2, 3])
    assert isinstance(p, Pattern) and not isinstance(p, EventPattern)
    # Every walk starts afresh: the pattern holds no state of its own.
    assert list(p) == [1, 2, 3]
    assert [x for x in p] == [1, 2, 3]
    assert repr(p) == "pseq([1, 2, 3])"


def test_a_pattern_keeps_a_copy_of_its_list():
    items = [1, 2]
    p = pseq(items)
    items.append(3)
    assert list(p) == [1, 2]
    # A one-shot iterable is read once, when the pattern is built.
    q = pseq(x * 10 for x in range(3))
    assert list(q) == [0, 10, 20] and list(q) == [0, 10, 20]


def test_an_endless_pattern_is_bounded_by_the_loop():
    assert list(itertools.islice(pseries(0, 1), 4)) == [0, 1, 2, 3]


def test_a_generator_function_becomes_a_pattern():
    @pattern
    def pcount(inval, n):
        "0, 1, ... n - 1."
        for i in range(n):
            inval = yield i
        return inval

    assert pcount.__name__ == "pcount" and pcount.__doc__ == "0, 1, ... n - 1."
    assert list(pcount(3)) == [0, 1, 2]
    assert list(pseq([pcount(2), 9])) == [0, 1, 9]


def test_the_incoming_value_reaches_the_generator_and_its_embeds():
    @pattern
    def pecho(inval, n):
        for _ in range(n):
            inval = yield inval

    @pattern
    def pwrap(inval, source):
        inval = yield from embed(source, inval)
        return inval

    s = pwrap(pecho(3)).stream()
    assert [s.next("a"), s.next("b"), s.next("c")] == ["a", "b", "c"]
    with pytest.raises(StopStream):
        s.next("d")


def test_a_pbind_hands_its_keys_the_event_being_built():
    @pattern
    def pdouble_degree(inval):
        while True:
            inval = yield inval["degree"] * 2

    events = list(pbind(degree=pseq([1, 2]), octave=pdouble_degree()))
    assert [e["octave"] for e in events] == [2, 4]


def test_the_kind_is_decided_when_the_pattern_is_built():
    phrase = pbind(degree=pseq([0, 1]))
    assert isinstance(phrase, EventPattern)
    assert isinstance(pseq([phrase, phrase]), EventPattern)
    assert isinstance(pn(phrase, 2), EventPattern)
    assert not isinstance(pseq([phrase, 1]), EventPattern)
    assert not hasattr(pseq([1, 2]), "play")


# ---- the operators ----

def test_an_operator_composes_a_pattern_of_values():
    _core_or_skip()
    assert list(pseq([74, 60, 46]) + 2) == [76, 62, 48]
    assert list(100 - pseq([1, 2])) == [99, 98]
    assert list(pseq([1, 2, 3]) * pseq([10, 20])) == [10, 40]   # ends with the shorter
    assert list(-pseq([1, -2])) == [-1, 2]
    assert list(pseq([69, 81]).midicps()) == [440.0, 880.0]
    assert list(pseq([1, 5, 9]).min(4)) == [1, 4, 4]
    assert list(pseq([13, 14]) % 12) == [1, 2]


def test_a_composed_pattern_is_a_pattern():
    _core_or_skip()
    p = pseq([1, 2]) + 60
    assert isinstance(p, Pattern) and repr(p) == "pbinop('add', pseq([1, 2]), 60)"
    assert list(pn(p, 2)) == [61, 62, 61, 62]
    assert [x for x in p + 1] == [62, 63]


def test_the_operator_patterns_by_name():
    _core_or_skip()
    assert list(punop("neg", pseq([1]))) == [-1]
    assert list(pbinop("max", pseq([1, 7]), 3)) == [3, 7]
    assert list(pnarop("linlin", pseq([0, 0.5, 1]), 0, 1, 0, 10)) == [0, 5, 10]
    assert list(pnarop("clip", pseq([-1, 2]), 0, 1)) == [0, 1]


def test_an_event_pattern_refuses_an_operator():
    with pytest.raises(TypeError, match="event pattern"):
        pbind(degree=0) + 2
    with pytest.raises(TypeError, match="event pattern"):
        pseq([1]) + pbind(degree=0)


def test_random_values_compose_under_the_seed():
    _core_or_skip()
    from clausters.base.main import main

    main.seed(5)
    a = list((pwhite(0, 1, 4) * 100).floor())
    main.seed(5)
    assert list((pwhite(0, 1, 4) * 100).floor()) == a
    assert all(0 <= x < 100 for x in a)


def test_a_stream_composes_by_operators():
    _core_or_skip()

    def counting():
        for i in range(3):
            yield i

    s = Routine(counting) * 10 + 1
    assert [s.next(), s.next(), s.next()] == [1, 11, 21]
    with pytest.raises(StopStream):
        s.next()
    assert [x for x in pseq([1, 2]).stream() + 1] == [2, 3]
