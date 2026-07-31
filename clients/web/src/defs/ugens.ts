// UGen graph as composable, lowercase callables (mirrors
// `clausters/defs/ugens.py`, adapted to TypeScript).
//
// Each function here is a small **lowercase** callable returning a `Ugen` node
// (one output); composing nodes with these functions and the nodes' math
// methods builds the graph a `SynthDef` serializes into the JSON
// `SynthDefSpec` the server's `/d_recv` consumes — the same JSON the Python
// builders emit, which the parity vectors in `tests/` hold.
//
// **Composition is by method, not by operator.** TypeScript has no operator
// overloading, so where the Python client writes `sine(freq) * amp` this one
// writes `sine(freq).mul(amp)`, and every other operator or math method
// (`mod`, `min`/`max`, comparisons, `.sin()`, `.midicps()`, `.distort()` …)
// is a method carrying the same operator **name** the wire uses — so the two
// clients emit identical specs. The free `add`/`sub`/`mul`/`div` functions
// take the number-on-the-left case (`sub(1, sig)`), which a method cannot.
//
// **Instance-based, no global build context**: the graph *is* the tree of
// composed objects, so several defs build concurrently.
//
// **Multichannel is an explicit container**, not implicit expansion: `dup`
// fans a signal out into a `ChannelList`, the math methods broadcast/zip over
// it (wrapping the shorter side modulo), `out(bus, chans)` lays the channels
// on consecutive buses, and `mix` folds a list back to one channel through
// the fused sums. Per-argument expansion (`sine(chans(440, 443))`) is
// deliberately **not** implemented — a channel list reaching a single-channel
// input is a type error, and a `TypeError` at serialization.
//
// Reserved controls `in` and `out` (the input/output buses, set with
// `/s_new … "in" b "out" b`) are added by the server, not declared here.

// The four arithmetic selectors keep their dedicated alias kinds, so the
// emitted graphs match the Python client's byte for byte.
const BINOP_UGEN: Record<string, string> = {
    add: "Add",
    sub: "Sub",
    mul: "Mul",
    div: "Div",
};

// Every other operator composes a generic `BinaryOpUGen`/`UnaryOpUGen` whose
// `op` is the operator **name** — the same name the server's builtins table
// resolves, so a graph op and an off-RT value agree. The selector *is* the
// wire name (no numeric index crosses the wire).
const BINOP_OPS = new Set([
    "mod", "pow", "min", "max", "atan2", "gt", "lt", "ge", "le", "eq", "ne",
    "bitand", "bitor", "bitxor", "lshift", "rshift", "hypot", "ring1", "ring2",
    "ring3", "ring4", "sumsqr", "difsqr", "sqrsum", "sqrdif", "absdif",
    "thresh", "clip2", "excess", "round", "trunc", "fold2", "wrap2", "gcd",
    "lcm", "hypot_apx",
]);
const UNOP_OPS = new Set([
    "neg", "abs", "sin", "cos", "tan", "asin", "acos", "atan", "exp", "log",
    "log10", "log2", "sqrt", "floor", "ceil", "rint", "as_int", "as_float",
    "squared", "cubed", "recip", "frac", "sign", "sinh", "cosh", "tanh",
    "distort", "softclip", "midicps", "cpsmidi", "midiratio", "ratiomidi",
    "dbamp", "ampdb", "octcps", "cpsoct",
]);

/**
 * One single-channel graph operand: a leaf node (`Ugen`/`Control`) or a
 * plain number (a constant).
 */
export type GraphInput = GraphLeaf | number;
/** What a math method accepts: a single channel, or a list of them. */
export type OpOperand = GraphInput | ChannelList | readonly GraphInput[];

/**
 * A math method's result: a list operand fans the result out, anything else
 * keeps the receiver's shape.
 */
export type OpResult<TSelf, TOther> = TOther extends ChannelList | readonly GraphInput[]
    ? ChannelList
    : TSelf;

const isLeaf = (x: unknown): x is GraphLeaf => x instanceof GraphLeaf;
const isList = (x: unknown): x is ChannelList | readonly GraphInput[] =>
    x instanceof ChannelList || Array.isArray(x);

/**
 * An expression of the **UGen graph**: a `Ugen`, a `Control` or a
 * `ChannelList` of them — something that composes a graph rather than a
 * value, and what `SynthDef` serializes. The Faust families compose their own
 * graphs (`signals`, and the box algebra once it lands), so they are peers of
 * this branch, not members of it; the name avoids `Graph*` because `GraphDef`
 * already means a configuration of member nodes wired by buses.
 *
 * It also carries the shared math surface: `add`/`sub`/`mul`/`div` compose
 * the dedicated alias kinds, everything else a generic
 * `BinaryOpUGen`/`UnaryOpUGen` carrying the operator name.
 */
export abstract class SynthExpr<TSelf> {
    protected abstract binop<T extends OpOperand>(
        selector: string,
        other: T,
    ): OpResult<TSelf, T>;
    protected abstract unop(selector: string): TSelf;

    // --- binary ---
    add<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("add", x); }
    sub<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("sub", x); }
    mul<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("mul", x); }
    div<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("div", x); }
    mod<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("mod", x); }
    pow<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("pow", x); }
    min<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("min", x); }
    max<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("max", x); }
    atan2<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("atan2", x); }
    gt<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("gt", x); }
    lt<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("lt", x); }
    ge<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("ge", x); }
    le<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("le", x); }
    eq<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("eq", x); }
    ne<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("ne", x); }
    bitand<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("bitand", x); }
    bitor<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("bitor", x); }
    bitxor<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("bitxor", x); }
    lshift<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("lshift", x); }
    rshift<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("rshift", x); }
    hypot<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("hypot", x); }
    /** The cheap hypotenuse approximation (`hypot_apx` on the wire). */
    hypotApx<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("hypot_apx", x); }
    ring1<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("ring1", x); }
    ring2<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("ring2", x); }
    ring3<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("ring3", x); }
    ring4<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("ring4", x); }
    sumsqr<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("sumsqr", x); }
    difsqr<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("difsqr", x); }
    sqrsum<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("sqrsum", x); }
    sqrdif<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("sqrdif", x); }
    absdif<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("absdif", x); }
    thresh<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("thresh", x); }
    clip2<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("clip2", x); }
    excess<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("excess", x); }
    round<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("round", x); }
    trunc<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("trunc", x); }
    fold2<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("fold2", x); }
    wrap2<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("wrap2", x); }
    gcd<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("gcd", x); }
    lcm<T extends OpOperand>(x: T): OpResult<TSelf, T> { return this.binop("lcm", x); }

    // --- unary ---
    neg(): TSelf { return this.unop("neg"); }
    abs(): TSelf { return this.unop("abs"); }
    sin(): TSelf { return this.unop("sin"); }
    cos(): TSelf { return this.unop("cos"); }
    tan(): TSelf { return this.unop("tan"); }
    asin(): TSelf { return this.unop("asin"); }
    acos(): TSelf { return this.unop("acos"); }
    atan(): TSelf { return this.unop("atan"); }
    exp(): TSelf { return this.unop("exp"); }
    log(): TSelf { return this.unop("log"); }
    log10(): TSelf { return this.unop("log10"); }
    log2(): TSelf { return this.unop("log2"); }
    sqrt(): TSelf { return this.unop("sqrt"); }
    floor(): TSelf { return this.unop("floor"); }
    ceil(): TSelf { return this.unop("ceil"); }
    rint(): TSelf { return this.unop("rint"); }
    /** Truncate towards zero to an integer value (`as_int` on the wire). */
    asInt(): TSelf { return this.unop("as_int"); }
    /** The identity that documents a value as a float (`as_float`). */
    asFloat(): TSelf { return this.unop("as_float"); }
    squared(): TSelf { return this.unop("squared"); }
    cubed(): TSelf { return this.unop("cubed"); }
    recip(): TSelf { return this.unop("recip"); }
    frac(): TSelf { return this.unop("frac"); }
    sign(): TSelf { return this.unop("sign"); }
    sinh(): TSelf { return this.unop("sinh"); }
    cosh(): TSelf { return this.unop("cosh"); }
    tanh(): TSelf { return this.unop("tanh"); }
    distort(): TSelf { return this.unop("distort"); }
    softclip(): TSelf { return this.unop("softclip"); }
    midicps(): TSelf { return this.unop("midicps"); }
    cpsmidi(): TSelf { return this.unop("cpsmidi"); }
    midiratio(): TSelf { return this.unop("midiratio"); }
    ratiomidi(): TSelf { return this.unop("ratiomidi"); }
    dbamp(): TSelf { return this.unop("dbamp"); }
    ampdb(): TSelf { return this.unop("ampdb"); }
    octcps(): TSelf { return this.unop("octcps"); }
    cpsoct(): TSelf { return this.unop("cpsoct"); }
}

