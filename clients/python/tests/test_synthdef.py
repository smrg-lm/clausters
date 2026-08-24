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
    chans,
    control,
    env_gen,
    local_in,
    local_out,
    out,
    sine,
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
    """A minimal client-side def: ``Sine(freq) * amp`` to buses 0 and 1.
    Used by the structural tests (spec shape, subgraph dedup)."""
    freq = control("freq", 440.0)
    amp = control("amp", 0.2)
    sig = sine(freq) * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


def _py_default_env(name="py_default_env") -> SynthDef:
    """A faithful client-side replica of the server's built-in ``default``:
    ``Sine(freq) * EnvGen(gate) * amp`` with a gated ASR (equal-power sine
    ramps, 0.01 s attack, 0.3 s release, ``FREE_SELF``) — the same graph the
    server registers, so it must render sample-identically."""
    freq = control("freq", 440.0)
    amp = control("amp", 0.2)
    gate = control("gate", 1.0)
    env = env_gen(
        Env.asr(attack=0.01, sustain=1.0, release=0.3, curve="sin"),
        gate=gate,
        done_action=DoneAction.FREE_SELF,
    )
    sig = sine(freq) * env * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


# ---- structure (no server) ----


def test_spec_matches_builtin_default():
    spec = _py_default().spec()
    assert spec["name"] == "py_default"
    assert spec["controls"] == [
        {"name": "freq", "default": 440.0},
        {"name": "amp", "default": 0.2},
    ]
    # topological: Sine(control 0), Mul(ugen 0, control 1), two Outs.
    assert spec["ugens"] == [
        {"kind": "Sine", "inputs": [{"control": 0}]},
        {"kind": "Mul", "inputs": [{"ugen": 0}, {"control": 1}]},
        {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]},
        {"kind": "Out", "inputs": [{"const": 1.0}, {"ugen": 1}]},
    ]


def test_shared_subgraph_emitted_once():
    # `sig` feeds both outs; it must appear once (dedup by identity), so the
    # spec has exactly Sine + Mul + two Outs, not the chain duplicated.
    kinds = [u["kind"] for u in _py_default().spec()["ugens"]]
    assert kinds == ["Sine", "Mul", "Out", "Out"]


def test_operators_map_to_arithmetic_ugens():
    a = sine(100.0)
    assert (a + 1.0).kind == "Add"
    assert (a - 1.0).kind == "Sub"
    assert (a * 2.0).kind == "Mul"
    assert (a / 2.0).kind == "Div"
    # reflected: constant on the left keeps operand order
    spec = SynthDef("x", out(0.0, 2.0 * a)).spec()
    mul = spec["ugens"][-2]
    assert mul == {"kind": "Mul", "inputs": [{"const": 2.0}, {"ugen": 0}]}


def test_math_operators_compose_op_ugens():
    # S3: math beyond + - * / now composes generic BinaryOpUGen/UnaryOpUGen
    # carrying the operator by NAME (mirrors the value side bit-for-bit; no
    # numeric index crosses the wire).
    a = sine(100.0)
    unary = SynthDef("u", out(0.0, a.sin())).spec()
    assert unary["ugens"][-2] == {
        "kind": "UnaryOpUGen", "op": "sin", "inputs": [{"ugen": 0}]
    }
    midi = SynthDef("m", out(0.0, sine(control("n", 60.0).midicps()))).spec()
    assert any(u.get("op") == "midicps" and u["kind"] == "UnaryOpUGen"
               for u in midi["ugens"])
    binary = SynthDef("b", out(0.0, a % 2.0)).spec()
    assert binary["ugens"][-2] == {
        "kind": "BinaryOpUGen", "op": "mod",
        "inputs": [{"ugen": 0}, {"const": 2.0}],
    }
    cmp = SynthDef("c", out(0.0, a.min(0.5))).spec()
    assert cmp["ugens"][-2]["op"] == "min"
    # A reflected op keeps operand order.
    refl = SynthDef("r", out(0.0, 2.0 - a)).spec()
    assert refl["ugens"][-2]["kind"] == "Sub"  # + - * / keep their alias kinds


