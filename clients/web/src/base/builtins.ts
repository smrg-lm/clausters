// Numeric builtins on numbers and lists, dispatched to the shared core
// (mirrors `clausters/base/builtins.py`).
//
// The value side of the operations the client applies to concrete numbers.
// They go through `clausters-core` -- so they are computed in **f32**, matching
// the server by construction; a JS number is f64 and would diverge. The
// music-theory conversions (`midicps`, `dbamp`, ...) go through the core too,
// so a value computed here and the same op running on the audio thread agree
// exactly.
//
// Each function takes a number or an array of them. With an array it returns
// an array, extending the shorter operand cyclically (sc3 semantics). The
// boundary stays flat: numbers in, numbers (or an array of them) out.
//
// The families, in the order below: the operators (the server's op tables,
// f32; where C and scsynth disagree on what one means -- a signed `sqrt` and
// `pow`, a floored `mod` -- it is scsynth's); `clip`/`wrap`/`fold` between two
// bounds; the perceptual scales; sclang's functions that are not operators
// (`degrad`, `snap`, `bilin`, `erf` ...) and the ones over a scale (`degkey`,
// `nearestinscale` ...) and over a whole list (`blend`, `normalize`, `resamp1`,
// `asrandomtable` ...), all three in f64 as sclang computes them; the lists
// reshaped (`mirror`, `stutter`, `rotate`, `pyramid` ...), whose order the core
// computes over the indices so any items are reshaped; and the range maps. The
// draws -- `rand`, `exprand`, `choose`, `tablerand`, `sputter` ... -- are random
// values and live in `base/rand`.
//
// A builtin given a pattern, a stream or a signal answers what that object's
// method does (`midicps(pseq([60, 72]))` is `pseq([60, 72]).midicps()`); a
// function over a whole list refuses one, since a stream may not end.

import {
    bark_to_hz as coreBarkToHz,
    binary as coreBinary,
    blendAt as coreBlendAt,
    bound as coreBound,
    list as coreList,
    calc as coreCalc,
    scaleCalc as coreScaleCalc,
    degree_to_midinote as coreDegreeToMidinote,
    hz_to_bark as coreHzToBark,
    hz_to_mel as coreHzToMel,
    map as coreMap,
    mel_to_hz as coreMelToHz,
    unary as coreUnary,
} from "../core/clausters_core_web.js";
import { AbstractObject } from "./absobject.ts";
import * as rand from "./rand.ts";
import * as builtins from "./builtins.ts";

/** A number or an array of them -- what every builtin accepts and returns. */
export type Num = number | readonly number[];

/**
 * Something that composes rather than computes: a pattern, a stream, a graph
 * node. A builtin given one answers what its own method does.
 */
// The base's own type parameters are what differ between the kinds; any of
// them composes.
export type Lifts = AbstractObject<any, any, any>;

/** What a builtin given `X` answers: what `X`'s own methods answer. */
export type Lifted<X> = X extends AbstractObject<infer S, any, any> ? S : never;

/** A unary builtin: array in, array out; a pattern in, a pattern out. */
export interface UnaryFn {
    (x: number): number;
    (x: readonly number[]): number[];
    (x: Num): Num;
    <X extends Lifts>(x: X): Lifted<X>;
}

/**
 * A binary builtin: an array on either side (or both) gives an array, and a
 * pattern on either side a pattern.
 */
export interface BinaryFn {
    (a: number, b: number): number;
    (a: Num, b: Num): Num;
    <X extends Lifts>(a: X, b: unknown): Lifted<X>;
    <X extends Lifts>(a: Num, b: X): Lifted<X>;
}

// ---- a builtin given a pattern, a stream or a signal ----
//
// An element-wise builtin given something that composes answers what that
// thing's own method does: `midicps(pseq([60, 72]))` is `pseq([60,
// 72]).midicps()`, `add(2, p)` is `p` on the right of `add`, and a function
// that is not an operator (`cbrt`, `degkey`) is composed by name, which a
// pattern or a stream computes value by value and a signal refuses. It
// composes over its first argument, as a method does, except a binary
// operator, which reflects. A list function refuses a pattern by name (mirrors
// `clausters/base/builtins.py`).

const composes = (x: unknown): x is Lifts => x instanceof AbstractObject;

type Fn = (...args: any[]) => any;
type LiftedFn<F> = F extends (x: any, ...rest: infer R) => any
    ? <X extends Lifts>(x: X, ...rest: R) => Lifted<X>
    : never;