/**
 * Shared behaviour of the graph leaves (`Ugen`, `Control`): a leaf op
 * against a scalar or another leaf yields a `Ugen`, against a list a
 * `ChannelList`.
 */
export abstract class GraphLeaf extends SynthExpr<Ugen> {
    protected binop<T extends OpOperand>(
        selector: string,
        other: T,
    ): OpResult<Ugen, T> {
        // A list operand fans the op out: this leaf stays on the left of
        // every pair, so it is the list that composes, with the sides
        // swapped.
        if (isList(other)) {
            return new ChannelList(other).rcomposeWith(
                selector,
                this,
            ) as OpResult<Ugen, T>;
        }
        return this.composeWith(selector, other as GraphInput) as OpResult<Ugen, T>;
    }

    protected unop(selector: string): Ugen {
        return this.unopWith(selector);
    }

    /** @internal — this leaf on the **left** of a binary op. */
    composeWith(selector: string, other: GraphInput): Ugen {
        return leafOp(selector, this, other);
    }

    /** @internal — this leaf on the **right** of a binary op. */
    rcomposeWith(selector: string, other: GraphInput): Ugen {
        return leafOp(selector, other, this);
    }

    /** @internal — the unary op, wherever it was reached from. */
    unopWith(selector: string): Ugen {
        if (!UNOP_OPS.has(selector)) {
            throw new TypeError(`no unary UGen for operator '${selector}'`);
        }
        return new Ugen("UnaryOpUGen", [this], { op: selector });
    }

    /** This node repeated (by reference) as `n` channels — see `dup`. */
    dup(n = 2): ChannelList {
        return new ChannelList(Array.from({ length: n }, () => this as GraphInput));
    }
}

/**
 * The one place a binary UGen is built: the four arithmetic selectors keep
 * their dedicated alias kinds, everything else becomes a `BinaryOpUGen`
 * carrying the operator name.
 */
function leafOp(selector: string, left: GraphInput, right: GraphInput): Ugen {
    const kind = BINOP_UGEN[selector];
    if (kind !== undefined) return new Ugen(kind, [left, right]);
    if (!BINOP_OPS.has(selector)) {
        throw new TypeError(`no binary UGen for operator '${selector}'`);
    }
    return new Ugen("BinaryOpUGen", [left, right], { op: selector });
}

/**
 * One UGen node (one output). `kind` is a server UGen name; `inputs` is a
 * list of operands, each a `Ugen`, a `Control`, or a plain number (a
 * constant). Build them with the lowercase callables below rather than
 * directly.
 *
 * `rate` is the optional output calculation rate (`"ir"`/`"kr"`/`"ar"`/
 * `"dr"`); `undefined` lets the server pick the kind's default. Set it
 * fluently with `atRate`. `op` is the operator **name** carried by the
 * generic `BinaryOpUGen`/`UnaryOpUGen`; `label` is the string tag the
 * side-effect UGens carry (`sendReply`'s command name, `poll`'s label);
 * `static` holds any other non-signal fields (the delay lines' `max_delay`,
 * the spectral UGens' `fft_size`), merged verbatim into the serialized spec.
 */
export class Ugen extends GraphLeaf {
    readonly kind: string;
    readonly inputs: GraphInput[];
    rate?: UgenRate;
    readonly op?: string;
    readonly label?: string;
    readonly staticFields?: Record<string, unknown>;

    constructor(
        kind: string,
        inputs: readonly GraphInput[],
        extra: {
            rate?: UgenRate;
            op?: string;
            label?: string;
            static?: Record<string, unknown>;
        } = {},
    ) {
        super();
        this.kind = kind;
        this.inputs = [...inputs];
        this.rate = extra.rate;
        this.op = extra.op;
        this.label = extra.label;
        this.staticFields = extra.static;
    }

    /**
     * Set this UGen's output rate and return it, e.g.
     * `sine(5.0).atRate("kr")` for a control-rate LFO.
     */
    atRate(rate: UgenRate): Ugen {
        this.rate = rate;
        return this;
    }
}

/** A UGen output's calculation rate. */
export type UgenRate = "ir" | "kr" | "ar" | "dr";

/** Control types the server accepts (with their spellings). */
export type ControlRate = "kr" | "control" | "tr" | "trigger" | "ir" | "scalar";

const CONTROL_RATES = new Set([
    "kr", "control", "tr", "trigger", "ir", "scalar",
]);

/**
 * A named control (a `/s_new`/`/n_set` parameter) with a default and an
 * optional **type** and **lag**, mirroring the server's control types:
 *
 * - `rate: "tr"` — a **trigger**: an `/n_set` holds for one block, then the
 *   server resets it to 0 (drives an `envGen` gate, a sample-and-hold).
 * - `rate: "ir"` — a **scalar**: read once at init and frozen; a later
 *   `/n_set` is ignored. As `ir` it may feed an `ir` input (`rand`, the
 *   buffer-info UGens).
 * - `lag` (seconds) — smooth a `kr` control's changes with an implicit
 *   one-pole the server inserts; `lagDown` gives a separate downward time.
 *
 * Used as a UGen input it serializes to a `{"control": index}` reference;
 * the `SynthDef` gathers the controls a graph references, in first-seen
 * order.
 */
export class Control extends GraphLeaf {
    readonly name: string;
    readonly default: number;
    readonly rate?: ControlRate;
    readonly lag?: number;
    readonly lagDown?: number;

    constructor(
        name: string,
        defaultValue = 0.0,
        options: { rate?: ControlRate; lag?: number; lagDown?: number } = {},
    ) {
        super();
        this.name = String(name);
        this.default = Number(defaultValue);
        this.rate = options.rate;
        this.lag = options.lag;
        this.lagDown = options.lagDown;
        if (this.rate !== undefined && !CONTROL_RATES.has(this.rate)) {
            throw new TypeError(
                `unknown control type '${this.rate}'; use one of ` +
                    `${[...CONTROL_RATES].sort().join(", ")}`,
            );
        }
        if (this.lagDown !== undefined && this.lag === undefined) {
            throw new TypeError("lagDown requires lag (the up time)");
        }
    }

    /** The full identity used to detect conflicting reuses of a name. */
    signature(): string {
        return JSON.stringify([this.default, this.rate ?? null, this.lag ?? null,
            this.lagDown ?? null]);
    }
}

/**
 * A named control (`/s_new`/`/n_set` parameter). `rate` is its type (`"tr"`
 * trigger, `"ir"` scalar, or the default `kr`); `lag` (with an optional
 * `lagDown`) smooths a `kr` control. See `Control`.
 */
export function control(
    name: string,
    defaultValue = 0.0,
    options: { rate?: ControlRate; lag?: number; lagDown?: number } = {},
): Control {
    return new Control(name, defaultValue, options);
}

function checkChannel(m: unknown): GraphInput {
    if (m instanceof ChannelList) {
        throw new TypeError(
            "nested channel lists are not supported: mix() the inner one down " +
                "or build a flat list",
        );
    }
    if (!isLeaf(m) && typeof m !== "number") {
        throw new TypeError(`not a UGen graph node: ${String(m)}`);
    }
    return m;
}

// The four arithmetic selectors on two plain numbers are exactly JS's `+ - *
// /`, bit-identical to the core's builtins, so a constant pair folds here.
// Any other selector between two constants would need the core's builtins
// table, which the web client does not carry yet — refuse rather than
// diverge numerically from the server.
const NUMERIC_FOLD: Record<string, (a: number, b: number) => number> = {
    add: (a, b) => a + b,
    sub: (a, b) => a - b,
    mul: (a, b) => a * b,
    div: (a, b) => a / b,
};

function channelBinop(a: GraphInput, selector: string, b: GraphInput): GraphInput {
    if (isLeaf(a)) return a.composeWith(selector, b);
    if (isLeaf(b)) return b.rcomposeWith(selector, a);
    const fold = NUMERIC_FOLD[selector];
    if (fold === undefined) {
        throw new TypeError(
            `'${selector}' between two constants needs the core's builtins, ` +
                "which this client does not carry yet: compute it in JS, or " +
                "make one side a graph node",
        );
    }
    return fold(a, b);
}

function channelUnop(m: GraphInput, selector: string): GraphInput {
    if (isLeaf(m)) return m.unopWith(selector);
    if (selector === "neg") return -m;
    throw new TypeError(
        `'${selector}' on a constant needs the core's builtins, which this ` +
            "client does not carry yet: compute it in JS, or make it a node",
    );
}

