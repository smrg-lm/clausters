"""A widget is built from the control it drives, and the range is the widget's.

What a def knows about a control is its **name** (what `/node_set` addresses)
and its **default**; a widget built from one reads those instead of being handed
the same strings twice. What a def does *not* know is how a knob should be
drawn: a control is a signal in a graph and a GraphDef port is a name the server
takes any float for, so `min`/`max` are spelled on the widget.

The one control that arrives with a range is a **Faust** parameter, because
`hslider(label, init, min, max, step)` cannot be written without one and the
compiled DSP reports it back. That is Faust's syntax showing through, not a range
this client declares — which is why `ControlInfo` has carried `min`/`max`/`step`
all along and only that family fills them.
"""

import pytest

from clausters.defs import FaustDef, GraphDef, SynthDef, control, out, sine
from clausters.defs.signals import checkbox, hslider
from clausters.gui import knob, number, slider, toggle


def _voice():
    freq = control("freq", 220.0)
    amp = control("amp", 0.2)
    return SynthDef("voice", out(0.0, sine(freq=freq) * amp)), freq, amp


# ---- the three families answer with one shape ----

def test_a_synthdefs_controls_are_a_name_and_a_default():
    sd, _, _ = _voice()
    assert [c.name for c in sd.controls] == ["freq", "amp"]
    assert sd["amp"].default == 0.2
    assert sd["freq"].range is None, "a graph control declares no range"


def test_a_faustdefs_controls_bring_their_range_from_the_dsp():
    """Faust declares init/min/max/step where the control is written, which is
    why this family was the only one filling the slot before."""
    fd = FaustDef.from_signals("f", [hslider("cutoff", 800.0, 20.0, 20_000.0, 1.0)
                                     * checkbox("go")])
    assert fd["cutoff"].range == (20.0, 20_000.0) and fd["cutoff"].step == 1.0
    assert fd["cutoff"].default == 800.0
    assert fd["go"].range == (0.0, 1.0), "a checkbox is a 0/1 control"


def test_a_graphdefs_ports_are_a_name_a_default_and_what_they_drive():
    g = GraphDef("g")
    m = g.add("voice")
    g.port("mix", m.amp, default=0.5)
    assert g["mix"].default == 0.5 and g["mix"].range is None
    assert g["mix"].targets == ((0, "amp", 1.0, 0.0),)


def test_a_missing_control_names_the_ones_there_are():
    sd, _, _ = _voice()
    with pytest.raises(KeyError, match="freq, amp"):
        sd["cutoff"]


def test_a_control_takes_no_range_at_all():
    """It is a signal, and a signal does not say how it is drawn. `min`/`max` on
    a signal are the binary operators, which is the other reason: an attribute
    of either name shadows the operator."""
    with pytest.raises(TypeError):
        control("x", 0.0, **{"min": 1.0, "max": 2.0})   # pyright: ignore[reportCallIssue]
    c = control("freq", 220.0)
    assert c.min(2.0).kind == "BinaryOpUGen"
    assert c.max(2.0).kind == "BinaryOpUGen"


def test_the_range_rides_no_wire():
    """The server takes any float for any control, so a range is a statement
    about the surface. Sending it would be inventing a field the schema has no
    room for."""
    sd, _, _ = _voice()
    for c in sd.spec()["controls"]:
        assert set(c) <= {"name", "default", "rate", "lag", "lag_down"}


# ---- a widget built from a control ----

def test_a_knob_is_built_from_the_control_it_drives():
    _, freq, _ = _voice()
    k = knob(freq, min=110.0, max=880.0)
    assert k["name"] == "freq" and k["label"] == "freq"
    assert (k["min"], k["max"], k["value"]) == (110.0, 880.0, 220.0)


def test_a_widget_takes_a_control_off_any_def():
    sd, _, _ = _voice()
    assert slider(sd["amp"], min=0.0, max=1.0)["value"] == 0.2
    assert number(sd["freq"], min=110.0, max=880.0)["name"] == "freq"


def test_a_whole_surface_is_derived_from_the_def():
    sd, _, _ = _voice()
    knobs = [knob(c, min=0.0, max=1000.0) for c in sd.controls]
    assert [k["name"] for k in knobs] == ["freq", "amp"]