function lifted<F extends Fn>(name: string, fn: F): F & LiftedFn<F> {
    const builtin = (...args: unknown[]): unknown => {
        if (!args.some(composes)) return fn(...args);
        const [x, ...rest] = args;
        if (!composes(x)) {
            throw new TypeError(
                `${name} composes over its first argument, as the method does: write ` +
                    `${name}(x, ...) with the pattern, stream or signal as x`,
            );
        }
        const clip = typeof rest[rest.length - 1] === "string" ? (rest.pop() as string) : "minmax";
        return x.composeOp("nary", name, rest, clip);
    };
    Object.defineProperty(builtin, "name", { value: name });
    return builtin as F & LiftedFn<F>;
}

function whole<F extends Fn>(name: string, fn: F): F {
    const builtin = (...args: unknown[]): unknown => {
        if (args.some(composes)) {
            throw new TypeError(
                `${name} reads a whole list, and a pattern or a stream is a sequence that ` +
                    "may not end: apply it to the list before the list becomes a pattern -- " +
                    `pseq(${name}([...]))`,
            );
        }
        return fn(...args);
    };
    Object.defineProperty(builtin, "name", { value: name });
    return builtin as F;
}

const isSeq = (x: Num): x is readonly number[] => Array.isArray(x);

/** `seq` cycled out to `n` values (sc3's shorter-operand extension). */
const extend = (seq: readonly number[], n: number): number[] =>
    Array.from({ length: n }, (_, i) => seq[i % seq.length]!);

function unop(op: string, x: Num): Num {
    if (isSeq(x)) return x.map((v) => coreUnary(op, v));
    return coreUnary(op, x);
}

function binop(op: string, a: Num, b: Num): Num {
    const aSeq = isSeq(a);
    const bSeq = isSeq(b);
    if (!aSeq && !bSeq) return coreBinary(op, a, b);
    if (aSeq && bSeq) {
        const n = Math.max(a.length, b.length);
        const [x, y] = [extend(a, n), extend(b, n)];
        return x.map((v, i) => coreBinary(op, v, y[i]!));
    }
    if (aSeq) return (a as readonly number[]).map((v) => coreBinary(op, v, b as number));
    return (b as readonly number[]).map((v) => coreBinary(op, a as number, v));
}

/**
 * One unary builtin by its core name -- the extensible door behind the named
 * exports below (`unary("midicps", 60)`).
 */
export const unary = (op: string, x: Num): Num => unop(op, x);

/** One binary builtin by its core name (`binary("clip2", 1.5, 1)`). */
export const binary = (op: string, a: Num, b: Num): Num => binop(op, a, b);

const un = (op: string): UnaryFn =>
    ((x: unknown) => (composes(x) ? x.composeOp("unary", op) : unop(op, x as Num))) as UnaryFn;
const bin = (op: string): BinaryFn =>
    ((a: unknown, b: unknown) =>
        composes(a) ? a.composeOp("binary", op, [b])
            : composes(b) ? b.composeOp("rbinary", op, [a])
                : binop(op, a as Num, b as Num)) as BinaryFn;

// ---- binary primitives ----

export const add = bin("add");
export const sub = bin("sub");
export const mul = bin("mul");
export const div = bin("div");
export const mod = bin("mod");
export const pow = bin("pow");
export const min = bin("min");
export const max = bin("max");
export const atan2 = bin("atan2");
export const gt = bin("gt");
export const lt = bin("lt");
export const ge = bin("ge");
export const le = bin("le");
export const eq = bin("eq");
export const ne = bin("ne");
export const bitand = bin("bitand");
export const bitor = bin("bitor");
export const bitxor = bin("bitxor");
export const leftshift = bin("lshift");
export const rightshift = bin("rshift");
export const hypot = bin("hypot");
export const hypotapx = bin("hypotapx");
export const ring1 = bin("ring1");
export const ring2 = bin("ring2");
export const ring3 = bin("ring3");
export const ring4 = bin("ring4");
export const sumsqr = bin("sumsqr");
export const difsqr = bin("difsqr");
export const sqrsum = bin("sqrsum");
export const sqrdif = bin("sqrdif");
export const absdif = bin("absdif");
export const thresh = bin("thresh");
export const clip2 = bin("clip2");
export const excess = bin("excess");
export const round = bin("round");
export const trunc = bin("trunc");
export const fold2 = bin("fold2");
export const wrap2 = bin("wrap2");
export const gcd = bin("gcd");
export const lcm = bin("lcm");
export const idiv = bin("idiv");
export const roundup = bin("roundup");
export const urshift = bin("urshift");
export const amclip = bin("amclip");
export const scaleneg = bin("scaleneg");
export const firstarg = bin("firstarg");

