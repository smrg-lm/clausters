// The graph itself: a UGen node, a control, a channel list (mirrors
// `clausters/defs/ugens/graph.py`).
//
// The types every other module in this package builds on — `Ugen` (one node,
// one output), `Control` (a def's parameter) and `ChannelList` (multichannel
// as an explicit container, never implicit expansion) — plus the fused
// arithmetic the server has dedicated kinds for.
//
// **Composition is by method, not by operator.** TypeScript has no operator
// overloading, so where the Python client writes `sine(freq) * amp` this one
// writes `sine(freq).mul(amp)`, and every other operator or math method
// (`mod`, `min`/`max`, comparisons, `.sin()`, `.midicps()`, `.distort()` …)
// is a method carrying the same operator **name** the wire uses — so the two
// clients emit identical specs. The free `add`/`sub`/`mul`/`div` functions at
// the bottom take the number-on-the-left case (`sub(1, sig)`), which a method
// cannot.

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
/**
 * @internal — the operator names a `BinaryOpUGen` may carry. Exported for
 * `defs/pv_expr`, which validates the same vocabulary per bin, as the Python
 * package keeps `_BINOP_OPS` importable for the same reason.
 */
export const BINOP_OPS = new Set([
    "mod", "pow", "min", "max", "atan2", "gt", "lt", "ge", "le", "eq", "ne",
    "bitand", "bitor", "bitxor", "lshift", "rshift", "hypot", "ring1", "ring2",
    "ring3", "ring4", "sumsqr", "difsqr", "sqrsum", "sqrdif", "absdif",
    "thresh", "clip2", "excess", "round", "trunc", "fold2", "wrap2", "gcd",
    "lcm", "hypot_apx",
]);
/** @internal — the operator names a `UnaryOpUGen` may carry; see `BINOP_OPS`. */
export const UNOP_OPS = new Set([
    "neg", "abs", "sin", "cos", "tan", "asin", "acos", "atan", "exp", "log",
    "log10", "log2", "sqrt", "floor", "ceil", "rint", "as_int", "as_float",
    "squared", "cubed", "recip", "frac", "sign", "sinh", "cosh", "tanh",
    "distort", "softclip", "midicps", "cpsmidi", "midiratio", "ratiomidi",
    "dbamp", "ampdb", "octcps", "cpsoct",
]);

/**
 * One channel of signal: a leaf node (`Ugen`/`Control`) or a plain number (a
 * constant). What every single-channel input accepts, and what a
 * `ChannelList` holds — the server only ever sees these.
 */
export type Channel = SynthLeaf | number;
/** What a math method accepts: a single channel, or a list of them. */
export type OpOperand = Channel | ChannelList | readonly Channel[];

/**
 * A math method's result: a list operand fans the result out, anything else
 * keeps the receiver's shape.
 */
export type OpResult<TSelf, TOther> = TOther extends ChannelList | readonly Channel[]
    ? ChannelList
    : TSelf;

const isLeaf = (x: unknown): x is SynthLeaf => x instanceof SynthLeaf;
/**
 * @internal — whether an operand is a multichannel one. Exported for the
 * family modules that branch on it (`pan`'s selectors, `io`'s writers), the
 * way the Python package shares its own underscored helpers between families.
 */
export const isList = (x: unknown): x is ChannelList | readonly Channel[] =>
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
 *
 * That surface is the one thing here that is not about UGens — it is the
 * operator vocabulary of the shared builtins table — so `TOperand` opens it
 * to a subclass whose operands are not channels: `defs/pv_expr`'s per-bin
 * terms compose the same names into their own tree. (The Python client keeps
 * the vocabulary in `base.absobject` for the same reason.)
 */
export abstract class SynthExpr<TSelf, TOperand = OpOperand> {
    protected abstract binop<T extends TOperand>(
        selector: string,
        other: T,
    ): OpResult<TSelf, T>;
    protected abstract unop(selector: string): TSelf;

