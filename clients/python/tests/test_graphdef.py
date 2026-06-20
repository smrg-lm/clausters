"""M18 client: the GraphDef builder and its end-to-end render.

Two halves: pure-structure asserts on the ``GraphDefSpec`` JSON the builder
produces (no server), and an NRT render proving a GraphDef instantiated and
driven through its named surface actually sounds."""

import pytest

from clausters import render
from clausters.base import OscNrtInterface, Routine, TempoClock
from clausters.defs import GraphDef, Server, SynthDef, control, in_, out, sin_osc

SR = 48000.0


# ---- structure (no server) ----


def test_builder_serializes_members_buses_surface_and_defaults():
    g = GraphDef("chain")
    mix = g.bus("mix")
    src = g.add("gsrc", out=mix, level=1.0)
    g.add("gsink", {"in": mix, "out": "OUT"})
    g.port("gain", src["level"], default=0.5)
    g.port("bright", src["level"].scaled(7800, 200))
    spec = g.spec()

    assert spec["name"] == "chain"
    assert spec["buses"] == [{"name": "mix", "rate": "audio", "channels": 1}]
    # A bus reference serializes to the bus name; "OUT" stays a string.
    assert spec["members"][0]["controls"] == {"out": "mix", "level": 1.0}
    assert spec["members"][1]["controls"] == {"in": "mix", "out": "OUT"}
    # Surface ports: one target plain, one scaled.
    assert spec["surface"]["gain"] == [{"member": 0, "control": "level"}]
    assert spec["surface"]["bright"] == [
        {"member": 0, "control": "level", "mul": 7800.0, "add": 200.0}
    ]
    assert spec["defaults"] == {"gain": 0.5}


def test_member_attr_access_matches_indexing():
    g = GraphDef("x")
    m = g.add("d")
    assert m["freq"]._as_dict() == m.freq._as_dict()


def test_port_needs_a_target_and_graph_needs_a_member():
    g = GraphDef("x")
    m = g.add("d")
    with pytest.raises(ValueError):
        g.port("p")  # no targets
    with pytest.raises(ValueError):
        GraphDef("empty").spec()  # no members
    assert g.spec()["members"] == [{"def": "d"}]
    del m


# ---- end-to-end render ----


def _members():
    freq, out_bus = control("freq", 440.0), control("out", 0.0)
    tone = SynthDef("tone", out(out_bus, sin_osc(freq) * 0.15))
    in_bus, gain = control("in", 0.0), control("gain", 0.4)
    amp = SynthDef("gain", out(0.0, in_(in_bus) * gain), out(1.0, in_(in_bus) * gain))
    return tone, amp


def _duo():
    g = GraphDef("duo")
    mix = g.bus("mix")
    t1 = g.add("tone", out=mix)
    t2 = g.add("tone", out=mix)
    amp = g.add("gain", **{"in": mix})
    g.port("freq", t1["freq"], t2["freq"].scaled(1.5), default=220.0)
    g.port("gain", amp["gain"], default=0.4)
    return g


def test_graphdef_instantiates_and_sounds():
    try:
        from clausters import _native

        _native.lib()
    except OSError as e:
        pytest.skip(f"embed library not built: {e}")

    server = Server(interface=OscNrtInterface())
    clock = TempoClock(tempo=1.0)

    def play(srv):
        for sdef in _members():
            srv.add_synthdef(sdef)
        srv.add_graphdef(_duo())
        inst = srv.graph("duo", {"freq": 220.0, "gain": 0.4})
        for f in (220.0, 277.0):
            srv.send_bundle(("/n_set", inst.id, "freq", f))
            yield 0.5
        srv.send_bundle(("/n_free", inst.id))

    clock.play(Routine(lambda: play(server)))
    clock.render()

    try:
        samples, frames = render(server.interface.score.bytes())
    except (OSError, RuntimeError, AttributeError) as e:
        pytest.skip(f"embed library not usable: {e}")

    assert frames > 0
    assert max(abs(s) for s in samples) > 0.05, "the GraphDef rendered silent"