// ---- two bounds ----
//
// The general form of `clip2`/`wrap2`/`fold2`, whose range is the symmetric
// `-b..b`: the value kept inside `lo..hi`. The server's `Clip`/`Wrap`/`Fold`
// UGens are the same function over a signal.

function boundop(op: string, x: Num, lo: Num, hi: Num): Num {
    const args = [x, lo, hi];
    const seqs = args.filter(isSeq);
    if (seqs.length === 0) return coreBound(op, x as number, lo as number, hi as number);
    const n = Math.max(...seqs.map((v) => v.length));
    // A sequence extends cyclically (sc3's rule); a number is a constant.
    const [xs, los, his] = args.map((v) => (isSeq(v) ? extend(v, n) : null));
    const at = (v: Num, s: number[] | null | undefined, i: number): number =>
        s ? s[i]! : (v as number);
    return Array.from({ length: n }, (_, i) =>
        coreBound(op, at(x, xs, i), at(lo, los, i), at(hi, his, i)),
    );
}

/** `x` held inside `lo..hi` at the nearer bound. */
export const clip = lifted("clip", (x: Num, lo: Num, hi: Num): Num => boundop("clip", x, lo, hi));

/** `x` wrapped into `lo..hi` -- taken modulo the range. */
export const wrap = lifted("wrap", (x: Num, lo: Num, hi: Num): Num => boundop("wrap", x, lo, hi));

/** `x` folded into `lo..hi` -- reflected at the bounds, as many times as it takes. */
export const fold = lifted("fold", (x: Num, lo: Num, hi: Num): Num => boundop("fold", x, lo, hi));

// ---- unary primitives ----

export const neg = un("neg");
export const abs = un("abs");
export const sin = un("sin");
export const cos = un("cos");
export const tan = un("tan");
export const asin = un("asin");
export const acos = un("acos");
export const atan = un("atan");
export const exp = un("exp");
export const exp10 = un("exp10");
export const log = un("log");
export const log2 = un("log2");
export const log10 = un("log10");
export const sqrt = un("sqrt");
export const floor = un("floor");
export const ceil = un("ceil");
export const rint = un("rint");
export const asinteger = un("asint");
export const asfloat = un("asfloat");
export const squared = un("squared");
export const cubed = un("cubed");
export const reciprocal = un("recip");
export const frac = un("frac");
export const sign = un("sign");
export const sinh = un("sinh");
export const cosh = un("cosh");
export const tanh = un("tanh");
export const distort = un("distort");
export const softclip = un("softclip");
export const bitnot = un("bitnot");
export const ramp = un("ramp");
export const scurve = un("scurve");

// ---- windows over a phase ----
//
// A window's shape read at one phase in `0..1`, zero outside it -- the
// operator, not an FFT frame.

export const rectwindow = un("rectwindow");
export const hanwindow = un("hanwindow");
export const welwindow = un("welwindow");
export const triwindow = un("triwindow");

// ---- music-theory conversions ----

export const midicps = un("midicps");
export const cpsmidi = un("cpsmidi");
export const midiratio = un("midiratio");
export const ratiomidi = un("ratiomidi");
export const dbamp = un("dbamp");
export const ampdb = un("ampdb");
export const octcps = un("octcps");
export const cpsoct = un("cpsoct");

// ---- perceptual frequency scales ----
//
// The two scales a spectrogram's frequency axis is labeled in, named the way
// SuperCollider names a conversion (`<from><to>`, hertz spelled `cps`), so they
// read beside `midicps` and `cpsoct` above. They are **not** members of the
// server's unary-op table -- no UGen computes a mel -- so they come from
// `clausters_core::scale` in **f64**, which is also what the GUI host's ruler
// and the spectrogram shader read: a headless script gets the number the window
// is drawing.

const scaleFn =
    (name: string, fn: (v: number) => number): UnaryFn =>
    lifted(name, (x: Num) => (isSeq(x) ? x.map(fn) : fn(x))) as unknown as UnaryFn;

/**
 * Hertz to **mel** (O'Shaughnessy): the perceptual pitch scale, ~1000 mel at
 * 1 kHz by construction and increasingly compressed above it.
 */
export const cpsmel = scaleFn("cpsmel", coreHzToMel);

/** Mel to hertz, the exact inverse of {@link cpsmel}. Negative input maps to 0. */
export const melcps = scaleFn("melcps", coreMelToHz);

/**
 * Hertz to **bark** (Traunmuller): the critical-band scale, ~8.5 bark at 1 kHz.
 * Zero hertz sits at `-0.53`, which is the formula and not an error -- the
 * scale is invertible across it.
 */
