"""The reference cases: small UGen graphs run through SuperCollider's code by
`gen.py` and through the server's by `tests/offline/sc_reference.rs`.

A case is a def's UGen list in the server's own spec form (`kind`, `rate`,
`inputs`, `op`), its last UGen (or `probe`) the wire compared. Where the two
spell a UGen differently, an `sc` entry on it says how scsynth spells it:
`kind` (`Sine` is scsynth's `SinOsc`), `inputs` (a channel the server takes as
an input is scsynth's output), `outputs` (how many scsynth's has) and `output`
(which of them the server's single output is).

`tol` is the largest difference allowed, relative to the reference's
magnitude where that is above one; `shift` how many samples the server's
output leads scsynth's, and `skip` how many of the first values are not
compared (a transient scsynth's constructor leaves), each with a `why`.
`sr` runs a case at a sample rate of its own (48 kHz otherwise), for a UGen
tuned for one. A case whose difference is a decision rather than an error
says so in `why`, and the comparison then checks only what the decision
keeps.
"""

# scsynth's operator indices (`SpecialSelectorsOperatorsAndClasses.h`), by
# the server's operator names.
_BINARY = [
    "add", "sub", "mul", "idiv", "div", "mod", "eq", "ne", "lt", "gt", "le", "ge",
    "min", "max", "bitand", "bitor", "bitxor", "lcm", "gcd", "round", "roundup",
    "trunc", "atan2", "hypot", "hypotapx", "pow", "lshift", "rshift", "urshift",
    "fill", "ring1", "ring2", "ring3", "ring4", "difsqr", "sumsqr", "sqrsum",
    "sqrdif", "absdif", "thresh", "amclip", "scaleneg", "clip2", "excess",
    "fold2", "wrap2", "firstarg", "rrand", "exprand",
]  # fmt: skip
_UNARY = [
    "neg", "not", "isnil", "notnil", "bitnot", "abs", "asfloat", "asint", "ceil",
    "floor", "frac", "sign", "squared", "cubed", "sqrt", "exp", "recip",
    "midicps", "cpsmidi", "midiratio", "ratiomidi", "dbamp", "ampdb", "octcps",
    "cpsoct", "log", "log2", "log10", "sin", "cos", "tan", "asin", "acos", "atan",
    "sinh", "cosh", "tanh", "rand", "rand2", "linrand", "bilinrand", "sum3rand",
    "distort", "softclip", "coin", "digitvalue", "silence", "thru", "rectwindow",
    "hanwindow", "welwindow", "triwindow", "ramp", "scurve",
]  # fmt: skip
SPECIAL = {}
SPECIAL.update({f"u:{n}": i for i, n in enumerate(_UNARY)})
SPECIAL.update({n: i for i, n in enumerate(_BINARY)})


def c(v: float) -> dict:
    return {"const": float(v)}


class G:
    """A graph being written: `add` appends a UGen and returns the input that
    reads it, so a case reads in the order its signal flows."""

    def __init__(self) -> None:
        self.ugens: list[dict] = []

    def add(self, kind: str, rate: str, *inputs, sc: dict | None = None, **extra) -> dict:
        d = {"kind": kind, "rate": rate, "inputs": [x if isinstance(x, dict) else c(x) for x in inputs]}
        if sc:
            sc = dict(sc)
            if "inputs" in sc:
                sc["inputs"] = [x if isinstance(x, dict) else c(x) for x in sc["inputs"]]
            d["sc"] = sc
        d.update(extra)
        self.ugens.append(d)
        return {"ugen": len(self.ugens) - 1}


def case(name: str, g: G, blocks: int = 4, **extra) -> dict:
    d = {"name": name, "blocks": blocks, "ugens": g.ugens}
    d.update(extra)
    return d


def one(name: str, kind: str, rate: str, *inputs, blocks: int = 4, sc: dict | None = None, **extra) -> dict:
    """A case of one UGen fed constants."""
    g = G()
    g.add(kind, rate, *inputs, sc=sc)
    return case(name, g, blocks, **extra)


def cases() -> list[dict]:
    out: list[dict] = []
    for f in [
        _sources,
        _oscillators,
        _lines,
        _operators,
        _filters,
        _delays,
        _triggers,
        _pans,
        _smoothers,
        _buffers,
        _envelopes,
        _demand,
        _random,
        _sections,
        _lfugens,
        _oscugens,
        _tables,
        _chaos,
        _dynamics,
        _running,
        _info,
        _delay_lines,
        _local_buffers,
        _spaces,
        _grains,
    ]:
        out.extend(f())
    return out


def _sources() -> list[dict]:
    """The deterministic sources the other cases drive their inputs with."""
    return [
        one("LFSaw.ar", "LFSaw", "ar", 440, 0),
        # The server's initial phase is in cycles, scsynth's in [0, 2).
        one("LFSaw.kr", "LFSaw", "kr", 7, 0.25, blocks=64, sc={"inputs": [7, 0.5]}),
        one("Impulse.ar", "Impulse", "ar", 1000, 0),
        one("Line.ar", "Line", "ar", 0, 1, 0.004, 0, blocks=6),
    ]


def sig(g: G, rate: str = "ar") -> dict:
    """A deterministic broadband signal: two saws at unrelated frequencies,
    multiplied, so every filter and nonlinearity has something to act on. At
    control rate the frequencies stay below the control rate's Nyquist, where
    scsynth's control-rate saw still wraps."""
    f1, f2 = (997, 130.8) if rate == "ar" else (97.3, 13.08)
    a = g.add("LFSaw", rate, f1, 0)
    b = g.add("LFSaw", rate, f2, 0.15, sc={"inputs": [f2, 0.3]})
    return g.add("BinaryOpUGen", rate, a, b, op="mul")


def impulse(g: G) -> dict:
    """Impulses a few samples in. Not at the first sample: scsynth's
    constructors run one sample of the calculation to prime the output, and
    the filters keep the state it leaves, so an impulse there is counted
    twice -- an artifact of scsynth's priming, which the server does not
    reproduce."""
    return g.add("Impulse", "ar", 150, 0.99)


def through(name: str, kind: str, rate: str, *params, blocks: int = 4, sc_params=None, source=sig, **extra) -> dict:
    """A case of `kind` fed `source` and then `params` as constants."""
    g = G()
    x = source(g) if source is not impulse else impulse(g)
    sc = None
    if sc_params is not None:
        sc = {"inputs": [x, *sc_params]}
    g.add(kind, rate, x, *params, sc=sc)
    return case(name, g, blocks, **extra)


def _oscillators() -> list[dict]:
    out = []
    # Frequencies whose phase never lands exactly on a wrap: where it does,
    # which side of the wrap a sample falls on is the rounding of the two
    # accumulators, and the two differ in width (scsynth's is `f32` at
    # control rate).
    for rate, blocks, freqs in [("ar", 4, [441.3, 1234.5]), ("kr", 64, [7.7, 213.3])]:
        tol = 1e-5 if rate == "kr" else 2e-6
        why = "scsynth accumulates a control-rate phase in f32" if rate == "kr" else None
        kw = {"tol": tol, "why": why} if why else {}
        for freq in freqs:
            out.append(one(f"LFSaw.{rate}.{freq}", "LFSaw", rate, freq, 0, blocks=blocks, **kw))
            out.append(one(f"LFTri.{rate}.{freq}", "LFTri", rate, freq, 0, blocks=blocks, **kw))
            out.append(one(f"LFPulse.{rate}.{freq}", "LFPulse", rate, freq, 0, 0.3, blocks=blocks, **kw))
            out.append(one(f"VarSaw.{rate}.{freq}", "VarSaw", rate, freq, 0, 0.3, blocks=blocks, **kw))
            out.append(one(f"Impulse.{rate}.{freq}", "Impulse", rate, freq, 0, blocks=blocks))
            out.append(
                one(
                    f"Sine.{rate}.{freq}", "Sine", rate, freq, blocks=blocks, sc={"kind": "SinOsc", "inputs": [freq, 0]},
                    tol=1e-5, why="scsynth's SinOsc reads an 8192-point table; the server computes sin()",
                )
            )  # fmt: skip
        for ph in [0.15, 0.4]:
            f = 301.3 if rate == "ar" else 31.3
            out.append(one(f"LFSaw.{rate}.phase{ph}", "LFSaw", rate, f, ph, blocks=blocks, sc={"inputs": [f, 2 * ph]}, **kw))
            out.append(one(f"LFTri.{rate}.phase{ph}", "LFTri", rate, f, ph, blocks=blocks, sc={"inputs": [f, 4 * ph]}, **kw))
            out.append(one(f"LFPulse.{rate}.phase{ph}", "LFPulse", rate, f, ph, 0.5, blocks=blocks, **kw))
            out.append(one(f"VarSaw.{rate}.phase{ph}", "VarSaw", rate, f, ph, 0.5, blocks=blocks, **kw))
            out.append(one(f"Impulse.{rate}.phase{ph}", "Impulse", rate, f, ph, blocks=blocks))
        out.append(one(f"Impulse.{rate}.negative", "Impulse", rate, -500 if rate == "ar" else -50, 0.3, blocks=blocks))
    # Modulated frequencies and widths.
    for kind, extra in [("LFSaw", [0]), ("LFTri", [0]), ("LFPulse", [0, 0.5]), ("VarSaw", [0, 0.5]), ("Impulse", [0])]:
        g = G()
        f = g.add("LFSaw", "ar", 50, 0)
        f = g.add("MulAdd", "ar", f, 2000, 2400)
        g.add(kind, "ar", f, *extra)
        out.append(case(f"{kind}.fm", g, blocks=8))
    for kind in ["LFPulse", "VarSaw"]:
        # A control-rate width: scsynth reads an audio-rate one at the first
        # sample of the block, where the server reads it at the sample the
        # cycle starts on.
        g = G()
        w = g.add("LFTri", "kr", 23, 0)
        w = g.add("MulAdd", "kr", w, 0.45, 0.5)
        g.add(kind, "ar", 1501.7, 0, w)
        out.append(case(f"{kind}.width", g, blocks=8))
    for rate in ["ar", "kr"]:
        g = G()
        f = g.add("LFSaw", rate, 50 if rate == "ar" else 5, 0)
        f = g.add("MulAdd", rate, f, 200, 400)
        g.add("Sine", "ar", f, sc={"kind": "SinOsc", "inputs": [f, 0]})
        out.append(case(f"Sine.fm.{rate}", g, tol=1e-5, why="scsynth's SinOsc reads an 8192-point table"))
    for rate in ["ar", "kr"]:
        # Phase modulation: the server's offset is in cycles, scsynth's in
        # radians.
        g = G()
        p = g.add("LFTri", rate, 330 if rate == "ar" else 7, 0)
        p_sc = g.add("MulAdd", rate, p, 6.283185307179586, 0)
        g.add("Sine", "ar", 440, p, sc={"kind": "SinOsc", "inputs": [440, p_sc]})
        out.append(case(f"Sine.pm.{rate}", g, tol=1e-5, why="scsynth's SinOsc reads an 8192-point table"))
    # An audio-rate offset needs an audio-rate frequency for scsynth to read
    # it per sample: with a constant frequency it reads the offset once a
    # block, where the server reads it per sample either way.
    g = G()
    p = g.add("LFTri", "ar", 330, 0)
    f = g.add("Line", "ar", 700, 700, 1, 0)
    g.add("Impulse", "ar", f, p)
    out.append(case("Impulse.phase.ar", g, blocks=8))
    g = G()
    p = g.add("LFTri", "kr", 7, 0)
    g.add("Impulse", "ar", 700, p)
    out.append(case("Impulse.phase.kr", g, blocks=8))
    g = G()
    trig = g.add("Impulse", "ar", 3000, 0)
    g.add("Phasor", "ar", trig, 0.01, -0.5, 0.5, 0.25)
    out.append(case("Phasor.ar", g, blocks=8))
    out.append(one("Phasor.kr", "Phasor", "kr", 0, 0.05, 0, 1, 0, blocks=64))
    return out


