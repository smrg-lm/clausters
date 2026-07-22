"""The programmatic directed patcher (`clausters.defs.GraphPatch`) and the shared
cord->bus pass it compiles through (`clausters._native.compile_patch`).

The GUI is only a view of this: everything here is buildable and sendable in code.
"""

import pytest

from clausters.defs import GraphPatch


def _pass_or_skip():
    """The cord->bus pass needs the ABI-matched native core built."""
    try:
        from clausters import _native

        _native.lib()
    except OSError as e:
        pytest.skip(f"clausters-ffi not built / ABI mismatch: {e}")


def chain() -> GraphPatch:
    """tone -> dac -> speakers: the seed the example starts from."""
    p = GraphPatch()
    tone = p.add("tone", outlets=["out"])
    dac = p.add("dac", inlets=["in"], outlets=["out"])
    out = p.sink()
    p.connect(tone, "out", dac, "in")
    p.connect(dac, "out", out, "in")
    return p


def test_compile_names_one_bus_per_net_and_reaches_out():
    _pass_or_skip()
    c = chain().compile()
    # One private bus for tone->dac; the dac->OUT net is the hardware, not a bus.
    assert [b["name"] for b in c["buses"]] == ["b0"]
    # Two members (the hardware OUT box is not one).
    assert [m["def"] for m in c["members"]] == ["tone", "dac"]
    tone, dac = c["members"]
    assert tone["controls"] == [{"control": "out", "bus": "b0"}]
    assert {w["control"]: w["bus"] for w in dac["controls"]} == {"in": "b0", "out": "OUT"}


def test_fan_in_sums_onto_one_bus():
    _pass_or_skip()
    p = GraphPatch()
    a = p.add("tone", outlets=["out"])
    b = p.add("tone", outlets=["out"])
    dac = p.add("dac", inlets=["in"], outlets=["out"])
    out = p.sink()
    p.connect(a, "out", dac, "in")
    p.connect(b, "out", dac, "in")   # both into dac.in -> they sum on one bus
    p.connect(dac, "out", out, "in")
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
    assert members["dac"] == {"in": "b0", "out": "OUT"}


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
    # tone: no inlets, one outlet; dac: one inlet, one outlet; OUT: one inlet.
    assert w["boxes"][0] == {"def": "tone", "inlets": [], "outlets": ["out"]}
    assert w["boxes"][1] == {"def": "dac", "inlets": ["in"], "outlets": ["out"], "x": 120.0, "y": 40.0}
    assert w["boxes"][2] == {"def": "OUT", "inlets": ["in"], "outlets": []}
    # Cords as [from_box, outlet_idx, to_box, inlet_idx] quadruples.
    assert w["cords"] == [0, 0, 1, 0, 1, 0, 2, 0]


def test_connect_is_idempotent_and_disconnect_removes():
    p = chain()
    n = len(p.cords)
    p.connect(0, "out", 1, "in")     # already there
    assert len(p.cords) == n
    p.disconnect(0, "out", 1, "in")
    assert len(p.cords) == n - 1