export const cpsbark = scaleFn("cpsbark", coreHzToBark);

/** Bark to hertz, the analytic inverse of {@link cpsbark}. */
export const barkcps = scaleFn("barkcps", coreBarkToHz);

// ---- the numeric functions that are not operators ----
//
// sclang's functions on a number that no UGen computes, so they are not in the
// operator table and not f32: computed in f64 as sclang computes them, once, in
// `clausters_core::numeric`. Each takes a number or an array for its first
// argument; the rest are plain numbers.

const calcOp = (op: string, x: Num, ...args: number[]): Num =>
    isSeq(x) ? x.map((v) => coreCalc(op, Float64Array.from([v, ...args]))) :
        coreCalc(op, Float64Array.from([x, ...args]));
const calc1 = (op: string): UnaryFn => lifted(op, (x: Num) => calcOp(op, x)) as unknown as UnaryFn;
const asBool = (r: Num): boolean | boolean[] =>
    Array.isArray(r) ? r.map((v) => v !== 0) : r !== 0;

/** Degrees to radians. */
export const degrad = calc1("degrad");
/** Radians to degrees. */
export const raddeg = calc1("raddeg");
/** The real cube root. */
export const cbrt = calc1("cbrt");
/** `log(1 + x)`, exact near zero. */
export const log1p = calc1("log1p");
/** `exp(x) - 1`, exact near zero. */
export const expm1 = calc1("expm1");
/** The error function. */
export const erf = calc1("erf");
/** The complementary error function, `1 - erf(x)`, exact in its tail. */
export const erfc = calc1("erfc");
/** The gamma function. */
export const tgamma = calc1("tgamma");
/** `log |gamma(x)|`. */
export const lgamma = calc1("lgamma");
/**
 * `x`, or 0 where it is a denormal, an infinity, a NaN or past 1e15 -- what
 * keeps a feedback path from carrying one.
 */
export const zapgremlins = calc1("zapgremlins");
/** `x` wrapped into `-1..1` by one shift. */
export const wrap1 = calc1("wrap1");
/** `x` folded into `-1..1` by one reflection. */
export const fold1 = calc1("fold1");
/** Whether the whole part of `x` is even. */
export const even = lifted("even", (x: Num): boolean | boolean[] => asBool(calcOp("even", x)));
/** Whether the whole part of `x` is odd. */
export const odd = lifted("odd", (x: Num): boolean | boolean[] => asBool(calcOp("odd", x)));
/** The Gray code of the whole part of `x`: `x ^ (x >> 1)`. */
export const graycode = calc1("graycode");
/** The least power of `base` at or above `x` (a power maps to itself). */
export const nextpowerof = lifted("nextpowerof", (x: Num, base: number): Num => calcOp("nextpowerof", x, base));
/** The least power of two at or above `x`. */
export const nextpoweroftwo = lifted("nextpoweroftwo", (x: Num): Num => nextpowerof(x, 2));
/** The least power of three at or above `x`. */
export const nextpowerofthree = lifted("nextpowerofthree", (x: Num): Num => nextpowerof(x, 3));
/** The power of `base` below the one `nextpowerof` gives. */
export const previouspowerof = lifted("previouspowerof", (x: Num, base: number): Num =>
    calcOp("previouspowerof", x, base));
/**
 * The distance from `x` to `b` on a circle of circumference `mod`: 0.9 and
 * 0.1 are 0.2 apart on the unit circle, not 0.8.
 */
export const moddif = lifted("moddif", (x: Num, b = 0.0, mod = 1.0): Num => calcOp("moddif", x, b, mod));
/**
 * `x` pulled toward its nearest multiple of `resolution` by `strength` when it
 * is **farther** than `margin` from it.
 */
export const softround = lifted("softround", (x: Num, resolution = 1.0, margin = 0.05, strength = 1.0): Num =>
    calcOp("softround", x, resolution, margin, strength));
/**
 * `x` pulled toward its nearest multiple of `resolution` by `strength` when it
 * is **within** `margin` of it -- a magnet for the grid.
 */
export const snap = lifted("snap", (x: Num, resolution = 1.0, margin = 0.05, strength = 1.0): Num =>
    calcOp("snap", x, resolution, margin, strength));
/** The logistic curve `a * (m*e^-x + 1) / (n*e^-x + 1)`. */
export const lcurve = lifted("lcurve", (x: Num, a = 1.0, m = 0.0, n = 1.0, tau = 1.0): Num =>
    calcOp("lcurve", x, a, m, n, tau));