def _lines() -> list[dict]:
    return [
        one("Line.ar.done", "Line", "ar", 0.5, -1, 0.003, 2, blocks=6),
        one("Line.kr", "Line", "kr", 0.5, -1, 0.02, 0, blocks=32),
        one("XLine.ar", "XLine", "ar", 1, 0.01, 0.004, 0, blocks=6),
        one("XLine.kr", "XLine", "kr", 0.01, 2, 0.02, 0, blocks=32),
    ]


def _operators() -> list[dict]:
    out = []
    for op in [
        "add", "sub", "mul", "div", "mod", "pow", "min", "max", "atan2", "gt", "lt", "ge", "le",
        "eq", "ne", "hypot", "hypotapx", "ring1", "ring2", "ring3", "ring4", "sumsqr", "difsqr",
        "sqrsum", "sqrdif", "absdif", "thresh", "amclip", "scaleneg", "clip2", "excess", "fold2",
        "wrap2", "round", "roundup", "trunc", "firstarg", "idiv", "gcd", "lcm", "bitand", "bitor",
        "bitxor", "lshift", "rshift",
    ]:  # fmt: skip
        g = G()
        a = sig(g)
        b = g.add("LFSaw", "ar", 31, 0.25, sc={"inputs": [31, 0.5]})
        b = g.add("MulAdd", "ar", b, 0.7, 0.1)
        if op in ("idiv", "gcd", "lcm", "bitand", "bitor", "bitxor", "lshift", "rshift"):
            a = g.add("MulAdd", "ar", a, 20, 3)
            b = g.add("MulAdd", "ar", b, 5, 4)
        g.add("BinaryOpUGen", "ar", a, b, op=op)
        out.append(case(f"BinaryOpUGen.{op}", g))
    for op in [
        "neg", "abs", "sin", "cos", "tan", "asin", "acos", "atan", "exp", "log", "log2", "log10",
        "sqrt", "floor", "ceil", "asfloat", "squared", "cubed", "recip", "frac", "sign",
        "sinh", "cosh", "tanh", "midicps", "cpsmidi", "midiratio", "ratiomidi", "dbamp", "ampdb",
        "octcps", "cpsoct", "distort", "softclip", "bitnot", "rectwindow", "hanwindow", "welwindow",
        "triwindow", "ramp", "scurve",
    ]:  # fmt: skip
        g = G()
        a = sig(g)
        if op == "recip":
            a = g.add("MulAdd", "ar", a, 0.5, 1)
        if op in ("log", "log2", "log10", "sqrt", "cpsmidi", "ratiomidi", "ampdb", "cpsoct"):
            a = g.add("MulAdd", "ar", a, 0.5, 0.75)
        if op in ("midicps", "octcps"):
            a = g.add("MulAdd", "ar", a, 24, 60 if op == "midicps" else 4)
        if op in ("rectwindow", "hanwindow", "welwindow", "triwindow", "ramp", "scurve"):
            a = g.add("MulAdd", "ar", a, 0.7, 0.5)
        if op == "bitnot":
            a = g.add("MulAdd", "ar", a, 20, 0)
        g.add("UnaryOpUGen", "ar", a, op=op, sc={"special": SPECIAL[f"u:{op}"]})
        out.append(case(f"UnaryOpUGen.{op}", g))
    for kind in ["Clip", "Wrap", "Fold"]:
        g = G()
        a = sig(g)
        a = g.add("MulAdd", "ar", a, 3, 0)
        g.add(kind, "ar", a, -0.4, 0.7)
        out.append(case(f"{kind}.ar", g))
    g = G()
    a = sig(g)
    g.add("MulAdd", "ar", a, 0.3, -0.2)
    out.append(case("MulAdd.ar", g))
    g = G()
    a = sig(g)
    b = g.add("LFTri", "ar", 77, 0)
    d = g.add("LFSaw", "ar", 1500, 0)
    g.add("Sum3", "ar", a, b, d)
    g.add("Sum4", "ar", a, b, d, {"ugen": 3})
    out.append(case("Sum3.ar", g, probe=5))
    out.append(case("Sum4.ar", g))
    return out


def _filters() -> list[dict]:
    out = []
    for src in [sig, impulse]:
        tag = "imp" if src is impulse else "sig"
        for freq in [300, 3000]:
            out.append(through(f"LPF.{tag}.{freq}", "LPF", "ar", freq, source=src))
            out.append(through(f"HPF.{tag}.{freq}", "HPF", "ar", freq, source=src))
            for rq in [1, 0.1]:
                for kind in ["RLPF", "RHPF", "BPF", "BRF", "Resonz"]:
                    out.append(through(f"{kind}.{tag}.{freq}.{rq}", kind, "ar", freq, rq, source=src))
        out.append(through(f"OnePole.{tag}", "OnePole", "ar", 0.9, source=src))
        out.append(through(f"OnePole.{tag}.neg", "OnePole", "ar", -0.6, source=src))
        out.append(through(f"OneZero.{tag}", "OneZero", "ar", 0.5, source=src))
        out.append(through(f"LeakDC.{tag}", "LeakDC", "ar", 0.995, source=src))
        out.append(through(f"Integrator.{tag}", "Integrator", "ar", 0.99, source=src))
        out.append(through(f"Lag.{tag}", "Lag", "ar", 0.002, source=src))
        out.append(through(f"Decay.{tag}", "Decay", "ar", 0.002, source=src))
        out.append(through(f"Decay2.{tag}", "Decay2", "ar", 0.0005, 0.002, source=src))
    out.append(through("Lag.kr", "Lag", "kr", 0.05, source=lambda g: sig(g, "kr"), blocks=64))
    out.append(through("OnePole.kr", "OnePole", "kr", 0.9, source=lambda g: sig(g, "kr"), blocks=64))
    return out


def _delays() -> list[dict]:
    out = []
    for kind in ["DelayN", "DelayL", "DelayC"]:
        for t in [0.001, 0.0012345]:
            g = G()
            x = sig(g)
            g.add(kind, "ar", x, t, max_delay=0.01, sc={"inputs": [x, 0.01, t]})
            out.append(case(f"{kind}.{t}", g, blocks=6))
    for kind in ["CombN", "CombL", "CombC", "AllpassN", "AllpassL", "AllpassC"]:
        for t, dec in [(0.001, 0.05), (0.0012345, -0.03)]:
            # Past the line's first fill: while scsynth's cubic allpass is
            # filling its line it outputs nothing, not the feedforward term,
            # where its other allpasses (and the server's) do.
            g = G()
            x = g.add("Impulse", "ar", 150, 0.3)
            g.add(kind, "ar", x, t, dec, max_delay=0.01, sc={"inputs": [x, 0.01, t, dec]})
            out.append(case(f"{kind}.{t}", g, blocks=8))
    # A modulated delay time.
    for kind in ["DelayL", "DelayC", "CombL"]:
        g = G()
        x = sig(g)
        t = g.add("LFTri", "ar", 40, 0)
        t = g.add("MulAdd", "ar", t, 0.0005, 0.001)
        ins = [x, t] + ([0.03] if kind == "CombL" else [])
        g.add(kind, "ar", *ins, max_delay=0.01, sc={"inputs": [x, 0.01, t] + ([0.03] if kind == "CombL" else [])})
        out.append(case(f"{kind}.modulated", g, blocks=6))
    return out


def _triggers() -> list[dict]:
    out = []

    def trig_case(name, kind, *params, rate="ar", blocks=8, sc_params=None, freq=1700, **extra):
        g = G()
        t = g.add("Impulse", rate, freq, 0)
        sc = None if sc_params is None else {"inputs": [t, *sc_params]}
        g.add(kind, rate, t, *params, sc=sc)
        return case(name, g, blocks, **extra)

    for rate, blocks, freq in [("ar", 8, 1700), ("kr", 64, 37)]:
        out.append(trig_case(f"Trig.{rate}", "Trig", 0.0003, rate=rate, blocks=blocks, freq=freq))
        out.append(trig_case(f"Trig1.{rate}", "Trig1", 0.0003, rate=rate, blocks=blocks, freq=freq))
        out.append(trig_case(f"TDelay.{rate}", "TDelay", 0.0003, rate=rate, blocks=blocks, freq=freq))
        out.append(trig_case(f"ToggleFF.{rate}", "ToggleFF", rate=rate, blocks=blocks, freq=freq))
        out.append(trig_case(f"PulseCount.{rate}", "PulseCount", 0, rate=rate, blocks=blocks, freq=freq))
        out.append(trig_case(f"PulseDivider.{rate}", "PulseDivider", 3, 1, rate=rate, blocks=blocks, freq=freq))
        out.append(trig_case(f"Stepper.{rate}", "Stepper", 0, -2, 5, 2, 1, rate=rate, blocks=blocks, freq=freq))
        out.append(trig_case(f"Timer.{rate}", "Timer", rate=rate, blocks=blocks, freq=freq))
        out.append(trig_case(f"Sweep.{rate}", "Sweep", 3, rate=rate, blocks=blocks, freq=freq))
    for rate, blocks in [("ar", 8), ("kr", 64)]:
        g = G()
        x = sig(g, rate)
        t = g.add("Impulse", rate, 1700 if rate == "ar" else 37, 0)
        g.add("Latch", rate, x, t)
        out.append(case(f"Latch.{rate}", g, blocks))
        g = G()
        x = sig(g, rate)
        t = g.add("LFPulse", rate, 913.7 if rate == "ar" else 20.3, 0, 0.5)
        g.add("Gate", rate, x, t)
        out.append(case(f"Gate.{rate}", g, blocks))
        g = G()
        x = sig(g, rate)
        g.add("Schmidt", rate, x, -0.1, 0.2)
        out.append(case(f"Schmidt.{rate}", g, blocks))
        g = G()
        t = g.add("Impulse", rate, 1700 if rate == "ar" else 37, 0)
        r = g.add("Impulse", rate, 701.3 if rate == "ar" else 13.3, 0.5)
        g.add("SetResetFF", rate, t, r)
        out.append(case(f"SetResetFF.{rate}", g, blocks))
    out.append(trig_case("Line.trigger.TDelay", "TDelay", 0.001))
    return out


