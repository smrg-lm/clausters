// Faust Box API as composable, lowercase callables (mirrors
// `clausters/defs/boxes.py`, emitting the same JSON box tree).
//
// The box counterpart of `./signals.ts`, and a complete def-building API in
// its own right: each function returns a `Box` and composing boxes builds the
// JSON **box tree** the server's `/def_send faust` consumes (see the server's
// `faust::boxes` for the schema). Boxes are Faust's point-free algebra —
// `seq`/`par`/`split`/`merge`/`rec` compose whole **processors** by their
// input/output arities. Where `signals` describes one output at a time
// referentially (`input(n)`), boxes describe multi-channel blocks that plug
// into each other — the natural shape for routing, chains, and anything
// conceived as units with inputs and outputs.
//
// On top of the algebra, `faust` compiles any Faust **expression** into a
// `Box` that composes like a primitive. That addition puts the whole Faust
// library ecosystem (`os.osc`, `fi.lowpass`, `re.`, `pm.`, …) inside the same
// algebra without transcribing anything: library functions become boxes among
// boxes.
//
// Choosing a form: a fixed processing chain written top to bottom often reads
// best as plain Faust (`FaustDef.fromSource`); graphs assembled one output at
// a time from arithmetic and feedback suit `./signals.ts`. Regular banks ("N
// copies with index-dependent parameters") are best written in Faust itself —
// `par(i, N, …)`, widget labels with `%i`, `ba.take` — and parametrized from
// TypeScript by splicing `N` and lists through `faust`'s eval arguments. Boxes
// shine when the graph is conceived as composed processors, when its structure
// is decided by the page's own data, and whenever library DSP has to mix with
// pieces built here.
//
// Two stages of application, kept separate on purpose:
//
// - `faust("fi.lowpass", 3)` — arguments to `faust` are **evaluation-stage**:
//   spliced into the Faust source text (`fi.lowpass(3)`), where structural
//   parameters like a filter order must live.
// - `fiLp.call(cutoff, wire())` — arguments to a `Box`'s **call** are
//   **composition-stage**: boxes wired as the box's signal inputs, sugar for
//   `seq(par(cutoff, wire()), fiLp)`. The reference client spells this one
//   `fi_lp(cutoff, wire())`, a Python `__call__`; a class instance is not
//   callable in TypeScript, so the application has a name — and for the same
//   reason `st[0]` is `st.get(0)` here. Same calls, same order, same JSON.
//
// The wire rule (the big difference from `signals`): **each `wire` is a
// distinct input**. There is no referential `input(n)` here — two wires in two
// positions are two input channels. Reusing the *same* `wire()` (or `cut()`)
// object in more than one position is almost always a mistake, and
// `FaustDef.fromBox` rejects it; route explicitly with `split`, or write that
// stretch inside a `faust` fragment (`_ <: …`). Every *other* box value can be
// reused freely: a repeated subexpression is computed once (the server shares
// identical subtrees).
//
// Reserved controls `in` and `out` (set with `/synth_new … "in" b "out" b`)
// choose the input/output buses; they are added by the server, not declared
// here.

import { FaustExpr } from "./expr.ts";

/** A JSON-able box-tree node, or a bare number (a constant). */
export type BoxNode = number | { [field: string]: unknown };

/** What every callable here accepts where a box is expected. */
export type BoxInput = Box | number;

// The method selector -> box schema op name. The box schema has no lsh/rsh/rem;
// `mod` maps to Faust's `fmod`.
const BINARY: Record<string, string> = {
    add: "add", sub: "sub", mul: "mul", div: "div", mod: "fmod",
    pow: "pow", min: "min", max: "max", atan2: "atan2",
    gt: "gt", lt: "lt", ge: "ge", le: "le", eq: "eq", ne: "ne",
    bitand: "and", bitor: "or", bitxor: "xor",
};
const UNARY: Record<string, string> = {
    abs: "abs", floor: "floor", ceil: "ceil", sin: "sin", cos: "cos",
    tan: "tan", asin: "asin", acos: "acos", atan: "atan", exp: "exp",
    log: "log", log10: "log10", sqrt: "sqrt", asint: "intcast",
    asfloat: "floatcast",
};

