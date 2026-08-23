"""The warp family: reading a value out of one range and writing it into another.

`linlin` and its seven siblings are SuperCollider's, and they are asserted
against sclang's own values rather than described — the point of having them is
that a piece written against SuperCollider's numbers gets SuperCollider's
numbers. They are computed in the shared core, in f32, so a value mapped here
and the same map on the audio thread agree by construction.

The formulas live in `clausters_core::warp` and nowhere else, which is what
makes the envelope curves, the server's `XLine` and these one curve instead of
three.
"""

import math

import pytest

from clausters.base import builtins as B


# ---- the four range maps ----

def test_the_linear_map_is_sclangs():
    assert B.linlin(0.5, 0, 1, 20, 20000) == pytest.approx(10010.0)
    assert B.linlin(0.0, 0, 1, 20, 20000) == pytest.approx(20.0)
    assert B.linlin(1.0, 0, 1, 20, 20000) == pytest.approx(20000.0)


def test_the_exponential_map_is_sclangs():
    """A fader's midpoint on a 20..20000 range is 632 Hz, not 10 kHz — the whole
    reason a frequency control is not drawn linearly."""
    assert B.linexp(0.5, 0, 1, 20, 20000) == pytest.approx(632.4555, rel=1e-5)
    assert B.explin(632.4555, 20, 20000, 0, 1) == pytest.approx(0.5, abs=1e-5)
    assert B.expexp(632.4555, 20, 20000, 1, 100) == pytest.approx(10.0, rel=1e-4)


def test_each_map_reads_what_its_inverse_writes():
    for x in (0.0, 0.25, 0.5, 0.75, 1.0):
        assert B.explin(B.linexp(x, 0, 1, 20, 20000), 20, 20000, 0, 1) == pytest.approx(x, abs=1e-5)
        assert B.curvelin(B.lincurve(x, 0, 1, 0, 1), 0, 1, 0, 1) == pytest.approx(x, abs=1e-4)


def test_the_bent_map_is_sclangs_and_zero_curvature_is_the_linear_one():
    assert B.lincurve(0.5, 0, 1, 0, 1, -4.0) == pytest.approx(0.8807971, rel=1e-6)
    assert B.lincurve(0.3, 0, 1, 10, 20, 0.0) == B.linlin(0.3, 0, 1, 10, 20)


# ---- what a range does with a value from outside it ----

def test_an_out_of_range_input_is_trimmed_by_default():
    """sclang's `prune`, whose default is both ends."""
    assert B.linlin(2.0, 0, 1, 0, 10) == pytest.approx(10.0)
    assert B.linlin(-1.0, 0, 1, 0, 10) == pytest.approx(0.0)


def test_clip_none_extrapolates_instead():
    assert B.linlin(2.0, 0, 1, 0, 10, "none") == pytest.approx(20.0)
    assert B.linlin(-1.0, 0, 1, 0, 10, "min") == pytest.approx(0.0)
    assert B.linlin(2.0, 0, 1, 0, 10, "min") == pytest.approx(20.0)


def test_an_unknown_clip_mode_says_so():
    with pytest.raises(KeyError):
        B.linlin(0.5, 0, 1, 0, 10, "sometimes")


# ---- the bipolar pair ----

def test_a_bipolar_value_spans_the_range_and_is_not_trimmed():
    assert B.range(-1.0, 100, 200) == pytest.approx(100.0)
    assert B.range(0.0, 100, 200) == pytest.approx(150.0)
    assert B.range(1.0, 100, 200) == pytest.approx(200.0)
    # A bare value cannot declare itself bipolar, so an overshoot stays one.
    assert B.range(2.0, 100, 200) == pytest.approx(250.0)
    assert B.exprange(0.0, 1, 100) == pytest.approx(10.0, rel=1e-5)


# ---- zero has no ratio ----

def test_an_exponential_end_at_zero_is_nudged_rather_than_a_nan():
    """Where sclang gives NaN this gives a very steep rise — the same answer
    the server's `XLine` and the envelope's exponential segment give, because
    all three read one rule."""
    y = B.linexp(0.5, 0, 1, 0.0, 1.0)
    assert math.isfinite(y) and 0.0 < y < 1.0


# ---- sequences ----

def test_a_sequence_maps_elementwise_and_a_range_is_the_idiomatic_one():
    """`range(0, 120)` is how a Python author writes 120 values, so it is what
    the builtins take — not only the two types a first pass happened to name."""
    assert list(B.linlin(range(0, 3), 0, 2, 60, 72)) == [60.0, 66.0, 72.0]
    assert list(B.linlin([0.0, 0.5, 1.0], 0, 1, 0, 10)) == [0.0, 5.0, 10.0]
    assert list(B.linlin((i / 2 for i in range(3)), 0, 1, 0, 100)) == [0.0, 50.0, 100.0]


def test_the_whole_family_composes_with_the_unary_builtins():
    """The example the family exists for: 120 semitones onto their frequencies,
    with one crossing per call and no Python arithmetic in between."""
    notes = B.linlin(range(0, 3), 0, 2, 60, 72)
    assert list(B.midicps(notes)) == pytest.approx(
        [261.62555, 369.99442, 523.2511], rel=1e-5)
    assert len(list(B.midicps(range(0, 120)))) == 120


def test_one_range_serves_the_whole_sequence():
    """The bounds are scalars on purpose: a range is a statement about the
    surface a value is read on, not a property of each element."""
    assert list(B.linexp([0.0, 1.0], 0, 1, 20, 20000)) == pytest.approx([20.0, 20000.0])


def test_an_exponential_input_end_at_zero_takes_the_same_rule():
    """The other side of the same question: a value *at* a range's low end,
    where that end had to be nudged off zero, lands on the end rather than on
    `log(0)`."""
    assert B.explin(0.0, 0, 1, 0, 1) == pytest.approx(0.0)
    assert math.isfinite(B.expexp(0.0, 0, 1, 1, 100))