_PAN_LAW = {
    "tol": 6e-4,
    "why": "the pan law is a polynomial, scsynth's a table "
    "(docs/decisions/defs.md, 'The pan law is a polynomial, not a table')",
}


def _pans() -> list[dict]:
    out = []
    for rate, blocks in [("ar", 4), ("kr", 32)]:
        for kind in ["Pan2", "LinPan2"]:
            for chan in [0, 1]:
                g = G()
                x = sig(g, rate)
                p = g.add("LFTri", rate, 30 if rate == "ar" else 1, 0)
                g.add(kind, rate, x, p, 0.8, chan, sc={"inputs": [x, p, 0.8], "outputs": 2, "output": chan})
                out.append(case(f"{kind}.{rate}.{chan}", g, blocks, **(_PAN_LAW if kind == "Pan2" else {})))
        for chan in [0, 1]:
            g = G()
            x = sig(g, rate)
            y = g.add("LFSaw", rate, 211, 0)
            p = g.add("LFTri", rate, 30 if rate == "ar" else 1, 0)
            g.add("Balance2", rate, x, y, p, 0.8, chan, sc={"inputs": [x, y, p, 0.8], "outputs": 2, "output": chan})
            out.append(case(f"Balance2.{rate}.{chan}", g, blocks, **_PAN_LAW))
            # scsynth's Rotate2 reads its angle once a block whatever its
            # rate, so the angle is a control here.
            g = G()
            x = sig(g, rate)
            y = g.add("LFSaw", rate, 211, 0)
            p = g.add("LFTri", "kr", 3, 0)
            g.add("Rotate2", rate, x, y, p, chan, sc={"inputs": [x, y, p], "outputs": 2, "output": chan})
            out.append(
                case(
                    f"Rotate2.{rate}.{chan}", g, blocks, tol=1.5e-3,
                    why="scsynth's Rotate2 reads its sine table at a truncated index; the server computes it",
                )
            )  # fmt: skip
        for kind in ["XFade2", "LinXFade2"]:
            g = G()
            x = sig(g, rate)
            y = g.add("LFSaw", rate, 211, 0)
            p = g.add("LFTri", rate, 30 if rate == "ar" else 1, 0)
            # scsynth's LinXFade2 has no level input: sclang multiplies after.
            level = 0.8 if kind == "XFade2" else 1.0
            sc = {"inputs": [x, y, p, 0.8]} if kind == "XFade2" else {"inputs": [x, y, p]}
            g.add(kind, rate, x, y, p, level, sc=sc)
            out.append(case(f"{kind}.{rate}", g, blocks, **(_PAN_LAW if kind == "XFade2" else {})))
        for n, chan in [(2, 1), (4, 2), (5, 4)]:
            g = G()
            x = sig(g, rate)
            p = g.add("LFSaw", rate, 30 if rate == "ar" else 1, 0)
            g.add("PanAz", rate, x, p, 0.8, 2.5, 0.5, n, chan, sc={"inputs": [x, p, 0.8, 2.5, 0.5], "outputs": n, "output": chan})
            out.append(case(f"PanAz.{rate}.{n}.{chan}", g, blocks, **_PAN_LAW))
    for kind in ["Pan2", "LinPan2", "XFade2", "LinXFade2", "Balance2", "PanAz"]:
        # A control-rate position on an audio-rate pan: the gains move in a
        # line across each block.
        g = G()
        x = sig(g)
        p = g.add("LFTri", "kr", 7, 0)
        if kind in ("Pan2", "LinPan2"):
            g.add(kind, "ar", x, p, 1, 1, sc={"inputs": [x, p, 1], "outputs": 2, "output": 1})
        elif kind == "PanAz":
            g.add(kind, "ar", x, p, 1, 2, 0.5, 4, 1, sc={"inputs": [x, p, 1, 2, 0.5], "outputs": 4, "output": 1})
        elif kind == "Balance2":
            y = g.add("LFSaw", "ar", 211, 0)
            g.add(kind, "ar", x, y, p, 1, 0, sc={"inputs": [x, y, p, 1], "outputs": 2, "output": 0})
        else:
            y = g.add("LFSaw", "ar", 211, 0)
            g.add(kind, "ar", x, y, p, 1, sc={"inputs": [x, y, p, 1] if kind == "XFade2" else [x, y, p]})
        out.append(case(f"{kind}.krpos", g, blocks=16, **(_PAN_LAW if kind not in ("LinPan2", "LinXFade2") else {})))
    g = G()
    w = g.add("LFTri", "ar", 300, 0)
    w = g.add("MulAdd", "ar", w, 1.5, 1.5)
    a = sig(g)
    b = g.add("LFSaw", "ar", 211, 0)
    d = g.add("LFTri", "ar", 50, 0)
    g.add("Select", "ar", w, a, b, d)
    out.append(case("Select.ar", g))
    return out


def _smoothers() -> list[dict]:
    out = []
    for rate, blocks in [("ar", 6), ("kr", 64)]:
        g = G()
        x = g.add("LFPulse", rate, 301.3 if rate == "ar" else 3.7, 0, 0.4)
        g.add("LagUD", rate, x, 0.0004 if rate == "ar" else 0.02, 0.0015 if rate == "ar" else 0.07)
        out.append(case(f"LagUD.{rate}", g, blocks))
        # scsynth's VarLag reads its target once a block whatever its rate,
        # so the target is a control here.
        g = G()
        x = g.add("LFPulse", "kr", 31.3 if rate == "ar" else 3.7, 0, 0.4)
        g.add("VarLag", rate, x, 0.0011 if rate == "ar" else 0.05, -0.5)
        out.append(case(f"VarLag.{rate}", g, blocks))
    # A lag whose time moves.
    g = G()
    x = g.add("LFPulse", "ar", 301.3, 0, 0.4)
    t = g.add("LFTri", "kr", 30, 0)
    t = g.add("MulAdd", "kr", t, 0.0005, 0.001)
    g.add("Lag", "ar", x, t)
    out.append(case("Lag.timemod", g, 8))
    g = G()
    x = g.add("LFPulse", "kr", 3.7, 0, 0.4)
    t = g.add("LFTri", "kr", 1.3, 0)
    t = g.add("MulAdd", "kr", t, 0.02, 0.04)
    g.add("VarLag", "kr", x, t, 0)
    out.append(case("VarLag.timemod", g, 64))
    return out


def _table(n: int, channels: int = 1) -> list[float]:
    """A buffer's samples: a few unrelated partials, so every frame differs
    from its neighbours and an interpolation shows."""
    import math

    return [
        0.6 * math.sin(2 * math.pi * 3 * i / n + c) + 0.3 * math.sin(2 * math.pi * 7.3 * i / n) * (1 + c)
        for i in range(n)
        for c in range(channels)
    ]


def _buffers() -> list[dict]:
    out = []
    buf = [{"num": 0, "channels": 1, "frames": 300, "data": _table(300)}]
    stereo = [{"num": 1, "channels": 2, "frames": 200, "data": _table(200, 2)}]
    for rate in [1.0, 0.73, -1.37, 2.5]:
        for loop in [0, 1]:
            for chan, bufs, num, outs in [(0, buf, 0, 1), (1, stereo, 1, 2)]:
                g = G()
                start = 0 if rate > 0 else 150
                g.add(
                    "PlayBuf", "ar", num, chan, rate, loop, 0, start, 0,
                    sc={"inputs": [num, rate, 0, start, loop, 0], "outputs": outs, "output": chan},
                )  # fmt: skip
                out.append(case(f"PlayBuf.{rate}.{loop}.{chan}", g, blocks=8, buffers=bufs))
    # A retrigger mid-play.
    g = G()
    t = g.add("Impulse", "ar", 400, 0.5)
    g.add("PlayBuf", "ar", 0, 0, 0.9, 0, t, 40, 0, sc={"inputs": [0, 0.9, t, 40, 0, 0]})
    out.append(case("PlayBuf.trigger", g, blocks=8, buffers=buf))
    for interp in [1, 2, 4]:
        for loop in [0, 1]:
            g = G()
            p = g.add("LFSaw", "ar", 211, 0)
            p = g.add("MulAdd", "ar", p, 180, 140)
            g.add("BufRd", "ar", 0, 0, p, loop, interp, sc={"inputs": [0, p, loop, interp]})
            out.append(case(f"BufRd.{interp}.{loop}", g, buffers=buf))
    for kind in ["BufFrames", "BufChannels", "BufSampleRate", "BufRateScale", "BufDur"]:
        g = G()
        g.add(kind, "kr", 1)
        out.append(case(f"{kind}.kr", g, buffers=[dict(stereo[0], sr=44100.0)]))
    # A wavetable buffer: scsynth's format, written by `/buffer_gen sine1`
    # in either server; these frames are the format's pairs.
    import math

    n = 512
    sig = [math.sin(2 * math.pi * i / n) + 0.3 * math.sin(2 * math.pi * 5 * i / n) for i in range(n)]
    wt = []
    for i in range(n):
        a, b = sig[i], sig[(i + 1) % n]
        wt += [2 * a - b, b - a]
    table = [{"num": 2, "channels": 1, "frames": 2 * n, "data": wt}]
    for kind in ["Osc", "OscN"]:
        for freq in [441.3, -97.1]:
            g = G()
            g.add(kind, "ar", 2, freq, 0)
            out.append(
                case(
                    f"{kind}.{freq}", g, buffers=table if kind == "Osc" else [dict(table[0], data=sig, frames=n)],
                    tol=2e-4, why="scsynth's table oscillators keep their phase in 16.16 fixed point",
                )
            )  # fmt: skip
    g = G()
    ph = g.add("LFTri", "ar", 150, 0)
    g.add("Osc", "ar", 2, 300, ph)
    out.append(case("Osc.pm", g, buffers=table, tol=2e-4, why="scsynth's table oscillators keep their phase in 16.16 fixed point"))
    g = G()
    x = sig_ugen = None  # noqa: F841
    x = g.add("LFTri", "ar", 220, 0)
    g.add("Shaper", "ar", 2, x)
    out.append(case("Shaper.ar", g, buffers=table))
    return out


