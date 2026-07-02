"""C5 leftover: the instance-based UGen graph / SynthDef port.

Two halves: pure-structure asserts on the JSON spec a graph serializes to (no
server needed), and a render-parity golden — a client-built SynthDef equivalent
to the server's built-in ``default`` def renders **byte-identically**, proving
the graph builder emits exactly the spec the server compiles."""

import pytest

from clausters import render
from clausters.base import OscNrtInterface, TempoClock
from clausters.defs import (
    DoneAction,
    Env,
    SynthDef,
    Ugen,
    control,
    env_gen,
    local_in,
    local_out,
    out,
    sin_osc,
    white_noise,
)
from clausters.defs import Server
from clausters.seq import Pbind, Pseq

FREQS = [262.0, 330.0, 392.0, 523.0]


def _embed_or_skip():
    try:
        from clausters import _native
        _native.lib()
    except OSError as e:
        pytest.skip(f"clausters-ffi not built: {e}")


def _py_default(name="py_default") -> SynthDef:
    """The client-side equivalent of the server's built-in ``default``:
    ``SinOsc(freq) * amp`` to buses 0 and 1."""
    freq = control("freq", 440.0)
    amp = control("amp", 0.2)
    sig = sin_osc(freq) * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


# ---- structure (no server) ----


def test_spec_matches_builtin_default():
    spec = _py_default().spec()
    assert spec["name"] == "py_default"
    assert spec["controls"] == [
        {"name": "freq", "default": 440.0},
        {"name": "amp", "default": 0.2},
    ]
    # topological: SinOsc(control 0), Mul(ugen 0, control 1), two Outs.
    assert spec["ugens"] == [
        {"kind": "SinOsc", "inputs": [{"control": 0}]},
        {"kind": "Mul", "inputs": [{"ugen": 0}, {"control": 1}]},
        {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]},
        {"kind": "Out", "inputs": [{"const": 1.0}, {"ugen": 1}]},
    ]


def test_shared_subgraph_emitted_once():
    # `sig` feeds both outs; it must appear once (dedup by identity), so the
    # spec has exactly SinOsc + Mul + two Outs, not the chain duplicated.
    kinds = [u["kind"] for u in _py_default().spec()["ugens"]]
    assert kinds == ["SinOsc", "Mul", "Out", "Out"]


def test_operators_map_to_arithmetic_ugens():
    a = sin_osc(100.0)
    assert (a + 1.0).kind == "Add"
    assert (a - 1.0).kind == "Sub"
    assert (a * 2.0).kind == "Mul"
    assert (a / 2.0).kind == "Div"
    # reflected: constant on the left keeps operand order
    spec = SynthDef("x", out(0.0, 2.0 * a)).spec()
    mul = spec["ugens"][-2]
    assert mul == {"kind": "Mul", "inputs": [{"const": 2.0}, {"ugen": 0}]}


def test_unsupported_operators_raise():
    a = sin_osc(100.0)
    with pytest.raises(TypeError):
        a.sin()          # no math UGen
    with pytest.raises(TypeError):
        a % 2.0          # no Mod UGen
    with pytest.raises(TypeError):
        a.min(0.5)


def test_constant_only_inputs_and_no_controls():
    spec = SynthDef("noise", out(0.0, white_noise() * 0.1)).spec()
    assert spec["controls"] == []
    assert [u["kind"] for u in spec["ugens"]] == ["WhiteNoise", "Mul", "Out"]


def test_conflicting_control_defaults_raise():
    g = sin_osc(control("freq", 440.0)) + sin_osc(control("freq", 200.0))
    with pytest.raises(ValueError):
        SynthDef("bad", out(0.0, g)).spec()


