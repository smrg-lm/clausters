"""G25 client leg: `GuiHost` transport selection.

Pure-unit, no live host: the constructor picks the interface for the carrier
(TCP by default -- a `/gui_def` tree is not bounded by a datagram -- UDP on
request) without touching the network; connecting is `start()`'s job. The live
TCP round-trip against a real host is exercised by the GUI examples and by
the host's own Rust tests (`clients/gui/src/host/tcp.rs`).
"""

import threading

import pytest

from clausters.base import OscTcpInterface, OscUdpInterface
from clausters.defs import control as _control
from clausters.errors import CommandError
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
    """The seam a carrier this module does not know about comes in through --
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

    def fileno(self):
        # An interface with no descriptor -- the loop waits on it "blind",
        # through `recv`, exactly as `OscInterface` allows.
        return None

    def recv(self, timeout=0.0):
        import time as _time

        _time.sleep(timeout)            # a carrier that waits, and says nothing
        return None

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
    `composer.py` now): pressing *open* left every button in the
    window dead.

    An editor redrawing its window re-defines the same root.
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
    own contents -- and the two ways of getting that wrong both raise."""
    from clausters.gui import guidef

    # A leaf with no contents takes no positional at all...
    with pytest.raises(TypeError, match="positional"):
        guidef.text(7)  # pyright: ignore[reportCallIssue] - the point of the test
    # ...and a control widget's positional is the *def control* it is built
    # from, so an id-shaped number is refused as the non-control it is. Every
    # one of the five takes it, `button` included: what it drives is a gate or
    # a trigger, which is a control like any other.
    for build in (guidef.knob, guidef.slider, guidef.number, guidef.toggle, guidef.button):
        with pytest.raises(TypeError, match="not a def's control"):
            build(7)  # pyright: ignore[reportCallIssue] - the point of the test
    # ...and a container's positionals are its children, so a stray id-shaped
    # placeholder is refused as the non-node it is.
    with pytest.raises(TypeError, match="must be a widget node"):
        guidef.panel(None, guidef.button())
    # The argument that *is* positional reads without a keyword.
    assert guidef.label("hello")["text"] == "hello"
    assert guidef.meter(4)["bus"] == 4
    assert guidef.knob(_control("freq", 220.0), min=110.0, max=880.0)["name"] == "freq"
    assert guidef.choice(["sine", "saw"])["options"] == ["sine", "saw"]
    assert guidef.panel(guidef.button())["children"][0]["type"] == "button"


def test_a_meters_zones_are_a_flag_or_a_list_of_stops():
    """`zones` rides as the flag every boolean prop rides as, or as the stops
    the host reads -- a value, a colour and an optional shape -- each value a
    float so both clients write the same JSON."""
    from clausters.gui import guidef
    assert guidef.meter(0, zones=True)["zones"] == 1
    assert guidef.meter(0, zones=False)["zones"] == 0
    assert guidef.meter(0, zones=[(-1, "#d04040"), (0, "meter_low", "lin"),
                                  (0.5, "meter_mid", -3.0)])["zones"] == [
        [-1.0, "#d04040"], [0.0, "meter_low", "lin"], [0.5, "meter_mid", -3.0]]
    assert "zones" not in guidef.meter(0)


def test_load_names_a_persisted_def_and_allocates_nothing():
    """`/gui_load` replays a def the host saved, under the id it was saved
    with -- so the client neither allocates ids nor resolves names for it."""
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


def test_verb_names_a_window_and_a_verb():
    """``/gui_verb`` performs a verb on a window: the window by its id, or
    by the handle `open` returned."""
    host = GuiHost(interface=_Recorder())
    host.verb(7, "split")
    host.verb(type("Handle", (), {"id": 8})(), "select_all")
    assert host._osc.sent == [("/gui_verb", 7, "split"), ("/gui_verb", 8, "select_all")]


