"""The programmatic directed patcher (`clausters.defs.GraphPatch`) and the shared
cord->bus pass it compiles through (`clausters._native.compile_patch`).

The GUI is only a view of this: everything here is buildable and sendable in code.
"""

import pytest

from clausters.defs import (
    GraphPatch,
    SynthDef,
    control,
    in_,
    in_ctl,
    out,
    out_ctl,
    sine,
    synthdef_ports,
)


def _pass_or_skip():
    """The cord->bus pass needs the ABI-matched native core built."""
    try:
        from clausters import _native

        _native.lib()
    except OSError as e:
        pytest.skip(f"clausters-ffi not built / ABI mismatch: {e}")


def chain() -> GraphPatch:
    """tone -> dac (a terminal sink): the seed the example starts from."""
    p = GraphPatch()
    tone = p.add("tone", outlets=["out"])
    dac = p.add("dac", inlets=["in"])   # terminal: reaches hardware itself
    p.connect(tone, "out", dac, "in")
    return p


def test_compile_names_one_private_bus_per_net():
    _pass_or_skip()
    c = chain().compile()
    assert [b["name"] for b in c["buses"]] == ["b0"]   # the one link; no OUT bus
    assert [m["def"] for m in c["members"]] == ["tone", "dac"]
    tone, dac = c["members"]
    assert tone["controls"] == [{"control": "out", "bus": "b0"}]
    assert dac["controls"] == [{"control": "in", "bus": "b0"}]


def test_fan_in_sums_onto_one_bus():
    _pass_or_skip()
    p = GraphPatch()
    a = p.add("tone", outlets=["out"])
    b = p.add("tone", outlets=["out"])
    dac = p.add("dac", inlets=["in"])
    p.connect(a, "out", dac, "in")
    p.connect(b, "out", dac, "in")   # both into dac.in -> they sum on one bus
    c = p.compile()
    assert len(c["buses"]) == 1
    # Both sources write the same bus.
    src_buses = {c["members"][a]["controls"][0]["bus"], c["members"][b]["controls"][0]["bus"]}
    assert src_buses == {"b0"}


def test_to_graphdef_builds_a_sendable_spec():
    _pass_or_skip()
    spec = chain().to_graphdef("chain").spec()
    assert spec["name"] == "chain"
    assert [b["name"] for b in spec["buses"]] == ["b0"]
    members = {m["def"]: m.get("controls", {}) for m in spec["members"]}
    assert members["tone"] == {"out": "b0"}
    assert members["dac"] == {"in": "b0"}


def test_a_control_rate_cord_makes_a_control_bus():
    _pass_or_skip()
    p = GraphPatch()
    lfo = p.add("lfo", outlets=[("out", "control")])
    amp = p.add("amp", inlets=[("gain", "control")])
    p.connect(lfo, "out", amp, "gain")
    assert p.compile()["buses"] == [{"name": "b0", "rate": "control"}]


def test_a_reversed_cord_is_reported():
    _pass_or_skip()
    p = GraphPatch()
    tone = p.add("tone", outlets=["out"])
    dac = p.add("dac", inlets=["in"])
    # Wire it the wrong way round by flat index (inlet -> outlet).
    p.cords.append({"from_box": dac, "from_port": 0, "to_box": tone, "to_port": 0})
    with pytest.raises(ValueError, match="inlet"):
        p.compile()


def test_to_widget_splits_ports_and_indexes_cords():
    # No native needed: to_widget is pure structure.
    w = chain().to_widget(geometry={1: (120.0, 40.0)})
    # tone: no inlets, one outlet; dac (terminal): one inlet, no outlet.
    assert w["boxes"][0] == {"def": "tone", "inlets": [], "outlets": ["out"]}
    assert w["boxes"][1] == {"def": "dac", "inlets": ["in"], "outlets": [], "x": 120.0, "y": 40.0}
    # One cord as [from_box, outlet_idx, to_box, inlet_idx].
    assert w["cords"] == [0, 0, 1, 0]


def test_synthdef_ports_are_derived_from_the_graph():
    # A control feeding an In is an inlet; one feeding an Out is an outlet; the
    # UGen family fixes the rate. `freq`/`amp` feed neither -> not ports.
    trem = SynthDef("trem", out(control("out"),
                               in_(control("in")) * sine(control("rate", 4.0))))
    inlets, outlets = synthdef_ports(trem)
    assert inlets == ["in"]
    assert outlets == ["out"]

    # A control-rate reader/writer yields control ports (name, "control").
    ctl = SynthDef("ctl", out_ctl(control("kout"), in_ctl(control("kin")) * 2.0))
    kin, kout = synthdef_ports(ctl)
    assert kin == [("kin", "control")]
    assert kout == [("kout", "control")]

    # A terminal def: writes hardware bus 0 (a constant, not a control) -> no
    # outlet, just the inlet a cord reaches.
    dac = SynthDef("dac", out(0, in_(control("in")) * control("amp", 0.4)))
    assert synthdef_ports(dac) == (["in"], [])


