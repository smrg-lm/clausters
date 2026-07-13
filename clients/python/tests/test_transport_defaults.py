"""C34: the client's transport defaults — probe over UDP, commands over TCP.

Pure-unit: no live server. The default `Server` interface is a lazy
`OscTcpInterface` (it connects on first send, so a handle can exist before a
reachable server), UDP stays an explicit opt-down, an oversized UDP send fails
early with an error naming TCP, and bulk chunks size themselves from the frame
ceiling `/server_info` advertises. The live TCP path is exercised by the Rust
integration tests and the E2E smoke in `GUIA.md`.
"""

import pytest

from clausters.base import OscTcpInterface, OscUdpInterface
from clausters.defs.server import Server, ServerInfo


def test_default_interface_is_lazy_tcp_at_the_target():
    server = Server("127.0.0.1", 57998)
    assert isinstance(server.interface, OscTcpInterface)
    assert (server.interface.host, server.interface.port) == ("127.0.0.1", 57998)
    # Lazy: no connection was opened (there is no server at that port).
    assert server.interface._sock is None
    server.close()


def test_udp_opt_down_and_unknown_transport():
    server = Server(transport="udp")
    assert isinstance(server.interface, OscUdpInterface)
    server.close()
    with pytest.raises(ValueError):
        Server(transport="carrier-pigeon")


def test_oversized_udp_send_fails_early_naming_tcp():
    iface = OscUdpInterface().start()
    try:
        blob = bytes(70_000)  # over the ~64 KB datagram cap
        with pytest.raises(ValueError, match="TCP"):
            iface.send_msg(("127.0.0.1", 57110), "/d_recv", blob)
    finally:
        iface.close()


def test_server_info_max_frame_falls_back_to_the_datagram_cap():
    # A 13-field reply (pre-M25) still parses; max_frame degrades to 64 KiB.
    info = ServerInfo(128, 1024, 2, 64, 48000.0, 48000.0)
    assert info.max_frame == 65536


def test_bulk_chunk_sizes_from_the_advertised_ceiling():
    server = Server("127.0.0.1", 57997)  # default tcp, never connected
    server._max_frame = 16 * 1024 * 1024  # as if /server_info had answered
    assert server._bulk_chunk(timeout=0.0) == (16 * 1024 * 1024 - 256) // 4
    server.close()

    udp = Server(transport="udp")
    assert udp._bulk_chunk(timeout=0.0) == 1024  # datagram-bounded
    udp.close()