def test_range_maps_compose_one_map_ugen():
    # The warp family over a signal: one `RangeMapUGen` carrying the map by
    # name, which is the same function the value side computes with. The clip
    # rides as static config and is written only when it is not the default.
    lfo = sine(0.2)
    spec = SynthDef("m", out(0.0, lfo.linexp(-1.0, 1.0, 200.0, 8000.0))).spec()
    assert spec["ugens"][-2] == {
        "kind": "RangeMapUGen", "op": "linexp",
        "inputs": [{"ugen": 0}, {"const": -1.0}, {"const": 1.0},
                   {"const": 200.0}, {"const": 8000.0}],
    }
    # The bent pair carries sclang's -4 default rather than the wire's inert 0.
    bent = SynthDef("b", out(0.0, sine(1.0).lincurve(-1.0, 1.0, 0.0, 1.0))).spec()
    assert bent["ugens"][-2]["inputs"][-1] == {"const": -4.0}
    assert "clip" not in bent["ugens"][-2]
    clipped = SynthDef(
        "c", out(0.0, sine(1.0).linlin(-1.0, 1.0, 0.0, 1.0, clip="none"))).spec()
    assert clipped["ugens"][-2]["clip"] == "none"
    # A bound may be a signal: a modulated range is a legal graph.
    moved = SynthDef(
        "v", out(0.0, sine(1.0).linlin(-1.0, 1.0, 0.0, sine(0.1)))).spec()
    assert moved["ugens"][-2]["inputs"][-1] == {"ugen": 1}
    # A channel list maps every channel.
    both = SynthDef(
        "l", out(0.0, chans(sine(1.0), sine(2.0)).linlin(-1.0, 1.0, 0.0, 1.0))
    ).spec()
    assert [u["kind"] for u in both["ugens"]].count("RangeMapUGen") == 2
    # The two bipolar maps read a polarity this graph does not track.
    with pytest.raises(TypeError):
        sine(1.0)._compose_narop("range", 0.0, 1.0)


def test_unknown_selector_raises():
    # A selector with no UGen (not part of the operator surface) still fails.
    a = sine(100.0)
    with pytest.raises(TypeError):
        a._compose_binop("bogus", 1.0)
    with pytest.raises(TypeError):
        a._compose_unop("bogus")


def test_constant_only_inputs_and_no_controls():
    spec = SynthDef("noise", out(0.0, white_noise() * 0.1)).spec()
    assert spec["controls"] == []
    assert [u["kind"] for u in spec["ugens"]] == ["WhiteNoise", "Mul", "Out"]


def test_conflicting_control_defaults_raise():
    g = sine(control("freq", 440.0)) + sine(control("freq", 200.0))
    with pytest.raises(ValueError):
        SynthDef("bad", out(0.0, g)).spec()


def test_control_types_and_lag_serialize():
    # S2: a trigger, a scalar, and a lagged control emit their type/lag fields.
    gate = control("gate", 0.0, rate="tr")
    seed = control("seed", 1.0, rate="ir")
    freq = control("freq", 440.0, lag=0.1, lag_down=0.3)
    sig = sine(freq) * gate + seed * 0.0
    spec = SynthDef("typed", out(0.0, sig)).spec()
    by_name = {c["name"]: c for c in spec["controls"]}
    assert by_name["gate"]["rate"] == "tr"
    assert by_name["seed"]["rate"] == "ir"
    assert by_name["freq"]["lag"] == 0.1 and by_name["freq"]["lag_down"] == 0.3
    # A plain control carries neither field.
    plain = SynthDef("p", out(0.0, sine(control("f", 100.0)))).spec()
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

    lagged = SynthDef("l", out(0.0, lag(sine(5.0), 0.2))).spec()
    assert lagged["ugens"][-2]["kind"] == "Lag"

    seq = dseq([1.0, 2.0, 3.0], repeats=2.0)
    drv = demand(impulse(4.0), 0.0, seq)
    spec = SynthDef("d", out(0.0, drv)).spec()
    by_kind = {u["kind"]: u for u in spec["ugens"]}
    assert by_kind["Dseq"]["rate"] == "dr"
    assert by_kind["Dseq"]["inputs"][0] == {"const": 2.0}  # repeats first
    assert "Demand" in by_kind

    r = SynthDef("r", out(0.0, rand(2.0, 5.0) * sample_rate() * 0.0 + sine(1.0))).spec()
    kinds = {u["kind"]: u.get("rate") for u in r["ugens"]}
    assert kinds["Rand"] == "ir" and kinds["SampleRate"] == "ir"
    # at_rate sets an explicit rate.
    assert var_lag(sine(1.0)).at_rate("kr").rate == "kr"


