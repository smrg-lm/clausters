// The operator surface every expression composes with (mirrors
// `clausters/base/absobject.py`).
//
// **Composition is by method, not by operator.** TypeScript has no operator
// overloading, so where the Python client writes `sine(freq) * amp` this one
// writes `sine(freq).mul(amp)`, and every other operator or math method
// (`mod`, `min`/`max`, comparisons, `.sin()`, `.midicps()`, `.distort()` …) is
// a method carrying the same operator **name** the wire uses — so the two
// clients emit identical specs.
//
// Every method routes through two hooks a subclass implements, `binop` and
// `unop`, which is what lets the *same* written expression compose a UGen
// graph (`defs/ugens/graph`) or a per-bin program (`defs/pv_expr`) depending
// on the subclass. The selectors are the shared builtins table's names, so
// this class is about the **vocabulary** and knows nothing about UGens.
//
// It lives here, beside the value functions that compute the same operators
// (`base/builtins`), for the reason the Python client keeps `AbstractObject`
// in `base/`: it is neither a def nor a graph. The two type parameters after
// `TSelf` is what a typed language needs and an untyped one does not: [`Fan`]
// says which operand *fans a result out* (a channel list, in the UGen graph)
// and into what, and is `Fan<never, never>` where nothing does.

/**
 * Which operand kind fans a result out, and into what. One type because the
 * two only ever travel together: `Fan<ChannelList, ChannelList>` for the UGen
 * graph, the default for an expression where a method always answers itself.
 */
export interface Fan<TList, TFan> {
    list: TList;
    fan: TFan;
}

/**
 * A math method's result: an operand of the fanning kind yields `F["fan"]`,
 * anything else keeps the receiver's shape.
 */
export type Composed<TSelf, TOther, F extends Fan<unknown, unknown>> =
    TOther extends F["list"] ? F["fan"] : TSelf;

/**
 * The operator vocabulary as methods, over the two composition hooks.
 *
 *  * `TSelf` is what a unary op answers, `TOperand` what a binary one accepts,
 * and `F` the fan-out pair described above.
 */
export abstract class AbstractObject<
    TSelf,
    TOperand,
    F extends Fan<unknown, unknown> = Fan<never, never>,
> {
    protected abstract binop<T extends TOperand>(
        selector: string,
        other: T,
    ): Composed<TSelf, T, F>;
    protected abstract unop(selector: string): TSelf;

    // --- binary ---
    add<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("add", x); }
    sub<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("sub", x); }
    mul<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("mul", x); }
    div<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("div", x); }
    mod<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("mod", x); }
    pow<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("pow", x); }
    min<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("min", x); }
    max<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("max", x); }
    atan2<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("atan2", x); }
    gt<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("gt", x); }
    lt<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("lt", x); }
    ge<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("ge", x); }
    le<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("le", x); }
    eq<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("eq", x); }
    ne<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("ne", x); }
    bitand<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("bitand", x); }
    bitor<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("bitor", x); }
    bitxor<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("bitxor", x); }
    leftshift<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("lshift", x); }
    rightshift<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("rshift", x); }
    hypot<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("hypot", x); }
    /** The cheap hypotenuse approximation (`hypotapx` on the wire). */
    hypotapx<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("hypotapx", x); }
    ring1<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("ring1", x); }
    ring2<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("ring2", x); }
    ring3<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("ring3", x); }
    ring4<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("ring4", x); }
    sumsqr<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("sumsqr", x); }
    difsqr<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("difsqr", x); }
    sqrsum<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("sqrsum", x); }
    sqrdif<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("sqrdif", x); }
    absdif<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("absdif", x); }
    thresh<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("thresh", x); }
    clip2<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("clip2", x); }
    excess<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("excess", x); }
    round<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("round", x); }
    trunc<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("trunc", x); }
    fold2<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("fold2", x); }
    wrap2<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("wrap2", x); }
    gcd<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("gcd", x); }
    lcm<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("lcm", x); }

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
    /** Truncate towards zero to an integer value (`asint` on the wire). */
    asinteger(): TSelf { return this.unop("asint"); }
    /** The identity that documents a value as a float (`asfloat`). */
    asfloat(): TSelf { return this.unop("asfloat"); }
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