def test_control_types_and_lag_serialize():
    # S2: a trigger, a scalar, and a lagged control emit their type/lag fields.
    gate = control("gate", 0.0, rate="tr")
    seed = control("seed", 1.0, rate="ir")
    freq = control("freq", 440.0, lag=0.1, lag_down=0.3)
    sig = sin_osc(freq) * gate + seed * 0.0
    spec = SynthDef("typed", out(0.0, sig)).spec()
    by_name = {c["name"]: c for c in spec["controls"]}
    assert by_name["gate"]["rate"] == "tr"
    assert by_name["seed"]["rate"] == "ir"
    assert by_name["freq"]["lag"] == 0.1 and by_name["freq"]["lag_down"] == 0.3
    # A plain control carries neither field.
    plain = SynthDef("p", out(0.0, sin_osc(control("f", 100.0)))).spec()
    assert "rate" not in plain["controls"][0]
    assert "lag" not in plain["controls"][0]


def test_bad_control_type_and_lag_down_raise():
    with pytest.raises(ValueError):
        control("c", 0.0, rate="xr")
    with pytest.raises(ValueError):
        control("c", 0.0, lag_down=0.1)


def test_new_ugens_and_rates_serialize():
    # Lag/VarLag, the ir scalars, and the demand pair with their rates.
    from clausters.defs import dseq, demand, rand, sample_rate, lag, var_lag, impulse

    lagged = SynthDef("l", out(0.0, lag(sin_osc(5.0), 0.2))).spec()
    assert lagged["ugens"][-2]["kind"] == "Lag"

    seq = dseq([1.0, 2.0, 3.0], repeats=2.0)
    drv = demand(impulse(4.0), 0.0, seq)
    spec = SynthDef("d", out(0.0, drv)).spec()
    by_kind = {u["kind"]: u for u in spec["ugens"]}
    assert by_kind["Dseq"]["rate"] == "dr"
    assert by_kind["Dseq"]["inputs"][0] == {"const": 2.0}  # repeats first
    assert "Demand" in by_kind

    r = SynthDef("r", out(0.0, rand(2.0, 5.0) * sample_rate() * 0.0 + sin_osc(1.0))).spec()
    kinds = {u["kind"]: u.get("rate") for u in r["ugens"]}
    assert kinds["Rand"] == "ir" and kinds["SampleRate"] == "ir"
    # at_rate sets an explicit rate.
    assert var_lag(sin_osc(1.0)).at_rate("kr").rate == "kr"


def test_localin_precedes_localout():
    # A feedback write fed back into the output graph: LocalIn must be emitted
    # before LocalOut (the server enforces the one-block-delay contract).
    fb = local_in(0)
    sig = sin_osc(200.0) + fb * 0.5
    sdef = SynthDef("fb", out(0.0, sig), local_out(0, sig))
    kinds = [u["kind"] for u in sdef.spec()["ugens"]]
    assert kinds.index("LocalIn") < kinds.index("LocalOut")


def test_outputs_must_be_ugens():
    with pytest.raises(TypeError):
        SynthDef("x", 1.0)
    with pytest.raises(ValueError):
        SynthDef("x")


def test_non_node_input_rejected():
    with pytest.raises(TypeError):
        SynthDef("x", out(0.0, Ugen("Out", ["nope"]))).spec()


# ---- envelopes (Env / env_gen) ----


def test_env_adsr_serializes_to_the_envgen_input_layout():
    # gate control, then the five fixed inputs, then the envelope array:
    # initLevel, numSegments, releaseNode, loopNode, and per segment
    # target/duration/shape/curve. ADSR's -4 curve maps to the custom shape 5.
    gate = control("gate", 1.0)
    node = env_gen(
        Env.adsr(attack=0.01, decay=0.3, sustain=0.5, release=1.0),
        gate=gate,
        done_action=DoneAction.FREE_SELF,
    )
    spec = SynthDef("adsr", out(0.0, node)).spec()
    env = spec["ugens"][0]
    assert env["kind"] == "EnvGen"
    assert env["inputs"] == [
        {"control": 0},   # gate
        {"const": 1.0},   # levelScale
        {"const": 0.0},   # levelBias
        {"const": 1.0},   # timeScale
        {"const": 2.0},   # doneAction (freeSelf)
        {"const": 0.0},   # initLevel
        {"const": 3.0},   # numSegments
        {"const": 2.0},   # releaseNode
        {"const": -1.0},  # loopNode
        {"const": 1.0}, {"const": 0.01}, {"const": 5.0}, {"const": -4.0},
        {"const": 0.5}, {"const": 0.3}, {"const": 5.0}, {"const": -4.0},
        {"const": 0.0}, {"const": 1.0}, {"const": 5.0}, {"const": -4.0},
    ]