/** The Gaussian bell `a * exp(-(x - b)^2 / (2 c^2))`. */
export const gausscurve = lifted("gausscurve", (x: Num, a = 1.0, b = 0.0, c = 1.0): Num =>
    calcOp("gausscurve", x, a, b, c));
/**
 * Lagrange cubic interpolation at `x1` in `0..1` between `b` and `c`, with
 * `a` and `d` their outer neighbours.
 */
export const lg3interp = lifted("lg3interp", (x1: Num, a: number, b: number, c: number, d: number): Num =>
    calcOp("lg3interp", x1, a, b, c, d));
/**
 * The feedback coefficient that decays a delay of `delaytime` by 60 dB in
 * `decaytime` (a negative `decaytime` inverts it).
 */
export const calcfeedback = lifted("calcfeedback", (delaytime: Num, decaytime: number): Num =>
    calcOp("calcfeedback", delaytime, decaytime));

const CLIP_NUMBER: Record<string, number> = { minmax: 0, min: 1, max: 2, none: 3 };

/**
 * A linear map with a centre: `x` read off `inLo..inCenter..inHi`, each half
 * mapped linearly onto its half of the output.
 */
export const bilin = lifted("bilin", (
    x: Num, inCenter: number, inLo: number, inHi: number,
    outCenter: number, outLo: number, outHi: number, clip: Clip = "minmax",
): Num => calcOp("bilin", x, inCenter, inLo, inHi, outCenter, outLo, outHi, CLIP_NUMBER[clip]!));

/** {@link bilin} read off an exponential input. */
export const biexp = lifted("biexp", (
    x: Num, inCenter: number, inLo: number, inHi: number,
    outCenter: number, outLo: number, outHi: number, clip: Clip = "minmax",
): Num => calcOp("biexp", x, inCenter, inLo, inHi, outCenter, outLo, outHi, CLIP_NUMBER[clip]!));

/** The Bessel function of the first kind, integer order `n`, at `x`. */
export const besselj = lifted("besselj", (n: number, x: Num): Num =>
    isSeq(x) ? x.map((v) => coreCalc("besselj", Float64Array.from([n, v]))) :
        coreCalc("besselj", Float64Array.from([n, x])));

/** The Bessel function of the second kind, integer order `n`, at `x`. */
export const bessely = lifted("bessely", (n: number, x: Num): Num =>
    isSeq(x) ? x.map((v) => coreCalc("bessely", Float64Array.from([n, v]))) :
        coreCalc("bessely", Float64Array.from([n, x])));

/**
 * `frac` of the way from `a` to `b`. Any of the three may be an array, and two
 * arrays blend point by point (the shorter extended cyclically).
 */
export const blend = lifted("blend", (a: Num, b: Num, frac: Num = 0.5): Num => {
    const args = [a, b, frac];
    const seqs = args.filter(isSeq);
    if (seqs.length === 0) return coreCalc("blend", Float64Array.from(args as number[]));
    const n = Math.max(...seqs.map((v) => v.length));
    const cols = args.map((v) => (isSeq(v) ? extend(v, n) : Array<number>(n).fill(v as number)));
    return Array.from({ length: n }, (_, i) =>
        coreCalc("blend", Float64Array.from([cols[0]![i]!, cols[1]![i]!, cols[2]![i]!])),
    );
});

// ---- scales and sorted lists ----

const scaleOp = (op: string, x: Num, values: readonly number[], steps: number): Num => {
    const list = Float64Array.from(values);
    return isSeq(x) ? x.map((v) => coreScaleCalc(op, v, list, steps)) :
        coreScaleCalc(op, x, list, steps);
};

/**
 * Scale degree to key: `degree` indexes `scale` (offsets within one octave of
 * `steps`) with octave carry, and a fraction is an alteration as SuperCollider
 * writes it -- `1.1` is degree 1 raised a semitone.
 */
export const degkey = lifted("degkey", (degree: Num, scale: readonly number[], steps = 12): Num =>
    scaleOp("degkey", degree, scale, steps));

/**
 * Key to scale degree, the inverse of {@link degkey}: a key between two
 * degrees is a fractional degree, linearly between them.
 */
export const keydeg = lifted("keydeg", (key: Num, scale: readonly number[], steps = 12): Num =>
    scaleOp("keydeg", key, scale, steps));

/** The key of `scale` nearest to `x`, in `x`'s own octave of `steps`. */
export const nearestinscale = lifted("nearestinscale", (x: Num, scale: readonly number[], steps = 12): Num =>
    scaleOp("nearestinscale", x, scale, steps));