/** An arity, or `null` where it is not known. */
type Arity = number | null;

/** Sums arities, propagating unknown (`null` absorbs). */
function sumArity(values: Iterable<Arity>): Arity {
    let total = 0;
    for (const v of values) {
        if (v === null) return null;
        total += v;
    }
    return total;
}

const nodeOf = (x: BoxInput): BoxNode => box(x).node;

/**
 * One node of a Faust box expression. Wrap a number to make a constant;
 * compose with the math methods, the module functions, or by applying the box
 * to its inputs with `call`.
 *
 * `numInputs`/`numOutputs` are the box's signal arity as computed on the
 * client from the composition rules; `null` when unknown (a `faust` fragment
 * with no `ins`/`outs`). The server does not read them — a real mismatch is
 * reported by Faust itself when the def compiles.
 */
export class Box extends FaustExpr<Box, BoxInput> {
    readonly node: BoxNode;
    readonly numInputs: number | null;
    readonly numOutputs: number | null;

    constructor(node: BoxNode, numInputs: number | null, numOutputs: number | null) {
        super();
        this.node = node;
        this.numInputs = numInputs;
        this.numOutputs = numOutputs;
    }

    toJSON(): BoxNode {
        return this.node;
    }

    // --- application sugar ---

    /**
     * Applies boxes to this box's inputs: `f.call(a, b)` is `seq(par(a, b),
     * f)` (with one argument, `seq(a, f)`) — Faust's partial-application style,
     * which the reference client writes as calling the box itself. The
     * arguments must cover *all* the box's inputs; use `wire` for the ones left
     * open.
     */
    call(...args: BoxInput[]): Box {
        if (args.length === 0) {
            throw new TypeError("a box call needs at least one argument box");
        }
        const applied = args.length > 1 ? par(...args) : box(args[0]);
        return seq(applied, this);
    }

    /**
     * Selects one output channel: `st.get(0)` is `seq(st, par(wire, cut, …))`
     * (the reference client's `st[0]`). Needs a known `numOutputs` (pass
     * `outs` to `faust` for fragments). The selected fragment is shared, not
     * recomputed, when several channels of the same box value are used.
     */
    get(index: number): Box {
        const n = this.numOutputs;
        if (n === null) {
            throw new RangeError(
                "cannot select an output: this box's arity is unknown "
                    + "(pass outs to faust())",
            );
        }
        if (!Number.isInteger(index)) {
            throw new TypeError(`box output index must be an integer, not ${index}`);
        }
        const k = index < 0 ? index + n : index;
        if (!(k >= 0 && k < n)) {
            throw new RangeError(`output ${index} out of range for a ${n}-output box`);
        }
        if (n === 1) return this;
        const taps = Array.from({ length: n }, (_v, i) => (i === k ? wire() : cut()));
        return seq(this, par(...taps));
    }

    /** All output channels: `const [l, r] = st.outs()`. */
    outs(): Box[] {
        if (this.numOutputs === null) {
            throw new RangeError(
                "cannot enumerate outputs: this box's arity is unknown "
                    + "(pass outs to faust())",
            );
        }
        return Array.from({ length: this.numOutputs }, (_v, k) => this.get(k));
    }

    // --- FaustExpr hooks: build graph nodes ---

    protected composeBinop(selector: string, other: BoxInput, swap: boolean): Box {
        const op = BINARY[selector];
        if (op === undefined) {
            throw new TypeError(`no Faust box op for binary '${selector}'`);
        }
        const rhs = box(other);
        const [first, second] = swap ? [rhs, this as Box] : [this as Box, rhs];
        return new Box(
            { op, in: [first.node, second.node] },
            sumArity([first.numInputs, second.numInputs]),
            1,
        );
    }