/**
 * An ordered list of channels — the client's multichannel container.
 *
 * Members are graph leaves (`Ugen`/`Control`) or plain numbers. The math
 * methods map over the members and return a new `ChannelList`: a scalar
 * operand **broadcasts** to every channel, a list operand **zips**
 * channel-wise, and unequal lengths wrap the shorter one modulo.
 *
 * The container never crosses the wire: `out` and friends unroll it onto
 * consecutive buses, and the `SynthDef` serialization flattens it — the
 * server only ever sees single-channel UGens. Feeding one to a
 * single-channel input is an error: index it or `mix` it down. Build one
 * with `dup`, `chans`, or a literal array where one is accepted.
 */
export class ChannelList extends SynthExpr<ChannelList> {
    readonly items: GraphInput[];

    constructor(items: ChannelList | readonly GraphInput[]) {
        super();
        const source = items instanceof ChannelList ? items.items : items;
        const members = [...source].map(checkChannel);
        if (members.length === 0) {
            throw new TypeError("a channel list needs at least one channel");
        }
        this.items = members;
    }

    get length(): number {
        return this.items.length;
    }

    at(i: number): GraphInput {
        const got = this.items.at(i);
        if (got === undefined) throw new RangeError(`no channel ${i}`);
        return got;
    }

    [Symbol.iterator](): Iterator<GraphInput> {
        return this.items[Symbol.iterator]();
    }

    /**
     * Channel pairs for a binary op: broadcast a scalar, zip a list
     * (wrapping the shorter side modulo).
     */
    private pairs(other: OpOperand): [GraphInput, GraphInput][] {
        if (isList(other)) {
            const o = new ChannelList(other).items;
            const n = Math.max(this.items.length, o.length);
            return Array.from({ length: n }, (_unused, i): [GraphInput, GraphInput] => [
                this.items[i % this.items.length]!,
                o[i % o.length]!,
            ]);
        }
        return this.items.map((m): [GraphInput, GraphInput] => [m, other]);
    }

    /** @internal — the leaf side of a mixed op reaches back through here. */
    composeWith(selector: string, other: OpOperand): ChannelList {
        return new ChannelList(
            this.pairs(other).map(([a, b]) => channelBinop(a, selector, b)),
        );
    }

    /** @internal — as `composeWith`, with the operands swapped. */
    rcomposeWith(selector: string, other: OpOperand): ChannelList {
        return new ChannelList(
            this.pairs(other).map(([a, b]) => channelBinop(b, selector, a)),
        );
    }

    protected binop<T extends OpOperand>(
        selector: string,
        other: T,
    ): OpResult<ChannelList, T> {
        return this.composeWith(selector, other) as OpResult<ChannelList, T>;
    }

    protected unop(selector: string): ChannelList {
        return new ChannelList(this.items.map((m) => channelUnop(m, selector)));
    }

    /** Sets every member's output rate (see `Ugen.atRate`). */
    atRate(rate: UgenRate): ChannelList {
        for (const m of this.items) if (m instanceof Ugen) m.atRate(rate);
        return this;
    }

    /** This list folded to one channel — see `mix`. */
    mix(): GraphInput {
        return mix(this);
    }
}

/** A `ChannelList` from the arguments (`chans(a, b)`) or from a single array. */
export function chans(
    ...items: readonly GraphInput[] | [ChannelList | readonly GraphInput[]]
): ChannelList {
    if (items.length === 1 && isList(items[0])) return new ChannelList(items[0]);
    return new ChannelList(items as readonly GraphInput[]);
}

/**
 * `x` as `n` channels.
 *
 * A graph node (or a number) is repeated **by reference** — the graph
 * serializes it once, fanned out to every channel, so `dup(sine(440))` is a
 * cheap mono→stereo: identical channels. A **function** is called `n` times
 * — `dup(whiteNoise, 8)` builds `n` *distinct* UGens, which is what a
 * decorrelated or detuned bank needs; duplicating a `whiteNoise` by
 * reference would give `n` copies of the same noise.
 */
export function dup(
    x: GraphInput | (() => GraphInput),
    n = 2,
): ChannelList {
    if (!Number.isInteger(n) || n < 1) {
        throw new TypeError(`dup needs a positive channel count, got ${n}`);
    }
    if (typeof x === "function") {
        return new ChannelList(Array.from({ length: n }, () => x()));
    }
    return new ChannelList(Array.from({ length: n }, () => x));
}

/**
 * `x` folded to one channel by summing.
 *
 * The inverse gesture of `dup`: a `ChannelList` (or plain array) becomes one
 * signal, folded with the fused sum kinds — `sum4`/`sum3` chunks instead of
 * an `Add` chain, so an 8-channel mix costs 2 UGens + 1, not 7. A scalar or
 * single node passes through.
 */
export function mix(x: OpOperand): GraphInput {
    if (!isList(x)) return x;
    let items = new ChannelList(x).items;
    if (items.every((m) => !isLeaf(m))) {
        return (items as number[]).reduce((total, m) => total + m, 0);
    }
    while (items.length > 1) {
        const folded: GraphInput[] = [];
        for (let k = 0; k < items.length; k += 4) {
            const chunk = items.slice(k, k + 4);
            if (chunk.length === 4) {
                folded.push(sum4(chunk[0]!, chunk[1]!, chunk[2]!, chunk[3]!));
            } else if (chunk.length === 3) {
                folded.push(sum3(chunk[0]!, chunk[1]!, chunk[2]!));
            } else if (chunk.length === 2) {
                folded.push(channelBinop(chunk[0]!, "add", chunk[1]!));
            } else {
                folded.push(chunk[0]!);
            }
        }
        items = folded;
    }
    return items[0]!;
}

// ---- lowercase UGen callables (the client's "instruction set") ----
// Input order matches the server's registry; see docs/schemas.md.

// --- oscillators and sources ---

/** Sine by f64 phase accumulation, starting at phase 0. */
export const sine = (freq: GraphInput = 440.0): Ugen => new Ugen("Sine", [freq]);

/**
 * A single-sample `1.0` every `freq` Hz, `0.0` between (`freq` 0 = one
 * impulse then silence). The first sample is always an impulse.
 */
export const impulse = (freq: GraphInput = 1.0): Ugen => new Ugen("Impulse", [freq]);

/** Uniform white noise in ±1. */
export const whiteNoise = (): Ugen => new Ugen("WhiteNoise", []);

/** Noise with equal power per octave (−3 dB/octave). */
export const pinkNoise = (): Ugen => new Ugen("PinkNoise", []);

/** Brownian noise (−6 dB/octave): a bounded random walk. */
export const brownNoise = (): Ugen => new Ugen("BrownNoise", []);

/** Noise whose spectrum is flat to the *ear* rather than to a meter. */
export const grayNoise = (): Ugen => new Ugen("GrayNoise", []);

/** Noise that is only ever −1 or +1: white noise hard-clipped. */
export const clipNoise = (): Ugen => new Ugen("ClipNoise", []);

/** Steps to a new random value `freq` times a second, holding it between. */
export const lfNoise0 = (freq: GraphInput = 500.0): Ugen => new Ugen("LFNoise0", [freq]);

/** Ramps linearly between random values at `freq` per second. */
export const lfNoise1 = (freq: GraphInput = 500.0): Ugen => new Ugen("LFNoise1", [freq]);

/** Quadratically interpolated random values at `freq` per second. */
export const lfNoise2 = (freq: GraphInput = 500.0): Ugen => new Ugen("LFNoise2", [freq]);

/** `lfNoise0`, clipped: steps between −1 and +1 only. */
export const lfClipNoise = (freq: GraphInput = 500.0): Ugen =>
    new Ugen("LFClipNoise", [freq]);

/** Random impulses in 0..1 at an average `density` per second. */
export const dust = (density: GraphInput = 1.0): Ugen => new Ugen("Dust", [density]);

/** `dust` with bipolar impulses (−1..1). */
export const dust2 = (density: GraphInput = 1.0): Ugen => new Ugen("Dust2", [density]);

/** A chaotic noise source (the logistic map); `chaos` in 0..2. */
export const crackle = (chaos: GraphInput = 1.5): Ugen => new Ugen("Crackle", [chaos]);

/** Band-limited sawtooth (PolyBLEP), falling from +1 to −1. */
export const saw = (freq: GraphInput = 440.0): Ugen => new Ugen("Saw", [freq]);

/** Band-limited pulse (PolyBLEP); `width` is the duty cycle in 0..1. */
export const pulse = (freq: GraphInput = 440.0, width: GraphInput = 0.5): Ugen =>
    new Ugen("Pulse", [freq, width]);

/** Naive (aliasing) sawtooth — cheap, meant for control rate. */
export const lfSaw = (freq: GraphInput = 440.0, iphase: GraphInput = 0.0): Ugen =>
    new Ugen("LFSaw", [freq, iphase]);

