"""The free-standing ``scope``: tap allocation, window-tree building per view
and resource release, without server or GUI host processes (fakes capture the
traffic)."""

import itertools

import pytest

from clausters.defs.bus import Bus
from clausters.defs.server import TapAllocator
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
    """A live server as scope() sees it: the tap registry, /tap and the shm."""

    def __init__(self, taps=8, shm="/dev/shm/fake"):
        self.taps = TapAllocator(size=taps)
        self.tapped = []
        self.shm = shm

    def tap(self, tap, bus):
        self.tapped.append((int(tap), int(bus)))


def _widget(tree: dict, kind: str) -> dict:
    assert tree["type"] == "window"
    (widget,) = tree["children"]
    assert widget["type"] == kind
    return widget


# ---- the tap registry ----

def test_tap_allocator_recycles_and_refuses_misuse():
    a = TapAllocator(size=4)
    first = a.alloc()
    pair = a.alloc(2)
    assert a.in_use == 3
    # The pair is adjacent (a phasescope reads taps t and t + 1).
    assert pair + 1 not in (first,)
    a.free(pair, 2)
    assert a.alloc(2) == pair, "a freed run is reusable"
    a.free(pair, 2)
    a.free(first)
    assert a.in_use == 0
    with pytest.raises(RuntimeError):
        a.free(first)  # double free is a client bug, raised loudly


def test_tap_allocator_exhaustion_raises():
    a = TapAllocator(size=1)
    a.alloc()
    with pytest.raises(RuntimeError):
        a.alloc()
    none = TapAllocator(size=0)  # a server without a tap region
    with pytest.raises(RuntimeError):
        none.alloc()


# ---- the verb, per view ----

def test_scope_signal_routes_a_tap_and_releases_it_on_close():
    server, host = FakeServer(), FakeHost()
    win = scope(2, trigger=0.1, server=server, host=host)
    assert isinstance(win, ScopeWindow)
    widget = _widget(host.opened[0], "scope")
    assert widget["tap"] == win.tap
    assert widget["trigger"] == 0.1
    assert server.tapped == [(win.tap, 2)]
    assert server.taps.in_use == 1
    # The handle retunes the display live and releases everything on close.
    win.set(window_ms=5.0)
    assert host.sets == [(win.widget_id, {"window_ms": 5.0})]
    win.close()
    assert server.tapped[-1] == (win.tap, -1), "the tap is stopped"
    assert server.taps.in_use == 0, "and returned to the registry"
    assert host.closed == [win.id]
    win.close()  # idempotent: no double free
    assert server.taps.in_use == 0


def test_scope_phase_takes_two_adjacent_taps_for_the_stereo_pair():
    server, host = FakeServer(), FakeHost()
    win = scope(0, view="phase", server=server, host=host)
    widget = _widget(host.opened[0], "phasescope")
    assert (widget["tap"], widget["tap2"]) == (win.tap, win.tap + 1)
    assert server.tapped == [(win.tap, 0), (win.tap + 1, 1)]
    assert server.taps.in_use == 2
    win.close()
    assert server.taps.in_use == 0
    assert (win.tap, -1) in server.tapped and (win.tap + 1, -1) in server.tapped


def test_scope_spectrum_carries_the_freq_scale():
    server, host = FakeServer(), FakeHost()
    win = scope(3, view="spectrum", freq_scale="mel", fft_size=1024,
                db_floor=-80.0, server=server, host=host)
    widget = _widget(host.opened[0], "spectrum")
    assert widget["freq_scale"] == "mel"
    assert widget["fft_size"] == 1024 and isinstance(widget["fft_size"], int)
    assert widget["db_floor"] == -80.0
    win.close()


def test_scope_accepts_a_bus_handle_and_labels_from_it():
    server, host = FakeServer(), FakeHost()
    win = scope(Bus(6, channels=2), view="phase", server=server, host=host)
    widget = _widget(host.opened[0], "phasescope")
    assert widget["label"] == "bus 6/7"
    assert server.tapped == [(win.tap, 6), (win.tap + 1, 7)]
    win.close()


def test_each_scope_takes_a_fresh_widget_id_and_tap():
    server, host = FakeServer(), FakeHost()
    a = scope(0, server=server, host=host)
    b = scope(1, server=server, host=host)
    assert a.widget_id != b.widget_id
    assert a.tap != b.tap, "two scopes never share a ring"
    a.close()
    b.close()


def test_scope_misuse_raises_and_leaks_nothing():
    server, host = FakeServer(taps=1), FakeHost()
    with pytest.raises(ValueError):
        scope(0, view="lissajous", server=server, host=host)
    # A phase view needs two taps; a 1-tap server exhausts before any /tap.
    with pytest.raises(RuntimeError):
        scope(0, view="phase", server=server, host=host)
    assert server.taps.in_use == 0 and server.tapped == []
    # A failed window open rolls the tap back.
    class Refusing(FakeHost):
        def open(self, tree, *blobs, id=None):
            raise OSError("host gone")
    with pytest.raises(OSError):
        scope(0, server=server, host=Refusing())
    assert server.taps.in_use == 0
    assert server.tapped == [(0, 0), (0, -1)], "routed, then stopped"


def test_scope_without_shm_needs_an_explicit_host():
    # The ambient host reads taps from the server's shared segment natively;
    # a handle without one must fail early instead of opening a dead scope.
    server = FakeServer(shm=None)
    with pytest.raises(RuntimeError, match="shared-memory"):
        scope(0, server=server)


# ---- the guidef builder ----

def test_guidef_spectrum_carries_freq_scale_with_log_freq_legacy():
    w = guidef.spectrum(7, 3, fft_size=512, freq_scale="bark",
                        averaging=0.8, peak_hold=True, label="spec")
    assert w["type"] == "spectrum" and w["tap"] == 3
    assert w["freq_scale"] == "bark"
    assert w["peak_hold"] == 1 and w["averaging"] == 0.8
    # The legacy boolean still rides (the host reads it as linear/log).
    legacy = guidef.spectrum(8, 0, log_freq=False)
    assert legacy["log_freq"] == 0 and "freq_scale" not in legacy
