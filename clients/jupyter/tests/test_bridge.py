"""The routing that gives each window its own cell.

One `GuiHost` has one carrier, but a notebook draws each window in its own
output area. These are the two halves of that: a packet reaches the widget
showing its window, and a window nobody is showing is remembered rather than
lost.
"""

import pytest

from clausters.base import _osclib
from clausters.gui import GuiHost, guidef

from clausters_jupyter.bridge import Bridge
from clausters_jupyter.carrier import GUI_CHANNEL


class FakeWidget:
    def __init__(self):
        self.got: list = []
        self.bridge = None
        self.height = 420          # the trait's default

    def send_packet(self, channel, payload):
        self.got.append(payload)

    def addrs(self):
        return [_osclib.decode(p)[0] for p in self.got]


def _host():
    made: list = []

    def factory():
        w = FakeWidget()
        made.append(w)
        return w

    bridge = Bridge(factory)
    return GuiHost(interface=bridge.carrier()), bridge, made


def _win(name):
    return guidef.window(guidef.slider(name=name), title=name)


def test_a_window_nobody_shows_sends_nowhere_and_is_remembered():
    host, bridge, made = _host()
    win = host.open(_win("a"))
    assert made == [], "no widget until the window is displayed"
    # Displaying it is what makes the widget, and the replay carries the tree.
    widget = bridge.widget_for(int(win))
    assert len(made) == 1
    replay = bridge.replay_for(widget)
    assert [_osclib.decode(p)[0] for p in replay] == ["/gui_def"]


def test_a_packet_reaches_only_the_widget_showing_its_window():
    host, bridge, _ = _host()
    first, second = host.open(_win("a")), host.open(_win("b"))
    wa = bridge.widget_for(int(first))
    wb = bridge.widget_for(int(second))
    wa.got.clear()
    wb.got.clear()
    first["a"].set(value=0.25)
    assert wa.addrs() == ["/gui_set"]
    assert wb.got == [], "the other cell must not see it"


def test_a_replay_carries_one_window_not_the_session():
    host, bridge, _ = _host()
    first, second = host.open(_win("a")), host.open(_win("b"))
    first["a"].set(value=0.5)
    wa = bridge.widget_for(int(first))
    ids = [_osclib.decode(p)[1][0] for p in bridge.replay_for(wa)]
    assert set(ids) <= {int(first), int(first["a"])}
    assert int(second) not in ids


def test_events_from_any_widget_reach_the_one_carrier():
    """The page drains one host into whatever comm is listening, so an event
    may come up through a widget other than the one that owns it."""
    host, bridge, _ = _host()
    win = host.open(_win("a"))
    other = host.open(_win("b"))
    bridge.widget_for(int(win))
    bridge.widget_for(int(other))
    seen = []
    win["a"].on_event(lambda *a: seen.append(a))
    wid = int(win["a"])
    bridge.inbound(GUI_CHANNEL, _osclib.message("/gui_event", wid, 0.75))
    assert host.pump() == 1
    assert seen and seen[0][0] == pytest.approx(0.75)


def test_a_displayed_window_is_the_same_widget_twice():
    host, bridge, made = _host()
    win = host.open(_win("a"))
    assert bridge.widget_for(int(win)) is bridge.widget_for(int(win))
    assert len(made) == 1


def test_a_cell_is_as_tall_as_the_window_asked_to_be():
    """A canvas has no intrinsic size, so without this a two-lane scope and a
    one-line plot come out identical."""
    host, bridge, _ = _host()
    tall = host.open(guidef.window(guidef.slider(name="a"), title="t", h=700))
    plain = host.open(guidef.window(guidef.slider(name="b"), title="u"))
    assert bridge.widget_for(int(tall)).height == 700
    assert bridge.widget_for(int(plain)).height == 420, "the default stands"


def test_closing_a_window_reaches_the_cell_showing_it():
    """`/gui_free` is the one packet whose route the journal forgets while
    recording it, so it used to be sent nowhere and the canvas stayed."""
    host, bridge, _ = _host()
    win = host.open(_win("a"))
    widget = bridge.widget_for(int(win))
    widget.got.clear()
    host.close(win)
    assert widget.addrs() == ["/gui_free"]


def test_audio_sent_before_any_cell_is_showing_is_not_lost():
    """A def and a synth sent from the first cell have no widget to ride yet.
    They are not replayable state -- replaying a /synth_new later would start
    a second voice -- but they were never delivered once, which is a queue,
    not a journal."""
    from clausters_jupyter.carrier import SERVER_CHANNEL

    _, bridge, _ = _host()
    audio = bridge.carrier(SERVER_CHANNEL)
    audio.send_msg(None, "/def_send", "synth", b"...")
    audio.send_msg(None, "/synth_new", "voice", 1000, 1, 0)

    widget = bridge.audio_widget()          # the first cell to show anything
    bridge.widget_ready(widget)             # ...once its module has mounted
    assert [_osclib.decode(p)[0] for p in widget.got] == ["/def_send", "/synth_new"]


def test_audio_waits_for_a_front_end_that_is_listening():
    """A widget exists as soon as a window is displayed, and its module in the
    page mounts later -- it is the ``ready`` message that says a handler is
    registered. Draining the queue at creation posted the opening def and
    /synth_new into that gap, where nothing was listening yet: the notebook
    drew, and never made a sound."""
    from clausters_jupyter.carrier import SERVER_CHANNEL

    host, bridge, _ = _host()
    audio = bridge.carrier(SERVER_CHANNEL)
    win = host.open(_win("a"))
    widget = bridge.widget_for(int(win))     # displayed, not yet mounted

    audio.send_msg(None, "/def_send", "synth", b"...")
    assert widget.got == [], "sent into a front end that is not listening"

    bridge.widget_ready(widget)
    assert [_osclib.decode(p)[0] for p in widget.got] == ["/def_send"]
    # And from here it goes straight out, without queueing again.
    audio.send_msg(None, "/synth_new", "voice", 1000, 1, 0)
    assert widget.addrs() == ["/def_send", "/synth_new"]


def test_each_notebook_gets_its_own_session_id():
    """JupyterLab is one page: two notebooks share a globalThis while
    allocating ids from the same base, so the front end keys its host by this
    rather than by the page."""
    a, b = Bridge(FakeWidget), Bridge(FakeWidget)
    assert a.session and b.session and a.session != b.session


def test_a_freed_window_stops_being_a_cell():
    """Its canvas is removed in the page, so keeping the entry here would
    route a later window that reused the id at a canvas nobody can see."""
    host, bridge, _ = _host()
    win = host.open(_win("a"))
    bridge.widget_for(int(win))
    host.close(win)
    assert int(win) not in bridge._widgets
    # A widget inside a window is a different thing: the window stands.
    other = host.open(_win("b"))
    bridge.widget_for(int(other))
    other["b"].free()
    assert int(other) in bridge._widgets
