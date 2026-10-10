"""Patterns as functions: the form (`pattern`, the incoming value, iteration),
the two kinds, and the operators a pattern and a stream compose with."""

import itertools

import pytest

from clausters.base.stream import Routine, StopStream
from clausters.seq import (INF, EventPattern, Pattern, embed, pattern, pbind,
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


# ---- value and random patterns ----

def test_an_argument_may_be_a_pattern_read_per_step():
    _core_or_skip()
    from clausters.seq import pgeom, pvalue

    assert list(pseries(0, pseq([1, 2, 3]))) == [0, 1, 3]       # ends with step
    assert list(pgeom(1, pseq([2, 3]), 3)) == [1, 2]             # ends with grow
    assert list(pseq([pvalue(pseq([1, 2])), pvalue(3)])) == [1, 2, 3]
    lows = list(pwhite(pseq([0, 10, 20]), pseq([1, 11, 21])))
    assert len(lows) == 3 and all(lo <= x < lo + 1 for x, lo in zip(lows, (0, 10, 20)))


def test_the_distributions_have_their_shapes():
    _core_or_skip()
    import statistics as st

    from clausters.seq import (pbeta, pcauchy, pexprand, pgauss, phprand,
                               plprand, pmeanrand, ppoisson, pprob)

    n = 4000
    mean = lambda p: st.mean(p)                                  # noqa: E731
    assert abs(mean(plprand(0, 1, n)) - 1 / 3) < 0.03
    assert abs(mean(phprand(0, 1, n)) - 2 / 3) < 0.03
    assert abs(mean(pmeanrand(0, 1, n)) - 0.5) < 0.03
    assert abs(mean(pbeta(0, 1, 2, 6, n)) - 0.25) < 0.03
    assert abs(mean(pgauss(60, 3, n)) - 60) < 0.3
    assert abs(mean(ppoisson(3, n)) - 3) < 0.15
    assert all(float(x).is_integer() for x in ppoisson(3, 50))
    assert abs(st.median(pcauchy(5, 0.5, n)) - 5) < 0.1
    xs = list(pexprand(100, 1000, n))
    assert all(100 <= x <= 1000 for x in xs)
    assert abs(st.median(xs) - 316.2) < 25                       # the geometric mean
    tri = list(pprob([0, 1, 0], 0, 10, None, n))
    assert abs(mean(tri) - 5) < 0.2 and 1.8 < st.pstdev(tri) < 2.3


def test_a_random_walk_stays_inside_its_bounds():
    _core_or_skip()
    from clausters.seq import pbrown, pgbrown

    walk = list(pbrown(0, 1, 0.1, 2000))
    assert all(0 <= x <= 1 for x in walk)
    assert max(abs(b - a) for a, b in zip(walk, walk[1:])) <= 0.2   # a fold may turn it
    assert all(100 <= x <= 400 for x in pgbrown(100, 400, 0.2, 500))


# ---- list patterns ----

def test_the_list_patterns_read_their_list():
    _core_or_skip()
    from clausters.seq import (INF, pdfsm, place, placep, pshuffle, pslide,
                               pswitch, pswitch1, ptuple, pwalk, pxrand)

    assert list(pseq([1, 2, 3], 2, 1)) == [2, 3, 1, 2, 3, 1]
    assert list(pslide([1, 2, 3, 4, 5], 3, 3, 1)) == [1, 2, 3, 2, 3, 4, 3, 4, 5]
    assert list(place([1, [2, 3]], 4)) == [1, 2, 1, 3, 1, 2, 1, 3]
    assert list(placep([pseq([1, 2]), pseq([10, 20, 30])], 5)) == [1, 10, 2, 20, 30]
    assert list(ptuple([pseq([1, 2]), pseq([10, 20, 30])])) == [[1, 10], [2, 20]]
    assert list(pswitch([pseq([1, 2]), 9], pseq([0, 1, 0]))) == [1, 2, 9, 1, 2]
    assert list(pswitch1([pseq([1, 2, 3]), pseq([10, 20])], pseq([0, 1, 0]))) == [1, 10, 2]
    bounce = pwalk([1, 2, 3, 4], pseq([1], 8), pseq([1, -1], INF))
    assert list(bounce) == [1, 2, 3, 4, 3, 2, 1, 2]
    xs = list(pxrand([1, 2, 3], 200))
    assert all(a != b for a, b in zip(xs, xs[1:]))
    order = list(pshuffle([1, 2, 3, 4], 3))
    assert sorted(order[:4]) == [1, 2, 3, 4] and order[:4] == order[4:8] == order[8:]
    machine = pdfsm([pseq([0, 1, 1, 0]),
                     {0: (0, "x"), 1: (1, "y"), "default": (0, "z")},
                     {0: (0, "p"), "default": (1, "q")}])
    assert list(machine) == ["x", "y", "q", "p"]


def test_a_list_of_event_patterns_plays():
    from clausters.seq import pshuffle, pslide, pwalk, pxrand

    phrase = pbind(degree=pseq([0, 1]))
    for p in (pxrand([phrase, phrase]), pshuffle([phrase]), pslide([phrase]),
              pwalk([phrase])):
        assert isinstance(p, EventPattern)


# ---- filter patterns ----

def test_the_filters_change_what_comes_out():
    _core_or_skip()
    from clausters.seq import (pclump, pcollect, pdiff, pdrop, pflatten,
                               platch, plen, pprorate, preject, pselect,
                               pstutter, pwrap)

    assert list(pcollect(lambda x: x * x, pseq([1, 2, 3]))) == [1, 4, 9]
    assert list(pselect(lambda x: x % 2, pseq([1, 2, 3]))) == [1, 3]
    assert list(preject(lambda x: x % 2, pseq([1, 2, 3]))) == [2]
    assert list(pcollect(lambda x, e: e, pseq([1]))) == [None]  # the incoming value
    assert list(pdrop(pseq([1, 2, 3]), 1)) == [2, 3]
    assert list(plen(pseries(0, 1), 3)) == [0, 1, 2]
    assert list(pstutter(pseq([1, 2]), pseq([2, 3]))) == [1, 1, 2, 2, 2]
    assert list(platch(pseries(0, 1), pseq([True, False, True]))) == [0, 0, 1]
    assert list(pwrap(pseq([-1, 5]), 0, 4)) == [3, 1]
    assert list(pclump(pseq([1, 2, 3]), 2)) == [[1, 2], [3]]
    assert list(pflatten(pseq([[1, [2, 3]], 4]), 2)) == [1, 2, 3, 4]
    assert list(pdiff(pseq([1, 4, 9]))) == [3, 5]
    assert list(pprorate(pseq([2]), pseq([0.25]))) == [0.5, 1.5]
    assert list(pseq([1, 2]).trace()) == [1, 2]


def test_a_gate_takes_a_value_when_a_repeat_starts():
    from clausters.seq import pgate, plen

    phrase = pbind(n=pn(pseq([0, 1]), INF, "g"), x=pgate(pseries(0, 1), INF, "g"))
    assert [e["x"] for e in plen(phrase, 6)] == [0, 0, 1, 1, 2, 2]


def test_the_time_filters_cut_and_delay_events():
    from clausters.seq import pdelta, pdur

    cut = list(pdur(2.5, pbind(dur=1, degree=pseq([0, 1, 2, 3]))))
    assert [e.delta() for e in cut] == [1, 1, 0.5]
    late = list(pdelta(1.5, pbind(dur=0.5, degree=pseq([0, 1]))))
    assert [e["type"] for e in late] == ["rest", "note", "note"]
    assert isinstance(pdur(1, pbind(degree=0)), EventPattern)
