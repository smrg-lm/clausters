"""G25 client leg: `GuiHost` transport selection.

Pure-unit, no live host: the constructor picks the interface for the carrier
(TCP by default — a `/gui_def` tree is not bounded by a datagram — UDP on
request) without touching the network; connecting is `start()`'s job. The live
TCP round-trip against a real host is exercised by the GUI examples and by
the host's own Rust tests (`clients/gui/src/host/tcp.rs`).
"""

import pytest

from clausters.base import OscTcpInterface, OscUdpInterface
from clausters.defs import control as _control
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
        self.started = False

    def start(self):
        self.started = True
        return self

    def send_msg(self, target, *args):
        self.sent.append(args)

    def close(self):
        self.started = False


def test_open_assigns_ids_in_the_document_it_sends_and_leaves_the_tree_alone():
    """Ids identify a live widget, so they belong to the instance, not to the
    document: the tree the caller wrote is never written into, and the ids ride
    only in the JSON that goes out."""
    import json

    from clausters.gui import guidef

    host = GuiHost("127.0.0.1", 57998)
    host._osc = _Recorder()
    k = guidef.knob(label="freq")                # no id: assigned at open
    s = guidef.slider(id=7)                      # explicit id: kept verbatim
    inner = guidef.button()
    pane = guidef.panel(inner)                   # nested id-less children too
    tree = guidef.window(k, s, pane)
    win_a = host.open(tree)
    win_b = host.open(guidef.window(guidef.knob()))

    assert "id" not in k and "id" not in pane and "id" not in inner
    assert s["id"] == 7, "an id the caller picked stays where it was written"

    sent = json.loads(host._osc.sent[0][2])
    knob_id, slider_id, panel_id = [c["id"] for c in sent["children"]]
    inner_id = sent["children"][2]["children"][0]["id"]
    assert slider_id == 7
    assigned = [knob_id, panel_id, inner_id, int(win_a), int(win_b)]
    assert len(set(assigned)) == len(assigned)   # host-unique across windows
    assert all(i >= 1000 for i in assigned)      # disjoint from hand ids


def test_one_tree_opens_twice_with_ids_of_its_own_each_time():
    """A def that cannot be instanced twice is not a def. Each `open` allocates
    a fresh run, and the two windows never share a widget id."""
    import json

    from clausters.gui import guidef

    host = GuiHost("127.0.0.1", 57995)
    host._osc = _Recorder()
    strip = guidef.window(guidef.knob(name="gain"), guidef.toggle(name="mute"))
    a, b = host.open(strip), host.open(strip)

    assert int(a) != int(b)
    assert a["gain"].id != b["gain"].id
    ids = [{c["id"] for c in json.loads(msg[2])["children"]}
           for msg in host._osc.sent]
    assert ids[0].isdisjoint(ids[1])


def test_the_same_subtree_nested_twice_gets_two_id_runs():
    """The nesting case the old in-place walk could not answer: one object, two
    places in the tree. Sharing one id run would have the host skip the second
    subtree ("widget id already in use") and the window would draw wrong."""
    import json

    from clausters.gui import guidef

    host = GuiHost("127.0.0.1", 57994)
    host._osc = _Recorder()
    strip = guidef.panel(guidef.knob())
    host.open(guidef.window(strip, strip))

    left, right = json.loads(host._osc.sent[0][2])["children"]
    assert left["id"] != right["id"]
    assert left["children"][0]["id"] != right["children"][0]["id"]


def test_a_duplicate_name_is_refused_when_the_tree_is_registered():
    """The handle addresses a widget by name, so a repeated name is refused
    here too -- not only where a `View` is built, since a hand-written dict
    tree reaches `open` without passing through a builder."""
    host = GuiHost("127.0.0.1", 57996)
    host._osc = _Recorder()
    tree = {"type": "window", "children": [{"type": "knob", "name": "freq"},
                                           {"type": "slider", "name": "freq"}]}
    with pytest.raises(ValueError, match="duplicate widget name 'freq'"):
        host.open(tree)