    protected unop(selector: string): Box {
        // Faust has no unary neg; 0 - x.
        if (selector === "neg") {
            return new Box({ op: "sub", in: [0.0, this.node] }, this.numInputs, 1);
        }
        const op = UNARY[selector];
        if (op === undefined) {
            throw new TypeError(`no Faust box op for unary '${selector}'`);
        }
        return new Box({ op, in: [this.node] }, this.numInputs, 1);
    }
}

/**
 * Coerces a number or `Box` into a `Box` (numbers are constants).
 *
 * A constant reaches the server as a bare JSON number, and the server reads an
 * integral one as a Faust **int** and any other as a **real** — so `box(2)` is
 * `int 2` in both clients and `box(0.5)` is `real 0.5` in both. The one case a
 * page cannot say is the reference client's `box(2.0)`, a *real* whose value is
 * integral: JavaScript has one number type and `2.0` is `2` by the time this
 * sees it. Recorded in `clients/web/PLAN.md` ("Found by use"), where the
 * escape a page needs is still open.
 */
export function box(x: BoxInput): Box {
    if (x instanceof Box) return x;
    if (typeof x === "number") return new Box(x, 0, 1);
    throw new TypeError(`cannot make a box out of ${String(x)}`);
}

// ---- primitives ----

/**
 * The identity box `_`: one open signal input. Every call is a **new,
 * distinct input** — reusing one wire object in two positions is an error (see
 * the module docs for the rule and the escapes).
 */
export const wire = (): Box => new Box({ op: "wire" }, 1, 1);

/**
 * The `!` box: swallows one signal. Like `wire`, each call is a new, distinct
 * position.
 */
export const cut = (): Box => new Box({ op: "cut" }, 1, 0);

// ---- composition (n-ary, folded left, like the server) ----

/** Sequential composition `a : b : …` (needs at least 2). */
export const seq = (...items: BoxInput[]): Box => compose("seq", items);

/** Parallel composition `a , b , …` (needs at least 2). */
export function par(...items: BoxInput[]): Box {
    const boxes = atLeastTwo("par", items).map(box);
    return new Box(
        { op: "par", in: boxes.map((b) => b.node) },
        sumArity(boxes.map((b) => b.numInputs)),
        sumArity(boxes.map((b) => b.numOutputs)),
    );
}

/** Split composition `a <: b` (needs at least 2). */
export const split = (...items: BoxInput[]): Box => compose("split", items);

/** Merge composition `a :> b` — excess outputs are summed (needs at least 2). */
export const merge = (...items: BoxInput[]): Box => compose("merge", items);

function atLeastTwo(op: string, items: readonly BoxInput[]): readonly BoxInput[] {
    if (items.length < 2) {
        throw new TypeError(`${op} needs at least 2 boxes, got ${items.length}`);
    }
    return items;
}

function compose(op: string, items: readonly BoxInput[]): Box {
    // seq/split/merge folded left: the composite reads like the first box and
    // writes like the last.
    const boxes = atLeastTwo(op, items).map(box);
    return new Box(
        { op, in: boxes.map((b) => b.node) },
        boxes[0].numInputs,
        boxes[boxes.length - 1].numOutputs,
    );
}

/**
 * Recursive composition `a ~ b`: `b` feeds `a`'s first inputs back from `a`'s
 * first outputs, with one implicit sample of delay. Point-free — for the
 * `rec((s) => …)` style, build the loop in a `faust` fragment or with
 * `./signals.ts` instead.
 */
export function rec(a: BoxInput, b: BoxInput): Box {
    const left = box(a);
    const right = box(b);
    let ins: Arity = null;
    if (left.numInputs !== null && right.numOutputs !== null) {
        // Not the shadowed module-level max: that one builds a Box.
        ins = Math.max(0, left.numInputs - right.numOutputs);
    }
    return new Box({ op: "rec", in: [left.node, right.node] }, ins, left.numOutputs);
}