def test_localin_precedes_localout():
    # A feedback write fed back into the output graph: LocalIn must be emitted
    # before LocalOut (the server enforces the one-block-delay contract).
    fb = local_in(0)
    sig = sine(200.0) + fb * 0.5
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


def test_side_effect_ugens_need_no_out():
    # S9 / C19: a def may consist only of side-effect UGens, with no Out.
    from clausters.defs import poll, send_reply, send_trig

    t = control("t", rate="tr")
    spec = SynthDef(
        "sfx",
        send_trig(t, 7, 0.5),
        send_reply(t, 1.5, 2.5, cmd="/custom", reply_id=42),
    ).spec()
    by_kind = {u["kind"]: u for u in spec["ugens"]}
    assert set(by_kind) == {"SendTrig", "SendReply"}
    # SendTrig: [in, id, value].
    assert by_kind["SendTrig"]["inputs"] == [
        {"control": 0},
        {"const": 7.0},
        {"const": 0.5},
    ]
    # SendReply: [trig, replyID, values...], custom address in `label`.
    assert by_kind["SendReply"]["label"] == "/custom"
    assert by_kind["SendReply"]["inputs"] == [
        {"control": 0},
        {"const": 42.0},
        {"const": 1.5},
        {"const": 2.5},
    ]
    # Poll passes its signal through, so it can sit under an Out.
    p = poll(t, sine(440.0), label="watch", trig_id=3)
    poll_spec = SynthDef("pl", out(0.0, p)).spec()
    poll_u = next(u for u in poll_spec["ugens"] if u["kind"] == "Poll")
    assert poll_u["label"] == "watch"
    assert poll_u["inputs"][2] == {"const": 3.0}


def test_fft_chain_serializes_with_static_fields():
    # S8: FFT opens a chain (carrying fft_size/hop/wintype as static fields),
    # PV_* filters transform it, IFFT closes it back to audio.
    from clausters.defs import fft, ifft, pv_brick_wall, pv_mag_above, white_noise

    chain = fft(white_noise(), fft_size=512, hop=0.25, wintype=1)
    chain = pv_brick_wall(chain, 0.7)
    chain = pv_mag_above(chain, 0.01)
    spec = SynthDef("fftdef", out(0.0, ifft(chain))).spec()
    by_kind = {u["kind"]: u for u in spec["ugens"]}
    assert set(by_kind) >= {"FFT", "PV_BrickWall", "PV_MagAbove", "IFFT", "Out"}

    f = by_kind["FFT"]
    # Static fields merge into the spec; only the FFT carries them.
    assert f["fft_size"] == 512 and f["hop"] == 0.25 and f["wintype"] == 1
    assert f["inputs"][0] == {"ugen": 0}  # the WhiteNoise source
    assert f["inputs"][1] == {"const": 1.0}  # active default
    assert "fft_size" not in by_kind["IFFT"]
    # The chain is threaded UGen-to-UGen in order.
    assert by_kind["PV_BrickWall"]["inputs"][0]["ugen"] < by_kind["PV_MagAbove"]["inputs"][0]["ugen"]