class _Answering(_Recorder):
    """A carrier that answers: what it was told to answer each address with,
    as encoded datagrams, after recording what was sent."""

    def __init__(self, answers):
        super().__init__()
        self.answers = answers
        self.inbox = []

    def send_msg(self, target, *args):
        super().send_msg(target, *args)
        from clausters.base import _osclib

        for addr, reply in self.answers.get(args[0], []):
            self.inbox.append(_osclib.message(addr, *reply))

    def recv(self, timeout=0.0):
        return self.inbox.pop(0) if self.inbox else None


def test_windows_lists_what_is_open_and_drops_what_closed():
    """`windows` is what a script that looks at every window walks: the ones
    opened and not closed, in order."""
    from clausters.gui import guidef

    host = GuiHost(interface=_Recorder())
    a = host.open(guidef.window(guidef.knob()))
    b = host.open(guidef.window(guidef.knob()))
    assert host.windows == sorted([int(a), int(b)])
    host.dispatch("/gui_closed", [int(a)])
    assert host.windows == [int(b)]


def test_pointer_names_a_window_a_kind_and_a_point():
    """``/gui_pointer`` plays a hand to a window: the kind and the point as
    floats, and the modifiers only when some are held."""
    host = GuiHost(interface=_Recorder())
    host.pointer(7, "press", 10, 20)
    host.pointer(type("Handle", (), {"id": 8})(), "move", 11.5, 21, "shift+ctrl")
    assert host._osc.sent == [("/gui_pointer", 7, "press", 10.0, 20.0),
                              ("/gui_pointer", 8, "move", 11.5, 21.0, "shift+ctrl")]


def test_capture_waits_for_the_turns_before_it():
    """A capture asks a question first and waits for its answer, delivering
    the events that come before it -- an owner's turns -- and only then asks
    for the picture; the answer is the path."""
    host = GuiHost(interface=_Answering({
        "/gui_query": [("/gui_event", [12, 3, 0, "value", 0.5]),
                       ("/gui_info", [7, "window"])],
        "/gui_capture": [("/gui_captured", [7, "shot.png"])],
    }))
    seen = []
    host.subscribe(lambda addr, args: seen.append((addr, host._osc.sent[-1][0])))
    assert host.capture(7, "shot.png") == "shot.png"
    assert [m[0] for m in host._osc.sent] == ["/gui_query", "/gui_capture"]
    assert host._osc.sent[1] == ("/gui_capture", 7, "shot.png")
    # The event was delivered while the question was the last thing sent.
    assert seen == [("/gui_event", "/gui_query")]


def test_activity_reads_what_a_window_costs():
    """``/gui_activity`` answers counters, whether the window is seen and the
    reasons it ticks; an event on the way is delivered, not taken for it."""
    host = GuiHost(interface=_Answering({
        "/gui_activity": [("/gui_event", [12, 3, 0, "value", 0.5]),
                          ("/gui_activity.reply",
                           [7, "ticks", 30, "live", 2, "visible", 1,
                            "demand", "fed,playhead"])],
    }))
    seen = []
    host.subscribe(lambda addr, args: seen.append(addr))
    assert host.activity(7) == {"ticks": 30, "live": 2, "visible": True,
                                "demand": ["fed", "playhead"]}
    assert host._osc.sent == [("/gui_activity", 7)]
    assert seen == ["/gui_event"]


def test_activity_of_a_settled_window_has_no_demand():
    host = GuiHost(interface=_Answering({
        "/gui_activity": [("/gui_activity.reply", [7, "ticks", 0, "demand", ""])],
    }))
    assert host.activity(7)["demand"] == []


class _Turns(_Answering):
    """Answers each send of an address with the next of its scripted turns."""

    def send_msg(self, target, *args):
        _Recorder.send_msg(self, target, *args)
        from clausters.base import _osclib

        turns = self.answers.get(args[0], [])
        for addr, reply in turns.pop(0) if turns else []:
            self.inbox.append(_osclib.message(addr, *reply))


