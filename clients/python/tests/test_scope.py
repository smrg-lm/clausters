"""The free-standing ``scope``: window-tree building per view and resource
release, without server or GUI host processes (fakes capture the traffic).

Nothing here mentions a recording ring: a script names a bus, and the GUI host
is what asks the server to record it."""

import itertools

import pytest

from clausters.defs.bus import Bus
from clausters.gui import guidef
from clausters.scope import ScopeWindow, scope


class FakeHost:
    """Captures what scope() would send to a GuiHost."""

    def __init__(self):
        self.opened = []
        self.sets = []
        self.closed = []
        self._ids = itertools.count(1000)

    def alloc_id(self):
        return next(self._ids)

    def open(self, tree, *blobs, id=None):
        self.opened.append(tree)
        return 1000 + len(self.opened)

    def set(self, id, **props):
        self.sets.append((id, props))

    def close(self, id):
        self.closed.append(id)


class FakeServer:
    """A live server as scope() sees it: just the shm path. Recording is the
    host's business now, so scope() sends the server nothing at all."""

    def __init__(self, shm="/dev/shm/fake"):
        self.shm = shm


def _widget(tree: dict, kind: str) -> dict:
    assert tree["type"] == "window"
    (widget,) = tree["children"]
    assert widget["type"] == kind
    return widget


# ---- the verb, per view ----

def test_scope_signal_names_its_bus_and_closes_cleanly():
    server, host = FakeServer(), FakeHost()
    win = scope(2, trigger=0.1, server=server, host=host)
    assert isinstance(win, ScopeWindow)
    widget = _widget(host.opened[0], "scope")
    assert widget["bus"] == 2 and win.bus == 2
    assert widget["rate"] == "audio"
    assert widget["trigger"] == 0.1
    # The handle retunes the display live and closes the window.
    win.set(window_ms=5.0)
    assert host.sets == [(win.widget_id, {"window_ms": 5.0})]
    win.close()
    assert host.closed == [win.id]
    win.close()  # idempotent
    assert host.closed == [win.id]


def test_scope_phase_is_the_bus_pair():
    server, host = FakeServer(), FakeHost()
    win = scope(0, view="phase", server=server, host=host)
    widget = _widget(host.opened[0], "phasescope")
    assert widget["bus"] == 0, "the right channel is bus + 1"
    win.close()


def test_scope_spectrum_carries_the_freq_scale():
    server, host = FakeServer(), FakeHost()
    win = scope(3, view="spectrum", freq_scale="mel", fft_size=1024,
                db_floor=-80.0, server=server, host=host)
    widget = _widget(host.opened[0], "spectrum")
    assert widget["bus"] == 3
    assert widget["freq_scale"] == "mel"
    assert widget["fft_size"] == 1024 and isinstance(widget["fft_size"], int)
    assert widget["db_floor"] == -80.0
    win.close()


def test_scope_signal_monitors_consecutive_channels():
    server, host = FakeServer(), FakeHost()
    win = scope(2, channels=3, server=server, host=host)
    widget = _widget(host.opened[0], "scope")
    assert (widget["bus"], widget["channels"]) == (2, 3)
    assert widget["label"] == "bus 2-4"
    win.close()


def test_scope_channels_default_from_a_bus_handle():
    server, host = FakeServer(), FakeHost()
    win = scope(Bus(4, channels=2), server=server, host=host)
    widget = _widget(host.opened[0], "scope")
    assert (widget["bus"], widget["channels"]) == (4, 2), "a Bus monitors all its channels"
    win.close()
    # An explicit channels= wins over the handle's count.
    win = scope(Bus(4, channels=2), channels=1, server=server, host=host)
    assert _widget(host.opened[1], "scope")["channels"] == 1
    win.close()


def test_scope_phase_is_the_fixed_two_channel_case():
    server, host = FakeServer(), FakeHost()
    with pytest.raises(ValueError, match="exactly 2"):
        scope(0, view="phase", channels=4, server=server, host=host)


def test_scope_spectrum_channels_and_strips_ride_the_wire():
    server, host = FakeServer(), FakeHost()
    win = scope(0, view="spectrum", channels=2, ruler_y=False,
                server=server, host=host)
    widget = _widget(host.opened[0], "spectrum")
    assert widget["channels"] == 2
    assert widget["ruler_y"] == "off"
    win.close()


def test_scope_accepts_a_bus_handle_and_labels_from_it():
    server, host = FakeServer(), FakeHost()
    win = scope(Bus(6, channels=2), view="phase", server=server, host=host)
    widget = _widget(host.opened[0], "phasescope")
    assert widget["label"] == "bus 6/7"
    assert widget["bus"] == 6
    win.close()


def test_each_scope_takes_a_fresh_widget_id():
    server, host = FakeServer(), FakeHost()
    a = scope(0, server=server, host=host)
    b = scope(1, server=server, host=host)
    assert a.widget_id != b.widget_id
    a.close()
    b.close()


def test_scope_misuse_raises():
    server, host = FakeServer(), FakeHost()
    with pytest.raises(ValueError):
        scope(0, view="lissajous", server=server, host=host)


def test_scope_without_shm_needs_an_explicit_host():
    # The ambient host reads the buses from the server's shared segment;
    # a handle without one must fail early instead of opening a dead scope.
    server = FakeServer(shm=None)
    with pytest.raises(RuntimeError, match="shared-memory"):
        scope(0, server=server)


# ---- the guidef builder ----

def test_guidef_spectrum_carries_freq_scale_with_log_freq_legacy():
    w = guidef.spectrum(3, id=7, fft_size=512, freq_scale="bark",
                        averaging=0.8, peak_hold=True, label="spec")
    assert w["type"] == "spectrum" and w["bus"] == 3
    assert w["freq_scale"] == "bark"
    assert w["peak_hold"] == 1 and w["averaging"] == 0.8
    # The legacy boolean still rides (the host reads it as linear/log).
    legacy = guidef.spectrum(0, id=8, log_freq=False)
    assert legacy["log_freq"] == 0 and "freq_scale" not in legacy


def test_a_registered_host_answers_the_shm_question_itself():
    """The shm demand is about the native host `scope` would boot. A host
    registered from outside (the notebook's, in the page) reads the taps its
    own way, and this module cannot reason about a front it cannot boot."""
    from clausters.gui import set_ambient_host

    host = FakeHost()
    server = FakeServer(shm=None)
    set_ambient_host(host)
    try:
        win = scope(bus=0, server=server)
    finally:
        set_ambient_host(None)
    assert _widget(host.opened[0], "scope")["bus"] == 0
