"""Per-session random context: each `Session` owns its RNG root, so two
sessions reproduce independently — ``seed(n)`` on one never perturbs another,
and the sound is independent of the order sessions were built in. The default
session (`main`) is just the fallback context when none is named.
"""

from clausters import Session, main
from clausters.base import rand
from clausters.seq import Pbind, Pwhite


def _score(seed):
    s = Session.nrt(tempo=1.0)
    s.seed(seed)
    s.play(Pbind(instrument="default", freq=Pwhite(100.0, 200.0, length=4), dur=0.5))
    s.clock.render()
    return s.server.interface.score.bytes()


def test_same_seed_sessions_reproduce():
    assert _score(7) == _score(7)


def test_different_seeds_differ():
    assert _score(7) != _score(8)


def test_sessions_are_order_independent():
    # Build both, then play in the opposite order: a's random sequencerial must not
    # depend on whether b was created/played first (separate roots, not one
    # shared global root spawned in creation order).
    a = Session.nrt(tempo=1.0)
    a.seed(7)
    b = Session.nrt(tempo=1.0)
    b.seed(99)
    b.play(Pbind(instrument="default", freq=Pwhite(100.0, 200.0, length=4), dur=0.5))
    a.play(Pbind(instrument="default", freq=Pwhite(100.0, 200.0, length=4), dur=0.5))
    a.clock.render()
    assert a.server.interface.score.bytes() == _score(7)
    b.clock.clear()  # b was played and deliberately never rendered


def test_seeding_one_session_does_not_touch_another():
    a = Session.nrt(tempo=1.0)
    b = Session.nrt(tempo=1.0)
    a.seed(1)
    b.seed(2)
    a.seed(1)  # re-seed a after b exists; a must still reproduce its seed-1 sequence
    with a:
        first = rand.next_f64()
    a.seed(1)
    with a:
        again = rand.next_f64()
    assert first == again


def test_current_session_routes_the_root_draw():
    a = Session.nrt(tempo=1.0)
    a.seed(123)
    b = Session.nrt(tempo=1.0)
    b.seed(123)
    with a:
        da = rand.next_f64()
    with b:
        db = rand.next_f64()
    assert da == db                     # same seed, same context routing
    # outside any session, draws come from the default session (main)
    main.seed(123)
    assert rand.next_f64() == da


def test_context_manager_restores_previous_session():
    a = Session.nrt(tempo=1.0)
    assert main.current_session is None
    with a:
        assert main.current_session is a
        b = Session.nrt(tempo=1.0)
        with b:
            assert main.current_session is b
        assert main.current_session is a   # restored, not clobbered
    assert main.current_session is None
