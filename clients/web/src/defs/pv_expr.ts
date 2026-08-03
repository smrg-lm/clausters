// Symbolic per-bin expressions for `pvKernel` — the general per-frame
// spectral mechanism (mirrors `clausters/defs/pv_expr.py`).
//
// The terms `mag`, `phase`, `binIndex`, `nbins`, `binfreq` and `param(i)` are
// symbolic per-bin values; composing them with the math methods (the same
// vocabulary UGen graphs use, by method rather than by operator — the
// package's composition rule) builds an expression tree that `pvKernel`
// serializes to the postfix token list the server's `PV_Kernel` validates and
// interprets — once per bin, on each fresh spectral frame.
//
// ```ts
// import { fft, ifft, pvKernel, control, out } from "clausters";
// import { mag, param } from "clausters/defs/pv_expr.js";
//
// let chain = fft(source);
// // A spectral gate: zero the bins below a threshold parameter.
// chain = pvKernel(chain, { mag: mag.mul(mag.ge(param(0))), params: [thresh] });
// const sig = ifft(chain);
// ```
//
// **What an expression can be**: a pure map from one bin's values — `(mag,
// phase, binIndex, nbins, binfreq, param(i)…)` — to the bin's new magnitude
// or phase. No state between bins or frames, no reading *other* bins:
// cross-frame ops (freeze, smear) and bin remaps (shift) stay with the
// dedicated `pv*` filters. Anything that *is* a per-bin map — gates, tilts,
// masks, magnitude algebra — is an expression here, never a new server UGen.
//
// The operator set is the shared table (`base/builtins.ts` /
// `clausters_core::builtins`): everything the value side and the UGen graphs
// compute is available per bin, with the same formulas — a rendered kernel is
// bit-identical between real-time and offline.

import { BINOP_OPS, SynthExpr, UNOP_OPS } from "./ugens/graph.ts";
import type { OpResult } from "./ugens/graph.ts";

// `+ - * /` compose dedicated alias kinds in UGen graphs, but in a bin
// expression every operator is a wire name; these four map straight through.
const ARITH = new Set(["add", "sub", "mul", "div"]);

/** What an expression composes with: another term, or a constant. */
export type PvOperand = PvExpr | number;

/**
 * A node of a symbolic per-bin expression. Build these by composing the
 * module's terms (`mag`, `phase`, …) with the math methods; pass the result
 * to `pvKernel`, which serializes it with `pvTokens`.
 */
export abstract class PvExpr extends SynthExpr<PvExpr, PvOperand> {
    protected binop<T extends PvOperand>(
        selector: string,
        other: T,
    ): OpResult<PvExpr, T> {
        return new PvBinNode(
            binopName(selector),
            this,
            operand(other),
        ) as unknown as OpResult<PvExpr, T>;
    }

    protected unop(selector: string): PvExpr {
        if (!UNOP_OPS.has(selector)) {
            throw new TypeError(`no per-bin operator '${selector}'`);
        }
        return new PvUnNode(selector, this);
    }

    /** @internal — this term on the **right** of a binary op. */
    rbinop(selector: string, other: PvOperand): PvExpr {
        return new PvBinNode(binopName(selector), operand(other), this);
    }
}

function binopName(selector: string): string {
    if (!ARITH.has(selector) && !BINOP_OPS.has(selector)) {
        throw new TypeError(`no per-bin operator '${selector}'`);
    }
    return selector;
}

function operand(x: unknown): PvExpr | number {
    if (x instanceof PvExpr) return x;
    if (typeof x !== "number") {
        throw new TypeError(
            "a per-bin expression operand must be a PvExpr term or a number, " +
                `got ${String(x)}`,
        );
    }
    return x;
}

/** A leaf term: one wire word (`"mag"`, `"bin"`, `"p0"`, …). */
class PvTerm extends PvExpr {
    readonly word: string;

    constructor(word: string) {
        super();
        this.word = word;
    }
}

class PvUnNode extends PvExpr {
    readonly op: string;
    readonly a: PvExpr;

    constructor(op: string, a: PvExpr) {
        super();
        this.op = op;
        this.a = a;
    }
}

class PvBinNode extends PvExpr {
    readonly op: string;
    readonly a: PvExpr | number;
    readonly b: PvExpr | number;

    constructor(op: string, a: PvExpr | number, b: PvExpr | number) {
        super();
        this.op = op;
        this.a = a;
        this.b = b;
    }
}

/** The bin's magnitude. */
export const mag: PvExpr = new PvTerm("mag");
/** The bin's phase in radians. */
export const phase: PvExpr = new PvTerm("phase");
/** The bin index, `0 .. nbins - 1` (named to avoid shadowing `bin`). */
export const binIndex: PvExpr = new PvTerm("bin");
/** The bin count (`fftSize / 2 + 1`). */
export const nbins: PvExpr = new PvTerm("nbins");
/** The bin's center frequency in Hz. */
export const binfreq: PvExpr = new PvTerm("binfreq");

/**
 * Parameter `i` — `pvKernel`'s `params[i]` signal input, sampled at the hop.
 * Parameters are how an expression stays *controllable*: a threshold, a tilt
 * amount, an LFO.
 */
export function param(i: number): PvExpr {
    const index = Math.trunc(i);
    if (!(index >= 0)) {
        throw new TypeError(`parameter index must be >= 0, got ${String(i)}`);
    }
    return new PvTerm(`p${String(index)}`);
}

/**
 * The free form of a binary op with the **constant on the left** —
 * `pvOp("sub", 1.0, mag)`, which a method cannot express. Mirrors the free
 * `add`/`sub`/`mul`/`div` the UGen graph exports for the same reason.
 */
export function pvOp(selector: string, a: PvOperand, b: PvOperand): PvExpr {
    if (a instanceof PvExpr || b instanceof PvExpr) {
        return new PvBinNode(binopName(selector), operand(a), operand(b));
    }
    throw new TypeError("a per-bin op needs at least one PvExpr term");
}

/**
 * Serializes an expression tree (or a plain number) to the postfix token list
 * the server's `PV_Kernel` consumes: numbers push constants, words are
 * per-bin loads or operator names.
 */
export function pvTokens(expr: PvOperand): (string | number)[] {
    const tokens: (string | number)[] = [];

    const walk = (node: PvExpr | number): void => {
        if (typeof node === "number") {
            tokens.push(node);
        } else if (node instanceof PvTerm) {
            tokens.push(node.word);
        } else if (node instanceof PvUnNode) {
            walk(node.a);
            tokens.push(node.op);
        } else if (node instanceof PvBinNode) {
            walk(node.a);
            walk(node.b);
            tokens.push(node.op);
        } else {
            throw new TypeError("not a per-bin expression node");
        }
    };

    walk(operand(expr));
    return tokens;
}
