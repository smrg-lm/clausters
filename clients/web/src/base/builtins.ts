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

/** A number or an array of them -- what every builtin accepts and returns. */
export type Num = number | readonly number[];

/** A unary builtin: array in, array out. */
export interface UnaryFn {
    (x: number): number;
    (x: readonly number[]): number[];
    (x: Num): Num;
}

/** A binary builtin: an array on either side (or both) gives an array. */
export interface BinaryFn {
    (a: number, b: number): number;
    (a: Num, b: Num): Num;
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

const un = (op: string): UnaryFn => ((x: Num) => unop(op, x)) as UnaryFn;
const bin = (op: string): BinaryFn => ((a: Num, b: Num) => binop(op, a, b)) as BinaryFn;

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
export const clip = (x: Num, lo: Num, hi: Num): Num => boundop("clip", x, lo, hi);

/** `x` wrapped into `lo..hi` -- taken modulo the range. */
export const wrap = (x: Num, lo: Num, hi: Num): Num => boundop("wrap", x, lo, hi);

/** `x` folded into `lo..hi` -- reflected at the bounds, as many times as it takes. */
export const fold = (x: Num, lo: Num, hi: Num): Num => boundop("fold", x, lo, hi);

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
    (fn: (v: number) => number): UnaryFn =>
    ((x: Num) => (isSeq(x) ? x.map(fn) : fn(x))) as UnaryFn;

/**
 * Hertz to **mel** (O'Shaughnessy): the perceptual pitch scale, ~1000 mel at
 * 1 kHz by construction and increasingly compressed above it.
 */
export const cpsmel = scaleFn(coreHzToMel);

/** Mel to hertz, the exact inverse of {@link cpsmel}. Negative input maps to 0. */
export const melcps = scaleFn(coreMelToHz);

/**
 * Hertz to **bark** (Traunmuller): the critical-band scale, ~8.5 bark at 1 kHz.
 * Zero hertz sits at `-0.53`, which is the formula and not an error -- the
 * scale is invertible across it.
 */
export const cpsbark = scaleFn(coreHzToBark);

/** Bark to hertz, the analytic inverse of {@link cpsbark}. */
export const barkcps = scaleFn(coreBarkToHz);

// ---- the numeric functions that are not operators ----
//
// sclang's functions on a number that no UGen computes, so they are not in the
// operator table and not f32: computed in f64 as sclang computes them, once, in
// `clausters_core::numeric`. Each takes a number or an array for its first
// argument; the rest are plain numbers.

const calcOp = (op: string, x: Num, ...args: number[]): Num =>
    isSeq(x) ? x.map((v) => coreCalc(op, Float64Array.from([v, ...args]))) :
        coreCalc(op, Float64Array.from([x, ...args]));
const calc1 = (op: string): UnaryFn => ((x: Num) => calcOp(op, x)) as UnaryFn;
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
export const even = (x: Num): boolean | boolean[] => asBool(calcOp("even", x));
/** Whether the whole part of `x` is odd. */
export const odd = (x: Num): boolean | boolean[] => asBool(calcOp("odd", x));
/** The Gray code of the whole part of `x`: `x ^ (x >> 1)`. */
export const graycode = calc1("graycode");
/** The least power of `base` at or above `x` (a power maps to itself). */
export const nextpowerof = (x: Num, base: number): Num => calcOp("nextpowerof", x, base);
/** The least power of two at or above `x`. */
export const nextpoweroftwo = (x: Num): Num => nextpowerof(x, 2);
/** The least power of three at or above `x`. */
export const nextpowerofthree = (x: Num): Num => nextpowerof(x, 3);
/** The power of `base` below the one `nextpowerof` gives. */
export const previouspowerof = (x: Num, base: number): Num =>
    calcOp("previouspowerof", x, base);
/**
 * The distance from `x` to `b` on a circle of circumference `mod`: 0.9 and
 * 0.1 are 0.2 apart on the unit circle, not 0.8.
 */
export const moddif = (x: Num, b = 0.0, mod = 1.0): Num => calcOp("moddif", x, b, mod);
/**
 * `x` pulled toward its nearest multiple of `resolution` by `strength` when it
 * is **farther** than `margin` from it.
 */
export const softround = (x: Num, resolution = 1.0, margin = 0.05, strength = 1.0): Num =>
    calcOp("softround", x, resolution, margin, strength);
/**
 * `x` pulled toward its nearest multiple of `resolution` by `strength` when it
 * is **within** `margin` of it -- a magnet for the grid.
 */
export const snap = (x: Num, resolution = 1.0, margin = 0.05, strength = 1.0): Num =>
    calcOp("snap", x, resolution, margin, strength);
/** The logistic curve `a * (m*e^-x + 1) / (n*e^-x + 1)`. */
export const lcurve = (x: Num, a = 1.0, m = 0.0, n = 1.0, tau = 1.0): Num =>
    calcOp("lcurve", x, a, m, n, tau);
/** The Gaussian bell `a * exp(-(x - b)^2 / (2 c^2))`. */
export const gausscurve = (x: Num, a = 1.0, b = 0.0, c = 1.0): Num =>
    calcOp("gausscurve", x, a, b, c);
/**
 * Lagrange cubic interpolation at `x1` in `0..1` between `b` and `c`, with
 * `a` and `d` their outer neighbours.
 */
export const lg3interp = (x1: Num, a: number, b: number, c: number, d: number): Num =>
    calcOp("lg3interp", x1, a, b, c, d);
/**
 * The feedback coefficient that decays a delay of `delaytime` by 60 dB in
 * `decaytime` (a negative `decaytime` inverts it).
 */
export const calcfeedback = (delaytime: Num, decaytime: number): Num =>
    calcOp("calcfeedback", delaytime, decaytime);

const CLIP_NUMBER: Record<string, number> = { minmax: 0, min: 1, max: 2, none: 3 };

/**
 * A linear map with a centre: `x` read off `inLo..inCenter..inHi`, each half
 * mapped linearly onto its half of the output.
 */
export const bilin = (
    x: Num, inCenter: number, inLo: number, inHi: number,
    outCenter: number, outLo: number, outHi: number, clip: Clip = "minmax",
): Num => calcOp("bilin", x, inCenter, inLo, inHi, outCenter, outLo, outHi, CLIP_NUMBER[clip]!);

/** {@link bilin} read off an exponential input. */
export const biexp = (
    x: Num, inCenter: number, inLo: number, inHi: number,
    outCenter: number, outLo: number, outHi: number, clip: Clip = "minmax",
): Num => calcOp("biexp", x, inCenter, inLo, inHi, outCenter, outLo, outHi, CLIP_NUMBER[clip]!);

/** The Bessel function of the first kind, integer order `n`, at `x`. */
export const besselj = (n: number, x: Num): Num =>
    isSeq(x) ? x.map((v) => coreCalc("besselj", Float64Array.from([n, v]))) :
        coreCalc("besselj", Float64Array.from([n, x]));

/** The Bessel function of the second kind, integer order `n`, at `x`. */
export const bessely = (n: number, x: Num): Num =>
    isSeq(x) ? x.map((v) => coreCalc("bessely", Float64Array.from([n, v]))) :
        coreCalc("bessely", Float64Array.from([n, x]));

/**
 * `frac` of the way from `a` to `b`. Any of the three may be an array, and two
 * arrays blend point by point (the shorter extended cyclically).
 */
export const blend = (a: Num, b: Num, frac: Num = 0.5): Num => {
    const args = [a, b, frac];
    const seqs = args.filter(isSeq);
    if (seqs.length === 0) return coreCalc("blend", Float64Array.from(args as number[]));
    const n = Math.max(...seqs.map((v) => v.length));
    const cols = args.map((v) => (isSeq(v) ? extend(v, n) : Array<number>(n).fill(v as number)));
    return Array.from({ length: n }, (_, i) =>
        coreCalc("blend", Float64Array.from([cols[0]![i]!, cols[1]![i]!, cols[2]![i]!])),
    );
};

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
export const degkey = (degree: Num, scale: readonly number[], steps = 12): Num =>
    scaleOp("degkey", degree, scale, steps);

/**
 * Key to scale degree, the inverse of {@link degkey}: a key between two
 * degrees is a fractional degree, linearly between them.
 */
export const keydeg = (key: Num, scale: readonly number[], steps = 12): Num =>
    scaleOp("keydeg", key, scale, steps);

/** The key of `scale` nearest to `x`, in `x`'s own octave of `steps`. */
export const nearestinscale = (x: Num, scale: readonly number[], steps = 12): Num =>
    scaleOp("nearestinscale", x, scale, steps);

/** The value of the sorted list `values` nearest to `x`. */
export const nearestinlist = (x: Num, values: readonly number[]): Num =>
    scaleOp("nearestinlist", x, values, 0);

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
export const blendat = (values: readonly number[], index: Num, method: BlendMethod = "clip"): Num => {
    const list = Float64Array.from(values);
    return isSeq(index) ? index.map((i) => coreBlendAt(list, i, method)) :
        coreBlendAt(list, index, method);
};

const listOp = (op: string, values: readonly number[], a = 0, b = 0): number[] =>
    Array.from(coreList(op, Float64Array.from(values), a, b));

/** `values` mapped linearly from their own range onto `lo..hi`. */
export const normalize = (values: readonly number[], lo = 0.0, hi = 1.0): number[] =>
    listOp("normalize", values, lo, hi);

/**
 * `values` divided by their sum, so they sum to 1 -- the weights `wchoose`
 * reads, made explicit.
 */
export const normalizesum = (values: readonly number[]): number[] => listOp("normalizesum", values);

/** The running sum of `values`. */
export const integrate = (values: readonly number[]): number[] => listOp("integrate", values);

/** Each value less the one before it (the first less 0). */
export const differentiate = (values: readonly number[]): number[] =>
    listOp("differentiate", values);

/** `values` resampled to `size` values by taking the nearest. */
export const resamp0 = (values: readonly number[], size: number): number[] =>
    listOp("resamp0", values, size);

/** `values` resampled to `size` values by linear interpolation. */
export const resamp1 = (values: readonly number[], size: number): number[] =>
    listOp("resamp1", values, size);

/**
 * The table `tablerand` draws from, with `values` as the probability shape
 * (higher is likelier), `size` entries long (the shape's own length unless
 * given).
 */
export const asrandomtable = (values: readonly number[], size?: number): number[] =>
    listOp("asrandomtable", values, size ?? values.length);

/** The fractional index of `x` in the sorted list `values`. */
export const indexinbetween = (values: readonly number[], x: Num): Num =>
    scaleOp("indexinbetween", x, values, 0);

/**
 * The first index of the sorted list `values` greater than `x`, or `undefined`
 * where none is.
 */
export const indexofgreaterthan = (values: readonly number[], x: number): number | undefined => {
    const i = coreScaleCalc("indexofgreaterthan", x, Float64Array.from(values), 0);
    return i < 0 ? undefined : i;
};

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
export const linlin = (
    x: Num, inLo: number, inHi: number, outLo: number, outHi: number, clip: Clip = "minmax",
): Num => mapOp("linlin", x, inLo, inHi, outLo, outHi, 0, clip);

/**
 * `x` off a linear range onto an exponential one -- a fader position to a
 * frequency. The output ends must not straddle zero; one *at* zero is nudged to
 * the smallest same-signed value rather than giving a NaN.
 */
export const linexp = (
    x: Num, inLo: number, inHi: number, outLo: number, outHi: number, clip: Clip = "minmax",
): Num => mapOp("linexp", x, inLo, inHi, outLo, outHi, 0, clip);

/** `x` off an exponential range onto a linear one -- a frequency to a fader position. */
export const explin = (
    x: Num, inLo: number, inHi: number, outLo: number, outHi: number, clip: Clip = "minmax",
): Num => mapOp("explin", x, inLo, inHi, outLo, outHi, 0, clip);

/** `x` off an exponential range onto another. */
export const expexp = (
    x: Num, inLo: number, inHi: number, outLo: number, outHi: number, clip: Clip = "minmax",
): Num => mapOp("expexp", x, inLo, inHi, outLo, outHi, 0, clip);

/**
 * `x` off a linear range onto one **bent** by `curve`: 0 is linear, negative
 * builds fast then slow -- most of the output spent on the first half of the
 * input -- and positive the reverse, which is the fine-at-the-bottom feel a
 * frequency or an amplitude control wants. Unlike {@link linexp} the bend spans
 * zero freely.
 */
export const lincurve = (
    x: Num, inLo: number, inHi: number, outLo: number, outHi: number,
    curve = -4, clip: Clip = "minmax",
): Num => mapOp("lincurve", x, inLo, inHi, outLo, outHi, curve, clip);

/** The inverse of {@link lincurve}: off a bent range onto a linear one. */
export const curvelin = (
    x: Num, inLo: number, inHi: number, outLo: number, outHi: number,
    curve = -4, clip: Clip = "minmax",
): Num => mapOp("curvelin", x, inLo, inHi, outLo, outHi, curve, clip);

/**
 * A **bipolar** value (-1..1) onto `lo`..`hi`, linearly.
 *
 * Nothing is trimmed: a UGen knows its own signal range and a bare value does
 * not, so an input past -1..1 overshoots the output by the same proportion
 * instead of being clipped to an assumption.
 */
export const range = (x: Num, lo = 0, hi = 1): Num =>
    mapOp("range", x, -1, 1, lo, hi, 0, "none");

/** A **bipolar** value onto `lo`..`hi` exponentially. Untrimmed, for the reason {@link range} gives. */
export const exprange = (x: Num, lo = 0.01, hi = 1): Num =>
    mapOp("exprange", x, -1, 1, lo, hi, 0, "none");

/**
 * Scale degree -> MIDI note number in the pitch space `octave`/`root`, with
 * floored octave wrapping (sclang semantics), `alter` semitones added and a
 * fractional degree read as SuperCollider writes an alteration. An empty
 * `scale` yields middle C. The rule is the core's, so every client resolves a
 * degree identically.
 */
export const degreeToMidinote = (
    degree: number,
    alter: number,
    octave: number,
    root: number,
    scale: readonly number[],
): number =>
    coreDegreeToMidinote(degree, alter, octave, root, Float32Array.from(scale));