// ---- the escape hatch: Faust source fragments ----

/** An evaluation-stage argument: spliced into the generated Faust source. */
export type EvalArg = number | string | readonly EvalArg[];

/** `faust`'s trailing options — the reference client's keyword arguments. */
export interface FaustOptions {
    /** Auxiliary Faust definitions prepended to the generated program. */
    defs?: string;
    /** The fragment's declared signal input arity. */
    ins?: number | null;
    /** The fragment's declared signal output arity — needed for `get`/`outs`. */
    outs?: number | null;
}

/**
 * A Faust **expression** compiled into a box — the door to the Faust libraries
 * (`stdfaust.lib` is imported for you). The resulting box is indistinguishable
 * from a primitive: compose it, apply it, do arithmetic on it.
 *
 * The arguments after `src` are **evaluation-stage**, spliced into the source
 * text as Faust application — `faust("fi.lowpass", 3)` compiles
 * `fi.lowpass(3)`. That is where structural parameters (a filter order, a
 * table size, a list of coefficients) must go; they cannot travel as signals.
 * Formatting: a number as a literal (integral values keep their integer
 * spelling), an array as a Faust list `(a, b, c)`, a string verbatim (for
 * expressions or library functions passed as arguments). Signal inputs are
 * then wired by applying the box:
 *
 * ```ts
 * const lp = faust("fi.lowpass", 3);      // fi.lowpass(3): inputs (fc, x)
 * const y = lp.call(cutoff, wire());
 * ```
 *
 * A trailing options object carries what the reference client passes as
 * keywords: `defs` prepends auxiliary Faust definitions (helper functions,
 * pattern matching) to the generated program, and `ins`/`outs` declare the
 * fragment's signal arity — only the Faust compiler knows it, so pass `outs`
 * when you need channel selection (`st.get(0)` / `.outs()`); a wrong
 * declaration is caught by Faust when the def compiles.
 *
 * Each distinct generated source is compiled (and cached) separately on the
 * server; reusing one fragment *value* many times compiles and computes it
 * once.
 */
export function faust(src: string, ...rest: (EvalArg | FaustOptions)[]): Box {
    let options: FaustOptions = {};
    const args = [...rest];
    const last = args[args.length - 1];
    if (isOptions(last)) {
        options = last;
        args.pop();
    }
    const evalArgs = args as EvalArg[];
    const applied = evalArgs.length > 0
        ? `${src}(${evalArgs.map(evalArg).join(", ")})`
        : src;
    const defs = options.defs ?? "";
    const program = defs
        ? `import("stdfaust.lib"); ${defs} process = ${applied};`
        : `import("stdfaust.lib"); process = ${applied};`;
    return new Box(
        { op: "faust", src: program },
        options.ins ?? null,
        options.outs ?? null,
    );
}

/** Whether a trailing argument is the options bag rather than an eval-arg. */
function isOptions(x: unknown): x is FaustOptions {
    return typeof x === "object" && x !== null && !Array.isArray(x)
        && !(x instanceof Box);
}

function evalArg(a: EvalArg): string {
    if (a instanceof Box) {
        throw new TypeError(
            "a box cannot be an evaluation-stage argument; boxes are applied "
                + "by calling the fragment: faust(src, …).call(box, …)",
        );
    }
    if (typeof a === "number") {
        if (!Number.isFinite(a)) {
            throw new TypeError(`cannot splice ${a} into Faust source`);
        }
        // An integral value keeps its integer spelling, the way the reference
        // client's `repr(3)` does: Faust reads `3` and `3.0` as different types.
        return Number.isInteger(a) ? a.toFixed(0) : String(a);
    }
    if (typeof a === "string") return a;
    if (Array.isArray(a)) return `(${a.map(evalArg).join(", ")})`;
    throw new TypeError(`cannot splice ${String(a)} into Faust source`);
}

// ---- structure ----

