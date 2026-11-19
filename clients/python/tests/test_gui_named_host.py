"""The host an editor names for its session: read from the environment, and
pointed at the session's server with ``/gui_server``."""

from clausters.base.ids import IdShare
from clausters.gui.host import HOST_VARIABLE, GuiHost, named_host


def test_the_environment_names_the_host_by_carrier(monkeypatch):
    monkeypatch.delenv(HOST_VARIABLE, raising=False)
    assert named_host() is None
    monkeypatch.setenv(HOST_VARIABLE, "udp://127.0.0.1:40001 ws://127.0.0.1:40002")
    assert named_host() == ("127.0.0.1", 40001), "the udp:// one, whose port takes TCP"
    monkeypatch.setenv(HOST_VARIABLE, "ws://127.0.0.1:40002")
    assert named_host() is None, "nothing this client speaks"
    monkeypatch.setenv(HOST_VARIABLE, "localhost:40003")
    assert named_host() == ("localhost", 40003), "a bare address is UDP's"


def test_a_host_is_pointed_at_a_server():
    host = GuiHost()
    sent = []
    host._send = lambda addr, *args: sent.append((addr, *args))
    host.server("127.0.0.1:57110", shm="/clausters", id_share=IdShare(1, 2))
    host.server("127.0.0.1:57110")
    assert sent == [
        ("/gui_server", "127.0.0.1:57110", "/clausters", "1/2"),
        ("/gui_server", "127.0.0.1:57110", "", ""),
    ]
