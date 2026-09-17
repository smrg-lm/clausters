"""A timeline plays itself: its own tempo map, children, and the verbs.

Offline, so every onset is an exact second of the score. A timeline plays on a
clock of its own, born on its beat 0; a child's beats go to seconds through its
own map; one engine, the root's, wakes the whole tree.
"""

import math

import pytest

from clausters import Session
from clausters.base import Routine
from clausters.seq import Event
from clausters.base.main import main
from clausters.seq.timeline import OscItem, Timeline


@pytest.fixture(autouse=True)
def _no_ambient_session_left():
    previous = main.current_session
    yield
    main.current_session = previous


def _secs(session, addr):
    """The score seconds of every bundle carrying ``addr``, in order."""
    needle = addr.encode() + b"\0"
    return sorted(round(when, 9) for when, packet in session.server.interface.score.bundles
                  if needle in packet)


def _render(session, until=60):
    session.clock.render(until_beat=until)


def _at(session, secs, action):
    """Run ``action`` at ``secs`` on the session's clock (tempo 1)."""
    def body():
        yield secs
        action()
    session.clock.sched_abs(0.0, Routine(body))


def test_a_timeline_plays_on_its_own_tempo():
    s = Session.nrt().activate()
    tl = Timeline([(0, OscItem("/a")), (1, OscItem("/a")), (2, OscItem("/a"))], tempo=2.0)
    tl.play(destination=s.server)
    _render(s)
    assert _secs(s, "/a") == [0.0, 0.5, 1.0]
    assert tl.finished and not tl.playing


def test_a_child_keeps_its_own_units():
    s = Session.nrt().activate()
    child = Timeline([(0, OscItem("/b")), (1, OscItem("/b"))], tempo=2.0)
    parent = Timeline([(0, OscItem("/a")), (3, OscItem("/a"))], tempo=1.0)
    parent.add(2, child)
    parent.play(destination=s.server)
    _render(s)
    assert _secs(s, "/a") == [0.0, 3.0]
    assert _secs(s, "/b") == [2.0, 2.5]


def test_siblings_at_different_tempi_start_together():
    s = Session.nrt().activate()
    slow = Timeline([(1, OscItem("/a"))], tempo=1.0)
    fast = Timeline([(2, OscItem("/b"))], tempo=2.0)
    parent = Timeline(tempo=0.5)
    parent.add(1, slow)
    parent.add(1, fast)
    parent.play(destination=s.server)
    _render(s)
    # Both start at parent beat 1 = second 2; slow's beat 1 and fast's beat 2
    # both fall one second later.
    assert _secs(s, "/a") == [3.0]
    assert _secs(s, "/b") == [3.0]


def test_a_parent_covers_its_children():
    child = Timeline([(4, OscItem("/b"))], tempo=2.0)       # 2 s long
    parent = Timeline([(1, OscItem("/a"))], tempo=1.0)
    parent.add(2, child)
    assert parent.duration() == pytest.approx(4.0)
    child.loop(0, 1)
    assert parent.duration() == math.inf


def test_one_parent_per_timeline_and_no_cycles():
    child = Timeline()
    a, b = Timeline(), Timeline()
    a.add(0, child)
    with pytest.raises(ValueError, match="copy"):
        b.add(0, child)
    with pytest.raises(ValueError, match="ancestor"):
        child.add(0, a)
    with pytest.raises(ValueError, match="ancestor"):
        a.add(0, a)
    b.add(0, child.copy())
    a.clear()
    assert child.parent is None
    b.add(1, child)


def test_copy_is_independent():
    child = Timeline([(0, OscItem("/b"))], tempo=2.0)
    tl = Timeline([(0, OscItem("/a"))], tempo=1.0)
    tl.add(1, child)
    twin = tl.copy()
    twin.map.push(0.0, 3.0)
    assert tl.map.tempo_at(0.0) == 1.0
    copied_child = twin[1][1]
    assert copied_child is not child and copied_child.parent is twin
    assert twin[0][1] is tl[0][1], "stateless items are shared"


def test_a_loop_goes_on_in_physical_time():
    s = Session.nrt().activate()
    tl = Timeline([(0, OscItem("/a")), (1, OscItem("/a"))]).loop(0, 2)
    tl.play(destination=s.server)
    _at(s, 5.5, tl.stop)
    _render(s)
    assert _secs(s, "/a") == [0.0, 1.0, 2.0, 3.0, 4.0, 5.0]