def test_fft_defaults():
    from clausters.defs import fft, ifft, white_noise

    spec = SynthDef("d", out(0.0, ifft(fft(white_noise())))).spec()
    f = next(u for u in spec["ugens"] if u["kind"] == "FFT")
    assert f["fft_size"] == 1024 and f["hop"] == 0.5 and f["wintype"] == 0


def test_pv_kernel_serializes_bin_expressions():
    # M29's mechanism: the symbolic per-bin expressions compile to the postfix
    # token lists the server's PV_Kernel validates; params become inputs 1..
    # read as p0, p1, ...
    from clausters.defs import control, fft, ifft, pv_kernel, white_noise
    from clausters.defs.pv_expr import bin_index, mag, nbins, param, phase, pv_tokens

    # Postfix serialization: operands in order, operator last.
    assert pv_tokens(mag * (mag >= param(0))) == ["mag", "mag", "p0", "ge", "mul"]
    assert pv_tokens(phase + 1.5) == ["phase", 1.5, "add"]
    assert pv_tokens(2.0) == [2.0]
    assert pv_tokens((bin_index / nbins).sqrt()) == ["bin", "nbins", "div", "sqrt"]

    chain = fft(white_noise())
    k = pv_kernel(
        chain,
        mag=mag * (mag >= param(0)),
        params=[control("thresh", 1.0)],
    )
    spec = SynthDef("kern", out(0.0, ifft(k))).spec()
    u = next(u for u in spec["ugens"] if u["kind"] == "PV_Kernel")
    assert u["mag_expr"] == ["mag", "mag", "p0", "ge", "mul"]
    assert "phase_expr" not in u  # omitted = identity, stays off the wire
    assert u["inputs"][1] == {"control": 0}  # the threshold parameter

    # An identity kernel serializes with no expression fields at all.
    plain = SynthDef("idk", out(0.0, ifft(pv_kernel(fft(white_noise()))))).spec()
    u = next(u for u in plain["ugens"] if u["kind"] == "PV_Kernel")
    assert "mag_expr" not in u and "phase_expr" not in u

    # A non-numeric operand fails client-side, before anything hits the wire.
    import pytest

    with pytest.raises(TypeError):
        mag * "loud"


def test_table_oscillators_and_shaper_serialize():
    # S5: the table readers take (bufnum, freq, phase) — bufnum a constant for
    # Osc/OscN, a signal for VOsc — and Shaper maps a signal through a table.
    from clausters.defs import osc, oscn, shaper, vosc

    pos = sine(0.5) * 2.0 + 3.0
    spec = SynthDef(
        "tables",
        out(0.0, osc(0, 220.0)),
        out(1.0, oscn(1, 220.0, 1.5)),
        out(2.0, vosc(pos, 110.0)),
        out(3.0, shaper(2, sine(330.0))),
    ).spec()
    by_kind = {u["kind"]: u for u in spec["ugens"]}
    assert by_kind["Osc"]["inputs"] == [
        {"const": 0.0},
        {"const": 220.0},
        {"const": 0.0},
    ]
    assert by_kind["OscN"]["inputs"][2] == {"const": 1.5}
    add_index = spec["ugens"].index(by_kind["Add"])
    assert by_kind["VOsc"]["inputs"][0] == {"ugen": add_index}  # bufpos is a signal
    assert by_kind["Shaper"]["inputs"][0] == {"const": 2.0}


def test_disk_io_serializes_with_static_fields():
    # DiskIn/DiskOut carry path/loop/format as static fields next to inputs.
    from clausters.defs import disk_in, disk_out

    spec = SynthDef(
        "disk",
        out(0.0, disk_in("/tmp/in.wav", chan=1.0, loop=True)),
        disk_out("/tmp/rec.wav", sine(440.0) * 0.2, format="float"),
    ).spec()
    by_kind = {u["kind"]: u for u in spec["ugens"]}
    din, dout = by_kind["DiskIn"], by_kind["DiskOut"]
    assert din["inputs"] == [{"const": 1.0}]
    assert din["path"] == "/tmp/in.wav" and din["loop"] is True
    assert dout["path"] == "/tmp/rec.wav" and dout["format"] == "float"
    mul_index = spec["ugens"].index(by_kind["Mul"])
    assert dout["inputs"] == [{"ugen": mul_index}]