def _env(init: float, segments: list[tuple], release: int = -99, loop: int = -99) -> list[float]:
    """An envelope's inputs after the five header ones, in scsynth's layout:
    the initial level, the segment count, the release and loop nodes, then
    `(level, time, shape, curve)` per segment."""
    out = [init, len(segments), release, loop]
    for seg in segments:
        out += list(seg)
    return out


def _envelopes() -> list[dict]:
    out = []
    shapes = {"step": 0, "lin": 1, "exp": 2, "sin": 3, "wel": 4, "curve": 5, "sqr": 6, "cub": 7, "hold": 8}
    for name, shape in shapes.items():
        for rate, blocks, scale in [("ar", 6, 1.0), ("kr", 48, 20.0)]:
            lo = 0.01 if name == "exp" else 0.0
            env = _env(lo, [(1.0, 0.0021 * scale, shape, -3), (0.3, 0.0013 * scale, shape, 4), (lo, 0.0017 * scale, shape, 2)])
            g = G()
            g.add("EnvGen", rate, 1, 1, 0, 1, 0, *env)
            tol = {"tol": 1e-4, "why": "scsynth steps its shapes by recurrence; the server evaluates them"}
            out.append(case(f"EnvGen.{name}.{rate}", g, blocks, **tol))
    # Sustain at the release node, then a release when the gate falls; a
    # loop between the loop node and the release node; a done action.
    g = G()
    gate = g.add("LFPulse", "kr", 40, 0, 0.6)
    g.add("EnvGen", "ar", gate, 0.8, 0.1, 1, 0, *_env(0, [(1, 0.002, 1, 0), (0.5, 0.003, 3, 0), (0, 0.004, 5, -4)], release=1))
    out.append(case("EnvGen.release", g, 24, tol=1e-4, why="scsynth steps its shapes by recurrence"))
    g = G()
    gate = g.add("LFPulse", "kr", 12, 0, 0.8)
    g.add("EnvGen", "ar", gate, 1, 0, 1, 0, *_env(0, [(1, 0.001, 1, 0), (0.2, 0.002, 1, 0), (0.7, 0.0015, 1, 0), (0, 0.003, 1, 0)], release=2, loop=0))
    out.append(case("EnvGen.loop", g, 48, tol=1e-4, why="scsynth steps its shapes by recurrence"))
    g = G()
    g.add("EnvGen", "ar", 1, 1, 0, 0.5, 2, *_env(0, [(1, 0.004, 1, 0), (0, 0.004, 1, 0)]))
    out.append(case("EnvGen.done", g, 6))
    g = G()
    g.add("Line", "ar", 0, 1, 0.003, 2)
    g.add("DetectSilence", "ar", {"ugen": 0}, 0.0001, 0.001, 2)
    out.append(case("DetectSilence.done", g, 8, probe=0))
    return out


def _demand() -> list[dict]:
    out = []

    def pulled(name, *dugens, blocks=8, freq=3100.0, sc_dugens=None, **extra):
        """`Demand` pulling the last of `dugens` (built by the callable) on an
        impulse train."""
        g = G()
        t = g.add("Impulse", "ar", freq, 0)
        src = None
        for kind, inputs in dugens:
            src = g.add(kind, "dr", *[x(g) if callable(x) else x for x in inputs])
        g.add("Demand", "ar", t, 0, src)
        return case(name, g, blocks, **extra)

    out.append(pulled("Dseq", ("Dseq", [3, 1, 2.5, -1, 4])))
    out.append(pulled("Dseq.nested", ("Dseq", [2, 7, 8]), ("Dseq", [2, 1, {"ugen": 1}, 3])))
    out.append(pulled("Dseries", ("Dseries", [20, 2, 0.5])))
    out.append(pulled("Dgeom", ("Dgeom", [12, 1, 1.5])))
    out.append(pulled("Dstutter", ("Dseq", [10, 1, 2, 3]), ("Dstutter", [3, {"ugen": 1}])))
    out.append(pulled("Dswitch1", ("Dseq", [100, 0, 2, 1, 1]), ("Dswitch1", [{"ugen": 1}, 10, 20, 30])))
    buf = [{"num": 0, "channels": 1, "frames": 5, "data": [0.5, -1, 2, 3, 7]}]
    out.append(
        pulled("Dbufrd", ("Dseries", [30, 0, 1]), ("Dbufrd", [0, {"ugen": 1}, 1]), buffers=buf,)
    )  # fmt: skip
    # Demand with a reset.
    g = G()
    t = g.add("Impulse", "ar", 3100, 0)
    r = g.add("Impulse", "ar", 801.7, 0.5)
    d = g.add("Dseries", "dr", 1000, 0, 1)
    g.add("Demand", "ar", t, r, d)
    out.append(case("Demand.reset", g, 8))
    # Duty and TDuty: durations from a sequence, levels from another.
    for kind in ["Duty", "TDuty"]:
        for rate, blocks in [("ar", 8), ("kr", 96)]:
            g = G()
            scale = 1 if rate == "ar" else 30
            durs = g.add("Dseq", "dr", 100, 0.0003 * scale, 0.0005 * scale, 0.00071 * scale)
            lev = g.add("Dseq", "dr", 100, 1, 0.5, -0.25, 2)
            if kind == "Duty":
                g.add(kind, rate, durs, 0, lev, 0, sc={"inputs": [durs, 0, 0, lev]})
            else:
                g.add(kind, rate, durs, 0, lev, 0, 0, sc={"inputs": [durs, 0, 0, lev, 0]})
            out.append(case(f"{kind}.{rate}", g, blocks))
    # A sequence that ends: Duty's done action.
    g = G()
    durs = g.add("Dseq", "dr", 1, 0.0003, 0.0005)
    g.add("Duty", "ar", durs, 0, 1, 2, sc={"inputs": [durs, 0, 2, 1]})
    out.append(case("Duty.done", g, 8))
    return out


_PRIMED = "scsynth's constructor runs one sample of the calculation, so its stream starts a sample's draws in"


def _random() -> list[dict]:
    """The random UGens, drawn from scsynth's generator with the seed the
    harness and the server both seed their tables with: the same numbers."""
    out = []
    for kind in ["WhiteNoise", "PinkNoise", "BrownNoise", "GrayNoise", "ClipNoise"]:
        for rate, blocks in [("ar", 4), ("kr", 32)]:
            # BrownNoise's constructor draws its start and does not prime.
            primed = {} if kind == "BrownNoise" else {"shift": 1, "why": _PRIMED}
            out.append(one(f"{kind}.{rate}", kind, rate, blocks=blocks, seed=1956, **primed))
    for kind in ["LFNoise0", "LFNoise1", "LFNoise2", "LFClipNoise"]:
        for rate, blocks, freq in [("ar", 8, 2333.0), ("kr", 64, 41.0)]:
            out.append(one(f"{kind}.{rate}", kind, rate, freq, blocks=blocks, seed=77, shift=1, why=_PRIMED))
    for kind in ["Dust", "Dust2"]:
        out.append(one(f"{kind}.ar", kind, "ar", 3000, blocks=8, seed=5, shift=1, why=_PRIMED))
    out.append(one("Crackle.ar", "Crackle", "ar", 1.5, blocks=2, seed=9, shift=1, why=_PRIMED))
    # Drawn once: the constructor's draw is the value.
    out.append(one("Rand.ir", "Rand", "ir", -2, 3, seed=11))
    out.append(one("IRand.ir", "IRand", "ir", -5, 30, seed=12))
    out.append(one("ExpRand.ir", "ExpRand", "ir", 20, 2000, seed=13))
    out.append(one("LinRand.ir", "LinRand", "ir", 0, 10, 1, seed=14))
    out.append(one("NRand.ir", "NRand", "ir", -1, 1, 4, seed=15))
    for kind, lo, hi in [("TRand", -1, 1), ("TIRand", 0, 9), ("TExpRand", 100, 1000)]:
        for rate, blocks, freq in [("ar", 8, 1700.0), ("kr", 64, 37.0)]:
            g = G()
            t = g.add("Impulse", rate, freq, 0.5)
            g.add(kind, rate, lo, hi, t)
            out.append(case(f"{kind}.{rate}", g, blocks, seed=21))
    g = G()
    t = g.add("Impulse", "ar", 1700, 0.5)
    g.add("CoinGate", "ar", 0.4, t)
    out.append(case("CoinGate.ar", g, 8, seed=22))
    # RandSeed reseeds the generator at the start: the noise is the stream of
    # 1234, whatever the case's seed.
    g = G()
    g.add("RandSeed", "ir", 1, 1234)
    g.add("WhiteNoise", "ar")
    out.append(case("RandSeed.ir", g, 4, seed=23, shift=1, why=_PRIMED))
    # RandID: the noise draws from generator 5, seeded with the seed plus 5.
    g = G()
    g.add("RandID", "ir", 5)
    g.add("WhiteNoise", "ar")
    out.append(case("RandID.ir", g, 4, seed=24, shift=1, why=_PRIMED))
    return out


