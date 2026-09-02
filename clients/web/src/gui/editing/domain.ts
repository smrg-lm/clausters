/**
 * The data adapter: one structure's own vocabulary, on both sides of an edit.
 *
 * An editor orchestrates; a **domain** is what it orchestrates over. Given a
 * gesture it says what payload that gesture is in the structure's vocabulary,
 * given a payload it writes it onto the client object, and it names the entry an
 * undo menu shows. Three answers, one per structure kind — a break-point curve,
 * a buffer's samples, a timeline of events.
 *
 * Two things it deliberately does not do, and both are boundaries rather than
 * omissions:
 *
 * - **It does not know how an edit inverts.** That is `history::Editable` in the
 *   shared crate (`apply`, `current`, `coalesceKey`), because an inverse written
 *   once per language is an inverse that disagrees with itself. What a domain
 *   asks the crate for is `current` — the state a payload is about to replace,
 *   which is the inverse — and hands the pair to the history.
 * - **It does not draw.** A picture of a curve is a {@link View}, and the two
 *   are separate because one structure is drawn several ways (a curve is a `bpf`
 *   on its own and a body inside a clip) while its vocabulary is one.
 *
 * @module
 */

import { domainCoalesceKey } from "../../document.ts";

/**
 * What one kind of structure is, to an editor.
 *
 * Subclass it per structure kind; `name` is the vocabulary its payloads are
 * written in, which is what {@link Editor} registers with the history and what
 * routes a leg coming back out of one.
 */
export abstract class Domain<S = unknown> {
    /**
     * The crate's own name for this vocabulary — `"points"`, `"samples"`,
     * `"events"`. It is carried by the history and read by nothing in the crate;
     * what reads it is whoever routes a leg the pile hands back.
     */
    readonly name: string = "";

    /**
     * The gesture as a payload in this vocabulary, or `null` when the tag is not
     * this domain's.
     *
     * `null` is the ordinary answer, not a failure: a view emits tags for
     * everything it can do and a domain answers for the ones that are edits of
     * *its* structure.
     */
    abstract payload(structure: S, tag: string, values: readonly unknown[]): unknown;

    /**
     * The state `payload` is about to replace — **the inverse**.
     *
     * Read before the edit lands, which is why it is a method here rather than
     * something an editor derives afterwards: after the write there is nothing
     * left to read.
     */
    abstract current(structure: S, payload: unknown): unknown;

    /**
     * Writes a payload onto the client object, and says whether it changed
     * anything.
     *
     * The one door, so an edit, the projection of an inverse and the adoption of
     * a redone state cannot disagree about which of the three happened.
     */
    abstract project(structure: S, payload: unknown): boolean;

    /** What an undo menu calls this edit. */
    label(_payload: unknown): string {
        return "edit";
    }

    /**
     * What makes two edits *the same thing done the same way*, so a run of small
     * adjustments becomes one undo.
     *
     * The crate's answer by default: one vocabulary, one key, in the shared
     * implementation both clients bind. A domain with no key never coalesces,
     * which is the safe end of the trade.
     */
    coalesceKey(payload: unknown): string | undefined {
        if (!this.name) return undefined;
        return domainCoalesceKey(this.name, payload) || undefined;
    }
}
