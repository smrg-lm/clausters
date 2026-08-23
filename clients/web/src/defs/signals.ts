// Faust Signal API as composable, lowercase callables (mirrors
// `clausters/defs/signals.py`, emitting the same JSON signal tree).
//
// The user-facing way to build a `FaustDef`. Each function here returns a
// `Signal`; composing signals with their math methods builds the JSON
// **signal tree** the server's `/def_send faust` consumes (`{"signals": [ <node>, …
// ]}`, one node per output).
//
// As in `./ugens.ts`, composition is by **method** rather than by operator —
// TypeScript has no operator overloading — so `hslider("freq", …).sin()` and
// `sin(x).mul(0.2)` both compose the graph. Plain numbers are constants
// (Faust `int`/`real`); explicit feedback uses `recursion`/`self_` (one
// sample of delay), and `input(n)` reads audio input `n`.
//
// Reserved controls `in` and `out` (set with `/synth_new … "in" b "out" b`)
// choose the input/output buses; they are added by the server, not declared
// here.

/** A JSON-able signal-tree node, or a bare number (a constant). */
export type SignalNode = number | { [field: string]: unknown };

/** What every callable here accepts where a signal is expected. */
export type SignalInput = Signal | number;

// The method selector -> Faust Signal API op name. Where the two differ the
// method takes the idiomatic name and the wire keeps Faust's.
const BINARY: Record<string, string> = {
    add: "add", sub: "sub", mul: "mul", div: "div", mod: "rem",
    pow: "pow", min: "min", max: "max", atan2: "atan2",
    gt: "gt", lt: "lt", ge: "ge", le: "le", eq: "eq", ne: "ne",
    bitand: "and", bitor: "or", bitxor: "xor", lshift: "lsh", rshift: "rsh",
};
const UNARY: Record<string, string> = {
    abs: "abs", floor: "floor", ceil: "ceil", sin: "sin", cos: "cos",
    tan: "tan", asin: "asin", acos: "acos", atan: "atan", exp: "exp",
    log: "log", log10: "log10", sqrt: "sqrt", as_int: "intcast",
    as_float: "floatcast",
};

const nodeOf = (x: SignalInput): SignalNode => (x instanceof Signal ? x.node : x);

/**
 * One node of a Faust signal graph (one output). Wrap a number to make a
 * constant; compose with the methods below or the module functions.
 */
export class Signal {
    readonly node: SignalNode;

    constructor(node: SignalNode) {
        this.node = node;
    }

    toJSON(): SignalNode {
        return this.node;
    }

    private compose(selector: string, other: SignalInput, swap = false): Signal {
        const op = BINARY[selector];
        if (op === undefined) {
            throw new TypeError(`no Faust signal op for binary '${selector}'`);
        }
        const operands = swap
            ? [nodeOf(other), this.node]
            : [this.node, nodeOf(other)];
        return new Signal({ op, in: operands });
    }

    private composeUnary(selector: string): Signal {
        // Faust has no unary neg; 0 - x.
        if (selector === "neg") return new Signal({ op: "sub", in: [0.0, this.node] });
        const op = UNARY[selector];
        if (op === undefined) {
            throw new TypeError(`no Faust signal op for unary '${selector}'`);
        }
        return new Signal({ op, in: [this.node] });
    }

    // --- binary ---
    add(x: SignalInput): Signal { return this.compose("add", x); }
    sub(x: SignalInput): Signal { return this.compose("sub", x); }
    mul(x: SignalInput): Signal { return this.compose("mul", x); }
    div(x: SignalInput): Signal { return this.compose("div", x); }
    /** The remainder (Faust's `rem`). */
    mod(x: SignalInput): Signal { return this.compose("mod", x); }
    pow(x: SignalInput): Signal { return this.compose("pow", x); }
    min(x: SignalInput): Signal { return this.compose("min", x); }
    max(x: SignalInput): Signal { return this.compose("max", x); }
    atan2(x: SignalInput): Signal { return this.compose("atan2", x); }
    gt(x: SignalInput): Signal { return this.compose("gt", x); }
    lt(x: SignalInput): Signal { return this.compose("lt", x); }
    ge(x: SignalInput): Signal { return this.compose("ge", x); }
    le(x: SignalInput): Signal { return this.compose("le", x); }
    eq(x: SignalInput): Signal { return this.compose("eq", x); }
    ne(x: SignalInput): Signal { return this.compose("ne", x); }
    bitand(x: SignalInput): Signal { return this.compose("bitand", x); }
    bitor(x: SignalInput): Signal { return this.compose("bitor", x); }
    bitxor(x: SignalInput): Signal { return this.compose("bitxor", x); }
    leftshift(x: SignalInput): Signal { return this.compose("lshift", x); }
    rightshift(x: SignalInput): Signal { return this.compose("rshift", x); }