/** Naive (aliasing) pulse — cheap, meant for control rate. */
export const lfPulse = (
    freq: GraphInput = 440.0,
    iphase: GraphInput = 0.0,
    width: GraphInput = 0.5,
): Ugen => new Ugen("LFPulse", [freq, iphase, width]);

/** Naive (aliasing) triangle — cheap, meant for control rate. */
export const lfTri = (freq: GraphInput = 440.0, iphase: GraphInput = 0.0): Ugen =>
    new Ugen("LFTri", [freq, iphase]);

/**
 * A sawtooth whose peak position is `width`: from a ramp up through a
 * triangle to a ramp down.
 */
export const varSaw = (
    freq: GraphInput = 440.0,
    iphase: GraphInput = 0.0,
    width: GraphInput = 0.5,
): Ugen => new Ugen("VarSaw", [freq, iphase, width]);

/**
 * A ramp from `start` to `end` advancing by `rate` per sample, wrapping and
 * restarting at `resetPos` on each trigger — the phase source `bufRd` reads.
 */
export const phasor = (
    trig: GraphInput = 0.0,
    rate: GraphInput = 1.0,
    start: GraphInput = 0.0,
    end: GraphInput = 1.0,
    resetPos: GraphInput = 0.0,
): Ugen => new Ugen("Phasor", [trig, rate, start, end, resetPos]);

// --- filters ---

/** Resolves the mutually exclusive `rq`/`q` pair into a wire `rq`. */
function resonance(rq?: GraphInput, q?: GraphInput): GraphInput {
    if (q === undefined) return rq ?? 1.0;
    if (rq !== undefined) throw new TypeError("give either rq or q, not both");
    if (typeof q === "number") {
        if (q === 0) {
            throw new TypeError("q must be non-zero; use rq=0 for infinite Q");
        }
        return 1.0 / q;
    }
    return q.recip();
}

/** The resonance of the two-pole filters: `rq` (1/Q, 0 = infinite) or `q`. */
export interface Resonance {
    rq?: GraphInput;
    q?: GraphInput;
}

/** Second-order Butterworth lowpass: −3 dB at `freq`, −12 dB/octave. */
export const lpf = (signal: GraphInput, freq: GraphInput = 440.0): Ugen =>
    new Ugen("LPF", [signal, freq]);

/** Second-order Butterworth highpass: −3 dB at `freq`, −12 dB/octave. */
export const hpf = (signal: GraphInput, freq: GraphInput = 440.0): Ugen =>
    new Ugen("HPF", [signal, freq]);

/** Resonant lowpass; unity gain at DC. */
export const rlpf = (
    signal: GraphInput,
    freq: GraphInput = 440.0,
    res: Resonance = {},
): Ugen => new Ugen("RLPF", [signal, freq, resonance(res.rq, res.q)]);

/** Resonant highpass; unity gain at Nyquist. */
export const rhpf = (
    signal: GraphInput,
    freq: GraphInput = 440.0,
    res: Resonance = {},
): Ugen => new Ugen("RHPF", [signal, freq, resonance(res.rq, res.q)]);

/** Bandpass with **unity gain at the centre**; `rq` is its bandwidth ratio. */
export const bpf = (
    signal: GraphInput,
    freq: GraphInput = 440.0,
    res: Resonance = {},
): Ugen => new Ugen("BPF", [signal, freq, resonance(res.rq, res.q)]);

/** Band reject (notch); unity gain in both passbands, a true null at `freq`. */
export const brf = (
    signal: GraphInput,
    freq: GraphInput = 440.0,
    res: Resonance = {},
): Ugen => new Ugen("BRF", [signal, freq, resonance(res.rq, res.q)]);

/** Resonator with unity gain at the peak — the same structure as `bpf`. */
export const resonz = (
    signal: GraphInput,
    freq: GraphInput = 440.0,
    res: Resonance = {},
): Ugen => new Ugen("Resonz", [signal, freq, resonance(res.rq, res.q)]);

/** One-pole filter: `coef` positive lowpasses, negative highpasses. */
export const onePole = (signal: GraphInput, coef: GraphInput = 0.5): Ugen =>
    new Ugen("OnePole", [signal, coef]);

/** One-zero filter: `coef` positive lowpasses, negative highpasses. */
export const oneZero = (signal: GraphInput, coef: GraphInput = 0.5): Ugen =>
    new Ugen("OneZero", [signal, coef]);

/**
 * Removes the DC offset with a very low corner — what a feedback loop or an
 * asymmetric waveshaper leaves behind.
 */
export const leakDc = (signal: GraphInput, coef: GraphInput = 0.995): Ugen =>
    new Ugen("LeakDC", [signal, coef]);

/** A leaky integrator: `y[n] = x[n] + coef·y[n-1]`. */
export const integrator = (signal: GraphInput, coef: GraphInput = 0.999): Ugen =>
    new Ugen("Integrator", [signal, coef]);

// --- delay lines ---
//
// `maxDelay` is **static**: it sizes the line the server allocates when the
// synth is built, so it cannot grow later and a `delaytime` past it is
// clamped. Left unset it follows a constant `delaytime`, which is what a
// fixed delay wants; a *modulated* delaytime has to state its longest reach.

function lineSize(
    kind: string,
    delaytime: GraphInput,
    maxDelay?: number,
): Record<string, unknown> {
    if (maxDelay === undefined) {
        if (typeof delaytime !== "number") {
            throw new TypeError(
                `${kind}: a modulated delaytime needs an explicit maxDelay ` +
                    "(it sizes the line, and the line is allocated once)",
            );
        }
        maxDelay = delaytime;
    }
    return { max_delay: Number(maxDelay) };
}

/** Pure delay, no interpolation: the delay is rounded to whole samples. */
export const delayN = (
    signal: GraphInput,
    delaytime: GraphInput = 0.2,
    maxDelay?: number,
): Ugen =>
    new Ugen("DelayN", [signal, delaytime], {
        static: lineSize("DelayN", delaytime, maxDelay),
    });

/** Delay with linear interpolation — the one a modulated delaytime wants. */
export const delayL = (
    signal: GraphInput,
    delaytime: GraphInput = 0.2,
    maxDelay?: number,
): Ugen =>
    new Ugen("DelayL", [signal, delaytime], {
        static: lineSize("DelayL", delaytime, maxDelay),
    });

/** Delay with cubic interpolation: smoother under modulation than `delayL`. */
export const delayC = (
    signal: GraphInput,
    delaytime: GraphInput = 0.2,
    maxDelay?: number,
): Ugen =>
    new Ugen("DelayC", [signal, delaytime], {
        static: lineSize("DelayC", delaytime, maxDelay),
    });

/**
 * Comb filter (feedback delay), no interpolation. `decaytime` is the time to
 * fall 60 dB; negative inverts the feedback.
 */
export const combN = (
    signal: GraphInput,
    delaytime: GraphInput = 0.2,
    decaytime: GraphInput = 1.0,
    maxDelay?: number,
): Ugen =>
    new Ugen("CombN", [signal, delaytime, decaytime], {
        static: lineSize("CombN", delaytime, maxDelay),
    });

/** Comb filter with linear interpolation. */
export const combL = (
    signal: GraphInput,
    delaytime: GraphInput = 0.2,
    decaytime: GraphInput = 1.0,
    maxDelay?: number,
): Ugen =>
    new Ugen("CombL", [signal, delaytime, decaytime], {
        static: lineSize("CombL", delaytime, maxDelay),
    });

/** Comb filter with cubic interpolation. */
export const combC = (
    signal: GraphInput,
    delaytime: GraphInput = 0.2,
    decaytime: GraphInput = 1.0,
    maxDelay?: number,
): Ugen =>
    new Ugen("CombC", [signal, delaytime, decaytime], {
        static: lineSize("CombC", delaytime, maxDelay),
    });

/** Schroeder allpass (the reverb building block), no interpolation. */
export const allpassN = (
    signal: GraphInput,
    delaytime: GraphInput = 0.2,
    decaytime: GraphInput = 1.0,
    maxDelay?: number,
): Ugen =>
    new Ugen("AllpassN", [signal, delaytime, decaytime], {
        static: lineSize("AllpassN", delaytime, maxDelay),
    });

/** Schroeder allpass with linear interpolation. */
export const allpassL = (
    signal: GraphInput,
    delaytime: GraphInput = 0.2,
    decaytime: GraphInput = 1.0,
    maxDelay?: number,
): Ugen =>
    new Ugen("AllpassL", [signal, delaytime, decaytime], {
        static: lineSize("AllpassL", delaytime, maxDelay),
    });

/** Schroeder allpass with cubic interpolation. */
export const allpassC = (
    signal: GraphInput,
    delaytime: GraphInput = 0.2,
    decaytime: GraphInput = 1.0,
    maxDelay?: number,
): Ugen =>
    new Ugen("AllpassC", [signal, delaytime, decaytime], {
        static: lineSize("AllpassC", delaytime, maxDelay),
    });