def test_buf_info_queries_default_to_kr():
    from clausters.defs import buf_channels, buf_dur, buf_rate_scale, play_buf

    spec = SynthDef(
        "bufinfo",
        out(0.0, play_buf(0, rate=buf_rate_scale(0)) * (buf_dur(0) + buf_channels(0))),
    ).spec()
    for kind in ("BufRateScale", "BufDur", "BufChannels"):
        u = next(x for x in spec["ugens"] if x["kind"] == kind)
        assert u["rate"] == "kr"


def test_dup_by_reference_shares_the_node():
    # dup(node) repeats the reference; identity dedup serializes ONE Sine
    # fanned out to consecutive buses.
    from clausters.defs import dup

    spec = SynthDef("st", out(0.0, dup(sine(440.0)) * 0.1)).spec()
    kinds = [u["kind"] for u in spec["ugens"]]
    assert kinds.count("Sine") == 1
    outs = [u for u in spec["ugens"] if u["kind"] == "Out"]
    assert [o["inputs"][0] for o in outs] == [{"const": 0.0}, {"const": 1.0}]


def test_dup_of_a_callable_builds_distinct_nodes():
    from clausters.defs import dup

    spec = SynthDef("nz", out(0.0, dup(white_noise, 3) * 0.1)).spec()
    kinds = [u["kind"] for u in spec["ugens"]]
    assert kinds.count("WhiteNoise") == 3
    # The method form is always by reference.
    assert [u["kind"] for u in SynthDef(
        "m", out(0.0, white_noise().dup(3) * 0.1)
    ).spec()["ugens"]].count("WhiteNoise") == 1


def test_channel_ops_broadcast_and_wrap():
    from clausters.defs import chans

    # scalar broadcasts, both sides
    cl = chans(sine(440.0), sine(660.0))
    assert [u.kind for u in cl * 0.5] == ["Mul", "Mul"]
    assert [u.kind for u in 0.5 * cl] == ["Mul", "Mul"]
    # a plain list zips; the shorter side wraps modulo (the value-side rule)
    three = chans(sine(1.0), sine(2.0), sine(3.0))
    prod = three * [0.1, 0.2]
    assert [u.inputs[1] for u in prod] == [0.1, 0.2, 0.1]
    # a scalar node broadcasts too, shared by reference
    amp = control("amp", 0.1)
    assert all(u.inputs[1] is amp for u in three * amp)
    # numeric channels compute on the value side
    assert list(chans(1.0, 2.0) + 1.0) == [2.0, 3.0]


def test_mix_folds_with_the_fused_sums():
    from clausters.defs import dup, mix

    spec = SynthDef("mx", out(0.0, mix(dup(white_noise, 8)) * 0.1)).spec()
    kinds = [u["kind"] for u in spec["ugens"]]
    # 8 -> Sum4 + Sum4 -> Add: 3 sum UGens, no Add chain of 7.
    assert kinds.count("Sum4") == 2 and kinds.count("Add") == 1
    assert mix(2.0) == 2.0
    assert mix([1.0, 2.0, 3.0]) == 6.0


def test_channel_list_as_def_and_root_flattening():
    from clausters.defs import dup
    from clausters.defs.asdef import as_def

    sdef = as_def(dup(sine(440.0)) * 0.1, name="st")
    outs = [u for u in sdef.spec()["ugens"] if u["kind"] == "Out"]
    assert [o["inputs"][0]["const"] for o in outs] == [0.0, 1.0]


