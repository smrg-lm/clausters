"""Box-builder tests: every callable emits the server's box-schema JSON, the
two application stages (eval-args splicing vs. box call), client-side arity,
channel selection, and the wire-reuse lint. The server-side half of the same
contract (duplicated subtrees share their computation) lives in the Rust CSE
suite (`tests/faust_box.rs`)."""

import pytest

from clausters.defs import FaustDef
from clausters.defs import boxes as B


# ---- constructors emit the schema JSON ----

def test_primitives_and_numbers():
    assert B.wire().to_json() == {"op": "wire"}
    assert B.cut().to_json() == {"op": "cut"}
    assert B.box(2).to_json() == 2          # int -> Faust int
    assert B.box(2.0).to_json() == 2.0      # float -> Faust real
    with pytest.raises(TypeError):
        B.box("nope")


def test_composition_ops_are_nary():
    a, b, c = B.wire(), B.wire(), B.wire()
    assert B.seq(a, b, c).to_json() == {
        "op": "seq", "in": [a.node, b.node, c.node]}
    assert B.par(a, b).to_json() == {"op": "par", "in": [a.node, b.node]}
    assert B.split(a, b).to_json() == {"op": "split", "in": [a.node, b.node]}
    assert B.merge(a, b).to_json() == {"op": "merge", "in": [a.node, b.node]}
    with pytest.raises(TypeError):
        B.seq(a)


def test_operators_and_functions():
    freq = B.hslider("freq", 330.0, 20.0, 2000.0, 0.1)
    node = (B.sin(freq * 2.0) * 0.5).to_json()
    assert node == {
        "op": "mul",
        "in": [
            {"op": "sin", "in": [{"op": "mul", "in": [
                {"op": "hslider", "label": "freq", "init": 330.0,
                 "min": 20.0, "max": 2000.0, "step": 0.1},
                2.0]}]},
            0.5,
        ],
    }
    # Python % is Faust's fmod in the box schema (there is no rem).
    assert (B.wire() % 2.0).node["op"] == "fmod"
    # neg is 0 - x, as in signals.
    assert (-B.wire()).node == {"op": "sub", "in": [0.0, {"op": "wire"}]}
    # No shift ops in the box schema.
    with pytest.raises(ValueError):
        B.wire() << 2


def test_structure_controls_tables():
    assert B.delay(B.wire(), 100).to_json() == {
        "op": "delay", "in": [{"op": "wire"}, 100]}
    assert B.delay1(B.wire()).to_json() == {
        "op": "delay", "in": [{"op": "wire"}, 1]}
    assert B.select2(B.button("gate"), 1.0, 2.0).to_json() == {
        "op": "select2",
        "in": [{"op": "button", "label": "gate"}, 1.0, 2.0]}
    assert B.hgroup("g", B.wire()).to_json() == {
        "op": "hgroup", "label": "g", "in": [{"op": "wire"}]}
    assert B.fconst("int", "fSamplingFreq", "<math.h>").to_json() == {
        "op": "fconst", "ctype": "int", "name": "fSamplingFreq",
        "file": "<math.h>"}
    wf = B.waveform([0, 1, 0, -1])
    assert wf.to_json() == {"op": "waveform", "values": [0.0, 1.0, 0.0, -1.0]}
    assert B.rdtable(wf, B.wire()).to_json() == {
        "op": "rdtable", "in": [wf.node, {"op": "wire"}]}
    assert B.rdtable(4, 0.0, B.wire()).node["op"] == "rdtable"
    with pytest.raises(TypeError):
        B.rdtable(4)
    assert B.sr().node["op"] == "min"  # the ma.SR clamp, as in signals


# ---- the faust escape hatch: eval-args splicing ----

def test_faust_wraps_an_expression():
    assert B.faust("os.osc(440.0)").to_json() == {
        "op": "faust",
        "src": 'import("stdfaust.lib"); process = os.osc(440.0);'}


def test_faust_splices_eval_args():
    assert B.faust("fi.lowpass", 3).node["src"] == (
        'import("stdfaust.lib"); process = fi.lowpass(3);')
    # int/float literals, strings verbatim, lists as Faust lists.
    assert B.faust("f", 3, 0.5, "ba.take", [400.0, 900.0]).node["src"] == (
        'import("stdfaust.lib"); process = f(3, 0.5, ba.take, (400.0, 900.0));')
    with pytest.raises(TypeError):
        B.faust("f", B.wire())  # boxes are composition-stage, not eval-stage
    with pytest.raises(TypeError):
        B.faust("f", True)


def test_faust_defs_prepends_definitions():
    src = B.faust("dup(3)", defs="dup(n) = par(i, n, _);").node["src"]
    assert src == ('import("stdfaust.lib"); dup(n) = par(i, n, _); '
                   'process = dup(3);')


# ---- application: __call__ is seq(par(args), self) ----