// --- the stereo field ---
//
// Every panner takes the channel it is computing as its last input; that is
// the builder's business, and never an argument here.

/**
 * Places a mono `signal` between two channels at `pos` (−1 left, 0 centre,
 * 1 right), at **equal power**: the two gains hold `l² + r² = 1`, so a
 * source keeps one loudness as it crosses the field. The price is that the
 * centre is 0.707 in each channel — use `linPan2` when it is the summed
 * amplitude that has to stay put.
 */
export const pan2 = (
    signal: GraphInput,
    pos: GraphInput = 0.0,
    level: GraphInput = 1.0,
): ChannelList =>
    chans([0.0, 1.0].map((c) => new Ugen("Pan2", [signal, pos, level, c])));

/**
 * `pan2` with the **constant-amplitude** law: the two gains sum to `level`
 * at every position, 0.5 each at the centre.
 */
export const linPan2 = (
    signal: GraphInput,
    pos: GraphInput = 0.0,
    level: GraphInput = 1.0,
): ChannelList =>
    chans([0.0, 1.0].map((c) => new Ugen("LinPan2", [signal, pos, level, c])));

/**
 * Shifts an **already stereo** pair towards one side by attenuating the
 * other, at equal power. A centred `balance2` is not a pass-through: both
 * sides come back 3 dB down.
 */
export const balance2 = (
    left: GraphInput,
    right: GraphInput,
    pos: GraphInput = 0.0,
    level: GraphInput = 1.0,
): ChannelList =>
    chans([0.0, 1.0].map((c) => new Ugen("Balance2", [left, right, pos, level, c])));

/** Equal-power crossfade between two signals: −1 is all `a`, 1 is all `b`. */
export const xfade2 = (
    a: GraphInput,
    b: GraphInput,
    pan: GraphInput = 0.0,
    level: GraphInput = 1.0,
): Ugen => new Ugen("XFade2", [a, b, pan, level]);

/**
 * Crossfade with the constant-amplitude law — the right one for correlated
 * sources.
 */
export const linXfade2 = (
    a: GraphInput,
    b: GraphInput,
    pan: GraphInput = 0.0,
    level: GraphInput = 1.0,
): Ugen => new Ugen("LinXFade2", [a, b, pan, level]);

function sources(list: readonly GraphInput[] | [ChannelList | readonly GraphInput[]]) {
    const items = list.length === 1 && isList(list[0])
        ? new ChannelList(list[0]).items
        : (list as readonly GraphInput[]);
    if (items.length === 0) throw new TypeError("a selector needs at least one source");
    return [...items];
}

/**
 * Outputs one of `sources`, chosen by the `which` index (truncated, and
 * clamped to the ends rather than wrapping). Every source runs whether or
 * not it is selected — they are UGens in the graph, not branches — so this
 * picks what is *heard*, never what is computed.
 */
export const select = (
    which: GraphInput,
    ...items: readonly GraphInput[] | [ChannelList | readonly GraphInput[]]
): Ugen => new Ugen("Select", [which, ...sources(items)]);

/**
 * `select` with the index's fraction crossfading to the next source, at
 * equal power: `which = 0.5` is halfway between the first two.
 */
export const selectX = (
    which: GraphInput,
    ...items: readonly GraphInput[] | [ChannelList | readonly GraphInput[]]
): Ugen => new Ugen("SelectX", [which, ...sources(items)]);

/**
 * Spreads `signals` evenly across the stereo field and mixes them down to
 * two channels — one `pan2` per signal, summed. A client-side convenience,
 * not a UGen; unlike sclang's, it does not normalize behind your back.
 */
export function splay(
    signals: ChannelList | readonly GraphInput[],
    spread = 1.0,
    level: GraphInput = 1.0,
    center = 0.0,
): ChannelList {
    const items = new ChannelList(signals).items;
    const n = items.length;
    const span = n === 1
        ? [0.0]
        : Array.from({ length: n }, (_unused, i) => (i / (n - 1)) * 2.0 - 1.0);
    const panned = items.map((s, i) => pan2(s, center + span[i]! * spread, level));
    // Mix each side down separately, so the fold uses the fused sums instead
    // of an Add chain per channel.
    return chans([
        mix(panned.map((p) => p.at(0))),
        mix(panned.map((p) => p.at(1))),
    ]);
}

// --- bus I/O ---

/**
 * Reads an audio bus (sampled per block). Named `in_` because `in` is a
 * reserved word — the wire name is still `In`.
 */
export const in_ = (bus: GraphInput = 0.0): Ugen => new Ugen("In", [bus]);

/** Reads a control-bus value, constant over the block. */
export const inCtl = (bus: GraphInput = 0.0): Ugen => new Ugen("InCtl", [bus]);

/**
 * One writer per channel on consecutive buses (`bus`, `bus+1`, …) — the
 * point where a channel list becomes buses. The base `bus` must be a number:
 * a signal bus cannot be offset per channel client-side.
 */
function outChannels(
    kind: string,
    bus: GraphInput,
    signal: ChannelList | readonly GraphInput[],
): ChannelList {
    if (typeof bus !== "number") {
        throw new TypeError(
            `a multichannel ${kind} needs a constant bus to lay channels on ` +
                "consecutive buses",
        );
    }
    const sig = new ChannelList(signal);
    return new ChannelList(sig.items.map((s, i) => new Ugen(kind, [bus + i, s])));
}

/**
 * Sums `signal` into the audio `bus` (output happens only here). A channel
 * list writes its channels to consecutive buses: `out(0, dup(sig))` is a
 * stereo output.
 */
export function out(bus: GraphInput, signal: GraphInput): Ugen;
export function out(bus: GraphInput, signal: ChannelList | readonly GraphInput[]): ChannelList;
export function out(
    bus: GraphInput,
    signal: GraphInput | ChannelList | readonly GraphInput[],
): Ugen | ChannelList {
    if (isList(signal)) return outChannels("Out", bus, signal);
    return new Ugen("Out", [bus, signal]);
}

/** Overwrites the audio `bus` with `signal` instead of summing. */
export function replaceOut(bus: GraphInput, signal: GraphInput): Ugen;
export function replaceOut(
    bus: GraphInput,
    signal: ChannelList | readonly GraphInput[],
): ChannelList;
export function replaceOut(
    bus: GraphInput,
    signal: GraphInput | ChannelList | readonly GraphInput[],
): Ugen | ChannelList {
    if (isList(signal)) return outChannels("ReplaceOut", bus, signal);
    return new Ugen("ReplaceOut", [bus, signal]);
}

/**
 * Writes `signal`'s latest per-block value to a **control** `bus` — the
 * write side of `inCtl`. Passes `signal` through as its output.
 */
export function outCtl(bus: GraphInput, signal: GraphInput): Ugen;
export function outCtl(
    bus: GraphInput,
    signal: ChannelList | readonly GraphInput[],
): ChannelList;
export function outCtl(
    bus: GraphInput,
    signal: GraphInput | ChannelList | readonly GraphInput[],
): Ugen | ChannelList {
    if (isList(signal)) return outChannels("OutCtl", bus, signal);
    return new Ugen("OutCtl", [bus, signal]);
}

// --- side-effect UGens: reply / observe, no `out` required ---
//
// These emit OSC replies or console posts on a trigger instead of audio. A
// SynthDef may contain only these and no `out(...)` at all; pass them as
// roots of the `SynthDef` (nothing else would reach them).

/**
 * On each trigger of `trig`, sends `/tr nodeID id value` to `/notify`
 * clients. Output is silence; pass it as a `SynthDef` root.
 */
export const sendTrig = (
    trig: GraphInput,
    id: GraphInput = 0,
    value: GraphInput = 0.0,
): Ugen => new Ugen("SendTrig", [trig, id, value]);

/**
 * On each trigger of `trig`, sends the OSC message `cmd nodeID replyId
 * value…` to `/notify` clients. Output is silence; pass it as a root.
 */
export const sendReply = (
    trig: GraphInput,
    values: readonly GraphInput[] = [],
    { cmd = "/reply", replyId = -1 }: { cmd?: string; replyId?: number } = {},
): Ugen => new Ugen("SendReply", [trig, replyId, ...values], { label: cmd });

/**
 * On each trigger of `trig`, posts `label: value` to the server console and,
 * when `trigId >= 0`, also sends `/tr nodeID trigId value`. `signal` passes
 * through the output, so `poll` can sit mid-chain.
 */
export const poll = (
    trig: GraphInput,
    signal: GraphInput,
    label = "poll",
    trigId: GraphInput = -1,
): Ugen => new Ugen("Poll", [trig, signal, trigId], { label });

// --- buffer players and table oscillators ---