def _sections() -> list[dict]:
    """scsynth's small sections and the two-pole rows with named parameters."""
    out = []
    for kind in ["LPZ1", "HPZ1", "LPZ2", "HPZ2", "BPZ2", "BRZ2", "Slope", "Flip"]:
        for rate, blocks in [("ar", 4), ("kr", 32)]:
            out.append(through(f"{kind}.{rate}", kind, rate, blocks=blocks, source=lambda g, r=rate: sig(g, r)))
    out.append(through("Delay1.ar", "Delay1", "ar", 0.3))
    out.append(through("Delay2.ar", "Delay2", "ar", 0.3, -0.2))
    for kind, params in [
        ("TwoPole", [800, 0.95]), ("TwoZero", [800, 0.9]), ("Ringz", [1200, 0.01]),
        ("Formlet", [900, 0.002, 0.01]), ("MidEQ", [1000, 0.5, 6]),
        ("BLowPass", [1500, 0.7]), ("BHiPass", [1500, 0.7]), ("BAllPass", [1500, 0.7]),
        ("BBandPass", [1500, 1.2]), ("BBandStop", [1500, 1.2]), ("BPeakEQ", [1500, 0.7, -9]),
        ("BLowShelf", [600, 1, 6]), ("BHiShelf", [3000, 1, -6]),
    ]:  # fmt: skip
        loose = {"tol": 5e-5, "why": "a resonance near a gain of ten amplifies the two realizations' rounding"}
        out.append(through(f"{kind}.sig", kind, "ar", *params, **(loose if kind == "TwoPole" else {})))
        out.append(through(f"{kind}.imp", kind, "ar", *params, source=impulse, blocks=8))
    out.append(through("FOS.ar", "FOS", "ar", 0.5, 0.3, 0.6))
    out.append(through("SOS.ar", "SOS", "ar", 0.3, 0.2, 0.1, 1.2, -0.5))
    out.append(through("Ramp.ar", "Ramp", "ar", 0.0007, blocks=8))
    out.append(through("Slew.ar", "Slew", "ar", 400, 900))
    # Not Lag3UD: scsynth's third stage reads the second's comparison, a
    # copy of the line above it, where the server's reads its own.
    for kind, params in [("Lag2", [0.001]), ("Lag3", [0.001]), ("Lag2UD", [0.0005, 0.002])]:
        out.append(through(f"{kind}.ar", kind, "ar", *params, blocks=6))
    return out


def _lfugens() -> list[dict]:
    """`LFUGens.cpp`: the rest of the shapes, the modulators, the envelopes
    read at a position, the maps and the rate converters."""
    out = []
    # The server's initial phase is in cycles; scsynth's LFCub runs two units
    # a cycle and its LFPar four.
    for kind, units in [("LFCub", 2), ("LFPar", 4)]:
        for rate, blocks, f in [("ar", 4, 441.3), ("kr", 64, 7.7)]:
            kw = {"tol": 1e-5, "why": "scsynth accumulates a control-rate phase in f32"} if rate == "kr" else {}
            out.append(one(f"{kind}.{rate}", kind, rate, f, 0, blocks=blocks, **kw))
            out.append(one(f"{kind}.{rate}.phase", kind, rate, f, 0.15, blocks=blocks, sc={"inputs": [f, 0.15 * units]}, **kw))
        g = G()
        f = g.add("LFSaw", "ar", 50, 0)
        f = g.add("MulAdd", "ar", f, 2000, 2400)
        g.add(kind, "ar", f, 0)
        out.append(case(f"{kind}.fm", g, blocks=8))
    out.append(one("SyncSaw.ar", "SyncSaw", "ar", 301.3, 977.1, blocks=6, shift=1, why=_PRIMED, tol=1e-5))
    # Not a modulated SyncSaw: scsynth's priming steps the phases once with
    # the first sample's rates, so against a moving rate no shift lines the
    # two up.
    # Vibrato: no delay and no onset, which scsynth's priming shortens by a
    # sample. scsynth reads `freq` as a buffer of the block, so a constant
    # there is read past its one value: its own documentation feeds it a
    # `DC.ar`, and so do these.
    for rate, blocks in [("ar", 8), ("kr", 64)]:
        g = G()
        f = g.add("DC", rate, 440)
        g.add("Vibrato", rate, f, 230 if rate == "ar" else 23, 0.05, 0, 0, 0.2, 0.3, 0.1, 0)
        out.append(case(f"Vibrato.{rate}", g, blocks, seed=31, tol=1e-5))
    # A trigger that starts low, so scsynth's priming draws nothing. Without
    # a delay or an onset: they are the start's too, which the priming
    # shortens (`tests/offline/lfo.rs` holds them).
    g = G()
    f = g.add("DC", "ar", 440)
    t = g.add("LFPulse", "kr", 60, 0.6, 0.5)
    g.add("Vibrato", "ar", f, 170, 0.1, 0, 0, 0.3, 0.5, 0.6, t)
    out.append(case("Vibrato.trig", g, blocks=24, seed=32, tol=1e-5))
    for loop in [1, 0]:
        out.append(one(f"LFGauss.loop{loop}", "LFGauss", "ar", 0.002, 0.3, 0, loop, 0, blocks=6))
    out.append(one("LFGauss.phase", "LFGauss", "ar", 0.0031, 0.2, 0.4, 1, 0, blocks=6))
    out.append(one("LFGauss.kr", "LFGauss", "kr", 0.11, 0.25, 0, 1, 0, blocks=64))
    # IEnvGen: an index sweeping past both ends of the envelope. Integer
    # curvatures: scsynth truncates a curvature to an integer.
    shapes = {"step": 0, "lin": 1, "exp": 2, "sin": 3, "wel": 4, "curve": 5, "sqr": 6, "cub": 7}
    for name, shape in shapes.items():
        lo = 0.05 if name == "exp" else 0.0
        segs = [(0.3, shape, -3, 1.0), (0.2, shape, 4, 0.25), (0.5, shape, 2, lo)]
        env = [lo, len(segs), sum(d for d, *_ in segs)]
        for seg in segs:
            env += list(seg)
        for rate, blocks, f in [("ar", 4, 211.3), ("kr", 48, 9.3)]:
            g = G()
            x = g.add("LFSaw", rate, f, 0)
            x = g.add("MulAdd", rate, x, 0.7, 0.45)
            g.add("IEnvGen", rate, x, -0.1, *env)
            out.append(case(f"IEnvGen.{name}.{rate}", g, blocks))
    # Linen: control rate, a gate that opens after the start (so scsynth's
    # priming finds it closed) and stays open longer than the rise.
    g = G()
    gate = g.add("LFPulse", "kr", 3, 0.7, 0.6)
    g.add("Linen", "kr", gate, 0.05, 0.7, 0.04, 0)
    out.append(case("Linen.kr", g, blocks=600))
    # The maps.
    g = G()
    x = sig(g)
    g.add("LinExp", "ar", x, -1, 1, 30, 9000)
    out.append(case("LinExp.ar", g))
    for kind, params in [("InRange", [-0.2, 0.3]), ("Unwrap", [-1, 1])]:
        out.append(through(f"{kind}.ar", kind, "ar", *params))
    g = G()
    x = g.add("LFSaw", "ar", 997, 0)
    y = g.add("LFTri", "ar", 130.8, 0)
    g.add("InRect", "ar", x, y, -0.5, -0.2, 0.4, 0.6)
    out.append(case("InRect.ar", g))
    for rate in ["ar", "kr"]:
        g = G()
        x = g.add("LFSaw", "ar", 997, 0)
        y = g.add("LFTri", rate, 130.8 if rate == "ar" else 13.08, 0)
        m = g.add("MulAdd", rate, y, 0.2, 0.9)
        g.add("ModDif", "ar", x, y, m)
        out.append(case(f"ModDif.{rate}", g))
    # Unwrap of a wrapped ramp: a saw read back as the line it wraps.
    g = G()
    x = g.add("LFSaw", "ar", 1201.3, 0)
    g.add("Unwrap", "ar", x, -1, 1)
    out.append(case("Unwrap.saw", g))
    for kind, params in [("AmpComp", [300, 0.3333]), ("AmpCompA", [50, 0.32, 1])]:
        for rate in ["ar", "kr"]:
            g = G()
            f = g.add("LFSaw", rate, 230 if rate == "ar" else 13, 0)
            f = g.add("MulAdd", rate, f, 7000, 7100)
            g.add(kind, rate, f, *params)
            out.append(case(f"{kind}.{rate}", g, 4 if rate == "ar" else 64))
    # The rate converters.
    g = G()
    x = g.add("LFSaw", "kr", 31, 0)
    g.add("K2A", "ar", x)
    out.append(case("K2A.ar", g, 16))
    for kind, src in [("A2K", ("LFSaw", 997, 0)), ("T2K", ("Impulse", 1700, 0))]:
        g = G()
        x = g.add(src[0], "ar", *src[1:])
        g.add(kind, "kr", x)
        out.append(case(f"{kind}.kr", g, 64))
    g = G()
    t = g.add("LFPulse", "kr", 300, 0, 0.5)
    t = g.add("MulAdd", "kr", t, 0.75, 0)
    g.add("T2A", "ar", t, 5)
    out.append(case("T2A.ar", g, 32))
    for rate in ["ar", "kr"]:
        out.append(one(f"DC.{rate}", "DC", rate, 0.37))
    return out


def _wavetables(*harmonics: tuple, first: int = 2) -> list[dict]:
    """Wavetable buffers from `first` on, scsynth's format: one per tuple of
    harmonic amplitudes, 512 points each."""
    import math

    n = 512
    out = []
    for k, amps in enumerate(harmonics):
        sig = [sum(a * math.sin(2 * math.pi * (h + 1) * i / n) for h, a in enumerate(amps)) for i in range(n)]
        wt = []
        for i in range(n):
            a, b = sig[i], sig[(i + 1) % n]
            wt += [2 * a - b, b - a]
        out.append({"num": first + k, "channels": 1, "frames": 2 * n, "data": wt})
    return out


