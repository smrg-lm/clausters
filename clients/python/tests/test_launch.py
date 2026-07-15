"""The process launchers' guard rails (nothing here spawns a binary)."""

import socket

import pytest

from clausters.errors import ServerError
from clausters.launch import GuiProcess


def test_start_refuses_a_port_already_in_use():
    # A stale host on the port used to be adopted silently: the fresh child
    # could not bind, but the readiness poll got the stale one's reply. The
    # bind probe turns that into a clear error before anything is spawned.
    hold = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    hold.bind(("127.0.0.1", 0))
    port = hold.getsockname()[1]
    try:
        with pytest.raises(ServerError, match="already in use"):
            GuiProcess(port=port).start()
    finally:
        hold.close()
