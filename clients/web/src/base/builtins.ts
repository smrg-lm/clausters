// Numeric builtins on numbers and lists, dispatched to the shared core
// (mirrors `clausters/base/builtins.py`).
//
// The value side of the operations the client applies to concrete numbers.
// They go through `clausters-core` — so they are computed in **f32**, matching
// the server by construction; a JS number is f64 and would diverge. The
// music-theory conversions (`midicps`, `dbamp`, …) go through the core too,
// so a value computed here and the same op running on the audio thread agree
// exactly.
//
// Each function takes a number or an array of them. With an array it returns
// an array, extending the shorter operand cyclically (sc3 semantics). The
// boundary stays flat: numbers in, numbers (or an array of them) out.

import {
    bark_to_hz as coreBarkToHz,
    binary as coreBinary,
    degree_to_midinote as coreDegreeToMidinote,
    hz_to_bark as coreHzToBark,
    hz_to_mel as coreHzToMel,
    map as coreMap,
    mel_to_hz as coreMelToHz,
    unary as coreUnary,
} from "../core/clausters_core_web.js";

/** A number or an array of them — what every builtin accepts and returns. */
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
 * One unary builtin by its core name — the extensible door behind the named
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
 * `x` off a linear range onto an exponential one — a fader position to a
 * frequency. The output ends must not straddle zero; one *at* zero is nudged to
 * the smallest same-signed value rather than giving a NaN.
 */
export const linexp = (
    x: Num, inLo: number, inHi: number, outLo: number, outHi: number, clip: Clip = "minmax",
): Num => mapOp("linexp", x, inLo, inHi, outLo, outHi, 0, clip);

/** `x` off an exponential range onto a linear one — a frequency to a fader position. */
export const explin = (
    x: Num, inLo: number, inHi: number, outLo: number, outHi: number, clip: Clip = "minmax",
): Num => mapOp("explin", x, inLo, inHi, outLo, outHi, 0, clip);

/** `x` off an exponential range onto another. */
export const expexp = (
    x: Num, inLo: number, inHi: number, outLo: number, outHi: number, clip: Clip = "minmax",
): Num => mapOp("expexp", x, inLo, inHi, outLo, outHi, 0, clip);

/**
 * `x` off a linear range onto one **bent** by `curve`: 0 is linear, negative
 * builds fast then slow — most of the output spent on the first half of the
 * input — and positive the reverse, which is the fine-at-the-bottom feel a
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
 * A **bipolar** value (−1..1) onto `lo`..`hi`, linearly.
 *
 * Nothing is trimmed: a UGen knows its own signal range and a bare value does
 * not, so an input past −1..1 overshoots the output by the same proportion
 * instead of being clipped to an assumption.
 */
export const range = (x: Num, lo = 0, hi = 1): Num =>
    mapOp("range", x, -1, 1, lo, hi, 0, "none");

/** A **bipolar** value onto `lo`..`hi` exponentially. Untrimmed, for the reason {@link range} gives. */
export const exprange = (x: Num, lo = 0.01, hi = 1): Num =>
    mapOp("exprange", x, -1, 1, lo, hi, 0, "none");

/**
 * Scale degree → MIDI note number in the pitch space `octave`/`root`, with
 * floored octave wrapping (sclang semantics). An empty `scale` yields middle
 * C. The rule is the core's, so every client resolves a degree identically.
 */
export const degreeToMidinote = (
    degree: number,
    octave: number,
    root: number,
    scale: readonly number[],
): number =>
    coreDegreeToMidinote(degree, octave, root, Float32Array.from(scale));