/**
 * Mono buffer player with linear interpolation; `rate` is frames per output
 * sample (1.0 = server rate).
 */
export const playBuf = (
    bufnum: GraphInput,
    chan: GraphInput = 0.0,
    rate: GraphInput = 1.0,
    loop: GraphInput = 0.0,
): Ugen => new Ugen("PlayBuf", [bufnum, chan, rate, loop]);

/** Reads a buffer at a `phase` signal in frames (linear interpolation). */
export const bufRd = (
    bufnum: GraphInput,
    chan: GraphInput,
    phase: GraphInput,
    loop: GraphInput = 0.0,
): Ugen => new Ugen("BufRd", [bufnum, chan, phase, loop]);

/**
 * Interpolating wavetable oscillator; `bufnum` must hold a
 * **wavetable-format** buffer.
 */
export const osc = (
    bufnum: GraphInput,
    freq: GraphInput = 440.0,
    phase: GraphInput = 0.0,
): Ugen => new Ugen("Osc", [bufnum, freq, phase]);

/** Non-interpolating oscillator over a **plain** (non-wavetable) buffer. */
export const oscN = (
    bufnum: GraphInput,
    freq: GraphInput = 440.0,
    phase: GraphInput = 0.0,
): Ugen => new Ugen("OscN", [bufnum, freq, phase]);

/**
 * Like `osc` but the buffer number is a signal: reads wavetables `bufpos`
 * and `bufpos + 1` and crossfades by the fractional part.
 */
export const vosc = (
    bufpos: GraphInput,
    freq: GraphInput = 440.0,
    phase: GraphInput = 0.0,
): Ugen => new Ugen("VOsc", [bufpos, freq, phase]);

/**
 * Waveshaper: maps `signal` (in ±1, clamped) through a transfer table in
 * wavetable format (typically a `cheby` `/b_gen`).
 */
export const shaper = (bufnum: GraphInput, signal: GraphInput): Ugen =>
    new Ugen("Shaper", [bufnum, signal]);

/** The number of frames in a buffer, block-constant (`kr`). */
export const bufFrames = (bufnum: GraphInput): Ugen =>
    new Ugen("BufFrames", [bufnum], { rate: "kr" });

/** The buffer's own sample rate (Hz), block-constant (`kr`). */
export const bufSampleRate = (bufnum: GraphInput): Ugen =>
    new Ugen("BufSampleRate", [bufnum], { rate: "kr" });

/**
 * `fileSr / serverSr`, block-constant (`kr`); feed `playBuf`'s `rate` to
 * play at the file's true pitch without the client knowing either rate.
 */
export const bufRateScale = (bufnum: GraphInput): Ugen =>
    new Ugen("BufRateScale", [bufnum], { rate: "kr" });

/** The buffer's channel count, block-constant (`kr`). */
export const bufChannels = (bufnum: GraphInput): Ugen =>
    new Ugen("BufChannels", [bufnum], { rate: "kr" });

/** The buffer's duration in seconds, block-constant (`kr`). */
export const bufDur = (bufnum: GraphInput): Ugen =>
    new Ugen("BufDur", [bufnum], { rate: "kr" });

// --- synth-private feedback ---

/**
 * Reads synth-private feedback channel `channel` (a constant); pairs with
 * `localOut` for one-block feedback. `LocalIn` must precede its `LocalOut`
 * — the `SynthDef`'s topological order does that as long as the output
 * graph reaches the `localIn` before the `localOut`.
 */
export const localIn = (channel: GraphInput = 0.0): Ugen =>
    new Ugen("LocalIn", [channel]);

/**
 * Writes `signal` into synth-private feedback channel `channel`; also passes
 * `signal` through as its output (so it can be a SynthDef root, which keeps
 * the write in the graph).
 */
export const localOut = (channel: GraphInput, signal: GraphInput): Ugen =>
    new Ugen("LocalOut", [channel, signal]);

// --- fused arithmetic (the forms the server optimizes) ---

/** `a*b + c` in one UGen (the multiply-accumulate the server fuses). */
export const madd = (a: GraphInput, b: GraphInput, c: GraphInput): Ugen =>
    new Ugen("MulAdd", [a, b, c]);

/** `a + b + c` in one UGen. */
export const sum3 = (a: GraphInput, b: GraphInput, c: GraphInput): Ugen =>
    new Ugen("Sum3", [a, b, c]);

/** `a + b + c + d` in one UGen. */
export const sum4 = (
    a: GraphInput,
    b: GraphInput,
    c: GraphInput,
    d: GraphInput,
): Ugen => new Ugen("Sum4", [a, b, c, d]);

// --- one-pole smoothers ---

/**
 * One-pole smoother: `signal` lagged over `time` seconds (symmetric); `time`
 * 0 passes through. The same UGen the server inserts for a lagged control.
 */
export const lag = (signal: GraphInput, time: GraphInput = 0.1): Ugen =>
    new Ugen("Lag", [signal, time]);

/** One-pole smoother with separate rise (`up`) and fall (`down`) times. */
export const varLag = (
    signal: GraphInput,
    up: GraphInput = 0.1,
    down: GraphInput = 0.1,
): Ugen => new Ugen("VarLag", [signal, up, down]);

// --- triggers and control ---
//
// A **trigger** is a signal crossing from <= 0 up to > 0 — one definition,
// shared by every callable here and by `demand`, `sendTrig` and friends.

/**
 * Holds the **level the input had at the trigger** for `dur` seconds, then
 * 0. Use `trig1` when all you want is a 1.
 */
export const trig = (signal: GraphInput, dur: GraphInput = 0.1): Ugen =>
    new Ugen("Trig", [signal, dur]);

/** Holds 1 for `dur` seconds after each trigger, whatever level triggered it. */
export const trig1 = (signal: GraphInput, dur: GraphInput = 0.1): Ugen =>
    new Ugen("Trig1", [signal, dur]);

/**
 * One sample of 1, `dur` seconds after each trigger. A trigger arriving
 * while one is already in flight is **dropped**, not queued.
 */
export const tDelay = (signal: GraphInput, dur: GraphInput = 0.1): Ugen =>
    new Ugen("TDelay", [signal, dur]);

/**
 * Sample and hold: takes one sample of `signal` at each rising edge of
 * `trig` and holds it until the next one.
 */
export const latch = (signal: GraphInput, trigger: GraphInput = 0.0): Ugen =>
    new Ugen("Latch", [signal, trigger]);

/**
 * Passes `signal` while `trigger` is above zero and **freezes** at the last
 * value when it is not — transparent for as long as the gate is open.
 */
export const gate = (signal: GraphInput, trigger: GraphInput = 0.0): Ugen =>
    new Ugen("Gate", [signal, trigger]);

/**
 * A comparator with hysteresis: 1 once `signal` rises past `hi`, 0 once it
 * falls past `lo`, unchanged in between.
 */
export const schmidt = (
    signal: GraphInput,
    lo: GraphInput = 0.0,
    hi: GraphInput = 1.0,
): Ugen => new Ugen("Schmidt", [signal, lo, hi]);

/**
 * Flips between 0 and 1 on each trigger — a divider by two of the
 * *triggers*, not of the signal.
 */
export const toggleFf = (trigger: GraphInput = 0.0): Ugen =>
    new Ugen("ToggleFF", [trigger]);

/**
 * 1 from the first `trigger`, 0 from the next `reset`. Both on the same
 * sample leaves it at 0: reset is applied second.
 */
export const setResetFf = (
    trigger: GraphInput = 0.0,
    reset: GraphInput = 0.0,
): Ugen => new Ugen("SetResetFF", [trigger, reset]);

/** Counts triggers, from 1; a rising `reset` puts it back to 0. */
export const pulseCount = (
    trigger: GraphInput = 0.0,
    reset: GraphInput = 0.0,
): Ugen => new Ugen("PulseCount", [trigger, reset]);

/**
 * One trigger out for every `div` in. `start` is where the counter begins,
 * read once — set it to `div - 1` to fire on the very first trigger.
 */
export const pulseDivider = (
    trigger: GraphInput = 0.0,
    div: GraphInput = 2.0,
    start: GraphInput = 0.0,
): Ugen => new Ugen("PulseDivider", [trigger, div, start]);

/**
 * A counter that walks `[min, max]` — **both ends included** — one `step`
 * per trigger, wrapping. It sits at `resetval` until the first trigger,
 * which lands on `resetval + step`.
 */
export const stepper = (
    trigger: GraphInput = 0.0,
    reset: GraphInput = 0.0,
    min: GraphInput = 0.0,
    max: GraphInput = 7.0,
    step: GraphInput = 1.0,
    resetval: GraphInput = 0.0,
): Ugen => new Ugen("Stepper", [trigger, reset, min, max, step, resetval]);