    // --- binary ---
    add<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("add", x); }
    sub<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("sub", x); }
    mul<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("mul", x); }
    div<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("div", x); }
    mod<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("mod", x); }
    pow<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("pow", x); }
    min<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("min", x); }
    max<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("max", x); }
    atan2<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("atan2", x); }
    gt<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("gt", x); }
    lt<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("lt", x); }
    ge<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("ge", x); }
    le<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("le", x); }
    eq<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("eq", x); }
    ne<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("ne", x); }
    bitand<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("bitand", x); }
    bitor<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("bitor", x); }
    bitxor<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("bitxor", x); }
    leftshift<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("lshift", x); }
    rightshift<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("rshift", x); }
    hypot<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("hypot", x); }
    /** The cheap hypotenuse approximation (`hypot_apx` on the wire). */
    hypotapx<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("hypot_apx", x); }
    ring1<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("ring1", x); }
    ring2<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("ring2", x); }
    ring3<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("ring3", x); }
    ring4<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("ring4", x); }
    sumsqr<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("sumsqr", x); }
    difsqr<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("difsqr", x); }
    sqrsum<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("sqrsum", x); }
    sqrdif<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("sqrdif", x); }
    absdif<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("absdif", x); }
    thresh<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("thresh", x); }
    clip2<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("clip2", x); }
    excess<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("excess", x); }
    round<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("round", x); }
    trunc<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("trunc", x); }
    fold2<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("fold2", x); }
    wrap2<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("wrap2", x); }
    gcd<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("gcd", x); }
    lcm<T extends TOperand>(x: T): OpResult<TSelf, T> { return this.binop("lcm", x); }

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
    asinteger(): TSelf { return this.unop("as_int"); }
    /** The identity that documents a value as a float (`as_float`). */
    asfloat(): TSelf { return this.unop("as_float"); }
    squared(): TSelf { return this.unop("squared"); }
    cubed(): TSelf { return this.unop("cubed"); }
    reciprocal(): TSelf { return this.unop("recip"); }
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
 * A single-channel expression: the leaves of the graph (`Ugen`, `Control`),
 * as opposed to the `ChannelList` that holds several. A leaf op against a
 * scalar or another leaf yields a `Ugen`, against a list a `ChannelList`.
 */
export abstract class SynthLeaf extends SynthExpr<Ugen> {
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
        return this.composeWith(selector, other as Channel) as OpResult<Ugen, T>;
    }

    protected unop(selector: string): Ugen {
        return this.unopWith(selector);
    }

    /** @internal — this leaf on the **left** of a binary op. */
    composeWith(selector: string, other: Channel): Ugen {
        return leafOp(selector, this, other);
    }

    /** @internal — this leaf on the **right** of a binary op. */
    rcomposeWith(selector: string, other: Channel): Ugen {
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
        return new ChannelList(Array.from({ length: n }, () => this as Channel));
    }
}

/**
 * The one place a binary UGen is built: the four arithmetic selectors keep
 * their dedicated alias kinds, everything else becomes a `BinaryOpUGen`
 * carrying the operator name.
 */
