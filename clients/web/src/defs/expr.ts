// What "a Faust expression" is, as a type (mirrors `clausters/defs/expr.py`).
//
// An **expression** is something that composes a DSP graph rather than a
// value. Its two branches are the two def families, which are peers:
// `SynthExpr` (`./ugens/graph.ts`) for the UGen graph, and `FaustExpr` here
// for Faust — a `Signal` (the signal API) or a `Box` (the box algebra).
//
// The two branches carry no common class of their own, and that is the one
// place this file reads differently from its Python sibling. There, `Expr` is
// a marker class under `AbstractObject`, so a runtime `isinstance` can ask
// "is this an expression at all?"; here the question is a compile-time one and
// the answer is the `Expr` union in `./asdef.ts`, which is what the ambient
// verbs accept. What both languages agree on is that `Signal` and `Box` share
// a roof and do not compose with each other: this class is that roof, and it
// does a real job — the operator vocabulary is `AbstractObject`'s, the *table*
// each family maps a selector through is its own (the box schema has no
// `lsh`/`rsh`, and `mod` is Faust's `rem` for a signal and `fmod` for a box).

import { AbstractObject } from "../base/absobject.ts";
import type { Composed, Fan } from "../base/absobject.ts";

/** What a math method answers here: nothing fans out, so always the receiver. */
type FaustResult<TSelf, T> = Composed<TSelf, T, Fan<never, never>>;

/**
 * An expression of a **Faust graph**: a `./signals.ts` `Signal` or a
 * `./boxes.ts` `Box`. What `FaustDef` compiles.
 *
 * A subclass implements two hooks — `composeBinop`, which takes the operand
 * order as a flag, and `unop` — and gets the whole operator vocabulary and the
 * reversed-operand methods from here. Python writes `1 - sig` and gets
 * `__rsub__`; a language with no operator overloading needs that case spelled
 * out, which is what `rsub`/`rdiv` are.
 */
export abstract class FaustExpr<TSelf, TOperand>
    extends AbstractObject<TSelf, TOperand> {
    /**
     * Builds the node for `selector` over this expression and `other`, in
     * written order unless `swap` puts `other` first — the two Python hooks
     * (`_compose_binop`, `_rcompose_binop`) as one.
     */
    protected abstract composeBinop(
        selector: string,
        other: TOperand,
        swap: boolean,
    ): TSelf;

    protected binop<T extends TOperand>(
        selector: string,
        other: T,
    ): FaustResult<TSelf, T> {
        return this.composeBinop(selector, other, false) as FaustResult<TSelf, T>;
    }

    /** This expression on the **right** of a subtraction: `x - this`. */
    rsub(x: TOperand): TSelf { return this.composeBinop("sub", x, true); }
    /** `x / this`; see `rsub`. */
    rdiv(x: TOperand): TSelf { return this.composeBinop("div", x, true); }
}