def _oscugens() -> list[dict]:
    """`OscUGens.cpp`'s oscillators."""
    import math

    out = []
    fixed = "scsynth's table oscillators keep their phase in 16.16 fixed point"
    # FSinOsc: the recurrence, scsynth's; its initial phase is in radians.
    for rate, blocks, f in [("ar", 8, 441.3), ("kr", 64, 7.7)]:
        out.append(one(f"FSinOsc.{rate}", "FSinOsc", rate, f, 0.1, blocks=blocks, sc={"inputs": [f, 0.2 * math.pi]}))
    g = G()
    f = g.add("LFPulse", "kr", 30, 0, 0.5)
    f = g.add("MulAdd", "kr", f, 300, 500)
    g.add("FSinOsc", "ar", f, 0)
    out.append(case("FSinOsc.step", g, 24))
    # SinOscFB: computed against scsynth's 8192-point table. Not a feedback
    # above 1: there the loop's gain passes 1 over part of the cycle and
    # amplifies any rounding, so two correct computations part.
    for fb in [0.0, 0.5, 0.95]:
        out.append(one(f"SinOscFB.{fb}", "SinOscFB", "ar", 331.7, fb, blocks=8, tol=5e-5, why="scsynth's SinOscFB reads an 8192-point table"))
    g = G()
    fb = g.add("LFTri", "kr", 9, 0)
    fb = g.add("MulAdd", "kr", fb, 0.4, 0.5)
    g.add("SinOscFB", "ar", 231.7, fb)
    out.append(case("SinOscFB.mod", g, 16, tol=2e-5, why="scsynth's SinOscFB reads an 8192-point table"))
    out.append(one("PSinGrain.ar", "PSinGrain", "ar", 1500, 0.004, 0.5, blocks=6, tol=2e-5, why="scsynth runs the grain's recurrence in f32"))
    # Blip: the Dirichlet kernel against scsynth's sine and cosecant tables.
    tables = "scsynth reads Blip's kernel from interpolated sine and cosecant tables"
    for f, n in [(201.3, 10), (1201.7, 200), (97.1, 1)]:
        out.append(one(f"Blip.{f}.{n}", "Blip", "ar", f, n, blocks=8, tol=4e-3, why=tables))
    g = G()
    n = g.add("LFPulse", "kr", 40, 0, 0.5)
    n = g.add("MulAdd", "kr", n, 6, 4)
    g.add("Blip", "ar", 301.3, n)
    out.append(case("Blip.crossfade", g, 16, tol=4e-3, why=tables))
    # Formant: the first period only. scsynth restarts each burst at a
    # phase its 32-bit product overflows; the server's restart is held by
    # `tests/offline/oscugens.rs`.
    out.append(one("Formant.ar", "Formant", "ar", 100, 1300, 700, blocks=7, tol=2e-3, why="scsynth reads Formant's sines from its table without interpolation"))
    out.append(one("Formant.wide", "Formant", "ar", 100, 2100, 50, blocks=7, tol=2e-3, why="scsynth reads Formant's sines from its table without interpolation"))
    # The wavetable oscillators.
    tables = _wavetables((1.0, 0.0, 0.3), (0.5, 0.5, 0.0, 0.25), (0.2, 0.0, 0.0, 0.0, 0.6))
    out.append(one("COsc.ar", "COsc", "ar", 2, 300.7, 3.3, blocks=8, buffers=tables, tol=4e-4, why=fixed))
    for pos in [2.0, 2.4, 3.7]:
        out.append(one(f"VOsc3.{pos}", "VOsc3", "ar", pos, 101.3, 202.9, 303.1, blocks=8, buffers=tables, tol=6e-4, why=fixed))
    g = G()
    pos = g.add("LFTri", "kr", 11, 0)
    pos = g.add("MulAdd", "kr", pos, 0.9, 3.0)
    g.add("VOsc3", "ar", pos, 101.3, 202.9, 303.1)
    out.append(case("VOsc3.sweep", g, 32, buffers=tables, tol=2e-3, why=fixed + ", and segments a table sweep by rounded counts"))
    g = G()
    pos = g.add("LFTri", "kr", 11, 0)
    pos = g.add("MulAdd", "kr", pos, 0.9, 3.0)
    g.add("VOsc", "ar", pos, 101.3, 0)
    out.append(case("VOsc.sweep", g, 32, buffers=tables, tol=1e-3, why=fixed + ", and segments a table sweep by rounded counts"))
    # Klang: phases in cycles, scsynth's in radians.
    spec = [(300.0, 0.3, 0.0), (451.7, 0.2, 0.15), (1203.1, 0.1, 0.6)]
    ins, sc_ins = [1.1, 7.0], [1.1, 7.0]
    for f, a, p in spec:
        ins += [f, a, p]
        sc_ins += [f, a, 2 * math.pi * p]
    out.append(one("Klang.ar", "Klang", "ar", *ins, blocks=8, sc={"inputs": sc_ins}, tol=2e-4, why="scsynth runs Klang's recurrences in f32, its coefficients rounded to f32"))
    # Klank, excited by impulses and by a broadband signal.
    spec = [800.0, 0.3, 0.05, 1231.7, 0.2, 0.02, 2900.0, 0.1, 0.004]
    out.append(through("Klank.imp", "Klank", "ar", 1.0, 0.0, 1.0, *spec, source=impulse, blocks=8, tol=2e-5, why="scsynth runs Klank's resonators in f32"))
    out.append(through("Klank.sig", "Klank", "ar", 0.9, 13.0, 0.7, *spec, blocks=8, tol=5e-5, why="scsynth runs Klank's resonators in f32"))
    return out


def _tables() -> list[dict]:
    """A buffer read as a table; `TWindex`; the small noises."""
    out = []
    data = [0.5, -1.0, 2.0, 3.0, 7.0, 1.5]
    table = [{"num": 0, "channels": 1, "frames": len(data), "data": data}]

    def indexed(name, kind, mul, add, *extra, buffers=table, blocks=4):
        g = G()
        x = g.add("LFSaw", "ar", 331.7, 0)
        x = g.add("MulAdd", "ar", x, mul, add)
        g.add(kind, "ar", 0, x, *extra)
        return case(name, g, blocks, buffers=buffers)

    for kind in ["Index", "WrapIndex", "FoldIndex"]:
        out.append(indexed(f"{kind}.ar", kind, 9, 2.5))
    # Not below 0: scsynth interpolates a negative index by its fraction.
    out.append(indexed("IndexL.ar", "IndexL", 3.2, 3.3))
    rising = [{"num": 0, "channels": 1, "frames": 6, "data": [0.0, 1.0, 2.5, 3.0, 7.0, 9.0]}]
    out.append(indexed("IndexInBetween.ar", "IndexInBetween", 5.5, 4.5, buffers=rising))
    # Not below -6: scsynth reads past the table at a whole negative octave.
    scale = [{"num": 0, "channels": 1, "frames": 7, "data": [0.0, 2.0, 4.0, 5.0, 7.0, 9.0, 11.0]}]
    out.append(indexed("DegreeToKey.ar", "DegreeToKey", 8, 2, 12, buffers=scale))
    g = G()
    x = g.add("LFSaw", "ar", 331.7, 0)
    x = g.add("MulAdd", "ar", x, 4, 4)
    v = g.add("Index", "ar", 1, x)
    g.add("DetectIndex", "ar", 0, v)
    probe = [table[0], {"num": 1, "channels": 1, "frames": 8, "data": [2.0, 5.0, 7.0, 0.5, -1.0, 4.0, 1.5, 3.0]}]
    out.append(case("DetectIndex.ar", g, 4, buffers=probe))
    for name, norm, weights in [("TWindex.ar", 0, [0.1, 0.5, 0.4]), ("TWindex.norm", 1, [1.0, 3.0, 2.0])]:
        g = G()
        t = g.add("Impulse", "ar", 3000, 0.5)
        g.add("TWindex", "ar", t, norm, *weights)
        out.append(case(name, g, 8, seed=41))
    g = G()
    t = g.add("Impulse", "kr", 300, 0.5)
    g.add("TWindex", "kr", t, 1, 2.0, 1.0, 1.0, 4.0)
    out.append(case("TWindex.kr", g, 64, seed=42))
    # Hasher is chaotic in its input's bits, so its input is one both compute
    # exactly: a phasor stepping by a power of two.
    g = G()
    x = g.add("Phasor", "ar", 0, 0.125, -5, 5, 0)
    g.add("Hasher", "ar", x)
    out.append(case("Hasher.ar", g))
    for bits in [3, 10]:
        out.append(through(f"MantissaMask.{bits}", "MantissaMask", "ar", bits))
    out.append(one("Logistic.ar", "Logistic", "ar", 3.7, 1200, 0.3, blocks=8, shift=40, why="scsynth's constructor iterates the map once before the first block"))
    out.append(one("Logistic.full", "Logistic", "ar", 3.9, 48000, 0.2, blocks=4, shift=1, why=_PRIMED, tol=1e-4))
    for kind in ["LFDNoise0", "LFDNoise1", "LFDNoise3", "LFDClipNoise"]:
        for rate, blocks, freq in [("ar", 8, 2333.0), ("kr", 64, 41.0)]:
            out.append(one(f"{kind}.{rate}", kind, rate, freq, blocks=blocks, seed=78, shift=1, why=_PRIMED))
        # Not a modulated rate: scsynth's priming reads the first sample's
        # rate once more, so against a moving one no shift lines them up
        # (`tests/offline/tables.rs` holds a change of rate).
    return out


def _chaos() -> list[dict]:
    """`ChaosUGens.cpp`'s maps and `GendynUGens.cpp`'s Gendy."""
    out = []
    maps = {
        "Cusp": ("NL", [1.0, 1.9, 0.0]),
        "Gbman": ("NL", [1.2, 2.1]),
        "Latoocarfian": ("NLC", [1.0, 3.0, 0.5, 0.5, 0.5, 0.5]),
        "LinCong": ("NLC", [1.1, 0.13, 1.0, 1.0]),
        "Lorenz": ("L", [10.0, 28.0, 2.667, 0.05, 0.1, 0.0, 0.0]),
        "Quad": ("NLC", [-1.0, 0.0, 0.75, 0.0]),
        "Standard": ("NL", [1.0, 0.5, 0.0]),
        "FBSine": ("NLC", [1.0, 0.1, 1.1, 0.5, 0.1, 0.1]),
        "Henon": ("C", [1.4, 0.3, 0.0, 0.0]),
    }
    # The cubic forms and LinCongL at the full rate only: their first period
    # starts, in scsynth, from a state that is not the map's (LinCong's at an
    # `xi` that is its own scaled value). Not HenonN or HenonL: scsynth's
    # iterate with the initial values exchanged. `tests/offline/chaos.rs`
    # holds the server's.
    for name, (forms, params) in maps.items():
        for form in forms:
            kind = name + form
            full = form == "C" or kind == "LinCongL"
            freqs = [48000.0] if full else [48000.0, 4000.0, 1234.5]
            for f in freqs:
                chaotic = name in ("Cusp", "Latoocarfian", "Lorenz", "Standard", "FBSine", "Henon")
                why = _PRIMED + ("; a chaotic map amplifies two correct computations' rounding" if chaotic else "")
                kw = {"tol": 1e-4} if chaotic else {}
                out.append(one(f"{kind}.{f}", kind, "ar", f, *params, blocks=4, shift=1, why=why, **kw))
    out.append(one("Gendy1.ar", "Gendy1", "ar", 1, 1, 1.0, 1.0, 440, 660, 0.5, 0.5, 12, 12, blocks=8, seed=51, tol=1e-4, why="Xenakis's distributions in f32 libm against glibc's"))
    for which in range(7):
        out.append(one(f"Gendy1.dist{which}", "Gendy1", "ar", which, which, 0.6, 0.4, 900, 1500, 0.3, 0.3, 8, 8, blocks=8, seed=52 + which, tol=1e-4, why="Xenakis's distributions in f32 libm against glibc's"))
    # Gendy2 with the walks' scales at 0: scsynth reflects a level below -1
    # out of the range, so the walk itself cannot be compared.
    out.append(one("Gendy2.ar", "Gendy2", "ar", 1, 1, 1.0, 1.0, 440, 660, 0.0, 0.0, 12, 12, 1.17, 0.31, blocks=8, seed=61))
    return out