    /**
     * This signal on the **right** of `x op this` — the number-on-the-left
     * case a method cannot otherwise express (`sig.rsub(1)` is `1 - sig`).
     */
    rsub(x: SignalInput): Signal { return this.compose("sub", x, true); }
    /** `x / this`; see `rsub`. */
    rdiv(x: SignalInput): Signal { return this.compose("div", x, true); }

    // --- unary ---
    neg(): Signal { return this.composeUnary("neg"); }
    abs(): Signal { return this.composeUnary("abs"); }
    floor(): Signal { return this.composeUnary("floor"); }
    ceil(): Signal { return this.composeUnary("ceil"); }
    sin(): Signal { return this.composeUnary("sin"); }
    cos(): Signal { return this.composeUnary("cos"); }
    tan(): Signal { return this.composeUnary("tan"); }
    asin(): Signal { return this.composeUnary("asin"); }
    acos(): Signal { return this.composeUnary("acos"); }
    atan(): Signal { return this.composeUnary("atan"); }
    exp(): Signal { return this.composeUnary("exp"); }
    log(): Signal { return this.composeUnary("log"); }
    log10(): Signal { return this.composeUnary("log10"); }
    sqrt(): Signal { return this.composeUnary("sqrt"); }
    /** Faust's `intcast`. */
    asinteger(): Signal { return this.composeUnary("as_int"); }
    /** Faust's `floatcast`. */
    asfloat(): Signal { return this.composeUnary("as_float"); }
}

/** Coerces a number or `Signal` into a `Signal`. */
export const signal = (x: SignalInput): Signal =>
    x instanceof Signal ? x : new Signal(x);

// ---- sources / structure ----

/** Audio input `index` (Faust `CsigInput`). */
export const input = (index = 0): Signal =>
    new Signal({ op: "input", index: Math.trunc(index) });

/** The one-sample-delayed output of the enclosing `recursion`. */
export const self_ = (): Signal => new Signal({ op: "self" });

/** Single feedback: `body` is a signal that may reference `self_`. */
export const recursion = (body: SignalInput): Signal =>
    new Signal({ op: "recursion", in: [nodeOf(body)] });

/**
 * Fluent feedback: `fn(s)` builds the body from its own delayed output `s`
 * (sugar over `recursion`/`self_`).
 */
export const rec = (fn: (s: Signal) => SignalInput): Signal =>
    recursion(fn(self_()));

/** `x` delayed by `n` samples (Faust `CsigDelay`). */
export const delay = (x: SignalInput, n: SignalInput): Signal =>
    new Signal({ op: "delay", in: [nodeOf(x), nodeOf(n)] });

/** `x` delayed by one sample. */
export const delay1 = (x: SignalInput): Signal =>
    new Signal({ op: "delay1", in: [nodeOf(x)] });

/**
 * A foreign **constant**: a scalar the server resolves once, at def-compile
 * time, from its runtime (Faust `CsigFConst`). `ctype` is `"int"` or
 * `"real"`, `name` the runtime symbol, `file` the include that declares it.
 * The building block of `sr` — prefer that helper for sample rate.
 */
export const fconst = (ctype: "int" | "real", name: string, file = ""): Signal =>
    new Signal({ op: "fconst", ctype, name, file });

/**
 * A foreign **variable**: like `fconst` but re-read each block (Faust
 * `CsigFVar`).
 */
export const fvar = (ctype: "int" | "real", name: string, file = ""): Signal =>
    new Signal({ op: "fvar", ctype, name, file });

/**
 * The engine's sample rate as a `Signal`, read from the server at
 * def-compile time — the port of Faust's `ma.SR`, clamp included.
 *
 * Use this instead of baking a JS constant: a def built with `sr` is correct
 * at whatever rate the server (or NRT renderer) actually runs.
 */