function leafOp(selector: string, left: Channel, right: Channel): Ugen {
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
export class Ugen extends SynthLeaf {
    readonly kind: string;
    readonly inputs: Channel[];
    rate?: UgenRate;
    readonly op?: string;
    readonly label?: string;
    readonly staticFields?: Record<string, unknown>;

    constructor(
        kind: string,
        inputs: readonly Channel[],
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
 * A named control (a `/synth_new`/`/node_set` parameter) with a default and an
 * optional **type** and **lag**, mirroring the server's control types:
 *
 * - `rate: "tr"` — a **trigger**: an `/node_set` holds for one block, then the
 *   server resets it to 0 (drives an `envGen` gate, a sample-and-hold).
 * - `rate: "ir"` — a **scalar**: read once at init and frozen; a later
 *   `/node_set` is ignored. As `ir` it may feed an `ir` input (`rand`, the
 *   buffer-info UGens).
 * - `lag` (seconds) — smooth a `kr` control's changes with an implicit
 *   one-pole the server inserts; `lagDown` gives a separate downward time.
 *
 * Used as a UGen input it serializes to a `{"control": index}` reference;
 * the `SynthDef` gathers the controls a graph references, in first-seen
 * order.
 */
export class Control extends SynthLeaf {
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
 * A named control (`/synth_new`/`/node_set` parameter). `rate` is its type (`"tr"`
 * trigger, `"ir"` scalar, or the default `kr`); `lag` (with an optional
 * `lagDown`) smooths a `kr` control. See `Control`.
 *
 * A control declares **no range**: it is a signal in a graph, and the range a
 * knob is drawn over is the knob's (`knob(freq, { min: 110.0, max: 880.0 })`).
 * The one exception is a FaustDef, whose `hslider` declares its range inside the
 * DSP and reports it back — see `ControlInfo`.
 */
export function control(
    name: string,
    defaultValue = 0.0,
    options: { rate?: ControlRate; lag?: number; lagDown?: number } = {},
): Control {
    return new Control(name, defaultValue, options);
}

function checkChannel(m: unknown): Channel {
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
// `min`/`max` fold beside them for a different reason: they *select* an
// operand rather than compute one, so there is no rounding to disagree about
// and the precision question does not arise. (`abs`, in `channelUnop` below,
// is the unary of the same kind — it only clears a sign bit.)
const NUMERIC_FOLD: Record<string, (a: number, b: number) => number> = {
    add: (a, b) => a + b,
    sub: (a, b) => a - b,
    mul: (a, b) => a * b,
    div: (a, b) => a / b,
    min: (a, b) => Math.min(a, b),
    max: (a, b) => Math.max(a, b),
};

/**
 * @internal — a binary op between two operands either of which may be a
 * constant. Exported for the family modules that compose arithmetic of their
 * own (`filter`'s `svfMorph`), the way the Python package shares its own
 * underscored helper.
 */
export function channelBinop(a: Channel, selector: string, b: Channel): Channel {
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

/** @internal — the unary counterpart of `channelBinop`. */
export function channelUnop(m: Channel, selector: string): Channel {
    if (isLeaf(m)) return m.unopWith(selector);
    if (selector === "neg") return -m;
    if (selector === "abs") return Math.abs(m);
    throw new TypeError(
        `'${selector}' on a constant needs the core's builtins, which this ` +
            "client does not carry yet: compute it in JS, or make it a node",
    );
}

/**
 * An ordered list of channels — the client's multichannel container.
 *
 * Members are `Channel`s — a leaf (`Ugen`/`Control`) or a number. The math
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
    readonly items: Channel[];

    constructor(items: ChannelList | readonly Channel[]) {
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

    at(i: number): Channel {
        const got = this.items.at(i);
        if (got === undefined) throw new RangeError(`no channel ${i}`);
        return got;
    }

    [Symbol.iterator](): Iterator<Channel> {
        return this.items[Symbol.iterator]();
    }

    /**
     * Channel pairs for a binary op: broadcast a scalar, zip a list
     * (wrapping the shorter side modulo).
     */
    private pairs(other: OpOperand): [Channel, Channel][] {
        if (isList(other)) {
            const o = new ChannelList(other).items;
            const n = Math.max(this.items.length, o.length);
            return Array.from({ length: n }, (_unused, i): [Channel, Channel] => [
                this.items[i % this.items.length]!,
                o[i % o.length]!,
            ]);
        }
        return this.items.map((m): [Channel, Channel] => [m, other]);
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
    mix(): Channel {
        return mix(this);
    }
}

/** A `ChannelList` from the arguments (`chans(a, b)`) or from a single array. */
export function chans(
    ...items: readonly Channel[] | [ChannelList | readonly Channel[]]
): ChannelList {
    if (items.length === 1 && isList(items[0])) return new ChannelList(items[0]);
    return new ChannelList(items as readonly Channel[]);
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
    x: Channel | (() => Channel),
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
export function mix(x: OpOperand): Channel {
    if (!isList(x)) return x;
    let items = new ChannelList(x).items;
    if (items.every((m) => !isLeaf(m))) {
        return (items as number[]).reduce((total, m) => total + m, 0);
    }
    while (items.length > 1) {
        const folded: Channel[] = [];
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

// ---- fused arithmetic (the forms the server optimizes) ----

/** `a*b + c` in one UGen (the multiply-accumulate the server fuses). */
export const madd = (a: Channel, b: Channel, c: Channel): Ugen =>
    new Ugen("MulAdd", [a, b, c]);

/** `a + b + c` in one UGen. */
export const sum3 = (a: Channel, b: Channel, c: Channel): Ugen =>
    new Ugen("Sum3", [a, b, c]);

/** `a + b + c + d` in one UGen. */
export const sum4 = (
    a: Channel,
    b: Channel,
    c: Channel,
    d: Channel,
): Ugen => new Ugen("Sum4", [a, b, c, d]);

// ---- the free binary functions (the number-on-the-left case) ----

/**
 * `a + b`, either side a node or a number — the free form of `.add()`, for
 * when the constant is on the left.
 */
export const add = (a: OpOperand, b: OpOperand): Channel | ChannelList =>
    freeBinop("add", a, b);
/** `a − b`; see `add`. */
export const sub = (a: OpOperand, b: OpOperand): Channel | ChannelList =>
    freeBinop("sub", a, b);
/** `a × b`; see `add`. */
export const mul = (a: OpOperand, b: OpOperand): Channel | ChannelList =>
    freeBinop("mul", a, b);
/** `a ÷ b`; see `add`. */
export const div = (a: OpOperand, b: OpOperand): Channel | ChannelList =>
    freeBinop("div", a, b);

function freeBinop(
    selector: string,
    a: OpOperand,
    b: OpOperand,
): Channel | ChannelList {
    if (isList(a)) return new ChannelList(a).composeWith(selector, b);
    if (isList(b)) return new ChannelList(b).rcomposeWith(selector, a);
    return channelBinop(a, selector, b);
}
