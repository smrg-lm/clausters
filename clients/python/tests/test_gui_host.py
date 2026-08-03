"""G25 client leg: `GuiHost` transport selection.

Pure-unit, no live host: the constructor picks the interface for the carrier
(TCP by default — a `/gui_def` tree is not bounded by a datagram — UDP on
request) without touching the network; connecting is `start()`'s job. The live
TCP round-trip against a real host is exercised by the GUI examples and by
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


def test_a_supplied_interface_is_used_as_is():
    """The seam a carrier this module does not know about comes in through —
    the same one `clausters.defs.Server` already has."""
    iface = _Recorder()
    host = GuiHost(interface=iface)
    assert host._osc is iface


def test_a_supplied_interface_wins_over_transport():
    """`transport` is not consulted at all, so an unknown one cannot raise:
    the carrier is the object, not the name."""
    iface = _Recorder()
    host = GuiHost(transport="ws", interface=iface)
    assert host._osc is iface


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
    slider = guidef.slider(id=7)                # explicit id: kept verbatim
    inner = guidef.button()
    panel = guidef.panel(inner)                 # nested id-less children too
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
        guidef.knob(id="freq")  # a label mistaken for the id


def test_the_id_is_never_positional():
    """The id is a keyword everywhere, so the positional slot is the widget's
    own material — and the two ways of getting that wrong both raise."""
    from clausters.gui import guidef

    # A leaf takes no positional at all (its material is all keywords)...
    with pytest.raises(TypeError, match="positional"):
        guidef.knob(7)
    # ...and a container's positionals are its children, so a stray id-shaped
    # placeholder is refused as the non-node it is.
    with pytest.raises(TypeError, match="must be a widget node"):
        guidef.panel(None, guidef.button())
    # The material that *is* positional reads without a keyword.
    assert guidef.label("hello")["text"] == "hello"
    assert guidef.meter(4)["bus"] == 4
    assert guidef.menu(["sine", "saw"])["options"] == ["sine", "saw"]
    assert guidef.panel(guidef.button())["children"][0]["type"] == "button"


def test_load_names_a_persisted_def_and_allocates_nothing():
    """`/gui_load` replays a def the host saved, under the id it was saved
    with — so the client neither allocates ids nor resolves names for it."""
    host = GuiHost(interface=_Recorder())
    host.load("mixer")
    assert host._osc.sent == [("/gui_load", "mixer")]
    assert host._alloc.in_use == 0
    assert host._children == {}
