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


# ---- the whole AbstractObject surface, and the builtins over a pattern ----

def test_a_pattern_has_the_n_ary_operators_a_signal_has():
    _core_or_skip()
    assert list(pseq([0, 0.5, 1]).linlin(0, 1, 200, 800)) == [200, 500, 800]
    assert list(pseq([-2, 0.5, 3]).clip(0, 1)) == [0, 0.5, 1]
    assert list(pseq([0, 10]).blend(20, 0.5)) == [10, 15]
    assert list(pseq([0, 180]).raddeg().degrad()) == [0, 180]
    assert list(pseq([0.9]).moddif(0.1)) == pytest.approx([0.2])
    assert list(pseq([0.25]).bilin(0.5, 0, 1, 10, 0, 100)) == [5.0]


def test_the_operators_sc3_had_and_this_one_did_not():
    _core_or_skip()
    assert list(~pseq([0])) == [-1]
    assert list(pseq([7]) // 2) == [3]
    assert list(pseq([0, 1]).not_()) == [1, 0]
    assert list(pseq([3, 4]).eq(3)) == [1, 0] and list(pseq([3]).ne(3)) == [0]
    assert list(pseq([6]).bitand(3)) == [2] and list(pseq([1]).leftshift(3)) == [8]
    assert list(pseq([2.5]).rint()) == [2]


def test_the_random_operators_draw_per_value_under_the_seed():
    _core_or_skip()
    from clausters.base.main import main

    main.seed(3)
    a = list(pseq([10, 10]).rand()) + list(pseq([1]).rrand(2))
    main.seed(3)
    assert list(pseq([10, 10]).rand()) + list(pseq([1]).rrand(2)) == a
    assert 0 <= a[0] < 10 and 1 <= a[2] <= 2


def test_a_builtin_given_a_pattern_answers_what_its_method_does():
    _core_or_skip()
    from clausters.base import builtins as b

    assert list(b.midicps(pseq([69, 81]))) == list(pseq([69, 81]).midicps())
    assert list(b.add(2, pseq([1, 2]))) == [3, 4]               # reflected
    assert list(b.clip(pseq([-1, 2]), 0, 1)) == [0, 1]
    assert list(b.cbrt(pseq([8, 27]))) == [2, 3]                 # by name
    assert list(b.degkey(pseq([0, 1, 2]), [0, 2, 4, 5, 7, 9, 11])) == [0, 2, 4]
    assert isinstance(b.midicps(pseq([60])), Pattern)
    with pytest.raises(TypeError, match="first argument"):
        b.clip(0.5, pseq([0]), 1)


def test_a_list_function_refuses_a_pattern_by_name():
    from clausters.base import builtins as b

    for fn in (b.normalize, b.integrate, b.normalizesum):
        with pytest.raises(TypeError, match="whole list"):
            fn(pseq([1, 2, 3]))


def test_a_graph_composes_the_value_functions_it_can_and_refuses_the_rest():
    from clausters.base import builtins as b
    from clausters.defs.signals import signal
    from clausters.defs.ugens import Ugen, sine

    assert repr(sine(1).degrad()) == repr(sine(1) * (3.141592653589793 / 180.0))
    assert isinstance(sine(1).blend(sine(2), 0.25), Ugen)
    assert isinstance(b.midicps(sine(1)), Ugen)
    for bad in (lambda: sine(1).moddif(0), lambda: b.cbrt(sine(1))):
        with pytest.raises(TypeError, match="no UGen computes"):
            bad()
    with pytest.raises(TypeError, match="has no 'linlin'"):
        signal(1.0).linlin(0, 1, 0, 2)