def test_add_derives_ports_from_a_passed_synthdef():
    tone = SynthDef("tone", out(control("out"), sine(control("freq", 220.0))))
    dac = SynthDef("dac", out(0, in_(control("in"))))
    p = GraphPatch()
    t = p.add(tone)                 # ports read off the def, no manual list
    d = p.add(dac)
    p.connect(t, "out", d, "in")
    w = p.to_widget()
    assert w["boxes"][0] == {"def": "tone", "inlets": [], "outlets": ["out"]}
    assert w["boxes"][1] == {"def": "dac", "inlets": ["in"], "outlets": []}
    # Explicit ports still override a def's derived ones (the escape hatch).
    p2 = GraphPatch()
    p2.add(tone, outlets=["custom"])
    assert p2.boxes[0]["ports"] == [{"name": "custom", "dir": "out", "rate": "audio"}]


def test_from_graphdef_round_trips_a_typed_chain():
    # A patch -> its GraphDef -> the patch decoded back: same boxes, same cords.
    # The decode types each box's ports from the SynthDef (passed via `defs`), so
    # the drawn connections survive the round trip through the bus wiring.
    _pass_or_skip()
    tone = SynthDef("tone", out(control("out"), sine(control("freq", 220.0))))
    trem = SynthDef("trem", out(control("out"),
                                in_(control("in")) * sine(control("rate", 4.0))))
    dac = SynthDef("dac", out(0, in_(control("in"))))
    p = GraphPatch()
    t, tr, d = p.add(tone), p.add(trem), p.add(dac)
    p.connect(t, "out", tr, "in")
    p.connect(tr, "out", d, "in")
    gdef = p.to_graphdef("chain")
    back = GraphPatch.from_graphdef(gdef, {"tone": tone, "trem": trem, "dac": dac})
    assert back.to_widget() == p.to_widget()


def test_from_graphdef_without_defs_draws_port_less_and_grows_no_cords():
    # No `defs` -> a member's ports cannot be typed, so it draws port-less and the
    # wiring cannot become cords (direction is never guessed).
    _pass_or_skip()
    tone = SynthDef("tone", out(control("out"), sine(control("freq", 220.0))))
    dac = SynthDef("dac", out(0, in_(control("in"))))
    p = GraphPatch()
    p.connect(p.add(tone), "out", p.add(dac), "in")
    back = GraphPatch.from_graphdef(p.to_graphdef("chain"))
    w = back.to_widget()
    assert w["boxes"] == [
        {"def": "tone", "inlets": [], "outlets": []},
        {"def": "dac", "inlets": [], "outlets": []},
    ]
    assert w["cords"] == []


class _FakeHost:
    """Captures the tree GraphDef.plot_def would open on a real GuiHost."""

    def __init__(self):
        self.opened = []
        self._ids = iter(range(1000, 2000))

    def alloc_id(self):
        return next(self._ids)

    def open(self, tree, *blobs, id=None):
        self.opened.append(tree)
        return 42

    def set(self, i, **props):
        pass

    def close(self, i):
        pass


def _find(node, kind):
    if node.get("type") == kind:
        return node
    for child in node.get("children", []):
        hit = _find(child, kind)
        if hit is not None:
            return hit
    return None


def test_graphdef_plot_def_opens_the_structure_as_a_patch_view():
    # plot_def decodes the GraphDef and opens a `patch` view (its structure), one
    # window per call — distinct from clausters.plot(def), which renders its sound.
    _pass_or_skip()
    tone = SynthDef("tone", out(control("out"), sine(control("freq", 220.0))))
    dac = SynthDef("dac", out(0, in_(control("in"))))
    p = GraphPatch()
    p.connect(p.add(tone), "out", p.add(dac), "in")
    gdef = p.to_graphdef("chain")

    host = _FakeHost()
    win = gdef.plot_def({"tone": tone, "dac": dac}, host=host)
    assert win.id == 42
    tree = host.opened[0]
    assert tree["type"] == "window"
    view = _find(tree, "patch")
    assert view is not None and view["label"] == "chain"
    assert [b["def"] for b in view["boxes"]] == ["tone", "dac"]
    assert view["cords"] == [0, 0, 1, 0]   # tone.out -> dac.in, typed from the defs
    # It rode no audio server and no bulk file — pure structure.
    assert _find(tree, "scroll") is not None   # the patch sits in a pan/zoom workspace


def test_graphdef_plot_def_without_defs_is_port_less():
    _pass_or_skip()
    tone = SynthDef("tone", out(control("out"), sine(control("freq", 220.0))))
    dac = SynthDef("dac", out(0, in_(control("in"))))
    p = GraphPatch()
    p.connect(p.add(tone), "out", p.add(dac), "in")
    host = _FakeHost()
    p.to_graphdef("chain").plot_def(host=host)   # no defs -> no typed ports
    view = _find(host.opened[0], "patch")
    assert [b["outlets"] for b in view["boxes"]] == [[], []]
    assert view["cords"] == []


def test_connect_is_idempotent_and_disconnect_removes():
    p = chain()
    n = len(p.cords)
    p.connect(0, "out", 1, "in")     # already there
    assert len(p.cords) == n
    p.disconnect(0, "out", 1, "in")
    assert len(p.cords) == n - 1
