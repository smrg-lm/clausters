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


def test_a_cell_that_stops_listening_stops_receiving():
    """A comm outlives the view on it.

    Re-running a cell clears its output and disposes the front end's view,
    while the model stays alive -- so the kernel goes on sending into a widget
    whose ``render`` is gone, and the front end drops every one of those
    without a trace. What it looked like is the second run of a notebook, from
    the top, in silence.
    """
    from clausters_jupyter.carrier import SERVER_CHANNEL

    host, bridge, _ = _host()
    audio = bridge.carrier(SERVER_CHANNEL)
    win = host.open(_win("a"))
    widget = bridge.widget_for(int(win))
    bridge.widget_ready(widget)
    audio.send_msg(None, "/def_send", "synth", b"...")
    assert widget.addrs() == ["/def_send"]

    bridge.widget_gone(widget)               # the cell was re-run
    audio.send_msg(None, "/synth_new", "voice", 1000, 1, 0)
    assert widget.addrs() == ["/def_send"], "sent into a view that is gone"


def test_running_the_whole_notebook_again_gets_a_live_audio_cell():
    """The sequence a notebook is put through every time: run it to the end,
    close what it opened, then run it again.

    The audio cell is memoized, so once one had been made the bridge believed
    something was on screen for the rest of the kernel's life -- and the second
    run's def, synth and everything after went to the cell of the first, which
    the re-run had disposed. Nothing said so at either end.
    """
    from clausters_jupyter.carrier import SERVER_CHANNEL

    host, bridge, made = _host()
    audio = bridge.carrier(SERVER_CHANNEL)

    # ---- the first run: a window, some audio, and then the teardown the
    # examples end with.
    win = host.open(_win("a"))
    first = bridge.widget_for(int(win))
    bridge.widget_ready(first)
    audio.send_msg(None, "/def_send", "synth", b"...")
    assert first.addrs() == ["/def_send"]
    host.free(int(win))                      # win.close()
    bridge.widget_gone(first)                # its cell was re-run
    assert not bridge.showing(), "nothing is on screen after the notebook ran"

    # ---- the second run, before any cell displays anything: the audio has to
    # find a cell of its own again.
    audio.send_msg(None, "/def_send", "synth", b"...")
    cell = bridge.audio_widget()
    bridge.widget_ready(cell)
    assert cell.addrs() == ["/def_send"], "the second run reached no engine"
    assert cell is not first


def test_the_audio_cell_is_forgotten_when_its_view_goes():
    """The audio cell is memoized, and that memo is what went stale.

    `showing` answers with it, and `_send_audio` prefers it -- so a disposed
    audio cell made the bridge believe both that something was on screen and
    that it had somewhere to send. Forgetting it is what lets the next thing
    with audio to send put a live cell up.
    """
    _, bridge, _ = _host()
    cell = bridge.audio_widget()
    bridge.widget_ready(cell)
    assert bridge.showing()

    bridge.widget_gone(cell)
    assert not bridge.showing(), "a disposed cell is not a cell on screen"
    assert bridge.audio_widget() is not cell


def test_nothing_is_displayed_outside_a_running_cell(monkeypatch):
    """`display` writes into whatever message is the kernel's parent, and on
    the kernel's own thread that is not always a cell: a comm message is
    handled there too, between executions, and its parent is the widget's comm
    -- whose own parent is some cell that finished long ago. An output would
    land in it, which is this package editing a notebook the reader is not
    running. Nothing may do that.
    """
    from clausters_jupyter import bridge as bridge_module

    class FakeKernel:
        def __init__(self, msg_type):
            self.msg_type = msg_type

        def get_parent(self, _channel=None):
            return {"header": {"msg_type": self.msg_type}}

    class FakeShell:
        def __init__(self, msg_type):
            self.kernel = FakeKernel(msg_type)

    def shell_of(msg_type):
        monkeypatch.setattr(bridge_module, "threading", threading_stub, raising=False)
        return FakeShell(msg_type)

    import threading as threading_stub          # the real one; main thread here

    import IPython
    monkeypatch.setattr(IPython, "get_ipython", lambda: shell_of("comm_msg"))
    assert bridge_module._cell_display() is None, "a comm message is not a cell"

    monkeypatch.setattr(IPython, "get_ipython", lambda: shell_of("execute_request"))
    assert bridge_module._cell_display() is not None, "a running cell may draw"

    monkeypatch.setattr(IPython, "get_ipython", lambda: None)
    assert bridge_module._cell_display() is None, "no kernel, no cell"