/** The value of the sorted list `values` nearest to `x`. */
export const nearestinlist = lifted("nearestinlist", (x: Num, values: readonly number[]): Num =>
    scaleOp("nearestinlist", x, values, 0));

// ---- lists ----
//
// sclang's functions that read or remake a whole list. The ones that draw
// (`tablerand`, `scramble`) are random values and live with the others in
// `base/rand`.

/** How an index past either end of a list reads. */
export type BlendMethod = "clip" | "wrap" | "fold";

/**
 * `values` read at the fractional `index`, blended between the two values
 * around it. `method` says what an index past either end reads: `"clip"`
 * (held at the end), `"wrap"` or `"fold"`.
 */
export const blendat = whole("blendat", (values: readonly number[], index: Num, method: BlendMethod = "clip"): Num => {
    const list = Float64Array.from(values);
    return isSeq(index) ? index.map((i) => coreBlendAt(list, i, method)) :
        coreBlendAt(list, index, method);
});

const listOp = (op: string, values: readonly number[], a = 0, b = 0): number[] =>
    Array.from(coreList(op, Float64Array.from(values), a, b));

/** `values` mapped linearly from their own range onto `lo..hi`. */
export const normalize = whole("normalize", (values: readonly number[], lo = 0.0, hi = 1.0): number[] =>
    listOp("normalize", values, lo, hi));

/**
 * `values` divided by their sum, so they sum to 1 -- the weights `wchoose`
 * reads, made explicit.
 */
export const normalizesum = whole("normalizesum", (values: readonly number[]): number[] => listOp("normalizesum", values));

/** The running sum of `values`. */
export const integrate = whole("integrate", (values: readonly number[]): number[] => listOp("integrate", values));

/** Each value less the one before it (the first less 0). */
export const differentiate = whole("differentiate", (values: readonly number[]): number[] =>
    listOp("differentiate", values));

/** `values` resampled to `size` values by taking the nearest. */
export const resamp0 = whole("resamp0", (values: readonly number[], size: number): number[] =>
    listOp("resamp0", values, size));

/** `values` resampled to `size` values by linear interpolation. */
export const resamp1 = whole("resamp1", (values: readonly number[], size: number): number[] =>
    listOp("resamp1", values, size));

/**
 * The table `tablerand` draws from, with `values` as the probability shape
 * (higher is likelier), `size` entries long (the shape's own length unless
 * given).
 */
export const asrandomtable = whole("asrandomtable", (values: readonly number[], size?: number): number[] =>
    listOp("asrandomtable", values, size ?? values.length));

/** The fractional index of `x` in the sorted list `values`. */
export const indexinbetween = whole("indexinbetween", (values: readonly number[], x: Num): Num =>
    scaleOp("indexinbetween", x, values, 0));

/**
 * The first index of the sorted list `values` greater than `x`, or `undefined`
 * where none is.
 */
export const indexofgreaterthan = whole("indexofgreaterthan", (values: readonly number[], x: number): number | undefined => {
    const i = coreScaleCalc("indexofgreaterthan", x, Float64Array.from(values), 0);
    return i < 0 ? undefined : i;
});

// ---- lists reshaped: sclang's `Array` functions that reorder, repeat or
// unfold a list. The order is the core's, over the indices, so any items are
// reshaped -- numbers, notes, patterns -- not only numbers.

const reshape = <T>(op: string, items: readonly T[], a = 0): T[] =>
    Array.from(coreList(op, Float64Array.from(items, (_, i) => i), a, 0), (i) => items[i]!);

/** The list and then back again, the last item once: `1 2 3 2 1`. */
export const mirror = whole("mirror", <T>(items: readonly T[]): T[] => reshape("mirror", items));

/** The list and then back again, neither end repeated: `1 2 3 2` -- the form that loops. */
export const mirror1 = whole("mirror1", <T>(items: readonly T[]): T[] => reshape("mirror1", items));

/** The list and then reversed, both ends twice: `1 2 3 3 2 1`. */
export const mirror2 = whole("mirror2", <T>(items: readonly T[]): T[] => reshape("mirror2", items));

/** Every item `n` times in a row. */
export const stutter = whole("stutter", <T>(items: readonly T[], n = 2): T[] =>
    reshape("stutter", items, n));

/** The list rotated `n` places to the right (negative to the left), wrapping. */
export const rotate = whole("rotate", <T>(items: readonly T[], n = 1): T[] =>
    reshape("rotate", items, n));