def test_profile_is_what_moved_between_two_readings():
    """`profile` reads the window twice and answers the difference, with the
    window's state as it stands at the end; the report reads it out."""
    from clausters.gui import format_window_profile

    def reading(ticks, whole, build, demand):
        return [("/gui_activity.reply",
                 [7, "ticks", ticks, "whole", whole, "live", 0, "still", 0,
                  "layout_us", 0, "build_us", build, "upload_us", 0,
                  "encode_us", 0, "present_us", 0, "uploaded_bytes", 0,
                  "placed", 10 * whole, "clock_queries", 0, "transport_queries", 0,
                  "read_backs", 0, "tree_queries", 0, "fetched_bytes", 0,
                  "visible", 1, "demand", demand])]

    host = GuiHost(interface=_Turns({
        "/gui_activity": [reading(30, 3, 900, ""), reading(90, 5, 2900, "playhead")],
    }))
    profile = host.profile(7, seconds=0)
    assert host._osc.sent == [("/gui_activity", 7), ("/gui_activity", 7)]
    assert (profile["ticks"], profile["whole"], profile["build_us"]) == (60, 2, 2000)
    assert profile["placed"] == 20 and profile["demand"] == ["playhead"]
    report = format_window_profile(profile)
    assert "2 whole" in report and "build 1.00" in report
    assert "10 widgets placed per whole frame" in report


def test_open_app_answers_the_window_and_call_app_its_answer():
    """``/gui_app_open`` answers the window an application opened in, and a
    ``/gui_app_call`` the value it answered, matched by seq: a stale answer
    to an earlier seq and an event on the way are not taken for it."""
    import json

    host = GuiHost(interface=_Answering({
        "/gui_app_open": [("/gui_app_reply", [1, json.dumps({"ok": {"window": 7}})])],
        "/gui_app_call": [("/gui_event", [12, 3, 0, "value", 0.5]),
                          ("/gui_app_reply", [1, json.dumps({"ok": "stale"})]),
                          ("/gui_app_reply", [2, json.dumps({"ok": {"frames": 24000}})])],
    }))
    seen = []
    host.subscribe(lambda addr, args: seen.append(addr))
    assert host.open_app("audio", {"path": "take.wav"}) == 7
    assert host._osc.sent[0] == ("/gui_app_open", 1, "audio", '{"path": "take.wav"}')
    assert host.call_app(7, {"member": {"verb": "file"}}) == {"frames": 24000}
    assert host._osc.sent[1] == ("/gui_app_call", 2, 7, '{"member": {"verb": "file"}}')
    assert seen == ["/gui_event"]


@pytest.mark.parametrize("kind, raised", [("value", ValueError),
                                          ("command", CommandError),
                                          ("runtime", RuntimeError)])
def test_call_app_raises_what_the_application_raised(kind, raised):
    import json

    answer = json.dumps({"error": {"type": kind, "message": "no"}})
    host = GuiHost(interface=_Answering({"/gui_app_call": [("/gui_app_reply", [1, answer])]}))
    with pytest.raises(raised, match="no"):
        host.call_app(7, {"query": "transport"})