def test_channel_list_rejected_as_single_channel_input():
    from clausters.defs import chans, dup, env_gen

    sig = env_gen(Env.perc(), gate=chans(1.0, control("g", 1.0)))
    with pytest.raises(TypeError, match="channel list"):
        SynthDef("bad", out(0.0, sig)).spec()
    with pytest.raises(TypeError, match="nested"):
        chans(dup(sine(1.0)), sine(2.0))


def test_multichannel_out_needs_a_constant_bus():
    from clausters.defs import dup

    with pytest.raises(TypeError, match="constant bus"):
        out(control("bus", 0.0), dup(sine(440.0)))


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


def test_env_step_holds_each_value_for_its_duration():
    # The value-with-duration interface: equal-length levels/times, expressed
    # (as in sclang) by prepending the first level over the step shape, whose
    # segments jump to their target at the start.
    e = Env.step([0.0, 1.0], [0.5, 0.5])
    assert e.levels == [0.0, 0.0, 1.0]
    assert e.times == [0.5, 0.5]
    assert e.to_inputs() == [
        0.0, 2.0, -1.0, -1.0,
        0.0, 0.5, 0.0, 0.0,  # jumps to 0 at t=0: 0 held for 0.5
        1.0, 0.5, 0.0, 0.0,  # jumps to 1 at t=0.5: 1 held for 0.5
    ]
    with pytest.raises(ValueError):
        Env.step([0.0, 1.0], [0.5])  # equal lengths, unlike the raw form
    with pytest.raises(ValueError):
        Env.step([], [])


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

    # The built-in "default" path (gate-released, as the player does for it).
    s0 = Server(interface=OscNrtInterface())
    c0 = TempoClock(tempo=1.0)
    Pbind(instrument="default", freq=Pseq(FREQS), dur=0.5, amp=0.2).play(c0, s0)
    c0.render()

    # The client-defined equivalent (same graph, incl. the gated envelope): add
    # it to the score, then the same Pbind — released by gate too (has_gate).
    s1 = Server(interface=OscNrtInterface())
    _py_default_env().send(s1)              # /def_send synth at time 0 in the score
    c1 = TempoClock(tempo=1.0)
    Pbind(instrument="py_default_env", freq=Pseq(FREQS), dur=0.5, amp=0.2,
          has_gate=True).play(c1, s1)
    c1.render()

    try:
        _st0 = render(s0.interface.score.bytes())
        builtin, b_frames = _st0.samples, _st0.frames
        _st1 = render(s1.interface.score.bytes())
        custom, c_frames = _st1.samples, _st1.frames
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


# ---- panning, the stereo field and selection (U7) ----


def test_pan_builders_emit_one_row_per_channel():
    """A UGen has one output, so a stereo panner is two rows sharing their
    inputs and differing only in the trailing channel index — which the builder
    fills and the caller never sees."""
    from clausters.defs.ugens import pan2

    sig = sine(440.0)
    spec = SynthDef("p", out(0.0, pan2(sig, 0.3))).spec()
    rows = [u for u in spec["ugens"] if u["kind"] == "Pan2"]
    assert len(rows) == 2
    # Same source (serialized once), same position, different channel.
    assert spec["ugens"].count(rows[0]) == 1
    assert [u["kind"] for u in spec["ugens"]].count("Sine") == 1
    assert rows[0]["inputs"][:3] == rows[1]["inputs"][:3]
    assert [r["inputs"][3] for r in rows] == [{"const": 0.0}, {"const": 1.0}]
    # ...and the pair lands on consecutive buses.
    outs = [u for u in spec["ugens"] if u["kind"] == "Out"]
    assert [o["inputs"][0] for o in outs] == [{"const": 0.0}, {"const": 1.0}]