/** `x` delayed by `n` samples (Faust `@`). */
export function delay(x: BoxInput, n: BoxInput): Box {
    const a = box(x);
    const b = box(n);
    return new Box(
        { op: "delay", in: [a.node, b.node] },
        sumArity([a.numInputs, b.numInputs]),
        1,
    );
}

/** One sample of delay (Faust `'`), sugar for `delay(x, 1)`. */
export const delay1 = (x: BoxInput): Box => delay(x, 1);

/** `sel ? b : a` (Faust `select2`). */
export function select2(sel: BoxInput, a: BoxInput, b: BoxInput): Box {
    const parts = [box(sel), box(a), box(b)];
    return new Box(
        { op: "select2", in: parts.map((p) => p.node) },
        sumArity(parts.map((p) => p.numInputs)),
        1,
    );
}

/** Three-way selection (Faust `select3`). */
export function select3(sel: BoxInput, a: BoxInput, b: BoxInput, c: BoxInput): Box {
    const parts = [box(sel), box(a), box(b), box(c)];
    return new Box(
        { op: "select3", in: parts.map((p) => p.node) },
        sumArity(parts.map((p) => p.numInputs)),
        1,
    );
}

/**
 * A foreign **constant**: a scalar the server resolves once, at def-compile
 * time, from its runtime. `ctype` is `"int"` or `"real"`. The building block
 * of `sr` — prefer that helper for sample rate.
 */
export const fconst = (ctype: "int" | "real", name: string, file = ""): Box =>
    new Box({ op: "fconst", ctype, name, file }, 0, 1);

/** A foreign **variable**: like `fconst` but re-read each block. */
export const fvar = (ctype: "int" | "real", name: string, file = ""): Box =>
    new Box({ op: "fvar", ctype, name, file }, 0, 1);

/**
 * The engine's sample rate as a `Box`, read from the server at def-compile
 * time — the port of Faust's `ma.SR`, with the stdlib's `[1, 192000]` clamp.
 *
 * Use this instead of baking a JS constant so the def is correct at whatever
 * rate the engine or NRT renderer runs.
 */
export function sr(): Box {
    const raw = fconst("int", "fSamplingFreq", "<math.h>");
    return min(box(192000.0), max(box(1.0), raw));
}

// ---- unary functions (also available as methods) ----

const unary = (op: string) => (x: BoxInput): Box =>
    new Box({ op, in: [nodeOf(x)] }, box(x).numInputs, 1);

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
export const round = unary("round");

// ---- binary functions ----

const binary = (op: string) => (a: BoxInput, b: BoxInput): Box => {
    const left = box(a);
    const right = box(b);
    return new Box(
        { op, in: [left.node, right.node] },
        sumArity([left.numInputs, right.numInputs]),
        1,
    );
};

export const min = binary("min");
export const max = binary("max");
export const pow = binary("pow");
export const atan2 = binary("atan2");
export const fmod = binary("fmod");

// ---- math constants (literals in Faust too; see signals) ----

export const PI = 3.141592653589793;
/** 2·PI; Faust has no `ma.TAU`, this is just the literal. */
export const TAU = 6.283185307179586;

// ---- controls (labels become control names) ----

export const hslider = (
    label: string, init: number, lo: number, hi: number, step: number,
): Box =>
    new Box({ op: "hslider", label, init, min: lo, max: hi, step }, 0, 1);

export const vslider = (
    label: string, init: number, lo: number, hi: number, step: number,
): Box =>
    new Box({ op: "vslider", label, init, min: lo, max: hi, step }, 0, 1);

export const nentry = (
    label: string, init: number, lo: number, hi: number, step: number,
): Box =>
    new Box({ op: "nentry", label, init, min: lo, max: hi, step }, 0, 1);

export const button = (label: string): Box =>
    new Box({ op: "button", label }, 0, 1);

export const checkbox = (label: string): Box =>
    new Box({ op: "checkbox", label }, 0, 1);