/** The time in seconds between the last two triggers, held between them. */
export const timer = (trigger: GraphInput = 0.0): Ugen =>
    new Ugen("Timer", [trigger]);

/**
 * A ramp rising at `rate` per second, restarted at each trigger. It is
 * already running before the first one, so `sweep(0, 1)` is the node's age.
 */
export const sweep = (
    trigger: GraphInput = 0.0,
    rate: GraphInput = 1.0,
): Ugen => new Ugen("Sweep", [trigger, rate]);

/**
 * 1 on any sample where `signal` moved by more than `threshold`. It compares
 * the **halved** difference, `|(x[n] − x[n−1]) / 2|`, matching sclang's
 * `HPZ1`-derived definition.
 */
export const changed = (
    signal: GraphInput,
    threshold: GraphInput = 0.0,
): Ugen => new Ugen("Changed", [signal, threshold]);

/**
 * Turns each impulse into an exponential falling 60 dB in `decaytime`. Its
 * attack is instantaneous, which clicks — see `decay2`.
 */
export const decay = (signal: GraphInput, decaytime: GraphInput = 1.0): Ugen =>
    new Ugen("Decay", [signal, decaytime]);

/** `decay` minus a second, faster decay, which rounds the attack. */
export const decay2 = (
    signal: GraphInput,
    attacktime: GraphInput = 0.01,
    decaytime: GraphInput = 1.0,
): Ugen => new Ugen("Decay2", [signal, attacktime, decaytime]);

// --- scalar / init-rate (ir) ---

/** The engine sample rate in Hz, computed once at init (`ir`). */
export const sampleRate = (): Ugen => new Ugen("SampleRate", [], { rate: "ir" });

/**
 * One uniform random value in `[lo, hi)`, drawn once at synth init and held
 * for the node's life (`ir`); `lo`/`hi` must be constants or `ir`.
 */
export const rand = (lo: GraphInput = 0.0, hi: GraphInput = 1.0): Ugen =>
    new Ugen("Rand", [lo, hi], { rate: "ir" });

// --- demand rate (dr) ---
//
// A demand UGen is a *stream*: it has no samples, only a next value, and it
// yields one each time a driver asks. Its inputs may be streams too.
// `repeats` is how many the stream yields before it ends: **0 means
// endlessly** (sclang writes `inf`, which a def cannot carry).

function demandValues(values: ChannelList | readonly GraphInput[]): GraphInput[] {
    const items = values instanceof ChannelList ? [...values.items] : [...values];
    if (items.length === 0) {
        throw new TypeError("a demand source needs at least one value");
    }
    return items;
}

/**
 * A demand sequence: yields `values` in order, `repeats` times (`0`
 * endlessly), then ends. A value may be another demand stream, and then it
 * is *drained* rather than taken once.
 */
export const dseq = (
    values: ChannelList | readonly GraphInput[],
    repeats: GraphInput = 0.0,
): Ugen => new Ugen("Dseq", [repeats, ...demandValues(values)], { rate: "dr" });

/**
 * `repeats` items drawn at random from `values`, each pick independent of
 * the last. Unlike `dseq`, the count is of items, not passes.
 */
export const drand = (
    values: ChannelList | readonly GraphInput[],
    repeats: GraphInput = 0.0,
): Ugen => new Ugen("Drand", [repeats, ...demandValues(values)], { rate: "dr" });

/**
 * Demand driver: pulls the next value from `source` on each rising edge of
 * `trigger` and holds it between triggers; a rising `reset` restarts the
 * stream. Once the stream ends the last value is held.
 */
export const demand = (
    trigger: GraphInput,
    reset: GraphInput,
    source: GraphInput,
): Ugen => new Ugen("Demand", [trigger, reset, source]);

// --- envelopes (EnvGen) ---

/**
 * The action `envGen` takes when its envelope finishes — scsynth's full
 * done-action set (0–15). The relative actions act on the synth's neighbours
 * in its group; a paused node is resumed with `Server.run` (`/n_run`).
 */
export const DoneAction = {
    /** Do nothing; the envelope just holds its final level. */
    NONE: 0,
    /** Pause the synth (stops processing; it stays in the tree). */
    PAUSE_SELF: 1,
    /** Free the synth — the usual choice for a one-shot or a released note. */
    FREE_SELF: 2,
    FREE_SELF_AND_PREV: 3,
    FREE_SELF_AND_NEXT: 4,
    FREE_SELF_AND_FREE_ALL_IN_PREV: 5,
    FREE_SELF_AND_FREE_ALL_IN_NEXT: 6,
    FREE_SELF_TO_HEAD: 7,
    FREE_SELF_TO_TAIL: 8,
    FREE_SELF_PAUSE_PREV: 9,
    FREE_SELF_PAUSE_NEXT: 10,
    FREE_SELF_AND_DEEP_FREE_PREV: 11,
    FREE_SELF_AND_DEEP_FREE_NEXT: 12,
    FREE_ALL_IN_GROUP: 13,
    /** Free the synth's whole enclosing group. */
    FREE_GROUP: 14,
    FREE_SELF_RESUME_NEXT: 15,
} as const;

export type DoneAction = (typeof DoneAction)[keyof typeof DoneAction];

/**
 * Envelope shape name → the server's shape number. A numeric curve value
 * maps to the custom-curvature shape (5) instead.
 */
const SHAPE_NUMBERS: Record<string, number> = {
    step: 0,
    lin: 1,
    linear: 1,
    exp: 2,
    exponential: 2,
    sin: 3,
    sine: 3,
    wel: 4,
    welch: 4,
    sqr: 6,
    squared: 6,
    cub: 7,
    cubed: 7,
    hold: 8,
};

/**
 * A segment shape: a name, or a numeric curvature (0 linear, positive starts
 * slow, negative starts fast).
 */
export type Curve = string | number;

/**
 * A shape name (`"lin"`, `"exp"`, `"sin"`, …) or a numeric curvature as the
 * wire's `[shape, curve]` pair. A number selects the custom-curvature shape,
 * so a drawn segment and a played one agree by construction — which is why
 * the GuiDef `bpf`/`clip` builders resolve their break-points through here.
 */
export function resolveCurve(spec: Curve): [number, number] {
    if (typeof spec === "string") {
        const shape = SHAPE_NUMBERS[spec];
        if (shape === undefined) {
            throw new TypeError(
                `unknown envelope shape '${spec}'; use one of ` +
                    `${[...new Set(Object.keys(SHAPE_NUMBERS))].sort().join(", ")} ` +
                    "or a numeric curvature",
            );
        }
        return [shape, 0.0];
    }
    return [5, Number(spec)];
}

/**
 * A breakpoint envelope: `levels` (one more than `times`), the segment
 * `times` in seconds, and a `curve` per segment (a shape name, a numeric
 * curvature, or an array of either, one per segment).
 *
 * `releaseNode` is the index into `levels` where the envelope sustains while
 * the gate is held (`undefined` = no sustain, plays straight through). Feed
 * it to `envGen`.
 */
export class Env {
    readonly levels: number[];
    readonly times: number[];
    readonly curves: Curve[];
    readonly releaseNode?: number;
    readonly loopNode?: number;

    constructor(
        levels: readonly number[],
        times: readonly number[],
        curve: Curve | readonly Curve[] = "lin",
        options: { releaseNode?: number; loopNode?: number } = {},
    ) {
        this.levels = levels.map(Number);
        this.times = times.map(Number);
        if (this.levels.length !== this.times.length + 1) {
            throw new TypeError(
                `levels (${this.levels.length}) must be one longer than ` +
                    `times (${this.times.length})`,
            );
        }
        if (Array.isArray(curve)) {
            if (curve.length !== this.times.length) {
                throw new TypeError(
                    `curve list (${curve.length}) must match the number of ` +
                        `segments (${this.times.length})`,
                );
            }
            this.curves = [...(curve as readonly Curve[])];
        } else {
            this.curves = this.times.map(() => curve as Curve);
        }
        this.releaseNode = options.releaseNode;
        this.loopNode = options.loopNode;
    }

    /**
     * A fixed-duration percussive hit: 0 → `level` → 0. No sustain, so a
     * rising gate triggers the whole thing.
     */
    static perc(attack = 0.01, release = 1.0, level = 1.0, curve: Curve = -4.0): Env {
        return new Env([0.0, level, 0.0], [attack, release], curve);
    }

    /**
     * The classic attack/decay/sustain/release. Sustains at `peak * sustain`
     * (the release node) until the gate falls.
     */
    static adsr(
        attack = 0.01,
        decay = 0.3,
        sustain = 0.5,
        release = 1.0,
        peak = 1.0,
        curve: Curve = -4.0,
    ): Env {
        return new Env(
            [0.0, peak, peak * sustain, 0.0],
            [attack, decay, release],
            curve,
            { releaseNode: 2 },
        );
    }

