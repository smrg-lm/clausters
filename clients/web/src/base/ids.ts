// The **share** of an id space: how two clients of one server (or one host)
// divide the ids they may allocate, without a channel between them to agree.
//
// The reference client keeps this in `clausters/base/ids.py`, and so does this
// one -- the module map is part of the port, since a reader who knows one
// client looks for the other at the same relative path.

/**
 * Which slice of a client-side id space this client takes, when **more than
 * one client shares one server** — mirrors the Python client's `IdShare`.
 *
 * The server partitions node ids into a client range, its own auto range and
 * its MIDI range, and every client allocates from that one client range. That
 * is exact while a server has one client and a fiction the moment it has two:
 * both registries start at the same base, hand out the same first id, and the
 * second `/synth_new` of the pair is refused as a duplicate — or, worse,
 * accepted against the other client's node.
 *
 * Two clients that cannot talk to each other can still agree here, because
 * there is nothing to negotiate: the shares are equal slices of the range in a
 * fixed order, so `{index: 0, of: 2}` and `{index: 1, of: 2}` are disjoint by
 * arithmetic. Whoever arranges the two — a driving client and its page, a
 * host embedding a second client — hands each its own index.
 *
 * It costs range, not capability: a share of two halves what either client may
 * hold live at once, which is why the default everywhere is the whole space
 * (`{index: 0, of: 1}`).
 */
export interface IdShare {
    /** This client's slice, from `0` to `of - 1`. */
    index: number;
    /** How many clients the space is split between. */
    of: number;
}

/** The whole space: what a server's only client takes. */
export const WHOLE_SHARE: IdShare = { index: 0, of: 1 };

/**
 * The `[base, span]` of `share` within a range of `span` ids at `base`.
 *
 * The last share takes the remainder, so the slices tile the range exactly
 * rather than leaving a few ids nobody may allocate. A share of a range too
 * small to split yields an empty span, and an empty registry reports
 * exhaustion from its first call — a client that cannot allocate says so,
 * which is the failure this whole mechanism exists to make loud.
 */
export function shareOf(
    base: number,
    span: number,
    share: IdShare = WHOLE_SHARE,
): [number, number] {
    const { index, of } = share;
    if (!Number.isInteger(of) || of < 1) {
        throw new RangeError(`an id share is split between 1 or more clients, not ${of}`);
    }
    if (!Number.isInteger(index) || index < 0 || index >= of) {
        throw new RangeError(`id share ${index} is outside a split of ${of}`);
    }
    const each = Math.floor(span / of);
    const last = index === of - 1;
    return [base + index * each, last ? span - index * each : each];
}
