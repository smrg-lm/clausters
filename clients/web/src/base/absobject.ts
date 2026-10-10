// The operator surface every expression composes with (mirrors
// `clausters/base/absobject.py`).
//
// **Composition is by method, not by operator.** TypeScript has no operator
// overloading, so where the Python client writes `sine(freq) * amp` this one
// writes `sine(freq).mul(amp)`, and every other operator or math method
// (`mod`, `min`/`max`, comparisons, `.sin()`, `.midicps()`, `.distort()` ...) is
// a method carrying the same operator **name** the wire uses -- so the two
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

    /**
     * The n-ary hook: `selector` names a range map, a bound, a draw or one of
     * sclang's value functions, `args` its arguments after this one and `clip`
     * a range map's trim. A subclass with no such operator refuses it by name,
     * which is what the base does.
     */
    protected narop(selector: string, _args: readonly unknown[], _clip: string): TSelf {
        throw new TypeError(`a ${this.constructor.name} has no '${selector}'`);
    }

    /**
     * The reflected binary hook, `other <op> this`: what a builtin given this
     * as its second operand composes (`builtins.add(2, p)`). The base refuses.
     */
    protected rbinop(selector: string, _other: unknown): TSelf {
        throw new TypeError(`a ${this.constructor.name} cannot be the right operand of '${selector}'`);
    }

    /** @internal -- the hooks, for `base/builtins`, which lifts a builtin over this. */
    composeOp(kind: "unary" | "binary" | "rbinary" | "nary", selector: string, args: readonly unknown[] = [], clip = "minmax"): TSelf {
        if (kind === "unary") return this.unop(selector);
        if (kind === "binary") return this.binop(selector, args[0] as TOperand) as TSelf;
        if (kind === "rbinary") return this.rbinop(selector, args[0]);
        return this.narop(selector, args, clip);
    }

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
    idiv<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("idiv", x); }
    roundup<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("roundup", x); }
    urshift<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("urshift", x); }
    amclip<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("amclip", x); }
    scaleneg<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("scaleneg", x); }
    firstarg<T extends TOperand>(x: T): Composed<TSelf, T, F> { return this.binop("firstarg", x); }

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
    exp10(): TSelf { return this.unop("exp10"); }
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
    bitnot(): TSelf { return this.unop("bitnot"); }
    rectwindow(): TSelf { return this.unop("rectwindow"); }
    hanwindow(): TSelf { return this.unop("hanwindow"); }
    welwindow(): TSelf { return this.unop("welwindow"); }
    triwindow(): TSelf { return this.unop("triwindow"); }
    ramp(): TSelf { return this.unop("ramp"); }
    scurve(): TSelf { return this.unop("scurve"); }
    /** `1` where this is not above zero, else `0` -- sclang's `not`. */
    not(): Composed<TSelf, number, F> { return this.binop("le", 0 as TOperand & number); }

    // ---- the range maps: sclang's own, the signal half of the value
    // functions in `base/builtins`. `clip` says what an out-of-range input is
    // trimmed to before it is mapped -- `"minmax"` (the default, both ends),
    // `"min"`, `"max"` or `"none"` (extrapolate) -- the same argument, spelled
    // the same way, as on the value side.

    /** This off a linear range onto a linear one. */
    linlin(inLo: TOperand, inHi: TOperand, outLo: TOperand, outHi: TOperand,
           clip = "minmax"): TSelf {
        return this.narop("linlin", [inLo, inHi, outLo, outHi], clip);
    }

    /**
     * Off a linear range onto an exponential one -- an LFO onto a frequency.
     * The output ends must not straddle zero; one *at* zero is nudged to the
     * smallest same-signed value rather than giving a NaN.
     */
    linexp(inLo: TOperand, inHi: TOperand, outLo: TOperand, outHi: TOperand,
           clip = "minmax"): TSelf {
        return this.narop("linexp", [inLo, inHi, outLo, outHi], clip);
    }

    /** Off an exponential range onto a linear one -- a frequency onto a fader
     * position. */
    explin(inLo: TOperand, inHi: TOperand, outLo: TOperand, outHi: TOperand,
           clip = "minmax"): TSelf {
        return this.narop("explin", [inLo, inHi, outLo, outHi], clip);
    }

    /** Off an exponential range onto another. */
    expexp(inLo: TOperand, inHi: TOperand, outLo: TOperand, outHi: TOperand,
           clip = "minmax"): TSelf {
        return this.narop("expexp", [inLo, inHi, outLo, outHi], clip);
    }

    /**
     * Off a linear range onto one **bent** by `curve`: 0 is linear, negative
     * builds fast then slow -- most of the output spent on the first half of
     * the input -- and positive the reverse, which is the fine-at-the-bottom
     * feel a frequency or an amplitude control wants. Unlike `linexp` the bend
     * spans zero freely.
     */
    lincurve(inLo: TOperand, inHi: TOperand, outLo: TOperand, outHi: TOperand,
             curve: TOperand | number = -4.0, clip = "minmax"): TSelf {
        return this.narop("lincurve", [inLo, inHi, outLo, outHi, curve], clip);
    }

    /** The inverse of `lincurve`: off a bent range onto a linear one. */
    curvelin(inLo: TOperand, inHi: TOperand, outLo: TOperand, outHi: TOperand,
             curve: TOperand | number = -4.0, clip = "minmax"): TSelf {
        return this.narop("curvelin", [inLo, inHi, outLo, outHi, curve], clip);
    }

    // ---- two bounds: the general form of `clip2`/`wrap2`/`fold2` (on a
    // signal, the `Clip`/`Wrap`/`Fold` UGen of the same name).

    /** This held inside `lo..hi` at the nearer bound. */
    clip(lo: TOperand, hi: TOperand): TSelf {
        return this.narop("clip", [lo, hi], "minmax");
    }

    /** This wrapped into `lo..hi` -- taken modulo the range. */
    wrap(lo: TOperand, hi: TOperand): TSelf {
        return this.narop("wrap", [lo, hi], "minmax");
    }

    /** This folded into `lo..hi` -- reflected at the bounds. */
    fold(lo: TOperand, hi: TOperand): TSelf {
        return this.narop("fold", [lo, hi], "minmax");
    }

    // ---- random draws: on a value or a pattern, one per value from the
    // random context; on a signal, a new value every sample, each node its own
    // seeded stream (sclang's random operators).

    /** A uniform draw in `[0, this)`. */
    rand(): TSelf {
        return this.narop("rand", [], "minmax");
    }

    /** A uniform draw in `[-this, this)`. */
    rand2(): TSelf {
        return this.narop("rand2", [], "minmax");
    }

    /** A draw in `[0, this)`, more likely toward 0. */
    linrand(): TSelf {
        return this.narop("linrand", [], "minmax");
    }

    /** A draw in `(-this, this)`, more likely toward 0. */
    bilinrand(): TSelf {
        return this.narop("bilinrand", [], "minmax");
    }

    /** A bell-shaped draw in `(-this, this)`. */
    sum3rand(): TSelf {
        return this.narop("sum3rand", [], "minmax");
    }

    /** `1` with probability `this`, else `0`. */
    coin(): TSelf {
        return this.narop("coin", [], "minmax");
    }

    /** A uniform draw between this and `hi`. */
    rrand(hi: TOperand): TSelf {
        return this.narop("rrand", [hi], "minmax");
    }

    /** An exponential draw between this and `hi`. */
    exprand(hi: TOperand): TSelf {
        return this.narop("exprand", [hi], "minmax");
    }

    // ---- the value functions sclang's AbstractFunction has beside them.

    /** `frac` of the way from this to `other`. */
    blend(other: TOperand, frac: TOperand | number = 0.5): TSelf {
        return this.narop("blend", [other, frac], "minmax");
    }

    /** Degrees to radians. */
    degrad(): TSelf {
        return this.narop("degrad", [], "minmax");
    }

    /** Radians to degrees. */
    raddeg(): TSelf {
        return this.narop("raddeg", [], "minmax");
    }

    /** The smaller distance between this and `other` around a circle of `mod`. */
    moddif(other: TOperand | number = 0, mod: TOperand | number = 1): TSelf {
        return this.narop("moddif", [other, mod], "minmax");
    }

    /**
     * Off a range split at `inCenter` onto one split at `outCenter`, each half
     * mapped linearly.
     */
    bilin(inCenter: TOperand, inLo: TOperand, inHi: TOperand, outCenter: TOperand,
          outLo: TOperand, outHi: TOperand, clip = "minmax"): TSelf {
        return this.narop("bilin", [inCenter, inLo, inHi, outCenter, outLo, outHi], clip);
    }

    /** As `bilin`, with each half read exponentially. */
    biexp(inCenter: TOperand, inLo: TOperand, inHi: TOperand, outCenter: TOperand,
          outLo: TOperand, outHi: TOperand, clip = "minmax"): TSelf {
        return this.narop("biexp", [inCenter, inLo, inHi, outCenter, outLo, outHi], clip);
    }
}