def test_a_keyword_wins_over_what_the_control_says():
    """The control says what it is; the call says how to draw it."""
    _, _, amp = _voice()
    assert slider(amp, label="level", min=0.0, max=1.0)["label"] == "level"
    fd = FaustDef.from_signals("f", [hslider("cut", 800.0, 20.0, 20_000.0, 1.0)])
    assert knob(fd["cut"], max=2.0)["max"] == 2.0, "the call wins over Faust's own"


def test_a_control_with_no_range_says_so_instead_of_being_guessed_at():
    """Which is every control but a Faust parameter: the widget spells it."""
    with pytest.raises(ValueError, match="no range to be drawn over"):
        knob(control("x", 0.0))
    assert knob(control("x", 0.0), min=0.0, max=1.0)["name"] == "x"


def test_a_toggle_needs_no_range():
    assert toggle(control("bypass", 0.0))["value"] == 0
    assert toggle(control("bypass", 1.0))["value"] == 1


def test_something_that_is_not_a_control_is_refused():
    with pytest.raises(TypeError, match="not a def's control"):
        knob(7)


# ---- the binding is made against the control ----

def _bound_host(port):
    from clausters.gui import GuiHost

    host = GuiHost("127.0.0.1", port)
    host._osc = _Recorder()
    return host


class _Recorder:
    def __init__(self):
        self.sent = []

    def start(self):
        return self

    def send_msg(self, target, *args):
        self.sent.append(args)


def test_the_whole_surface_binds_in_one_verb():
    """A widget built from a control already knows what it drives, so the
    binding stops being a string typed twice."""
    from clausters.gui import view

    sd, freq, amp = _voice()
    host = _bound_host(57988)
    w = view(knob(freq, min=110.0, max=880.0),
             slider(amp, min=0.0, max=1.0)).open(host=host)

    host._osc.sent.clear()
    w.bind(1001)
    assert sorted(host._osc.sent) == sorted([
        ("/gui_bind", w["freq"].id, "server", "/node_set", 1001, "freq"),
        ("/gui_bind", w["amp"].id, "server", "/node_set", 1001, "amp"),
    ])


def test_a_widget_named_apart_from_its_control_still_binds_the_control():
    """The name is the handle's index and the control is what the server is
    told; they are usually the same string and need not be."""
    from clausters.gui import view

    _, freq, _ = _voice()
    host = _bound_host(57987)
    w = view(knob(freq, name="pitch", min=110.0, max=880.0)).open(host=host)

    host._osc.sent.clear()
    w.bind(1002)
    assert host._osc.sent == [("/gui_bind", w["pitch"].id, "server",
                               "/node_set", 1002, "freq")]
    assert w.controls == {"pitch": "freq"}


def test_unbind_gives_every_control_widget_back_to_the_script():
    from clausters.gui import view

    _, freq, amp = _voice()
    host = _bound_host(57986)
    w = view(knob(freq, min=110.0, max=880.0),
             slider(amp, min=0.0, max=1.0)).open(host=host)

    host._osc.sent.clear()
    w.bind(1003).unbind()
    assert [m for m in host._osc.sent if len(m) == 2] == [
        ("/gui_bind", w["freq"].id), ("/gui_bind", w["amp"].id)]


def test_a_window_with_no_control_widget_says_so():
    from clausters.gui import label, view

    host = _bound_host(57985)
    w = view(label("nothing to bind")).open(host=host)
    with pytest.raises(ValueError, match="nothing to bind"):
        w.bind(1004)


def test_a_redraw_keeps_the_window_bindable():
    """The handle is refreshed in place on a redefine, and its controls with it
    — a rebound window must not be wiring ids that recycled."""
    from clausters.gui import view

    _, freq, _ = _voice()
    host = _bound_host(57984)
    w = view(knob(freq, min=110.0, max=880.0)).open(host=host)
    host.define(int(w), view(knob(freq, min=110.0, max=880.0)))

    host._osc.sent.clear()
    w.bind(1005)
    assert host._osc.sent == [("/gui_bind", w["freq"].id, "server",
                               "/node_set", 1005, "freq")]


def test_bind_takes_a_node_or_a_bare_id():
    """A `Synth` is what a script holds; an id is what a responder reports."""
    from clausters.gui import view

    _, freq, _ = _voice()
    host = _bound_host(57983)
    w = view(knob(freq, min=110.0, max=880.0)).open(host=host)

    class _FakeNode:
        id = 2001

    host._osc.sent.clear()
    w.bind(_FakeNode())
    assert host._osc.sent[0][4] == 2001