    /** Attack to `sustain`, hold there until release, then fall to 0. */
    static asr(attack = 0.01, sustain = 1.0, release = 1.0, curve: Curve = -4.0): Env {
        return new Env([0.0, sustain, 0.0], [attack, release], curve, {
            releaseNode: 1,
        });
    }

    /**
     * A step sequence: **each value held for its duration** — `levels` and
     * `times` have the *same* length, unlike the constructor.
     */
    static step(
        levels: readonly number[],
        times: readonly number[],
        options: { releaseNode?: number; loopNode?: number } = {},
    ): Env {
        if (levels.length !== times.length) {
            throw new TypeError(
                `Env.step: levels (${levels.length}) and times ` +
                    `(${times.length}) must have the same length`,
            );
        }
        if (levels.length === 0) throw new TypeError("Env.step needs at least one level");
        return new Env([levels[0]!, ...levels], times, "step", options);
    }

    /**
     * The envelope as the flat number list `envGen` appends after its fixed
     * inputs: `initLevel, numSegments, releaseNode, loopNode` then `target,
     * duration, shape, curve` per segment.
     */
    toInputs(): number[] {
        const n = this.times.length;
        const rel = this.releaseNode ?? -1.0;
        const loop = this.loopNode ?? -1.0;
        const out: number[] = [this.levels[0]!, n, rel, loop];
        for (let i = 0; i < n; i++) {
            const [shape, cval] = resolveCurve(this.curves[i]!);
            out.push(this.levels[i + 1]!, this.times[i]!, shape, cval);
        }
        return out;
    }
}

/**
 * An `Env` (levels / segment times / curves) as the flat `bpf` breakpoint
 * list `[t, v, shape, curve, …]`, with absolute times starting at `timeAt`.
 * The last point carries a linear placeholder (no segment leaves it). Feed
 * the result to the `bpf` widget or to a live `points` set.
 */
export function envToPoints(env: Env, { timeAt = 0.0 }: { timeAt?: number } = {}): number[] {
    const out: number[] = [];
    let t = timeAt;
    for (let i = 0; i < env.levels.length; i++) {
        const [shape, curve] =
            i < env.times.length ? resolveCurve(env.curves[i]!) : [1, 0.0];
        out.push(t, env.levels[i]!, shape, curve);
        if (i < env.times.length) t += env.times[i]!;
    }
    return out;
}

/**
 * A `bpf` breakpoint list — the flat `t v shape curve …` quads a `"points"`
 * event carries — as an `Env`: absolute times become segment durations and
 * each segment keeps its shape (the numeric curvature for the custom shape,
 * the shape name otherwise).
 *
 * A first breakpoint later than `timeAt` (default `0.0`) is a drawn initial
 * delay, encoded as a leading `hold` segment (the first level held for that
 * duration) so what was drawn and what plays stay identical. `releaseNode`
 * and `loopNode` pass through to the `Env`.
 */
export function pointsToEnv(
    points: readonly number[],
    {
        timeAt = 0.0,
        releaseNode,
        loopNode,
    }: { timeAt?: number; releaseNode?: number; loopNode?: number } = {},
): Env {
    const quads: number[][] = [];
    for (let i = 0; i + 4 <= points.length; i += 4) {
        quads.push(points.slice(i, i + 4) as number[]);
    }
    if (quads.length < 2) {
        throw new TypeError("an envelope needs at least two breakpoints");
    }
    // First name wins for the aliased numbers ("lin"/"exp"/… come before
    // their long forms in the table).
    const names = new Map<number, string>();
    for (const [name, num] of Object.entries(SHAPE_NUMBERS)) {
        if (!names.has(num)) names.set(num, name);
    }
    const levels = quads.map((q) => q[1]!);
    const times = quads.slice(1).map((q, i) => q[0]! - quads[i]![0]!);
    const curves: Curve[] = quads
        .slice(0, -1)
        .map((q) => (Math.trunc(q[2]!) === 5 ? q[3]! : (names.get(Math.trunc(q[2]!)) ?? "lin")));
    const delay = quads[0]![0]! - timeAt;
    if (delay > 1e-9) {
        levels.unshift(levels[0]!);
        times.unshift(delay);
        curves.unshift("hold");
    }
    return new Env(levels, times, curves, { releaseNode, loopNode });
}

/**
 * Plays an `Env`. A rising `gate` (re)triggers from the start; while the
 * gate is held the envelope sustains at the env's release node; when the
 * gate falls it plays the release segments. `levelScale`/`levelBias` affine
 * the output, `timeScale` stretches every segment. `doneAction` is taken
 * when the envelope finishes.
 */
export function envGen(
    env: Env,
    {
        gate: gateInput = 1.0,
        levelScale = 1.0,
        levelBias = 0.0,
        timeScale = 1.0,
        doneAction = DoneAction.NONE,
    }: {
        gate?: GraphInput;
        levelScale?: GraphInput;
        levelBias?: GraphInput;
        timeScale?: GraphInput;
        doneAction?: number;
    } = {},
): Ugen {
    return new Ugen("EnvGen", [
        gateInput,
        levelScale,
        levelBias,
        timeScale,
        Number(doneAction),
        ...env.toInputs(),
    ]);
}

/**
 * 1 once `signal` has stayed within ±`amp` for `time` seconds, with the
 * `doneAction` taken then. The counter restarts on the first sample that
 * exceeds `amp`, so what it measures is *uninterrupted* silence.
 */
export const detectSilence = (
    signal: GraphInput,
    amp: GraphInput = 0.0001,
    time: GraphInput = 0.1,
    doneAction: number = DoneAction.NONE,
): Ugen => new Ugen("DetectSilence", [signal, amp, time, Number(doneAction)]);

/**
 * A single ramp from `start` to `end` over `dur` seconds, then held — an
 * `envGen` with one linear segment, taking the same `DoneAction` set.
 */
export const line = (
    start: GraphInput = 0.0,
    end: GraphInput = 1.0,
    dur: GraphInput = 1.0,
    doneAction: number = DoneAction.NONE,
): Ugen => new Ugen("Line", [start, end, dur, Number(doneAction)]);

/**
 * `line` in equal *ratios* rather than equal steps — the shape that reads as
 * straight when it drives a frequency or a gain. `start` and `end` must be
 * non-zero and share a sign.
 */
export const xLine = (
    start: GraphInput = 0.01,
    end: GraphInput = 1.0,
    dur: GraphInput = 1.0,
    doneAction: number = DoneAction.NONE,
): Ugen => new Ugen("XLine", [start, end, dur, Number(doneAction)]);

/**
 * Frees the enclosing synth while `signal` is greater than zero, passing it
 * through unchanged — the trigger-driven counterpart of a `DoneAction`.
 */
export const freeSelf = (signal: GraphInput): Ugen => new Ugen("FreeSelf", [signal]);

/**
 * Pauses the enclosing synth while `signal` is greater than zero, passing it
 * through. Resume with `Server.run`.
 */
export const pauseSelf = (signal: GraphInput): Ugen => new Ugen("PauseSelf", [signal]);

/**
 * 1 once `source` has finished, 0 before — a trigger the rest of the graph
 * can read. `source` must be a UGen that *can* finish (`envGen`, `line`,
 * `xLine`); the server rejects the def by name otherwise.
 */
export const done = (source: GraphInput): Ugen => new Ugen("Done", [source]);

/**
 * Passes `source` through and frees the synth once it has finished — the
 * idiom for an envelope whose own `doneAction` is `NONE` because something
 * else in the graph still needs it.
 */
export const freeSelfWhenDone = (source: GraphInput): Ugen =>
    new Ugen("FreeSelfWhenDone", [source]);

// ---- the free binary functions (the number-on-the-left case) ----

/**
 * `a + b`, either side a node or a number — the free form of `.add()`, for
 * when the constant is on the left.
 */
export const add = (a: OpOperand, b: OpOperand): GraphInput | ChannelList =>
    freeBinop("add", a, b);
/** `a − b`; see `add`. */
export const sub = (a: OpOperand, b: OpOperand): GraphInput | ChannelList =>
    freeBinop("sub", a, b);
/** `a × b`; see `add`. */
export const mul = (a: OpOperand, b: OpOperand): GraphInput | ChannelList =>
    freeBinop("mul", a, b);
/** `a ÷ b`; see `add`. */
export const div = (a: OpOperand, b: OpOperand): GraphInput | ChannelList =>
    freeBinop("div", a, b);

function freeBinop(
    selector: string,
    a: OpOperand,
    b: OpOperand,
): GraphInput | ChannelList {
    if (isList(a)) return new ChannelList(a).composeWith(selector, b);
    if (isList(b)) return new ChannelList(b).rcomposeWith(selector, a);
    return channelBinop(a, selector, b);
}