def test_call_applies_boxes():
    lp = B.faust("fi.lowpass", 3, ins=2, outs=1)
    cutoff = B.hslider("cutoff", 800.0, 20.0, 8000.0, 0.1)
    y = lp(cutoff, B.wire())
    assert y.to_json() == {
        "op": "seq",
        "in": [{"op": "par", "in": [cutoff.node, {"op": "wire"}]}, lp.node]}
    # One argument: no par wrapper.
    osc = B.faust("os.osc", ins=1, outs=1)
    y = osc(440.0)
    assert y.to_json() == {"op": "seq", "in": [440.0, osc.node]}
    with pytest.raises(TypeError):
        osc()


# ---- client-side arity ----

def test_arity_propagation():
    assert (B.wire().num_inputs, B.wire().num_outputs) == (1, 1)
    assert (B.cut().num_inputs, B.cut().num_outputs) == (1, 0)
    assert B.hslider("a", 0, 0, 1, 0.1).num_inputs == 0
    x = B.wire() + B.wire()          # 2-in, 1-out
    assert (x.num_inputs, x.num_outputs) == (2, 1)
    p = B.par(B.wire(), x)
    assert (p.num_inputs, p.num_outputs) == (3, 2)
    s = B.seq(p, B.merge(B.par(B.wire(), B.wire()), B.wire()))
    assert (s.num_inputs, s.num_outputs) == (3, 1)
    # rec eats outs(b) from ins(a); one-pole: (+ ~ *(0.9)) is 1-in 1-out.
    loop = B.rec(B.wire() + B.wire(), B.wire() * 0.9)
    assert (loop.num_inputs, loop.num_outputs) == (1, 1)
    # A fragment's arity is the compiler's unless declared.
    assert B.faust("os.osc").num_outputs is None
    assert B.faust("pf.phaser2_stereo", outs=2).num_outputs == 2
    # Unknown absorbs through composition where it matters.
    assert (B.faust("x") + 1.0).num_inputs is None
    assert B.seq(B.wire(), B.faust("x")).num_inputs == 1


# ---- channel selection ----

def test_getitem_and_outs():
    st = B.faust("st_thing", ins=1, outs=2)(B.wire())
    left = st[0]
    assert left.to_json() == {
        "op": "seq",
        "in": [st.node, {"op": "par", "in": [{"op": "wire"}, {"op": "cut"}]}]}
    right = st[-1]
    assert right.node["in"][1] == {
        "op": "par", "in": [{"op": "cut"}, {"op": "wire"}]}
    # Both taps reference the SAME fragment subtree (shared, computed once).
    assert left.node["in"][0] is right.node["in"][0]
    l, r = st.outs()
    assert (l.num_inputs, l.num_outputs) == (1, 1)
    with pytest.raises(IndexError):
        st[2]
    with pytest.raises(ValueError):
        B.faust("unknown")[0]
    mono = B.faust("m", ins=1, outs=1)(B.wire())
    assert mono[0] is mono  # single output: selection is the identity


# ---- value reuse: duplicated subtrees are shared JSON (server CSEs them) ----

def test_value_reuse_duplicates_the_subtree_object():
    x = B.faust("os.osc", 330.0)
    y = x + x
    assert y.node["in"][0] is y.node["in"][1]  # same dict, shared computation
    # The explicit routing spells the same program.
    routed = B.split(x, B.wire() + B.wire())
    assert routed.node == {
        "op": "split",
        "in": [x.node, {"op": "add", "in": [{"op": "wire"}, {"op": "wire"}]}]}


# ---- the wire-reuse lint ----

def test_wire_reuse_is_rejected():
    w = B.wire()
    with pytest.raises(ValueError, match="wire"):
        FaustDef.from_box("bad", w + w)
    # ...also when the wire hides inside a reused expression.
    x = B.wire() * 2.0
    with pytest.raises(ValueError, match="wire"):
        FaustDef.from_box("bad", B.par(x, x))
    # ...and for cut objects.
    c = B.cut()
    with pytest.raises(ValueError, match="cut"):
        FaustDef.from_box("bad", B.seq(B.par(c, c), B.wire()))


def test_distinct_wires_and_shared_expressions_pass():
    y = B.wire() + B.wire()               # two wires, two objects: fine
    d = FaustDef.from_box("ok", y)
    assert d.kind == "box" and d._payload == y.node
    x = B.faust("os.osc", 330.0)          # wireless value reused: fine
    FaustDef.from_box("ok2", x + x)
    # Raw dicts (machine-generated) skip the lint, unchanged behavior.
    node = {"op": "add", "in": ["_", "_"]}
    assert FaustDef.from_box("raw", node)._payload is node


def test_control_names_sees_box_labels():
    cutoff = B.hslider("cutoff", 800.0, 20.0, 8000.0, 0.1)
    d = FaustDef.from_box("fx", B.faust("fi.lowpass", 3)(cutoff, B.wire()))
    assert d.control_names() == ["cutoff"]