def test_capture_raises_the_reason_the_host_gives():
    """A host that wrote nothing says why, and the call raises it."""
    from clausters.errors import CaptureError

    host = GuiHost(interface=_Answering({
        "/gui_query": [("/gui_info", [7, "window"])],
        "/gui_capture": [("/gui_captured", [7, "shot.png", "no such window"])],
    }))
    with pytest.raises(CaptureError, match="no such window"):
        host.capture(7, "shot.png")


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
    no UDP probe, so the verification is skipped there -- the line
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
    constructor, not a verb of its own -- and then `gui` launches nothing."""
    from clausters import Session
    from clausters.base import OscNrtInterface
    from clausters.defs import Server

    host = GuiHost(port=57997, interface=_Recorder())
    session = Session(Server(interface=OscNrtInterface()), gui=host)
    assert session.gui() is host
    assert session.gui_host is host
    assert host._process is None


# ---- the event loop owns the socket ----


class _Loopable(_Recorder):
    """A `_Recorder` an `EventLoop` can be told to watch: no descriptor, and
    whatever was queued into it."""

    def __init__(self, *replies):
        super().__init__()
        self.queued = list(replies)

    def fileno(self):
        return None

    def recv(self, timeout):
        if self.queued:
            return self.queued.pop(0)
        import time as _time

        _time.sleep(timeout)            # a carrier that waits, like a real one
        return None


def test_poll_and_pump_stand_down_while_the_loop_drains():
    """Two drains over one socket would race for every message and deliver half
    of them twice, so the manual pair answers "nothing" rather than reading.
    A script written around `poll` keeps running beside a loop; it simply stops
    being the one that delivers."""
    host = GuiHost(interface=_Loopable())
    assert host.looping is False
    host.loop                                   # built and started here
    try:
        assert host.looping is True
        assert host.poll(0.0) is None
        assert host.pump(0.0) == 0
    finally:
        host.loop.close()


def test_opening_a_window_starts_the_loop():
    """An open window is the thing that has events, so having one is what makes
    a loop necessary: `on_event` and `on_closed` fire from the moment they are
    registered, with nothing in the script driving them."""
    from clausters.gui import guidef

    host = GuiHost(interface=_Loopable())
    assert host.looping is False
    try:
        win = host.open(guidef.window(guidef.knob()))
        assert host.looping is True
        assert win.closed is False
    finally:
        host.stop()
    assert host.looping is False                # stop() ends the loop with it


def test_wait_returns_when_the_last_window_closes():
    """The one call a script ends with. It is not a drain -- the loop delivers
    the close on its own thread -- only a way to stay alive until then."""
    import threading

    from clausters.gui import guidef

    host = GuiHost(interface=_Loopable())
    try:
        win = host.open(guidef.window(guidef.knob()))
        threading.Timer(0.1, lambda: host.deliver("/gui_closed", [int(win)])).start()
        assert host.wait(timeout=2.0) is True
        assert win.closed is True
        assert host.wait(timeout=0.0) is True    # nothing open: returns at once
    finally:
        host.stop()


def test_wait_gives_up_on_a_window_nobody_closes():
    """A bounded wait is what a notebook cell asks for when it does want to
    block: it comes back saying the window is still there."""
    from clausters.gui import guidef

    host = GuiHost(interface=_Loopable())
    try:
        win = host.open(guidef.window(guidef.knob()))
        assert win.wait(timeout=0.15) is False
        assert win.closed is False
    finally:
        host.stop()


def test_waiting_from_the_loops_own_thread_is_refused():
    """Waiting for a close on the thread that has to deliver it is a deadlock
    the caller cannot see, so it raises instead of hanging."""
    from clausters.gui import guidef

    host = GuiHost(interface=_Loopable())
    try:
        host.open(guidef.window(guidef.knob()))
        caught = []
        done = threading.Event()

        def on_the_loop():
            try:
                host.wait(timeout=0.01)
            except RuntimeError as exc:
                caught.append(exc)
            finally:
                done.set()

        host.loop.post(on_the_loop)
        assert done.wait(2.0)
        assert caught and "event loop" in str(caught[0])
    finally:
        host.stop()


def test_a_subscriber_is_handed_every_message_before_the_callbacks():
    """The seam an owner of data plugs into: an editor applies an edit here,
    ahead of a callback the script registered on the same window."""
    host = GuiHost(interface=_Recorder())
    seen = []
    host.subscribe(lambda addr, args: seen.append((addr, args)) or True)
    host.deliver("/gui_event", [7, 1, 1, "points", 0.0])
    assert seen == [("/gui_event", [7, 1, 1, "points", 0.0])]
    host.unsubscribe(seen.append)               # not registered: not an error


def test_a_reply_goes_to_whoever_asked_and_an_event_does_not():
    """A reply is not an event. Dispatching one both loses it and hands a
    query's answer to a widget callback -- and reading it off the socket by hand
    takes whatever arrived next, which is how an event landing mid-query used to
    lose the reply with no loop running at all."""
    from clausters.gui.host import _HostSource

    host = GuiHost(interface=_Recorder())
    source = _HostSource(host)
    events = []
    host.subscribe(lambda addr, args: events.append(addr))
    source.deliver(("/gui_info", [7, "curve"]))
    source.deliver(("/gui_event", [7, 1, 1, "points"]))
    assert events == ["/gui_event"]
    assert host._replies.get_nowait() == ("/gui_info", [7, "curve"])


def test_a_menu_is_a_tree_of_entries_and_rides_as_a_prop():
    """A menu is not a node: it is the value of the three props that show one
    -- a window's bar, a button's list, any widget's context menu."""
    from clausters.gui import guidef

    file = guidef.menu(guidef.entry("Open", "open"), "-",
                       guidef.entry("Loop", "loop", checked=False),
                       guidef.entry("Recent", menu=guidef.menu("a.wav", "b.wav")),
                       guidef.entry("Export", enabled=False))
    assert file[0] == {"label": "Open", "verb": "open"}
    assert file[1] == "-"
    assert file[2]["checked"] is False
    assert file[3]["menu"] == ["a.wav", "b.wav"]
    assert file[4] == {"label": "Export", "enabled": False}
    with pytest.raises(TypeError, match="a menu holds entries"):
        guidef.menu(3)  # pyright: ignore[reportArgumentType] - the point of the test

    bar = guidef.menu(guidef.entry("File", menu=file))
    win = guidef.view(guidef.button(label="tools", menu=guidef.menu("Split")),
                      guidef.knob(context=guidef.menu("Reset")), menu=bar)
    assert win["menu"] == bar
    assert win["children"][0]["menu"] == ["Split"]
    assert win["children"][1]["context"] == ["Reset"]