def test_a_redraw_keeps_named_handlers_and_refreshes_the_handle():
    """Found by running the whole-loop example (`gui_daw.py` then,
    `gui_composer.py` now): pressing *open* left every button in the
    window dead.

    An editor redrawing its window (`Editor.load`) re-defines the same root.
    That returns the old subtree's ids to the pool and takes fresh ones, so a
    callback registered under an old id is orphaned -- and a handle captured
    before the redraw resolves every name to an id that no longer means it.
    Both are fixed by the same rule: a callback and a name belong to the
    *widget*, not to the number it happened to carry.
    """
    from clausters.gui import guidef

    host = GuiHost("127.0.0.1", 57997)
    host._osc = _Recorder()
    fired = []
    win = host.open(guidef.window(guidef.button(name="play"),
                                  guidef.button(name="stop")))
    win["play"].on_event(lambda value: fired.append(("play", value)))
    old_play = win["play"].id

    # The same window, drawn again from a fresh tree (no ids of its own).
    again = host.define(win, guidef.window(guidef.button(name="play"),
                                           guidef.button(name="stop"),
                                           guidef.button(name="undo")))

    assert again is win, "one window is one handle"
    assert win["play"].id != old_play, "the redraw took a fresh id"
    assert win["undo"], "and the handle resolves what the redraw added"

    # The handler follows the name onto the new id.
    host.dispatch("/gui_event", [win["play"].id, 1, 0, 1])
    assert fired == [("play", 1)]

    # And nothing answers for the id the redraw gave back.
    fired.clear()
    host.dispatch("/gui_event", [old_play, 2, 0, 1])
    assert fired == []


def test_a_non_integer_widget_id_is_refused():
    from clausters.gui import guidef

    with pytest.raises(TypeError, match="widget id"):
        guidef.knob(id="freq")  # a label mistaken for the id


def test_the_id_is_never_positional():
    """The id is a keyword everywhere, so the positional slot is the widget's
    own contents — and the two ways of getting that wrong both raise."""
    from clausters.gui import guidef

    # A leaf with no contents takes no positional at all...
    with pytest.raises(TypeError, match="positional"):
        guidef.button(7)  # pyright: ignore[reportCallIssue] - the point of the test
    # ...and a control's positional is the *def control* it is built from, so an
    # id-shaped number is refused as the non-control it is.
    with pytest.raises(TypeError, match="not a def's control"):
        guidef.knob(7)  # pyright: ignore[reportCallIssue] - the point of the test
    # ...and a container's positionals are its children, so a stray id-shaped
    # placeholder is refused as the non-node it is.
    with pytest.raises(TypeError, match="must be a widget node"):
        guidef.panel(None, guidef.button())
    # The argument that *is* positional reads without a keyword.
    assert guidef.label("hello")["text"] == "hello"
    assert guidef.meter(4)["bus"] == 4
    assert guidef.knob(_control("freq", 220.0), min=110.0, max=880.0)["name"] == "freq"
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


def test_font_hands_a_typeface_over_with_no_id():
    """``/gui_font`` carries the bytes and nothing else: a face is a property
    of the host, not of a window, so it names none and allocates nothing."""
    host = GuiHost(interface=_Recorder())
    host.font(b"\x00\x01\x00\x00face")
    assert host._osc.sent == [("/gui_font", b"\x00\x01\x00\x00face")]
    assert host._alloc.in_use == 0


# ---- attach: the host this handle did not start -------------------------

def test_attach_refuses_an_address_nobody_answers():
    """The whole point of the verb over a bare `start()`: a handle pointing
    nowhere says so here, instead of dropping every later `/gui_def` into a
    void that reports nothing back."""
    from clausters.errors import ServerError

    with pytest.raises(ServerError, match="no GUI host answers"):
        GuiHost(port=57997).attach(timeout=0.05)


def test_attach_does_not_probe_a_supplied_carrier():
    """A carrier this module does not know about may reach a host that answers
    no UDP probe, so the verification is skipped there — the line
    `clausters.defs.Server.attach` draws with ``_own_carrier``."""
    iface = _Recorder()
    host = GuiHost(port=57997, interface=iface).attach(adopt_ambient=False)
    assert iface.started


def test_attach_owns_no_process():
    """Ownership is what separates `attach` from `boot`, and `stop` reads it
    off this: no process here means the host is left standing."""
    host = GuiHost(port=57997, interface=_Recorder()).attach(adopt_ambient=False)
    assert host._process is None


def test_attach_adopts_the_ambient_host_first_wins():
    from clausters.gui import ambient_host, set_ambient_host

    set_ambient_host(None)
    try:
        first = GuiHost(port=57997, interface=_Recorder()).attach()
        assert ambient_host() is first
        second = GuiHost(port=57996, interface=_Recorder()).attach()
        assert ambient_host() is first          # already registered, not displaced
        third = GuiHost(port=57995, interface=_Recorder()).attach(adopt_ambient=False)
        assert ambient_host() is first
        first.stop()
        assert ambient_host() is None           # stopping gives the registration up
    finally:
        set_ambient_host(None)


def test_a_session_takes_a_host_it_did_not_boot():
    """The visual half of taking a `Server` the session did not start: the
    constructor, not a verb of its own — and then `gui` launches nothing."""
    from clausters import Session
    from clausters.base import OscNrtInterface
    from clausters.defs import Server

    host = GuiHost(port=57997, interface=_Recorder())
    session = Session(Server(interface=OscNrtInterface()), gui=host)
    assert session.gui() is host
    assert session.gui_host is host
    assert host._process is None