def test_a_loop_shorter_than_a_child_loops_its_part():
    s = Session.nrt().activate()
    child = Timeline([(b, OscItem("/b")) for b in range(4)])
    parent = Timeline().loop(0, 2)
    parent.add(0, child)
    parent.play(destination=s.server)
    _at(s, 5.5, parent.stop)
    _render(s)
    # Only the child's beats 0 and 1 are inside the window, pass after pass.
    assert _secs(s, "/b") == [0.0, 1.0, 2.0, 3.0, 4.0, 5.0]


def test_locate_while_playing_goes_on_from_there():
    s = Session.nrt().activate()
    tl = Timeline([(b, OscItem("/a")) for b in range(12)])
    tl.play(destination=s.server)
    _at(s, 1.5, lambda: tl.locate(10))
    _render(s)
    assert _secs(s, "/a") == [0.0, 1.0, 1.5, 2.5]


def test_entering_a_child_in_its_middle_starts_at_its_next_onset():
    s = Session.nrt().activate()
    child = Timeline([(0, OscItem("/b")), (0.5, OscItem("/b")), (1.5, OscItem("/b"))])
    parent = Timeline()
    parent.add(2, child)
    parent.play(at=3, destination=s.server)
    _render(s)
    # Parent beat 3 is the child's beat 1: its onsets at 0 and 0.5 are passed.
    assert _secs(s, "/b") == [0.5]


def test_a_routine_item_runs_in_its_timelines_beats_and_fresh_each_pass():
    s = Session.nrt().activate()

    def pulse():
        for _ in range(2):
            s.server.send_bundle(("/b",))
            yield 1

    child = Timeline([(0, Routine(pulse))], tempo=2.0)
    parent = Timeline().loop(0, 2)
    parent.add(0, child)
    parent.play(destination=s.server)
    _at(s, 3.9, parent.stop)
    _render(s)
    assert _secs(s, "/b") == [0.0, 0.5, 2.0, 2.5]


def test_a_routine_past_the_located_beat_is_not_recovered():
    s = Session.nrt().activate()

    def pulse():
        s.server.send_bundle(("/b",))
        yield 0

    tl = Timeline([(1, Routine(pulse)), (3, OscItem("/a"))])
    tl.play(at=2, destination=s.server)
    _render(s)
    assert _secs(s, "/b") == []
    assert _secs(s, "/a") == [1.0]


def test_an_events_sustain_is_in_its_timelines_beats():
    s = Session.nrt().activate()
    child = Timeline([(0, Event(instrument="default", dur=1.0, legato=1.0))], tempo=2.0)
    parent = Timeline()
    parent.add(1, child)
    parent.play(destination=s.server)
    _render(s)
    assert _secs(s, "/synth_new") == [1.0]
    released = _secs(s, "/node_set") + _secs(s, "/node_free")
    assert released == [1.5]


def test_pause_holds_and_play_resumes():
    s = Session.nrt().activate()
    tl = Timeline([(b, OscItem("/a")) for b in range(4)])
    tl.play(destination=s.server)
    _at(s, 1.5, tl.pause)
    _at(s, 3.0, tl.play)
    _render(s)
    assert tl.position() == pytest.approx(3.0)
    # Paused at beat 1.5 (second 1.5), resumed at second 3: beats 2 and 3 are
    # half a beat after it.
    assert _secs(s, "/a") == [0.0, 1.0, 3.5, 4.5]


def test_stop_goes_back_to_where_play_started():
    s = Session.nrt().activate()
    tl = Timeline([(b, OscItem("/a")) for b in range(8)])
    tl.play(at=2, destination=s.server)
    _at(s, 1.5, tl.stop)
    _render(s)
    assert tl.position() == 2.0
    assert _secs(s, "/a") == [0.0, 1.0]


def test_a_resume_does_not_move_the_mark():
    s = Session.nrt().activate()
    tl = Timeline([(b, OscItem("/a")) for b in range(8)])
    tl.play(at=1, destination=s.server)
    _at(s, 1.5, tl.pause)
    _at(s, 2.0, tl.play)
    _at(s, 2.5, tl.stop)
    _render(s)
    assert tl.position() == 1.0


def test_quant_lands_on_the_ambient_clocks_grid():
    s = Session.nrt().activate()
    tl = Timeline([(0, OscItem("/a"))], tempo=4.0)

    def later():
        yield 0.5
        tl.play(quant=1, destination=s.server)

    s.clock.play(Routine(later))
    _render(s)
    assert _secs(s, "/a") == [1.0]


def test_a_tempo_change_on_the_map_is_heard():
    s = Session.nrt().activate()
    tl = Timeline([(b, OscItem("/a")) for b in range(4)])
    tl.map.push(2.0, 2.0)
    tl.play(destination=s.server)
    _render(s)
    assert _secs(s, "/a") == [0.0, 1.0, 2.0, 2.5]
