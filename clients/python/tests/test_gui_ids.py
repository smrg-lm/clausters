"""Automatic widget-id allocation and the handle layer.

The client owns the GUI's id namespace the way it owns node ids: a recycling
allocator (`GuiIdAllocator`), ids filled in place for id-less widgets, freed
subtrees returned to the pool, and a name -> handle map so a script never writes
or matches an integer. Pure-unit — a stub OSC interface captures the wire; no
live host.
"""

from clausters.gui import GuiHost, WidgetHandle, guidef
from clausters.gui.guidef import button, knob, panel, window
from clausters.gui.ids import GuiIdAllocator


class _Recorder:
    """A stub OSC interface capturing what GuiHost would send."""

    def __init__(self):
        self.sent = []

    def send_msg(self, target, *args):
        self.sent.append(args)


def _host() -> GuiHost:
    host = GuiHost("127.0.0.1", 57990)
    host._osc = _Recorder()
    return host


# ---- the allocator ----

def test_allocator_recycles_within_a_bounded_window():
    # A small window makes the recycling observable: ids stay in [base, base+cap)
    # and a freed id is reused once the high-water mark reaches the top.
    a = GuiIdAllocator(base=1000, capacity=3)
    ids = [a.alloc() for _ in range(3)]
    assert ids == [1000, 1001, 1002]
    assert a.in_use == 3
    a.free(1001)
    assert a.in_use == 2
    assert a.alloc() == 1001  # the window is full, so the freed id comes back
    assert a.in_use == 3


def test_allocator_exhaustion_raises():
    import pytest

    a = GuiIdAllocator(base=1000, capacity=2)
    a.alloc()
    a.alloc()
    with pytest.raises(RuntimeError):
        a.alloc()


def test_allocator_ignores_ids_it_never_handed_out():
    a = GuiIdAllocator()
    a.alloc()
    a.free(5)          # a hand-picked id below the base: not ours
    a.free(999_999_999)  # above the window: not ours
    assert a.in_use == 1


# ---- ids filled in place, freed subtree recycled ----

def test_close_returns_the_whole_subtree_to_the_pool():
    host = _host()
    inner = button()
    pane = panel(inner)
    win = host.open(window(pane))
    ids = {int(win), pane["id"], inner["id"]}
    assert len(ids) == 3
    assert host._alloc.in_use == 3
    host.close(win)
    assert host._alloc.in_use == 0
    assert int(win) not in host._open


def test_redefine_recycles_the_old_subtree_instead_of_climbing():
    host = _host()
    win = host.open(window(panel(button(), button())))
    in_use = host._alloc.in_use
    # Re-define the same window with an equally sized tree: the old subtree's
    # ids return to the pool first, so the count does not climb.
    host.define(int(win), window(panel(button(), button())))
    assert host._alloc.in_use == in_use


# ---- the name -> handle map ----

def test_names_resolve_to_handles_and_never_ride_the_wire():
    host = _host()
    k = knob(name="cutoff", label="freq")
    win = host.open(window(k))
    # Subscript and attribute both resolve to the assigned id.
    assert isinstance(win["cutoff"], WidgetHandle)
    assert win["cutoff"].id == k["id"]
    assert win.cutoff.id == k["id"]
    assert "cutoff" in win and win.names() == ["cutoff"]
    # The client-only name is stripped from the /gui_def JSON.
    addr, _id, js = host._osc.sent[0][0], host._osc.sent[0][1], host._osc.sent[0][2]
    assert addr == "/gui_def"
    assert '"name"' not in js and "cutoff" not in js


def test_missing_name_is_a_keyerror_listing_the_names():
    host = _host()
    win = host.open(window(knob(name="cutoff")))
    try:
        win["nope"]
    except KeyError as e:
        assert "cutoff" in str(e)
    else:
        raise AssertionError("expected KeyError for an unknown name")


# ---- the handle delegates to the host ----

def test_handle_set_and_bind_delegate_to_the_host():
    host = _host()
    win = host.open(window(knob(name="k")))
    host._osc.sent.clear()
    win["k"].set(value=3.0).bind("/n_set", 1001, "freq")
    kinds = [msg[0] for msg in host._osc.sent]
    assert kinds == ["/gui_set", "/gui_bind"]
    assert host._osc.sent[0][1] == win["k"].id  # addressed by the resolved id


def test_handle_free_recycles_and_leaves_no_dangling_edge():
    host = _host()
    win = host.open(window(knob(name="k"), button(name="b")))
    kid = win["k"].id
    win["k"].free()
    assert not host._alloc._registry.contains(kid) or host._alloc.in_use == 2
    # The other widget still resolves.
    assert win["b"].id != kid


# ---- event dispatch to the handle callbacks ----

def test_dispatch_routes_event_and_close_to_the_callbacks():
    host = _host()
    win = host.open(window(button(name="go")))
    seen = []
    win["go"].on_event(lambda *payload: seen.append(payload))
    closed = []
    win.on_closed(lambda: closed.append(True))

    assert host.dispatch("/gui_event", [win["go"].id, 1]) is True
    assert seen == [(1,)]
    # A view's tagged edit-back forwards the tag and the flat values.
    host.dispatch("/gui_event", [win["go"].id, "points", 0.0, 1.0])
    assert seen[-1] == ("points", 0.0, 1.0)

    assert host.dispatch("/gui_closed", [int(win)]) is True
    assert closed == [True]
    assert int(win) not in host._open


def test_dispatch_ignores_an_unregistered_id():
    host = _host()
    host.open(window(button(name="go")))
    assert host.dispatch("/gui_event", [999_999, 1]) is False


def test_clearing_an_event_handler_with_none():
    host = _host()
    win = host.open(window(button(name="go")))
    seen = []
    h = win["go"]
    h.on_event(lambda *p: seen.append(p))
    h.on_event(None)
    assert host.dispatch("/gui_event", [h.id, 1]) is False
    assert seen == []


def test_every_widget_builder_takes_a_name_without_an_id():
    # A named widget must never require an id positionally -- the whole point of
    # naming it. This guards every builder at once (a missing default surfaces as
    # a TypeError only at call time, not at import).
    from clausters.gui import guidef as g

    # (builder, extra required kwargs) -- clip needs a dur; the rest take a name
    # and nothing else.
    cases = [
        (g.label, {}), (g.knob, {}), (g.slider, {}), (g.number, {}),
        (g.button, {}), (g.toggle, {}), (g.text, {}), (g.menu, {}),
        (g.waveform, {}), (g.spectrogram, {}), (g.pianoroll, {}), (g.meter, {}),
        (g.scope, {}), (g.phasescope, {}), (g.spectrum, {}), (g.nodetree, {}),
        (g.bpf, {}), (g.plot, {}), (g.canvas, {}), (g.score, {}), (g.piano, {}),
        (g.patch, {}), (g.panel, {}), (g.scroll, {}), (g.track, {}),
        (g.clip, {"dur": 1.0}),
    ]
    host = _host()
    children = [b(name=f"w{i}", **kw) for i, (b, kw) in enumerate(cases)]
    win = host.open(window(*children))
    for i in range(len(cases)):
        assert win[f"w{i}"].id >= 1000, cases[i][0].__name__
    assert '"name"' not in host._osc.sent[0][2]


def test_to_json_strips_names_even_without_the_host_walk():
    # A direct serialization (no GuiHost) must not leak the client-only name.
    js = guidef.to_json(window(knob(name="cutoff"), panel(button(name="b"))))
    assert "name" not in js and "cutoff" not in js
