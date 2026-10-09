"""The panic: one verb that stops what an environment plays -- its clocks
emptied, its timelines halted, its server silenced, the responders freed --
and leaves everything ready to play again."""

import pytest

from clausters import Session, panic
from clausters.base import OscReceiver, TempoClock
from clausters.base.main import main
from clausters.base.stream import Routine
from clausters.responders import MidiFunc, OscFunc
from clausters.base._midiinterface import MidiReceiver


def _forever():
    while True:
        yield 1.0


def test_a_session_panic_empties_its_clocks_and_no_other():
    a, b = Session.nrt(), Session.nrt()
    try:
        extra = a.adopt(TempoClock(timebase=a.timebase))
        for clock in (a.clock, extra, b.clock):
            clock.play(Routine(_forever))
        a.panic()
        assert a.clock.queued == 0 and extra.queued == 0
        assert b.clock.queued == 1
    finally:
        b.clock.clear()
        a.close()
        b.close()


def test_the_default_session_reaches_the_clocks_made_with_no_session(monkeypatch):
    monkeypatch.setattr(main, "server", None)
    loose = TempoClock()                    # nobody keeps it: the default session's
    session = Session.nrt()
    try:
        loose.play(Routine(_forever))
        session.clock.play(Routine(_forever))
        main.panic()
        assert loose.queued == 0
        assert session.clock.queued == 1    # another session's clock is its own
    finally:
        session.clock.clear()
        session.close()
        loose.close()


def test_the_free_verb_panics_the_session_in_force(monkeypatch):
    monkeypatch.setattr(main, "server", None)
    loose = TempoClock()
    session = Session.nrt()
    try:
        loose.play(Routine(_forever))
        session.clock.play(Routine(_forever))
        with session:
            assert panic() is session
        assert session.clock.queued == 0
        assert loose.queued == 1
    finally:
        loose.clear()
        session.close()
        loose.close()


def test_responders_are_freed_unless_permanent():
    recv = OscReceiver().start()
    midi = MidiReceiver()                   # its port is never opened
    session = Session.nrt()
    try:
        gone = OscFunc(lambda *a: None, "/gone", recv=recv)
        kept = OscFunc(lambda *a: None, "/kept", recv=recv, permanent=True)
        note = MidiFunc(lambda *a: None, "note_on", recv=midi)
        session.panic()
        assert not gone.enabled and not note.enabled
        assert kept.enabled
        kept.free()
    finally:
        session.close()
        recv.stop()


def test_an_offline_server_is_sent_nothing():
    session = Session.nrt()
    try:
        before = len(session.server.interface.score.bundles)
        session.server.panic()
        assert len(session.server.interface.score.bundles) == before
    finally:
        session.close()


def _embed_session_or_skip():
    try:
        from clausters import _native
        _native.lib()
    except OSError as e:
        pytest.skip(f"clausters-ffi not built: {e}")
    from clausters.errors import LibraryError, ServerError
    try:
        return Session.embed(latency=0.05)
    except (LibraryError, ServerError, OSError) as e:
        pytest.skip(f"embedded server not available: {e}")


def test_a_live_panic_silences_the_server_and_it_plays_again():
    from clausters.defs import Synth

    session = _embed_session_or_skip()
    try:
        server = session.server
        with session:
            def notes():
                while True:
                    Synth("default", {"freq": 880, "amp": 0.0}, server=server)
                    yield 0.05
            session.clock.play(Routine(notes))
            session.run(0.4)
            assert server.status().synths > 0
            session.panic()
            session.run(0.3)
            assert server.status().synths == 0
            assert session.clock.queued == 0
            Synth("default", {"freq": 880, "amp": 0.0}, server=server)
            session.run(0.1)
            assert server.status().synths == 1
    finally:
        session.close()