export function sr(): Signal {
    const raw = fconst("int", "fSamplingFreq", "<math.h>");
    return signal(192000.0).min(signal(1.0).max(raw));
}

/** `sel ? b : a` (Faust `select2`). */
export const select2 = (sel: SignalInput, a: SignalInput, b: SignalInput): Signal =>
    new Signal({ op: "select2", in: [nodeOf(sel), nodeOf(a), nodeOf(b)] });

/** Three-way selection (Faust `select3`). */
export const select3 = (
    sel: SignalInput,
    a: SignalInput,
    b: SignalInput,
    c: SignalInput,
): Signal =>
    new Signal({
        op: "select3",
        in: [nodeOf(sel), nodeOf(a), nodeOf(b), nodeOf(c)],
    });

// ---- unary functions (also available as methods) ----

const unary = (op: string) => (x: SignalInput): Signal =>
    new Signal({ op, in: [nodeOf(x)] });

export const sin = unary("sin");
export const cos = unary("cos");
export const tan = unary("tan");
export const asin = unary("asin");
export const acos = unary("acos");
export const atan = unary("atan");
export const exp = unary("exp");
export const exp10 = unary("exp10");
export const log = unary("log");
export const log10 = unary("log10");
export const sqrt = unary("sqrt");
export const abs = unary("abs");
export const floor = unary("floor");
export const ceil = unary("ceil");
export const rint = unary("rint");

// ---- binary functions ----

const binary = (op: string) => (a: SignalInput, b: SignalInput): Signal =>
    new Signal({ op, in: [nodeOf(a), nodeOf(b)] });

export const min = binary("min");
export const max = binary("max");
export const pow = binary("pow");
export const atan2 = binary("atan2");
export const fmod = binary("fmod");
export const rem = binary("rem");

// ---- math constants ----
//
// Unlike the sample rate, these are *literals* in Faust too (`ma.PI` is the
// double constant, not a runtime value), so a JS number is exactly what the
// compiler bakes in — no server round-trip is involved. They become constant
// signals as soon as they meet a Signal in an expression.
export const PI = 3.141592653589793;
/** 2·PI; Faust has no `ma.TAU`, this is just the literal. */
export const TAU = 6.283185307179586;

// ---- controls (labels become control names) ----

/** A horizontal slider; its `label` becomes the def's control name. */
export const hslider = (
    label: string,
    init: number,
    lo: number,
    hi: number,
    step: number,
): Signal => new Signal({ op: "hslider", label, init, min: lo, max: hi, step });

/** A vertical slider; its `label` becomes the def's control name. */
export const vslider = (
    label: string,
    init: number,
    lo: number,
    hi: number,
    step: number,
): Signal => new Signal({ op: "vslider", label, init, min: lo, max: hi, step });

/** A numeric entry; its `label` becomes the def's control name. */
export const nentry = (
    label: string,
    init: number,
    lo: number,
    hi: number,
    step: number,
): Signal => new Signal({ op: "nentry", label, init, min: lo, max: hi, step });

/** A momentary button (0 or 1); its `label` becomes the control name. */
export const button = (label: string): Signal => new Signal({ op: "button", label });

/** A latching checkbox (0 or 1); its `label` becomes the control name. */
export const checkbox = (label: string): Signal =>
    new Signal({ op: "checkbox", label });

// ---- tables ----

/** A constant table of `values`, read by index. */
export const waveform = (values: readonly number[]): Signal =>
    new Signal({ op: "waveform", values: values.map(Number) });

/** A read-only table of `size` filled by `init`, read at `ridx`. */
export const rdtable = (
    size: SignalInput,
    init: SignalInput,
    ridx: SignalInput,
): Signal =>
    new Signal({ op: "rdtable", in: [nodeOf(size), nodeOf(init), nodeOf(ridx)] });

/**
 * A read/write table of `size` filled by `init`, written `wsig` at `widx`
 * and read at `ridx`.
 */
export const rwtable = (
    size: SignalInput,
    init: SignalInput,
    widx: SignalInput,
    wsig: SignalInput,
    ridx: SignalInput,
): Signal =>
    new Signal({
        op: "rwtable",
        in: [nodeOf(size), nodeOf(init), nodeOf(widx), nodeOf(wsig), nodeOf(ridx)],
    });
