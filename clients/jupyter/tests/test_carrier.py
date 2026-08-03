"""The comm carrier and its replay journal, with a fake link for a front end.

No kernel and no browser: what is tested here is that the carrier is an
`OscInterface` like any other (a `GuiHost` drives it unchanged), that the
journal replays a tree rather than its history, and that a round trip inside a
cell refuses instead of hanging.
"""

import json
import threading

import pytest

from clausters.base import _osclib
from clausters.gui import GuiHost, guidef

from clausters_jupyter.carrier import (GUI_CHANNEL, SERVER_CHANNEL, CommInterface,
                                       RoundTripInCell)
from clausters_jupyter.journal import Journal


class FakeLink:
    """Stands in for the widget: records sends, delivers inbound by hand."""

    def __init__(self):
        self.sent: list = []
        self._subs: dict = {}

    def send_packet(self, channel, payload, root=None):
        self.sent.append((channel, payload))

    def subscribe(self, channel, cb):
        self._subs.setdefault(channel, []).append(cb)

    def unsubscribe(self, channel, cb):
        self._subs.get(channel, []).remove(cb)

    def deliver(self, channel, payload):
        """As if the page had sent this packet up."""
        for cb in self._subs.get(channel, []):
            cb(payload)

    def addrs(self):
        return [_osclib.decode(p)[0] for _, p in self.sent]


def _window(**kw):
    return guidef.window(guidef.slider(name="cutoff", **kw), title="w")


# ---- the interface ----

def test_a_guihost_drives_the_carrier_unchanged():
    link = FakeLink()
    host = GuiHost(interface=CommInterface(link))
    win = host.open(_window())
    assert link.addrs() == ["/gui_def"]
    win["cutoff"].set(value=0.5)
    assert link.addrs() == ["/gui_def", "/gui_set"]
    channel, packet = link.sent[-1]
    assert channel == GUI_CHANNEL
    addr, args = _osclib.decode(packet)
    assert addr == "/gui_set" and "value" in args


def test_a_bundle_rides_as_one_packet():
    link = FakeLink()
    iface = CommInterface(link)
    iface.send_bundle(None, 1.0, ("/gui_set", 1, "value", 0.25),
                      ("/gui_set", 2, "value", 0.5))
    (channel, packet), = link.sent
    assert channel == GUI_CHANNEL
    assert packet.startswith(b"#bundle")


def test_the_carrier_declares_itself_a_stream():
    """A comm message frames its own payload, so bulk is not datagram-bounded
    (the capability `Server._bulk_chunk` reads)."""
    assert CommInterface(FakeLink()).stream is True


def test_an_inbound_packet_reaches_the_handle_callback():
    link = FakeLink()
    host = GuiHost(interface=CommInterface(link))
    win = host.open(_window())
    seen = []
    win["cutoff"].on_event(lambda *a: seen.append(a))
    wid = int(win["cutoff"])
    link.deliver(GUI_CHANNEL, _osclib.message("/gui_event", wid, 0.75))
    assert host.pump() == 1
    assert seen and seen[0][0] == pytest.approx(0.75)


def test_close_unsubscribes():
    link = FakeLink()
    iface = CommInterface(link)
    iface.close()
    assert link._subs[GUI_CHANNEL] == []
    with pytest.raises(RuntimeError):
        iface.send_msg(None, "/gui_free", 1)


# ---- the refusal ----

def test_waiting_for_a_reply_in_the_cell_refuses_rather_than_hangs():
    link = FakeLink()
    iface = CommInterface(link)          # built on "the cell thread": this one
    with pytest.raises(RoundTripInCell) as excinfo:
        iface.recv(1.0)
    assert "two cells" in str(excinfo.value)


def test_a_zero_timeout_is_a_poll_and_always_works():
    """`GuiHost.pump` drains with timeout=0; that is not a round trip and must
    keep working inside a cell, since it is how a notebook reads events back."""
    link = FakeLink()
    iface = CommInterface(link)
    assert iface.recv(0.0) is None
    link.deliver(GUI_CHANNEL, _osclib.message("/gui_event", 7, 1.0))
    assert iface.recv(0.0) is not None