/**
 * The list unfolded into growing and shrinking runs, one after another: type 1
 * is every prefix (`1  1 2  1 2 3`), 2 every suffix, 3 and 4 the same
 * shrinking, and 5 to 10 a run up and back down in their combinations --
 * sclang's ten pattern types.
 */
export const pyramid = whole("pyramid", <T>(items: readonly T[], patternType = 1): T[] =>
    reshape("pyramid", items, patternType));

/** {@link pyramid} with each run its own array: `[[1], [1, 2], [1, 2, 3]]`. */
export const pyramidg = whole("pyramidg", <T>(items: readonly T[], patternType = 1): T[][] => {
    const flat = reshape("pyramid", items, patternType);
    const sizes = coreList("pyramidsizes", new Float64Array(items.length), patternType, 0);
    const groups: T[][] = [];
    let at = 0;
    for (const size of sizes) {
        groups.push(flat.slice(at, at + size));
        at += size;
    }
    return groups;
});

// ---- range maps (SuperCollider's warp family) ----
//
// Reading a value out of one range and writing it into another, which is what a
// control's position, a spec and an envelope's curve all are. The formulas live
// once in `clausters_core::warp`, so a value mapped here, the same map on the
// audio thread and the curve the GUI host draws are one curve.

/** What an out-of-range input is trimmed to before it is mapped. */
export type Clip = "minmax" | "min" | "max" | "none";

const mapOp = (
    op: string,
    x: Num,
    inLo: number,
    inHi: number,
    outLo: number,
    outHi: number,
    curve: number,
    clip: Clip,
): Num =>
    isSeq(x)
        ? x.map((v) => coreMap(op, clip, v, inLo, inHi, outLo, outHi, curve))
        : coreMap(op, clip, x, inLo, inHi, outLo, outHi, curve);

/** `x` off a linear range onto a linear one. */
export const linlin = lifted("linlin", (
    x: Num, inLo: number, inHi: number, outLo: number, outHi: number, clip: Clip = "minmax",
): Num => mapOp("linlin", x, inLo, inHi, outLo, outHi, 0, clip));

/**
 * `x` off a linear range onto an exponential one -- a fader position to a
 * frequency. The output ends must not straddle zero; one *at* zero is nudged to
 * the smallest same-signed value rather than giving a NaN.
 */
export const linexp = lifted("linexp", (
    x: Num, inLo: number, inHi: number, outLo: number, outHi: number, clip: Clip = "minmax",
): Num => mapOp("linexp", x, inLo, inHi, outLo, outHi, 0, clip));

/** `x` off an exponential range onto a linear one -- a frequency to a fader position. */
export const explin = lifted("explin", (
    x: Num, inLo: number, inHi: number, outLo: number, outHi: number, clip: Clip = "minmax",
): Num => mapOp("explin", x, inLo, inHi, outLo, outHi, 0, clip));

/** `x` off an exponential range onto another. */
export const expexp = lifted("expexp", (
    x: Num, inLo: number, inHi: number, outLo: number, outHi: number, clip: Clip = "minmax",
): Num => mapOp("expexp", x, inLo, inHi, outLo, outHi, 0, clip));

/**
 * `x` off a linear range onto one **bent** by `curve`: 0 is linear, negative
 * builds fast then slow -- most of the output spent on the first half of the
 * input -- and positive the reverse, which is the fine-at-the-bottom feel a
 * frequency or an amplitude control wants. Unlike {@link linexp} the bend spans
 * zero freely.
 */
export const lincurve = lifted("lincurve", (
    x: Num, inLo: number, inHi: number, outLo: number, outHi: number,
    curve = -4, clip: Clip = "minmax",
): Num => mapOp("lincurve", x, inLo, inHi, outLo, outHi, curve, clip));

/** The inverse of {@link lincurve}: off a bent range onto a linear one. */
export const curvelin = lifted("curvelin", (
    x: Num, inLo: number, inHi: number, outLo: number, outHi: number,
    curve = -4, clip: Clip = "minmax",
): Num => mapOp("curvelin", x, inLo, inHi, outLo, outHi, curve, clip));

/**
 * A **bipolar** value (-1..1) onto `lo`..`hi`, linearly.
 *
 * Nothing is trimmed: a UGen knows its own signal range and a bare value does
 * not, so an input past -1..1 overshoots the output by the same proportion
 * instead of being clipped to an assumption.
 */
export const range = lifted("range", (x: Num, lo = 0, hi = 1): Num =>
    mapOp("range", x, -1, 1, lo, hi, 0, "none"));