def _dynamics() -> list[dict]:
    """`FilterUGens.cpp`'s dynamics, `Median`, `MoogFF` and the analytic
    signal."""
    out = []
    for n in [1, 3, 7, 31]:
        g = G()
        x = sig(g)
        g.add("Median", "ar", n, x)
        out.append(case(f"Median.{n}", g))
    for rate in ["ar", "kr"]:
        g = G()
        x = sig(g, rate)
        g.add("Amplitude", rate, x, 0.002, 0.02)
        out.append(case(f"Amplitude.{rate}", g, 4 if rate == "ar" else 64))
    # Not at control rate over an audio input: scsynth's times there come
    # out a block's samples longer (`tests/offline/dynamics.rs`).
    for name, below, above in [("compress", 1.0, 0.3), ("expand", 3.0, 1.0), ("both", 0.5, 0.5)]:
        g = G()
        x = sig(g)
        g.add("Compander", "ar", x, x, 0.3, below, above, 0.002, 0.03)
        out.append(case(f"Compander.{name}", g, 8, skip=64, why="scsynth's first block fades in from a gain of 0"))
    # A signal whose every window peaks at 1: scsynth's windows fall a
    # sample later, which then changes nothing.
    for kind, level in [("Normalizer", 0.6), ("Limiter", 0.4), ("Limiter", 2.0)]:
        g = G()
        x = g.add("LFTri", "ar", 1000, 0)
        g.add(kind, "ar", x, level, 0.01)
        out.append(case(f"{kind}.{level}", g, 24, tol=3e-5, why="scsynth steps the gain in f32, which drifts from its aim by the rounding"))
    for freq, gain in [(800, 0.5), (2500, 3.5), (300, 2.0)]:
        out.append(through(f"MoogFF.{freq}", "MoogFF", "ar", freq, gain, 0))
        out.append(through(f"MoogFF.imp.{freq}", "MoogFF", "ar", freq, gain, 0, source=impulse, blocks=8))
    g = G()
    x = sig(g)
    k = g.add("LFTri", "kr", 9, 0)
    k = g.add("MulAdd", "kr", k, 1.8, 2.0)
    g.add("MoogFF", "ar", x, 1200, k, 0)
    out.append(case("MoogFF.gain", g, 16))
    for chan in [0, 1]:
        g = G()
        x = sig(g)
        g.add("Hilbert", "ar", x, chan, sc={"inputs": [x], "outputs": 2, "output": chan})
        out.append(case(f"Hilbert.{chan}", g, 8, tol=2e-5, why="scsynth's Hilbert runs its allpasses in f32"))
    # scsynth's oscillator runs a sample ahead: the server's phase is given
    # that sample.
    for f in [300.0, -721.3]:
        g = G()
        x = sig(g)
        g.add("FreqShift", "ar", x, f, f / 48000.0, sc={"inputs": [x, f, 0]})
        out.append(case(f"FreqShift.{f}", g, 8, tol=2e-5, why="scsynth reads its quadrature oscillator from a table"))
    return out


def _running() -> list[dict]:
    """`TriggerUGens.cpp`'s running statistics and `RunningSum`."""
    out = []
    for kind in ["Peak", "RunningMax", "RunningMin"]:
        for rate, trig_rate, blocks in [("ar", "ar", 8), ("ar", "kr", 8), ("kr", "kr", 64)]:
            g = G()
            x = sig(g, rate)
            t = g.add("Impulse", trig_rate, 703.1 if trig_rate == "ar" else 31, 0.5)
            g.add(kind, rate, x, t)
            out.append(case(f"{kind}.{rate}.{trig_rate}", g, blocks))
        g = G()
        x = sig(g)
        g.add(kind, "ar", x, 0)
        out.append(case(f"{kind}.notrig", g, 4))
    # Control rate over an audio input: scsynth's Peak alone sees the block,
    # and its SIMD form (the one it runs) starts over at the block's end.
    for trig_rate in ["ar", "kr"]:
        g = G()
        x = sig(g)
        t = g.add("Impulse", trig_rate, 310 if trig_rate == "ar" else 31, 0.5)
        g.add("Peak", "kr", x, t)
        out.append(case(f"Peak.atok.{trig_rate}", g, 64))
    for decay in [0.99, 0.9995]:
        out.append(through(f"PeakFollower.{decay}", "PeakFollower", "ar", decay))
    gathered = {"tol": 3e-5, "why": "a sum of many samples gathers the inputs' rounding"}
    out.append(through("RunningSum.40", "RunningSum", "ar", 40, blocks=8, **gathered))
    out.append(through("RunningSum.7", "RunningSum", "ar", 7, blocks=8))
    for delta in [0.01, 0.3]:
        out.append(through(f"LastValue.{delta}", "LastValue", "ar", delta))
    for kind in ["MostChange", "LeastChange"]:
        g = G()
        a = sig(g)
        b = g.add("LFTri", "ar", 310.7, 0)
        g.add(kind, "ar", a, b)
        out.append(case(f"{kind}.ar", g))
    for f in [441.3, 2003.7]:
        g = G()
        x = g.add("Sine", "ar", f, sc={"kind": "SinOsc", "inputs": [f, 0]})
        g.add("ZeroCrossing", "ar", x)
        out.append(case(f"ZeroCrossing.{f}", g, 8, tol=1e-4, why="scsynth's SinOsc reads a table, which moves the crossings"))
    return out


def _info() -> list[dict]:
    """The engine's timing facts. The bus and node counts are each server's
    configuration, held by `tests/offline/info.rs`."""
    out = []
    for kind in ["BlockSize", "ControlRate", "ControlDur", "SampleDur", "RadiansPerSample", "SubsampleOffset"]:
        # scsynth defines these at the initial rate only.
        out.append(one(f"{kind}.ir", kind, "ir", blocks=2))
    buf = [{"num": 0, "channels": 2, "frames": 37, "data": [0.0] * 74}]
    out.append(one("BufSamples.kr", "BufSamples", "kr", 0, blocks=2, buffers=buf))
    return out


def _delay_lines() -> list[dict]:
    """`DelayUGens.cpp`'s lines beyond the nine: `Pluck`, `PitchShift`, the
    tap pair."""
    out = []
    for name, delay, decay, coef in [("a", 0.0123, 1.5, 0.3), ("b", 0.00517, -0.4, -0.6), ("c", 0.002, 3.0, 0.9)]:
        g = G()
        x = sig(g)
        g.add("Pluck", "ar", x, 1, 0.02, delay, decay, coef)
        out.append(case(f"Pluck.{name}", g, 32))
    # A plucked string retriggered, its delay and coefficient moving.
    g = G()
    x = sig(g)
    t = g.add("Impulse", "ar", 90, 0.5)
    d = g.add("LFTri", "kr", 7, 0)
    d = g.add("MulAdd", "kr", d, 0.002, 0.006)
    k = g.add("LFTri", "kr", 3, 0)
    k = g.add("MulAdd", "kr", k, 0.4, 0.1)
    g.add("Pluck", "ar", x, t, 0.01, d, 0.8, k)
    out.append(case("Pluck.moving", g, 64))
    for ratio in [0.5, 1.0, 1.5, 2.0, 3.7]:
        g = G()
        x = sig(g)
        g.add("PitchShift", "ar", x, 0.031, ratio, 0, 0)
        out.append(case(f"PitchShift.{ratio}", g, 24, tol=2e-5, why="scsynth steps its heads' windows in f32"))
    buf = [{"num": 0, "channels": 1, "frames": 1000, "data": [0.0] * 1000}]
    for interp in [1, 2, 4]:
        for rate in ["dc", "kr"] if interp == 1 else ["dc", "ar", "kr"]:
            g = G()
            x = sig(g)
            w = g.add("DelTapWr", "ar", 0, x)
            extra = {}
            if rate == "dc":
                # A position that crosses no whole sample: one bit of the
                # time either side of one moves a truncating read a sample.
                t = g.add("DC", "ar", 0.0031234)
            elif rate == "ar":
                t = g.add("LFTri", "ar", 50, 0)
                t = g.add("MulAdd", "ar", t, 0.0011, 0.0031)
                extra = {"tol": 3e-5, "why": "the time signal's last bit, which a saw's jump between the samples read magnifies"}
            else:
                t = g.add("LFTri", "kr", 9, 0)
                t = g.add("MulAdd", "kr", t, 0.0011, 0.0031)
                extra = {"tol": 6e-4, "why": "scsynth steps a control-rate delay across the block in f32, whose drift a saw's jump magnifies"}
            g.add("DelTapRd", "ar", 0, w, t, interp)
            out.append(case(f"DelTapRd.{interp}.{rate}", g, 8, buffers=buf, **extra))
    return out


