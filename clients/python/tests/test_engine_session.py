"""A session on a GUI host's applications' engine: the server every
application of the host plays on, reached over its shared-memory segment's
ring. Run against a real host with no window on screen (``[gui] headless``),
which opens its engine at the first ask.
"""

import os

import pytest

from clausters import Session
from clausters.base import OscShmInterface
from clausters.errors import CommandError


@pytest.fixture
def engine(gui_host):
    try:
        return gui_host.engine()
    except CommandError as e:
        pytest.skip(f"the host has no engine here: {e}")


def test_the_host_says_where_its_engine_is_served(gui_host, engine):
    assert os.path.exists(engine["shm"])
    # the applications keep the other half of the engine's ids
    assert engine["share"] == [0, 2]
    assert gui_host.engine() == engine, "asked again, the same engine"


def test_a_session_plays_on_the_hosts_engine(gui_host, engine):
    session = Session.live(engine=gui_host)
    try:
        server = session.server
        assert isinstance(server.interface, OscShmInterface)
        assert server.on_engine and server.shm == engine["shm"]
        assert session.gui() is gui_host, "the host is the session's"
        assert server.sync() >= 0
        assert server.status() is not None
        # its clock reads the engine's sample counter in the segment
        assert type(session.timebase).__name__ == "SampleClockTimebase"
        # one client on a segment's ring: a second is refused, not corrupted
        with pytest.raises(RuntimeError, match="one client"):
            OscShmInterface(engine["shm"])
    finally:
        session.close()
    # a host the session was given is the giver's, and stays
    assert gui_host.engine() == engine
    OscShmInterface(engine["shm"]).close()


def test_the_environment_names_the_engine(gui_host, engine, monkeypatch):
    monkeypatch.setenv("CLAUSTERS_ENGINE_SHM", engine["shm"])
    monkeypatch.setenv("CLAUSTERS_ENGINE_SHARE", "0/2")
    session = Session.live()
    try:
        assert session.server.on_engine
        assert session.server.shm == engine["shm"]
    finally:
        session.close()
