"""The default session and the free-standing ``play``: sounding without a
`Session`. Everything that does not run in an explicit session runs in the
default session (`main`); a bare event plays immediately (no clock), self-
releasing on wall time.
"""

import pytest

from clausters import Event, play, main, default_session
from clausters.defs import Server
from clausters.base._oscinterface import OscNrtInterface
from clausters.seq.pattern import Pbind, Pseq


@pytest.fixture
def clean_default():
    """Save/restore the process-wide default-session slots a test mutates."""
    server, clock = main.server, main.default_clock
    main.server, main.default_clock = None, None
    yield
    main.server, main.default_clock = server, clock


def _nrt_server():
    return Server(interface=OscNrtInterface())


def test_default_session_is_main():
    assert default_session is main


def test_resolve_server_raises_when_unbooted(clean_default):
    with pytest.raises(RuntimeError):
        main.resolve_server()


def test_plain_server_does_not_adopt_default(clean_default):
    # Only a free-standing Server.boot() adopts the default session; a plain
    # constructor (as Session.nrt/embed use) must not.
    _nrt_server()
    assert main.server is None


def test_event_play_resolves_ambient_server(clean_default):
    server = _nrt_server()
    main.server = server               # stands in for a free-standing boot
    node_id = Event(degree=0).play()   # no destination: resolves the default
    assert node_id is not None
    # /s_new immediate at t=0, release scheduled at t=sustain (dur*legato=0.8).
    times = sorted(t for t, _ in server.interface.score.bundles)
    assert times == pytest.approx([0.0, 0.8])


def test_event_play_immediate_uses_send_msg_and_release(clean_default):
    server = _nrt_server()
    ev = Event(dur=2.0, legato=0.5)    # sustain = 1.0
    ev.play(server)                    # explicit destination, no clock
    times = sorted(t for t, _ in server.interface.score.bundles)
    assert times == pytest.approx([0.0, 1.0])


def test_free_play_dispatches_event(clean_default):
    server = _nrt_server()
    main.server = server
    node_id = play(Event(degree=2))
    assert node_id is not None
    assert len(server.interface.score.bundles) == 2


def test_free_play_rejects_unplayable(clean_default):
    with pytest.raises(TypeError):
        play(object())


def test_get_default_clock_belongs_to_default_session(clean_default):
    clock = main.get_default_clock(start=False)
    assert clock.session is main
    assert main.get_default_clock(start=False) is clock   # created once


def test_free_play_pattern_resolves_server_and_clock(clean_default):
    server = _nrt_server()
    main.server = server
    # NRT: drive the default clock's render rather than real time.
    player = play(Pbind(instrument="default", degree=Pseq([0, 2, 4]), dur=0.5),
                  clock=main.get_default_clock(start=False))
    main.default_clock.render()
    # three notes, each an /s_new + release bundle
    assert len(server.interface.score.bundles) == 6
    player.stop()