def test_pan_az_sizes_the_ring_and_numbers_its_channels():
    from clausters.defs.ugens import pan_az

    spec = SynthDef("az", out(0.0, pan_az(4, sine(440.0), 0.5))).spec()
    rows = [u for u in spec["ugens"] if u["kind"] == "PanAz"]
    assert len(rows) == 4
    # Every row is told the same ring size and its own place on it.
    assert [r["inputs"][5] for r in rows] == [{"const": 4.0}] * 4
    assert [r["inputs"][6] for r in rows] == [{"const": float(c)} for c in range(4)]

    with pytest.raises(ValueError):
        pan_az(0, sine(440.0))


def test_mid_side_round_trip_serializes_as_two_pairs():
    """The composable form: encode, do something to one axis, decode — four
    rows of the same kind, not a special decoder."""
    from clausters.defs.ugens import mid_side

    m, s = mid_side(sine(440.0), white_noise())
    spec = SynthDef("ms", out(0.0, mid_side(m, s * 1.5))).spec()
    assert [u["kind"] for u in spec["ugens"]].count("MidSide") == 4


def test_selectors_take_their_sources_as_arguments_or_a_list():
    from clausters.defs.ugens import select, select_x

    a, b = sine(440.0), white_noise()
    assert select(1.0, a, b).inputs == select(1.0, [a, b]).inputs
    spec = SynthDef("sel", out(0.0, select_x(0.5, a, b) * 0.1)).spec()
    row = next(u for u in spec["ugens"] if u["kind"] == "SelectX")
    assert len(row["inputs"]) == 3  # the index, then both sources
    with pytest.raises(ValueError):
        select(0.0)


def test_splay_spreads_and_folds_to_a_pair():
    """A client-side helper, so it must come out as plain rows: one `Pan2`
    pair per source, folded by the fused sums into two channels."""
    from clausters.defs.ugens import splay

    voices = [sine(220.0), sine(440.0), sine(660.0)]
    result = splay(voices)
    assert len(result) == 2
    spec = SynthDef("sp", out(0.0, result * 0.1)).spec()
    kinds = [u["kind"] for u in spec["ugens"]]
    assert kinds.count("Pan2") == 6
    assert kinds.count("Sum3") == 2
    # The outer voices are hard panned, the middle one is centred.
    positions = sorted({u["inputs"][1]["const"] for u in spec["ugens"]
                        if u["kind"] == "Pan2"})
    assert positions == [-1.0, 0.0, 1.0]


def test_a_demand_stream_may_be_the_value_of_another():
    """The property the whole family turns on: a source's input can be a
    source, and it comes out as an ordinary wire between two `dr` rows."""
    from clausters.defs.ugens import dseq, dseries, demand, impulse

    phrase = dseries(3, 0.0, 1.0)
    spec = SynthDef("nest", out(0.0, demand(impulse(4.0), 0.0, dseq([phrase, 100.0])))).spec()
    by_kind = {u["kind"]: u for u in spec["ugens"]}
    assert by_kind["Dseries"]["rate"] == "dr"
    assert by_kind["Dseq"]["rate"] == "dr"
    # repeats leads, then the wire to the nested stream, then the constant.
    inputs = by_kind["Dseq"]["inputs"]
    assert inputs[0] == {"const": 0.0}
    assert "ugen" in inputs[1]
    assert inputs[2] == {"const": 100.0}


def test_a_demand_source_needs_a_value():
    from clausters.defs.ugens import dseq

    with pytest.raises(ValueError):
        dseq([])


def test_the_duty_drivers_carry_their_own_clock():
    """`duty` needs no trigger — the arity is what says so — and `tduty` adds
    the opening gap."""
    from clausters.defs.ugens import dseq, duty, tduty

    levels = dseq([1.0, 2.0])
    spec = SynthDef("d", out(0.0, duty(0.25, level=levels) + tduty(0.5, level=levels))).spec()
    by_kind = {u["kind"]: u for u in spec["ugens"]}
    assert len(by_kind["Duty"]["inputs"]) == 4
    assert len(by_kind["TDuty"]["inputs"]) == 5
    assert by_kind["Duty"]["inputs"][0] == {"const": 0.25}