def test_another_thread_may_wait():
    """A routine on the clock thread does not hold the kernel's lock, so its
    reply can arrive and waiting for one is legitimate."""
    link = FakeLink()
    iface = CommInterface(link)
    out = []

    def worker():
        out.append(iface.recv(2.0))

    t = threading.Thread(target=worker)
    t.start()
    link.deliver(GUI_CHANNEL, _osclib.message("/gui_info", 3, "slider"))
    t.join(timeout=3.0)
    assert not t.is_alive() and out and out[0] is not None


# ---- the journal ----

def test_the_journal_replays_the_tree_not_its_history():
    link = FakeLink()
    host = GuiHost(interface=CommInterface(link))
    win = host.open(_window())
    for value in range(200):
        win["cutoff"].set(value=float(value))
    iface = host._osc
    packets = iface.replay()
    # One definition plus one packet for the single property edited, however
    # many times it was set.
    assert len(packets) == 2
    assert _osclib.decode(packets[0])[0] == "/gui_def"
    addr, args = _osclib.decode(packets[1])
    assert addr == "/gui_set" and args[-1] == pytest.approx(199.0)


def test_distinct_properties_are_kept_side_by_side():
    j = Journal()
    tree = json.dumps({"type": "window", "id": 1,
                       "children": [{"type": "slider", "id": 2}]})
    j.record(_osclib.message("/gui_def", 1, tree))
    j.record(_osclib.message("/gui_set", 2, "value", 0.5))
    j.record(_osclib.message("/gui_set", 2, "label", "hi"))
    j.record(_osclib.message("/gui_set", 2, "value", 0.9))
    packets = j.replay()
    assert len(packets) == 3            # def + value + label
    values = [_osclib.decode(p)[1][-1] for p in packets[1:]]
    assert "hi" in values
    assert any(v == pytest.approx(0.9) for v in values if isinstance(v, float))


def test_freeing_a_root_drops_its_whole_entry():
    j = Journal()
    tree = json.dumps({"type": "window", "id": 1,
                       "children": [{"type": "slider", "id": 2}]})
    j.record(_osclib.message("/gui_def", 1, tree))
    j.record(_osclib.message("/gui_set", 2, "value", 0.5))
    assert len(j) == 2
    j.record(_osclib.message("/gui_free", 1))
    assert j.replay() == []


def test_redefining_a_root_replaces_it():
    j = Journal()
    first = json.dumps({"type": "window", "id": 1, "children": []})
    second = json.dumps({"type": "window", "id": 1,
                         "children": [{"type": "slider", "id": 5}]})
    j.record(_osclib.message("/gui_def", 1, first))
    j.record(_osclib.message("/gui_def", 1, second))
    packets = j.replay()
    assert len(packets) == 1
    assert _osclib.decode(packets[0])[1][1] == second


def test_two_windows_replay_in_definition_order():
    j = Journal()
    for wid in (1, 2):
        j.record(_osclib.message(
            "/gui_def", wid, json.dumps({"type": "window", "id": wid})))
    ids = [_osclib.decode(p)[1][0] for p in j.replay()]
    assert ids == [1, 2]


def test_a_bind_supersedes_the_previous_one():
    j = Journal()
    j.record(_osclib.message("/gui_def", 1, json.dumps(
        {"type": "window", "id": 1, "children": [{"type": "knob", "id": 2}]})))
    j.record(_osclib.message("/gui_bind", 2, "server", "/node_set", 100, "freq"))
    j.record(_osclib.message("/gui_bind", 2))     # unbind
    packets = j.replay()
    assert len(packets) == 2
    assert _osclib.decode(packets[1])[1] == [2]


def test_a_query_is_not_journalled():
    j = Journal()
    j.record(_osclib.message("/gui_def", 1, json.dumps({"type": "window", "id": 1})))
    assert j.record(_osclib.message("/gui_query", 1)) is False
    assert len(j.replay()) == 1


def test_the_audio_channel_is_not_journalled():
    """Replaying `/synth_new` at a running engine would start a second voice,
    so a reloaded page rejoins the server instead of re-running its history."""
    link = FakeLink()
    iface = CommInterface(link, channel=SERVER_CHANNEL)
    assert iface.journal is None
    iface.send_msg(None, "/synth_new", "voice", 1000, 0, 0)
    assert iface.replay() == []
    assert len(link.sent) == 1
