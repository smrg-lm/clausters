/**
 * The data adapter: one structure's own vocabulary, on both sides of an edit.
 *
 * An editor orchestrates; a **domain** is what it orchestrates over. Given a
 * gesture it says what that gesture needs read with it, and given an applied
 * payload it writes it onto the client object. Two answers, one per structure
 * kind — a break-point curve, a buffer's samples, a timeline of events, a
 * multitrack — and they are the two halves a language genuinely owns.
 *
 * Three things it deliberately does not do, and all of them are boundaries
 * rather than omissions:
 *
 * - **It does not read a gesture itself.** What a tag and a flat list of values
 *   *mean* is {@link editingIntake}, in the shared crate, for the same reason
 *   the inverse is: sixteen small readers written twice, once per language, are
 *   sixteen chances for two clients to disagree about what a septuple says. What
 *   a domain adds is the **request** — what that vocabulary needs beside the
 *   report, which is the multitrack, the timeline, an axis or nothing at all.
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

import { domainCoalesceKey, editingIntake, type Intake } from "../../document.ts";

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
     * Whether the crate reads this vocabulary's gestures.
     *
     * True for the four structures it knows, and **false for a domain written
     * outside it** — a page's own `Domain` over its own object, which the
     * editing surface has always accepted. Such a domain answers with
     * {@link Domain.payload} and {@link Domain.label} as it always did, and
     * {@link Domain.read} assembles the same shape out of them, so nothing
     * downstream can tell the two apart.
     */
    readonly ingested: boolean = false;

    /**
     * What the last {@link Domain.read} came to, held for the length of one
     * gesture.
     *
     * The editor asks once and then wants three things off the answer — the
     * payloads, the label, and whether the run carried its own inverse — and
     * asking the crate again for each would be three readings of one gesture.
     */
    protected taken: Intake = { payloads: [], label: "edit" };

    /**
     * What this vocabulary needs beside the report, as {@link editingIntake}
     * reads it.
     *
     * The default is the report alone, which is what the two stateless
     * vocabularies take. A domain over a structure the reading depends on — the
     * multitrack, the timeline — states it here, and so does one whose axis is the
     * view's.
     */
    request(_structure: S, _tag: string, values: readonly unknown[]): Record<string, unknown> {
        return { values: [...values] };
    }

    /**
     * **What a gesture means**, in this vocabulary — the one door.
     *
     * The payloads, what an undo menu calls them, and `inverse` or `refusal`
     * where there is one. No payloads and no refusal is "nothing to say", which
     * is the ordinary answer rather than a failure: a view emits tags for
     * everything it can do and a domain answers for the ones that are edits of
     * *its* structure.
     */
    read(structure: S, tag: string, values: readonly unknown[]): Intake {
        if (this.ingested && this.name) {
            this.taken = editingIntake(this.name, tag, this.request(structure, tag, values));
            return this.taken;
        }
        const payloads = this.payloads(structure, tag, values);
        const reason = this.refusal(structure, tag, values);
        this.taken = {
            payloads,
            label: payloads.length > 0 ? this.label(payloads[0]) : "edit",
            ...(reason === null ? {} : { refusal: reason }),
        };
        return this.taken;
    }

    /**
     * The gesture as a payload in this vocabulary, or `null` when the tag is not
     * this domain's.
     *
     * The singular door, and what a domain written outside the crate
     * implements. {@link Domain.read} is what an editor actually goes through —
     * a report is the whole structure for two of the four vocabularies, so one
     * message is however many edits it takes.
     */
    payload(structure: S, tag: string, values: readonly unknown[]): unknown {
        if (!this.ingested) {
            throw new Error(`${this.constructor.name} states no payload for a gesture`);
        }
        const found = this.payloads(structure, tag, values);
        return found.length === 1 ? found[0] : null;
    }

    /**
     * The gesture as **however many payloads it takes**, in order.
     *
     * The plural door, and the default is the singular one wrapped: most
     * gestures are one edit, and a domain that never needs more never mentions
     * this. What needs it is a report that states the *whole structure* — a
     * multitrack's boxes after a block drag, where one message says a move, a
     * trim and a lane's new contents at once — and those are one entry in the
     * history, because they are one thing a hand did.
     */
    payloads(structure: S, tag: string, values: readonly unknown[]): unknown[] {
        if (this.ingested) return this.read(structure, tag, values).payloads;
        const payload = this.payload(structure, tag, values);
        return payload === null || payload === undefined ? [] : [payload];
    }

    /**
     * Why a gesture this domain *does* understand cannot be written — `null`
     * when there is no such case.
     *
     * The difference from {@link payload} answering `null` is the whole of it: a
     * tag that is not this domain's is nothing, and the host goes on drawing
     * what it drew because nothing here disagrees. A tag that *is* this domain's
     * and cannot be honoured is a **refusal**, and a refusal the host is not
     * told about leaves the picture and the data disagreeing silently — the one
     * failure the acknowledgement exists to make impossible. What comes back is
     * the sentence the user is shown.
     */
    refusal(structure: S, tag: string, values: readonly unknown[]): string | null {
        if (!this.ingested) return null;
        return this.read(structure, tag, values).refusal ?? null;
    }

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

    /**
     * What an undo menu calls this edit.
     *
     * **It travels with the gesture and not with the payload**, because for one
     * vocabulary it is not a function of the payload at all: both of a roll's
     * lanes state the same whole-list intent, so only the gesture knows whether
     * a hand edited the notes or the markers. {@link Domain.read} is what states
     * it; this reads it back off the last one.
     */
    label(_payload: unknown): string {
        return this.ingested ? this.taken.label : "edit";
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