def test_a_choice_names_its_view_and_refuses_one_it_does_not_have():
    from clausters.gui import guidef

    assert guidef.choice(["a", "b"], view="tabs")["view"] == "tabs"
    assert "view" not in guidef.choice(["a", "b"]), "a combo says nothing"
    with pytest.raises(ValueError, match="unknown choice view"):
        guidef.choice(["a"], view="carousel")
    with pytest.raises(ValueError, match="unknown toggle view"):
        guidef.toggle(view="lever")


def test_tabs_bind_their_strip_to_their_pages_by_the_ids_the_open_assigns():
    """`tabs` wires a chooser to a stack before either has an id: the binding
    names the stack *node*, and opening the tree resolves it -- per copy, so the
    same tabs nested twice wire each strip to its own pages."""
    import json

    from clausters.gui import guidef

    host = GuiHost("127.0.0.1", 57993)
    host._osc = _Recorder()
    book = guidef.tabs(guidef.label("one"), guidef.label("two"), titles=["A", "B"])
    host.open(guidef.window(book, book))

    left, right = json.loads(host._osc.sent[0][2])["children"]
    for copy in (left, right):
        strip, pages = copy["children"]
        assert strip["view"] == "tabs" and strip["options"] == ["A", "B"]
        assert pages["flow"] == "stack"
        assert strip["bind"] == ["widget", pages["id"], "index"]
    assert left["children"][0]["bind"] != right["children"][0]["bind"]
    # The tree the caller wrote still names the node: it opens again.
    assert isinstance(book["children"][0]["bind"][1], dict)
    with pytest.raises(ValueError, match="one title per page"):
        guidef.tabs(guidef.label("one"), titles=["A", "B"])
    # ...and it cannot be serialized before the ids exist.
    with pytest.raises(ValueError, match="binds a widget by reference"):
        guidef.to_json(book)