def test_env_curve_names_and_numbers_resolve_to_shapes():
    # A named shape carries curve 0; a number selects the custom shape (5).
    e = Env([0.0, 1.0, 0.0], [0.1, 0.2], curve=["exp", -2.0])
    assert e.to_inputs() == [
        0.0, 2.0, -1.0, -1.0,
        1.0, 0.1, 2.0, 0.0,   # "exp" -> shape 2, curve 0
        0.0, 0.2, 5.0, -2.0,  # -2.0  -> shape 5, curve -2
    ]


def test_env_perc_has_no_release_node():
    # A percussive env plays straight through: releaseNode = -1.
    assert Env.perc().to_inputs()[2] == -1.0


def test_env_release_and_loop_nodes_serialize():
    # releaseNode and loopNode are the 3rd and 4th values of the array; None
    # becomes -1 (disabled).
    header = Env([0.0, 1.0, 0.0, 0.3], [0.1, 0.1, 0.1], release_node=2, loop_node=0).to_inputs()[:4]
    assert header == [0.0, 3.0, 2.0, 0.0]
    assert Env([0.0, 1.0], [0.1]).to_inputs()[2:4] == [-1.0, -1.0]


def test_done_action_constants_match_the_server():
    assert (DoneAction.NONE, DoneAction.PAUSE_SELF, DoneAction.FREE_SELF, DoneAction.FREE_GROUP) == (
        0,
        1,
        2,
        14,
    )


def test_env_rejects_mismatched_levels_and_times():
    with pytest.raises(ValueError):
        Env([0.0, 1.0], [0.1, 0.2])          # 2 levels need 1 time
    with pytest.raises(ValueError):
        Env([0.0, 1.0, 0.0], [0.1, 0.2], curve=["lin"])  # 1 curve, 2 segments


def test_env_rejects_unknown_shape_name():
    with pytest.raises(ValueError):
        Env([0.0, 1.0], [0.1], curve="bogus").to_inputs()


# ---- render parity (needs the embed render) ----


def test_custom_synthdef_renders_like_builtin_default():
    _embed_or_skip()

    # The built-in "default" path.
    s0 = Server(interface=OscNrtInterface())
    c0 = TempoClock(tempo=1.0)
    Pbind(instrument="default", freq=Pseq(FREQS), dur=0.5, amp=0.2).play(c0, s0)
    c0.render()

    # The client-defined equivalent: add it to the score, then the same Pbind.
    s1 = Server(interface=OscNrtInterface())
    s1.add_synthdef(_py_default())          # /d_recv at time 0 in the score
    c1 = TempoClock(tempo=1.0)
    Pbind(instrument="py_default", freq=Pseq(FREQS), dur=0.5, amp=0.2).play(c1, s1)
    c1.render()

    try:
        builtin, b_frames = render(s0.interface.score.bytes())
        custom, c_frames = render(s1.interface.score.bytes())
    except (OSError, RuntimeError, AttributeError) as e:
        pytest.skip(f"embed library not built/usable: {e}")

    assert b_frames == c_frames
    assert list(custom) == list(builtin)
    assert max(abs(s) for s in custom) > 0.0


if __name__ == "__main__":
    import traceback

    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            try:
                fn()
                print(f"ok   {name}")
            except BaseException as e:  # noqa: BLE001
                kind = type(e).__name__
                skip = kind in ("Skipped", "OutcomeException")
                print(f"{'skip' if skip else 'FAIL'} {name}: {e}")
                if not skip:
                    traceback.print_exc()
