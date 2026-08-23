"""A control declares the range it is meant to be driven over, and a widget
reads it.

The range is the one thing about a control only the person writing the graph
knows, and a widget that copies those numbers by hand is a second declaration
nothing checks. The slot already existed — `ControlInfo` carries `min`/`max`/
`step` and a FaustDef fills it from its own `hslider` — and this is the other
three quarters of it.
"""

import pytest

from clausters.defs import FaustDef, GraphDef, SynthDef, control, out, sine
from clausters.defs.signals import checkbox, hslider
from clausters.gui import knob, number, slider, toggle


def _voice():
    freq = control("freq", 220.0, min=110.0, max=880.0)
    amp = control("amp", 0.2, min=0.0, max=1.0)
    return SynthDef("voice", out(0.0, sine(freq=freq) * amp)), freq, amp


# ---- the three families answer with one shape ----

def test_a_synthdefs_controls_carry_the_range_they_declared():
    sd, _, _ = _voice()
    assert [c.name for c in sd.controls] == ["freq", "amp"]
    assert sd["freq"].range == (110.0, 880.0)
    assert sd["amp"].default == 0.2


def test_a_faustdefs_controls_bring_their_range_from_the_dsp():
    """Faust declares init/min/max/step where the control is written, which is
    why this family was the only one filling the slot before."""
    fd = FaustDef.from_signals("f", [hslider("cutoff", 800.0, 20.0, 20_000.0, 1.0)
                                     * checkbox("go")])
    assert fd["cutoff"].range == (20.0, 20_000.0) and fd["cutoff"].step == 1.0
    assert fd["cutoff"].default == 800.0
    assert fd["go"].range == (0.0, 1.0), "a checkbox is a 0/1 control"


def test_a_graphdefs_ports_take_a_range_like_a_control():
    g = GraphDef("g")
    m = g.add("voice")
    g.port("mix", m.amp, default=0.5, min=0.0, max=1.0)
    assert g["mix"].range == (0.0, 1.0) and g["mix"].default == 0.5
    assert g["mix"].targets == ((0, "amp", 1.0, 0.0),)


def test_a_missing_control_names_the_ones_there_are():
    sd, _, _ = _voice()
    with pytest.raises(KeyError, match="freq, amp"):
        sd["cutoff"]


def test_the_range_is_declared_or_not_declared():
    with pytest.raises(ValueError, match="min \\*and\\* max"):
        control("x", 0.0, min=1.0)


def test_the_range_is_part_of_a_controls_identity():
    """Two uses of one name with different ranges are two different controls,
    which is the conflict `spec` already refuses on the default and the type."""
    a = control("freq", 220.0, min=110.0, max=880.0)
    b = control("freq", 220.0, min=20.0, max=20_000.0)
    with pytest.raises(ValueError, match="conflicting definitions"):
        SynthDef("bad", out(0.0, sine(freq=a) + sine(freq=b))).spec()


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
    k = knob(freq)
    assert k["name"] == "freq" and k["label"] == "freq"
    assert (k["min"], k["max"], k["value"]) == (110.0, 880.0, 220.0)


def test_a_widget_takes_a_control_off_any_def():
    sd, _, _ = _voice()
    assert slider(sd["amp"])["max"] == 1.0
    assert number(sd["freq"])["min"] == 110.0


def test_a_whole_surface_is_derived_from_the_def():
    sd, _, _ = _voice()
    knobs = [knob(c) for c in sd.controls]
    assert [k["name"] for k in knobs] == ["freq", "amp"]


def test_a_keyword_wins_over_what_the_control_says():
    """The control says what it is; the call says how to draw it."""
    _, _, amp = _voice()
    assert slider(amp, label="level")["label"] == "level"
    assert knob(amp, max=2.0)["max"] == 2.0


def test_a_control_with_no_range_says_so_instead_of_being_guessed_at():
    with pytest.raises(ValueError, match="declares no range"):
        knob(control("x", 0.0))
    # ...and spelling one here is the other way out.
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
    w = view(knob(freq), slider(amp)).open(host=host)

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
    w = view(knob(freq, name="pitch")).open(host=host)

    host._osc.sent.clear()
    w.bind(1002)
    assert host._osc.sent == [("/gui_bind", w["pitch"].id, "server",
                               "/node_set", 1002, "freq")]
    assert w.controls == {"pitch": "freq"}


def test_unbind_gives_every_control_widget_back_to_the_script():
    from clausters.gui import view

    _, freq, amp = _voice()
    host = _bound_host(57986)
    w = view(knob(freq), slider(amp)).open(host=host)

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
    w = view(knob(freq)).open(host=host)
    host.define(int(w), view(knob(freq)))

    host._osc.sent.clear()
    w.bind(1005)
    assert host._osc.sent == [("/gui_bind", w["freq"].id, "server",
                               "/node_set", 1005, "freq")]


def test_bind_takes_a_node_or_a_bare_id():
    """A `Synth` is what a script holds; an id is what a responder reports."""
    from clausters.gui import view

    _, freq, _ = _voice()
    host = _bound_host(57983)
    w = view(knob(freq)).open(host=host)

    class _FakeNode:
        id = 2001

    host._osc.sent.clear()
    w.bind(_FakeNode())
    assert host._osc.sent[0][4] == 2001
