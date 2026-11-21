"""Per-session random context: each `Session` owns its RNG root, so two
sessions reproduce independently -- ``seed(n)`` on one never perturbs another,
and the sound is independent of the order sessions were built in. The default
session (`main`) is just the fallback context when none is named.
"""

from clausters import Session, main
from clausters.base import rand
from clausters.seq import pbind, pwhite


def _score(seed):
    s = Session.nrt()
    s.seed(seed)
    s.play(pbind(instrument="default", freq=pwhite(100.0, 200.0, length=4), dur=0.5))
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
    a = Session.nrt()
    a.seed(7)
    b = Session.nrt()
    b.seed(99)
    b.play(pbind(instrument="default", freq=pwhite(100.0, 200.0, length=4), dur=0.5))
    a.play(pbind(instrument="default", freq=pwhite(100.0, 200.0, length=4), dur=0.5))
    a.clock.render()
    assert a.server.interface.score.bytes() == _score(7)
    b.clock.clear()  # b was played and deliberately never rendered


def test_seeding_one_session_does_not_touch_another():
    a = Session.nrt()
    b = Session.nrt()
    a.seed(1)
    b.seed(2)
    a.seed(1)  # re-seed a after b exists; a must still reproduce its seed-1 sequence
    with a:
        first = rand.rand()
    a.seed(1)
    with a:
        again = rand.rand()
    assert first == again


def test_current_session_routes_the_root_draw():
    a = Session.nrt()
    a.seed(123)
    b = Session.nrt()
    b.seed(123)
    with a:
        da = rand.rand()
    with b:
        db = rand.rand()
    assert da == db                     # same seed, same context routing
    # outside any session, draws come from the default session (main)
    main.seed(123)
    assert rand.rand() == da


def test_context_manager_restores_previous_session():
    a = Session.nrt()
    assert main.current_session is None
    with a:
        assert main.current_session is a
        b = Session.nrt()
        with b:
            assert main.current_session is b
        assert main.current_session is a   # restored, not clobbered
    assert main.current_session is None


def test_seeding_a_session_seeds_its_server():
    # One call pins both halves of a take: the session's random context and
    # the server's generators, which its random UGens draw from. Offline, the
    # reseed is an event of the score, at its start.
    s = Session.nrt()
    assert s.seed(1956) == 1956
    score = s.server.interface.score.bytes()
    assert b"/server_seed" in score and b"1956" in score


def test_a_first_draw_does_not_reseed_the_server():
    # The context seeds itself lazily on a first draw; that is its own seed,
    # not a call to seed the environment, and must not reach the server.
    s = Session.nrt()
    s.rng.rand()
    assert b"/server_seed" not in s.server.interface.score.bytes()