export function hgroup(label: string, inner: BoxInput): Box {
    const b = box(inner);
    return new Box(
        { op: "hgroup", label, in: [b.node] },
        b.numInputs,
        b.numOutputs,
    );
}

export function vgroup(label: string, inner: BoxInput): Box {
    const b = box(inner);
    return new Box(
        { op: "vgroup", label, in: [b.node] },
        b.numInputs,
        b.numOutputs,
    );
}

// ---- tables ----

/**
 * A fixed table; outputs the (size, content) pair, ready to stand in for
 * `rdtable`/`rwtable`'s leading (size, init) boxes.
 */
export const waveform = (values: readonly number[]): Box =>
    new Box({ op: "waveform", values: values.map(Number) }, 0, 2);

/**
 * `rdtable(size, init, ridx)` — or `rdtable(wf, ridx)` with a `waveform`
 * standing in for (size, init).
 */
export const rdtable = (...args: BoxInput[]): Box => table("rdtable", args, 2, 3);

/**
 * `rwtable(size, init, widx, wsig, ridx)` — or the 4-argument form with a
 * `waveform` up front.
 */
export const rwtable = (...args: BoxInput[]): Box => table("rwtable", args, 4, 5);

function table(op: string, args: readonly BoxInput[], lo: number, hi: number): Box {
    if (!(args.length >= lo && args.length <= hi)) {
        throw new TypeError(`${op} takes ${lo} or ${hi} boxes, got ${args.length}`);
    }
    const boxes = args.map(box);
    return new Box(
        { op, in: boxes.map((b) => b.node) },
        sumArity(boxes.map((b) => b.numInputs)),
        1,
    );
}

// ---- the wire-reuse lint (used by FaustDef.fromBox) ----

/** `{node object -> [op, occurrences]}` for the wire/cut nodes under a tree. */
type WireCounts = Map<object, [string, number]>;

/**
 * Rejects a tree where the same `wire`/`cut` **object** appears in more than
 * one position. Each wire is a distinct input in the box algebra; reusing one
 * object almost always means the graph silently reads more bus channels than
 * intended. Duplicating any *other* box value is fine (shared subtrees are
 * computed once).
 */
export function checkWires(node: unknown): void {
    const counts = ioCounts(node, new Map());
    const reused = new Set<string>();
    for (const [op, n] of counts.values()) {
        if (n > 1) reused.add(op);
    }
    if (reused.size > 0) {
        throw new TypeError(
            `a ${[...reused].sort().join("/")} box object was reused; each wire `
                + "(and cut) is a distinct position — every input needs its own "
                + "wire(): route explicitly with split(), or write that stretch "
                + 'inside a faust() fragment (e.g. "_ <: …")',
        );
    }
}

/**
 * The wire/cut nodes under `node`, counting textual **positions** — a shared
 * subtree multiplies whatever it holds, which is exactly the reuse this looks
 * for. `memo` keeps one result per visited node object.
 */
function ioCounts(node: unknown, memo: Map<object, WireCounts>): WireCounts {
    if (Array.isArray(node)) {
        const counts: WireCounts = new Map();
        for (const item of node) mergeCounts(counts, ioCounts(item, memo));
        return counts;
    }
    if (typeof node === "object" && node !== null) {
        const seen = memo.get(node);
        if (seen !== undefined) return seen;
        const op = (node as { op?: unknown }).op;
        let counts: WireCounts;
        if (op === "wire" || op === "cut") {
            counts = new Map([[node, [op, 1] as [string, number]]]);
        } else {
            counts = new Map();
            for (const value of Object.values(node)) {
                mergeCounts(counts, ioCounts(value, memo));
            }
        }
        memo.set(node, counts);
        return counts;
    }
    return new Map();
}

function mergeCounts(into: WireCounts, other: WireCounts): void {
    for (const [key, [op, n]] of other) {
        const prev = into.get(key);
        into.set(key, [op, n + (prev ? prev[1] : 0)]);
    }
}
