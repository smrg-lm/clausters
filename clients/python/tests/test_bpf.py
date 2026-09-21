"""`Bpf` -- an envelope in absolute coordinates, and the same datum an `Env` is.

What is checked here is the *interchangeability*: the two bases round-trip
without losing a shape or a sustain, `env_gen` plays either, and both answer the
curve protocol (``to_points``/``set_points``) an editor writes back through.
"""

import pytest

from clausters.defs.ugens import Bpf, Env, env_gen, quads


def test_quads_reads_both_spellings_and_drops_a_partial():
    flat = [0.0, 0.2, 1, 0.0, 2.0, 0.8, 2, 0.0]
    assert quads(flat) == quads([(0.0, 0.2, 1, 0.0), (2.0, 0.8, 2, 0.0)])
    # A trailing partial quad is dropped rather than guessed at.
    assert quads(flat + [3.0, 0.5]) == quads(flat)


def test_the_two_bases_round_trip():
    # The shape belongs to the segment that *leaves* a point, so an Env's
    # per-segment curves land on all but the last point and come back whole.
    env = Env([0.2, 0.8, 0.5], [1.0, 2.0], ["lin", "exp"])
    back = Bpf.from_env(env).to_env()
    assert back.levels == env.levels
    assert back.times == env.times
    assert back.to_inputs() == env.to_inputs()


def test_a_sustain_survives_the_trip_and_moves_with_a_drawn_delay():
    env = Env.adsr()
    curve = Bpf.from_env(env)
    assert curve.release_node == env.release_node
    assert curve.to_env().release_node == env.release_node

    # A first point later than the axis start is a drawn initial delay, which
    # `points_to_env` encodes as a leading `hold` segment -- so every index
    # after it moves by one.
    delayed = Bpf([(1.0, 0.0, 1, 0.0), (2.0, 1.0, 1, 0.0), (3.0, 0.0, 1, 0.0)],
                  release_node=1)
    made = delayed.to_env()
    assert made.times == [1.0, 1.0, 1.0]
    assert made.release_node == 2
    assert delayed.duration() == 2.0


def test_env_gen_plays_either():
    env = Env([0.0, 1.0], [2.0])
    assert env_gen(Bpf.from_env(env)).inputs == env_gen(env).inputs


def test_both_answer_the_curve_protocol():
    # What an editor asks of a structure is this pair, and nothing about its
    # type -- `is_curve` is written against it.
    from clausters.gui.editing.points import is_curve

    env, curve = Env([0.0, 1.0], [2.0]), Bpf([(0.0, 0.0, 1, 0.0), (2.0, 1.0, 1, 0.0)])
    assert is_curve(env) and is_curve(curve)
    assert not is_curve(object())

    drawn = [0.0, 0.5, 2, 0.0, 1.0, 0.1, 1, 0.0]
    assert curve.set_points(drawn).to_points() == drawn
    assert env.set_points(drawn).to_points() == drawn


def test_a_redrawn_curve_drops_a_sustain_it_no_longer_has():
    # Fewer points than the old sustain indexed: the curve that was drawn has no
    # say about where the old one held, so the index goes rather than dangles.
    env = Env.adsr()
    assert env.release_node == 2
    env.set_points([0.0, 0.0, 1, 0.0, 1.0, 1.0, 1, 0.0])
    assert env.release_node is None


def test_a_curve_needs_two_points():
    with pytest.raises(ValueError):
        Bpf([(0.0, 1.0, 1, 0.0)])
