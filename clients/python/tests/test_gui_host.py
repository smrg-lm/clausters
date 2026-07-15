"""G25 client leg: `GuiHost` transport selection.

Pure-unit, no live host: the constructor picks the interface for the carrier
(TCP by default — a `/gui_def` tree is not bounded by a datagram — UDP on
request) without touching the network; connecting is `start()`'s job. The live
TCP round-trip against a real host is exercised manually (`GUIA.md`) and by
the host's own Rust tests (`clients/gui/src/host/tcp.rs`).
"""

import pytest

from clausters.base import OscTcpInterface, OscUdpInterface
from clausters.gui import GuiHost


def test_default_transport_is_tcp_at_the_host_port():
    host = GuiHost("127.0.0.1", 57999)
    assert isinstance(host._osc, OscTcpInterface)
    assert (host._osc.host, host._osc.port) == ("127.0.0.1", 57999)


def test_udp_opt_down():
    host = GuiHost(transport="udp")
    assert isinstance(host._osc, OscUdpInterface)


def test_unknown_transport_is_refused():
    with pytest.raises(ValueError):
        GuiHost(transport="ws")


class _Recorder:
    """A stub OSC interface capturing what GuiHost would send."""

    def __init__(self):
        self.sent = []

    def send_msg(self, target, *args):
        self.sent.append(args)


def test_open_assigns_missing_widget_ids_in_place():
    from clausters.gui import guidef

    host = GuiHost("127.0.0.1", 57998)
    host._osc = _Recorder()
    knob = guidef.knob(label="freq")            # no id: assigned at open
    slider = guidef.slider(7)                   # explicit id: kept verbatim
    inner = guidef.button()
    panel = guidef.panel(None, inner)           # nested id-less children too
    win_a = host.open(guidef.window(knob, slider, panel))
    win_b = host.open(guidef.window(guidef.knob()))
    # Assigned in place, host-unique across windows, disjoint from hand ids.
    assigned = [knob["id"], panel["id"], inner["id"], win_a, win_b]
    assert len(set(assigned)) == len(assigned)
    assert all(i >= 1000 for i in assigned)
    assert slider["id"] == 7


def test_a_non_integer_widget_id_is_refused():
    from clausters.gui import guidef

    with pytest.raises(TypeError, match="widget id"):
        guidef.knob("freq")  # a label mistaken for the positional id