def _local_buffers() -> list[dict]:
    """A synth's own buffers, filled by `SetBuf` and read as a table."""
    out = []
    values = [0.5, -1.0, 2.0, 3.0, 7.0, 1.5]
    for channels in [1, 2]:
        g = G()
        g.add("MaxLocalBufs", "ir", 1)
        b = g.add("LocalBuf", "ir", channels, len(values) // channels)
        g.add("SetBuf", "ir", b, 0, len(values), *values)
        x = g.add("LFSaw", "ar", 331.7, 0)
        x = g.add("MulAdd", "ar", x, 4, 2.5)
        g.add("Index", "ar", b, x)
        out.append(case(f"LocalBuf.{channels}", g))
    # An offset, and values past the buffer's end left out.
    g = G()
    g.add("MaxLocalBufs", "ir", 1)
    b = g.add("LocalBuf", "ir", 1, 4)
    g.add("SetBuf", "ir", b, 2, 4, 9.0, 8.0, 7.0, 6.0)
    x = g.add("LFSaw", "ar", 331.7, 0)
    x = g.add("MulAdd", "ar", x, 3, 2)
    g.add("Index", "ar", b, x)
    out.append(case("SetBuf.offset", g))
    return out


_TABLE_ANGLE = "scsynth reads its sine table at a truncated index; the server computes the angle"


def _spaces() -> list[dict]:
    """`PanUGens.cpp`'s square and Ambisonics, `ReverbUGens.cpp`'s reverbs and
    `PhysicalModelingUGens.cpp`'s models."""
    out = []
    # Inside the square, moving, and outside it on each side.
    for name, xs, ys in [("moving", None, None), ("right", 1.6, 0.7), ("back", -0.4, -2.0)]:
        for chan in range(4):
            g = G()
            x = sig(g)
            if xs is None:
                px = g.add("LFTri", "kr", 3, 0)
                py = g.add("LFSaw", "kr", 2, 0)
            else:
                px, py = xs, ys
            g.add("Pan4", "ar", x, px, py, 0.8, chan, sc={"inputs": [x, px, py, 0.8], "outputs": 4, "output": chan})
            out.append(case(f"Pan4.{name}.{chan}", g, 16 if xs is None else 4, **_PAN_LAW))
    for chan in range(4):
        g = G()
        x = sig(g)
        az = g.add("LFSaw", "kr", 2, 0)
        el = g.add("LFTri", "kr", 3, 0)
        g.add("PanB", "ar", x, az, el, 0.7, chan, sc={"inputs": [x, az, el, 0.7], "outputs": 4, "output": chan})
        out.append(case(f"PanB.{chan}", g, 16, tol=1.5e-3, why=_TABLE_ANGLE))
    for chan in range(3):
        g = G()
        x = sig(g)
        az = g.add("LFSaw", "kr", 2, 0)
        g.add("PanB2", "ar", x, az, 0.7, chan, sc={"inputs": [x, az, 0.7], "outputs": 3, "output": chan})
        out.append(case(f"PanB2.{chan}", g, 16, tol=1.5e-3, why=_TABLE_ANGLE))
        g = G()
        x = sig(g)
        y = g.add("LFSaw", "ar", 211, 0)
        az = g.add("LFSaw", "kr", 2, 0)
        g.add("BiPanB2", "ar", x, y, az, 0.7, chan, sc={"inputs": [x, y, az, 0.7], "outputs": 3, "output": chan})
        out.append(case(f"BiPanB2.{chan}", g, 16, tol=1.5e-3, why=_TABLE_ANGLE))
    for n, orient in [(4, 0.5), (5, 0.0)]:
        for chan in range(n):
            g = G()
            w = sig(g)
            x = g.add("LFSaw", "ar", 211, 0)
            y = g.add("LFTri", "ar", 97, 0)
            g.add("DecodeB2", "ar", w, x, y, orient, n, chan, sc={"inputs": [w, x, y, orient], "outputs": n, "output": chan})
            out.append(case(f"DecodeB2.{n}.{chan}", g))
    # At 44.1 kHz, where Freeverb's tunings are scsynth's.
    for room, damp in [(0.5, 0.5), (0.9, 0.1)]:
        g = G()
        x = sig(g)
        g.add("FreeVerb", "ar", x, 0.6, room, damp)
        out.append(case(f"FreeVerb.{room}", g, 80, sr=44100.0))
        for chan in [0, 1]:
            g = G()
            x = sig(g)
            y = g.add("LFSaw", "ar", 211, 0)
            g.add("FreeVerb2", "ar", x, y, 0.6, room, damp, chan, sc={"inputs": [x, y, 0.6, room, damp], "outputs": 2, "output": chan})
            out.append(case(f"FreeVerb2.{room}.{chan}", g, 80, sr=44100.0))
    # A room whose longest line is already prime (48000 * 9.984 / 340 is
    # 1409.46): scsynth's first line is -1 samples long at any other. The
    # first impulse after the first block, where scsynth's levels rise from 0.
    for chan in [0, 1]:
        for spread in [15.0, 40.0]:
            g = G()
            x = g.add("Impulse", "ar", 150, 0.69)
            g.add("GVerb", "ar", x, 9.984, 1.5, 0.4, 0.6, spread, 0.8, 0.7, 0.5, 300, chan,
                  sc={"inputs": [x, 9.984, 1.5, 0.4, 0.6, spread, 0.8, 0.7, 0.5, 300], "outputs": 2, "output": chan})
            out.append(case(f"GVerb.{spread}.{chan}", g, 96))
    for k, damp in [(40.0, 0.001), (2000.0, 0.01)]:
        g = G()
        x = impulse(g)
        g.add("Spring", "ar", x, k, damp)
        out.append(case(f"Spring.{k}", g, 16))
    # A ball resting on the floor until it drops: at rest scsynth's
    # constructor's sample leaves it where the server's first does.
    for kind in ["Ball", "TBall"]:
        g = G()
        f = g.add("LFPulse", "ar", 30, 0, 0.5)
        f = g.add("MulAdd", "ar", f, 0.3, 0)
        g.add(kind, "ar", f, 10, 0.1, 0.01)
        out.append(case(f"{kind}.ar", g, 64, seed=3))
    return out


_FIXED_SINE = "scsynth's grains read their sines from a table in 16.16 fixed point"


def _grains() -> list[dict]:
    """`GrainUGens.cpp`'s grains, `TGrains` and `GrainTap`. Each trigger
    after the first sample: scsynth's constructor runs a sample of its
    triggers."""
    out = []

    def multi(name, kind, n, ins, sc_ins=None, blocks=16, **extra):
        for chan in range(n):
            g = G()
            trig = g.add("Impulse", "ar", 310.7, 0.5)
            # Round the whole ring within the case where there is one.
            pan = g.add("LFSaw", "kr", 47, 0) if n > 2 else g.add("LFTri", "kr", 3, 0)
            args = [a(g, trig, pan) if callable(a) else a for a in ins]
            sc = [a(g, trig, pan) if callable(a) else a for a in (sc_ins or ins)]
            g.add(kind, "ar", *args, n, chan, sc={"inputs": sc, "outputs": n, "output": chan})
            out.append(case(f"{name}.{n}.{chan}", g, blocks, **extra))

    t = lambda g, trig, pan: trig
    p = lambda g, trig, pan: pan
    for n in [1, 2, 4]:
        multi("GrainSin", "GrainSin", n, [t, 0.007, 731.3, p, -1, 512], tol=2e-4, why=_FIXED_SINE)
    multi("GrainFM", "GrainFM", 2, [t, 0.006, 531.3, 207.7, 2.3, p, -1, 512], tol=2e-3, why=_FIXED_SINE)
    sig_in = lambda g, trig, pan: sig(g)
    multi("GrainIn", "GrainIn", 2, [t, 0.005, sig_in, p, -1, 512])
    buf = [{"num": 0, "channels": 1, "frames": 500, "data": _table(500)}]
    win = [{"num": 1, "channels": 1, "frames": 64, "data": [__import__("math").sin(__import__("math").pi * i / 63) ** 3 for i in range(64)]}]
    for interp in [1, 2, 4]:
        multi(f"GrainBuf.{interp}", "GrainBuf", 2, [t, 0.004, 0, 1.37, 0.21, interp, p, -1, 512], buffers=buf)
    multi("GrainBuf.window", "GrainBuf", 2, [t, 0.004, 0, 0.73, 0.9, 4, p, 1, 512], buffers=buf + win)
    multi("TGrains", "TGrains", 2, [t, 0, 1.19, 0.004, 0.003, p, 0.6, 4], buffers=buf)
    multi("TGrains", "TGrains", 4, [t, 0, -0.83, 0.007, 0.002, p, 0.6, 2], buffers=buf)
    stereo = [{"num": 0, "channels": 2, "frames": 500, "data": _table(500, 2)}]
    # Both rows, in order, as a builder makes them: scsynth's channels draw
    # from the one generator in turn, a block each. The row not probed is
    # nothing on scsynth's side.
    _f32_phase = "scsynth keeps a grain's position through its first block in f32"
    for ratio, seed in [(0.0, 0), (0.3, 7)]:
        for chan in [0, 1]:
            g = G()
            ptr = g.add("LFSaw", "kr", 2, 0)
            ptr = g.add("MulAdd", "kr", ptr, 0.4, 0.5)
            rows = []
            for c in [0, 1]:
                args = [0, ptr, 1.21, 0.0031, -1, 4, ratio, 4]
                if c == chan:
                    sc = {"inputs": args, "outputs": 2, "output": chan}
                else:
                    sc = {"kind": "DC", "inputs": [0.0]}
                rows.append(g.add("Warp1", "ar", *args, c, sc=sc))
            probe = len(g.ugens) - 2 + chan
            out.append(case(f"Warp1.{ratio}.{chan}", g, 16, buffers=stereo, seed=seed, probe=probe, tol=4e-5, why=_f32_phase))
    # A delay line a `BufDelayN` writes as the tap reads it. Buffer 1:
    # scsynth's constructor leaves the buffer it last resolved unset, which
    # on the harness's zeroed memory reads as buffer 0 resolved to nothing.
    line = [{"num": 1, "channels": 1, "frames": 4096, "data": [0.0] * 4096}]
    for ratio, pdisp, tdisp, seed in [(1.0, 0.0, 0.0, 0), (1.6, 0.0, 0.0, 0), (0.7, 0.2, 0.01, 11)]:
        g = G()
        x = sig(g)
        g.add("BufDelayN", "ar", 1, 0, x, 0.01, sc={"inputs": [1, x, 0.01]})
        g.add("GrainTap", "ar", 1, 0.0071, ratio, pdisp, tdisp, 2.0)
        out.append(case(f"GrainTap.{ratio}.{pdisp}", g, 24, buffers=line, seed=seed))
    return out