def test_a_binding_by_reference_must_reach_a_widget_of_the_tree():
    from clausters.gui import guidef

    host = GuiHost("127.0.0.1", 57992)
    host._osc = _Recorder()
    elsewhere = guidef.stack(guidef.label("x"))
    stray = guidef.choice(["a"], bind=["widget", elsewhere, "index"])
    with pytest.raises(ValueError, match="not in the tree being opened"):
        host.open(guidef.window(stray))


def test_the_container_chrome_rides_as_flags_and_a_dialog_is_a_modal_layout():
    from clausters.gui import guidef

    group = guidef.panel(guidef.knob(), title="Filter", frame=True, collapsed=False)
    assert (group["title"], group["frame"], group["collapsed"]) == ("Filter", 1, 0)
    assert guidef.panel(guidef.knob(), guidef.knob(), flow="row", split=True)["split"] == 1
    assert guidef.scroll(bars=True)["bars"] == 1
    box = guidef.dialog(guidef.label("Discard the take?"), title="Confirm")
    assert (box["type"], box["modal"], box["title"]) == ("layout", 1, "Confirm")
    bar = guidef.toolbar(guidef.button(icon=guidef.ICON.play, flat=True),
                         guidef.separator(), guidef.separator(weight=1, line=False))
    assert (bar["flow"], bar["hug"], bar["pack"]) == ("row", 1, 1)
    assert bar["children"][0]["icon"] == "\u25b6" and bar["children"][0]["flat"] == 1
    assert bar["children"][2] == {"type": "separator", "line": 0, "weight": 1}
    assert guidef.progress(0.25)["value"] == 0.25
    assert "value" not in guidef.progress(), "no value is the indeterminate bar"


def test_rows_of_data_and_a_file_chooser_ride_as_plain_props():
    import json

    from clausters.gui import guidef

    t = guidef.table(rows=[["kick.wav", "12"], guidef.row("loops", depth=0, open=False),
                           guidef.row("a.wav", depth=1)],
                     columns=["name", {"title": "size", "w": 80.0}], multiple=True,
                     sort=[1, "down"])
    assert t["rows"][1] == {"cells": ["loops"], "depth": 0, "open": False}
    assert (t["multiple"], t["sort"]) == (1, [1, "down"])
    assert guidef.files("/takes", filter=["wav"]) == {
        "type": "files", "path": "/takes", "filter": ["wav"]}
    # The dialog binds its field to the chooser by the node, resolved at open.
    host = GuiHost("127.0.0.1", 57991)
    host._osc = _Recorder()
    host.open(guidef.window(guidef.file_dialog("/takes", title="Load")))
    box = json.loads(host._osc.sent[0][2])["children"][0]
    chooser, field = box["children"][0], box["children"][1]
    assert (box["modal"], box["title"]) == (1, "Load")
    assert chooser["bind"] == ["widget", field["id"], "value"]


def test_a_number_is_never_narrowed_on_the_way_to_the_host():
    """A frame of a take of hours arrives whole: a fraction rides as a double
    and an int past what an int32 counts as a long, where a float would have
    kept it to 256 frames and an int32 refused it."""
    from clausters.base import _osclib
    from clausters.gui.host import _prop_args

    day = 24 * 3600 * 48_000
    addr, args = _osclib.decode(_osclib.message("/gui_set", 5, *_prop_args(
        {"cursor": day + 1, "view_start": day + 0.5, "flag": True})))
    assert addr == "/gui_set"
    assert args == [5, "cursor", day + 1, "view_start", day + 0.5, "flag", 1]
    raw = _osclib.message("/x", day + 1, _osclib.Float64(0.5), 7)
    assert b",hdi" in raw
