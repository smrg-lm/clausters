"""The bundle writer (`clausters.bundle.Bundle`) and the core pass it validates
through.

The writer's job is that a written bundle *mounts* — so most of these assert
what it refuses, not just what it emits: a hole baked into a def payload, a
default that does not type-check, a symbol declared twice. The mount itself is
tested in Rust (`clausters_core::bundle`) and in the browser
(`clients/web/tests/components.html`); what is checked here is the file the
three legs read.
"""

import json
import os

import pytest

from clausters.bundle import Bundle
from clausters.defs import DoneAction, Env, SynthDef, control, env_gen, out, out_ctl, sine
from clausters.gui import knob, meter, window


def voice(name="voice") -> SynthDef:
    """A gated voice that publishes its envelope on a bus it is *given* — the
    authoring rule the format rests on: a bus reaches a def as a control."""
    freq = control("freq", 220.0)
    env_bus = control("env_bus", 0.0)
    env = env_gen(Env.perc(), done_action=DoneAction.FREE_SELF)
    sig = sine(freq) * env
    return SynthDef(name, out(0.0, sig), out_ctl(env_bus, env))


def baked(name="baked") -> SynthDef:
    """The wrong form: the bus number compiled into the def, so two instances
    would write the same bus."""
    env = env_gen(Env.perc(), done_action=DoneAction.FREE_SELF)
    return SynthDef(name, out(0.0, sine(220.0) * env), out_ctl(0.0, env))


def a_bundle() -> Bundle:
    b = Bundle("fm-voice")
    freq = b.param("freq", float, default=220.0, min=60.0, max=700.0)
    lfo = b.bus("lfo")
    node = b.node("voice")
    b.synthdef(voice())
    # The root is widget 1 (the record's id), so the children start at 2.
    b.gui(window(
        knob(id=2, label="freq", value=freq, min=60.0, max=700.0,
             bind=["/n_set", node, "freq"]),
        meter(lfo, id=3, label="env"),
        title="FM voice", layout="col",
    ))
    b.boot(["/s_new", "fm-voice.voice", node, 0, 0, "freq", freq, "env_bus", lfo])
    b.preset("bright", freq=660.0)
    return b


def test_the_manifest_declares_what_a_mount_allocates():
    m = a_bundle().manifest()
    assert m["name"] == "fm-voice"
    assert m["synthdefs"] == ["fm-voice.voice"], "def names carry the bundle's prefix"
    assert m["symbols"]["nodes"] == ["voice"]
    assert m["symbols"]["buses"] == [{"name": "lfo", "rate": "control", "channels": 1}]
    assert m["params"]["freq"] == {"type": "float", "default": 220.0, "min": 60.0, "max": 700.0}
    assert m["presets"] == ["bright"]
    # The id block is the highest widget id, root included — a width, not a
    # count: two instances are offset by it and must not overlap.
    assert m["widgets"] == 3


def test_symbols_read_as_placeholders_where_an_index_goes():
    b = Bundle("x")
    assert b.bus("lfo") == "@lfo"
    assert b.node("graph") == "@graph"
    assert b.buffer("hit", "audio/hit.wav") == "@hit"
    assert b.param("freq") == "$freq"


def test_the_record_carries_the_holes_and_the_boot_list():
    record = a_bundle().record()
    assert record["id"] == 1
    tree = record["gui"]
    assert tree["children"][0]["value"] == "$freq"
    assert tree["children"][0]["bind"] == ["/n_set", "@voice", "freq"]
    assert tree["children"][1]["bus"] == "@lfo"
    assert tree["boot"][0][2] == "@voice"


def test_a_bundle_that_would_not_mount_is_not_written(tmp_path):
    """The pre-flight is the whole point of `write` calling `validate` first."""
    b = a_bundle()
    b.gui(window(meter("@nope", id=2, label="typo")))
    with pytest.raises(ValueError, match="unknown symbol"):
        b.write(str(tmp_path / "out"))
    assert not os.path.exists(tmp_path / "out" / "bundle.json")


def test_a_hole_baked_into_a_def_is_refused(tmp_path):
    """A def payload must hold no placeholder: that is what lets two instances
    share the one def that was sent."""
    b = Bundle("bad")
    bus = b.bus("lfo")
    sdef = voice()
    # Simulate the mistake the authoring rule prevents: the placeholder baked
    # into the payload rather than passed as a control.
    spec = json.loads(sdef.dump_def())
    spec["controls"][1]["default"] = bus
    sdef.dump_def = lambda: json.dumps(spec)  # type: ignore[method-assign]
    b.synthdef(sdef)
    b.gui(window(meter(bus, id=2)))
    with pytest.raises(ValueError, match="@lfo"):
        b.write(str(tmp_path / "out"))


def test_a_default_that_does_not_type_check_is_refused():
    b = Bundle("bad")
    title = b.param("title", str, default="ok")
    b.gui(window(knob(id=2, label=title)))
    b._params["title"]["default"] = 3.5  # a float where a string was declared
    with pytest.raises(ValueError, match="wants a string"):
        b.validate()


def test_one_name_in_two_namespaces_is_refused():
    b = Bundle("x")
    b.bus("lfo")
    with pytest.raises(ValueError, match="already declared"):
        b.node("lfo")


def test_a_preset_may_only_set_declared_parameters():
    b = a_bundle()
    with pytest.raises(ValueError, match="undeclared"):
        b.preset("wrong", nope=1.0)


def test_write_emits_the_directory_and_its_module(tmp_path):
    out = tmp_path / "fm-voice"
    a_bundle().write(str(out), runtime="/dist/runtime.js")

    manifest = json.loads((out / "bundle.json").read_text())
    assert manifest["gui"] == "fm-voice"
    record = json.loads((out / "defs" / "guidefs" / "fm-voice.json").read_text())
    assert record["id"] == 1
    spec = json.loads((out / "defs" / "synthdefs" / "fm-voice.voice.json").read_text())
    assert spec["name"] == "fm-voice.voice"
    assert json.loads((out / "presets" / "bright.json").read_text()) == {"freq": 660.0}

    module = (out / "index.js").read_text()
    assert 'import { defineComponent } from "/dist/runtime.js";' in module
    assert 'defineComponent("fm-voice", new URL(".", import.meta.url));' in module


def test_the_baked_form_is_what_the_rule_prevents(tmp_path):
    """`baked()` compiles bus 0 into the def, so both instances would write it.
    The writer cannot see that (0 is a number, not a hole) — the rule is an
    authoring one, and this test records what it is about."""
    b = Bundle("legacy-voice")
    b.synthdef(baked())
    b.gui(window(meter(0.0, id=2, label="env")))
    b.write(str(tmp_path / "legacy"))  # writes: nothing here is a hole
    spec = json.loads((tmp_path / "legacy" / "defs" / "synthdefs" / "legacy-voice.baked.json").read_text())
    out_ctls = [u for u in spec["ugens"] if u["kind"] == "OutCtl"]
    assert out_ctls, "the def writes a control bus"
    # ... and the bus is a *constant* in the payload, which is exactly why a
    # second instance of this bundle would collide with the first on bus 0.
    assert out_ctls[0]["inputs"][0] == {"const": 0.0}
    # Written as `out_ctl(control("env_bus"), env)` it is a control instead,
    # and the mount passes each instance its own bus.
    fixed = json.loads(voice().dump_def())
    assert [u for u in fixed["ugens"] if u["kind"] == "OutCtl"][0]["inputs"][0] != {"const": 0.0}