/** A **bipolar** value onto `lo`..`hi` exponentially. Untrimmed, for the reason {@link range} gives. */
export const exprange = lifted("exprange", (x: Num, lo = 0.01, hi = 1): Num =>
    mapOp("exprange", x, -1, 1, lo, hi, 0, "none"));

/**
 * Scale degree -> MIDI note number in the pitch space `octave`/`root`, with
 * floored octave wrapping (sclang semantics), `alter` semitones added and a
 * fractional degree read as SuperCollider writes an alteration. An empty
 * `scale` yields middle C. The rule is the core's, so every client resolves a
 * degree identically.
 */
export const degreeToMidinote = lifted("degreeToMidinote", (
    degree: number,
    alter: number,
    octave: number,
    root: number,
    scale: readonly number[],
): number =>
    coreDegreeToMidinote(degree, alter, octave, root, Float32Array.from(scale)));

// ---- the operators by name ----
//
// What something that composes values (a pattern, a stream) computes the
// operator it was composed with by: the wire selector names the function, in
// the same four tables as `clausters/base/builtins.py`.

const UNARY_SELECTORS = [
    "neg", "abs", "sin", "cos", "tan", "asin", "acos", "atan", "exp", "exp10",
    "log", "log10", "log2", "sqrt", "floor", "ceil", "rint", "asint", "asfloat",
    "squared", "cubed", "recip", "frac", "sign", "sinh", "cosh", "tanh",
    "distort", "softclip", "midicps", "cpsmidi", "midiratio", "ratiomidi",
    "dbamp", "ampdb", "octcps", "cpsoct", "bitnot", "rectwindow", "hanwindow",
    "welwindow", "triwindow", "ramp", "scurve",
] as const;

const BINARY_SELECTORS = [
    "add", "sub", "mul", "div", "mod", "pow", "min", "max", "atan2", "gt", "lt",
    "ge", "le", "eq", "ne", "bitand", "bitor", "bitxor", "lshift", "rshift",
    "hypot", "ring1", "ring2", "ring3", "ring4", "sumsqr", "difsqr", "sqrsum",
    "sqrdif", "absdif", "thresh", "clip2", "excess", "round", "trunc", "fold2",
    "wrap2", "gcd", "lcm", "hypotapx", "idiv", "roundup", "urshift", "amclip",
    "scaleneg", "firstarg",
] as const;

/** The unary operators by wire selector. */
export const UNARY: Readonly<Record<string, UnaryFn>> = Object.fromEntries(
    UNARY_SELECTORS.map((s) => [s, un(s)]),
);

/** The binary operators by wire selector. */
export const BINARY: Readonly<Record<string, BinaryFn>> = Object.fromEntries(
    BINARY_SELECTORS.map((s) => [s, bin(s)]),
);

/** The operators between two bounds. */
export const BOUND: Readonly<Record<string, (x: Num, lo: Num, hi: Num) => Num>> = {
    clip, wrap, fold,
};

/**
 * The n-ary operators by name: the range maps, which take the input ends and
 * the output ends after the value, and sclang's value functions over a value
 * and its arguments.
 */
export const NARY: Readonly<Record<string, (x: Num, ...args: never[]) => unknown>> = {
    linlin, linexp, explin, expexp, lincurve, curvelin, blend, bilin, biexp, moddif,
};

/** The n-ary operators whose last argument is a trim (`clip`). @internal */
export const TRIMMED: ReadonlySet<string> = new Set([
    "linlin", "linexp", "explin", "expexp", "lincurve", "curvelin", "bilin", "biexp",
]);

/**
 * The random draws a pattern or a stream composes by name (`.rand()`,
 * `.rrand(hi)`), each from the random context, one per value.
 */
export const DRAW: readonly string[] = [
    "rand", "rand2", "linrand", "bilinrand", "sum3rand", "coin", "rrand", "exprand",
];

/**
 * The value function an operator selector names, in any of the tables, else
 * the builtin of that name: how something that composes values (a pattern, a
 * stream) computes the operator or the function it was composed with.
 */
export function valueOp(selector: string): (...values: never[]) => unknown {
    for (const table of [UNARY, BINARY, BOUND, NARY] as const) {
        const fn = (table as Record<string, unknown>)[selector];
        if (fn !== undefined) return fn as (...values: never[]) => unknown;
    }
    if (DRAW.includes(selector)) {
        return (rand as unknown as Record<string, (...values: never[]) => unknown>)[selector]!;
    }
    const fn = (builtins as unknown as Record<string, unknown>)[selector];
    if (typeof fn === "function") return fn as (...values: never[]) => unknown;
    throw new Error(`no value operator '${selector}'`);
}
