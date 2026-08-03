// Panning, the stereo field and selection (mirrors
// `clausters/defs/ugens/pan.py`).
//
// A UGen has one output, so every function here that produces two channels
// returns a `ChannelList` built from single-output nodes — the package's
// multichannel rule, applied to the stereo primitives.

import { ChannelList, Ugen, chans, isList, mix } from "./graph.ts";
import type { Channel } from "./graph.ts";

// Every panner takes the channel it is computing as its last input; that is
// the builder's business, and never an argument here.

/**
 * Places a mono `signal` between two channels at `pos` (−1 left, 0 centre,
 * 1 right), at **equal power**: the two gains hold `l² + r² = 1`, so a
 * source keeps one loudness as it crosses the field. The price is that the
 * centre is 0.707 in each channel — use `linPan2` when it is the summed
 * amplitude that has to stay put.
 */
export const pan2 = (
    signal: Channel,
    pos: Channel = 0.0,
    level: Channel = 1.0,
): ChannelList =>
    chans([0.0, 1.0].map((c) => new Ugen("Pan2", [signal, pos, level, c])));

/**
 * `pan2` with the **constant-amplitude** law: the two gains sum to `level`
 * at every position, 0.5 each at the centre.
 */
export const linPan2 = (
    signal: Channel,
    pos: Channel = 0.0,
    level: Channel = 1.0,
): ChannelList =>
    chans([0.0, 1.0].map((c) => new Ugen("LinPan2", [signal, pos, level, c])));

/**
 * Shifts an **already stereo** pair towards one side by attenuating the
 * other, at equal power. A centred `balance2` is not a pass-through: both
 * sides come back 3 dB down.
 */
export const balance2 = (
    left: Channel,
    right: Channel,
    pos: Channel = 0.0,
    level: Channel = 1.0,
): ChannelList =>
    chans([0.0, 1.0].map((c) => new Ugen("Balance2", [left, right, pos, level, c])));

/**
 * Rotates the plane the two signals span by `pos` **half turns** (0.25 is
 * 45°, 1 is a half turn). On a stereo pair it turns the image without
 * changing its size or its level — the rotation is equal power at every
 * angle.
 *
 * At a quarter turn the rotation *is* the change of basis between left/right
 * and mid/side, which is what `midSide` names directly. To move an image
 * rather than resize it, this is the tool; for the size, see `stereoWidth`.
 */
export const rotate2 = (
    x: Channel,
    y: Channel,
    pos: Channel = 0.0,
): ChannelList => chans([0.0, 1.0].map((c) => new Ugen("Rotate2", [x, y, pos, c])));

/**
 * The mid/side matrix, normalized so it is **its own inverse**: the same call
 * encodes `(left, right)` into `(mid, side)` and decodes it back.
 *
 * Its point is what you can do in between — treat the centre and the sides of
 * a mix as separate signals:
 *
 * ```ts
 * const [m, s] = midSide(left, right);
 * const [left2, right2] = midSide(lpf(m, 400), s.mul(1.5));
 * ```
 *
 * A mono pair has no side at all (exactly zero). The normalization is `1/√2`
 * rather than the `1/2` a DAW meter shows, which is what makes the round trip
 * exact; it puts the mid 3 dB above the convention, a plain gain. For a width
 * knob and nothing in between, `stereoWidth` is one row instead of two.
 */
export const midSide = (a: Channel, b: Channel): ChannelList =>
    chans([0.0, 1.0].map((c) => new Ugen("MidSide", [a, b, c])));

/**
 * Widens or narrows a stereo image by scaling its side component: `0`
 * collapses to mono, `1` is exactly the identity, `2` is the textbook
 * widening, and a negative width swaps the sides.
 *
 * The same thing `midSide` does in two steps, in one row. Note what widening
 * does **not** do: it leaves the mono sum exactly where it was, because only
 * the side component is scaled and the mid is what survives a fold-down. So
 * every dB it adds to a channel is a dB a mono listener never hears — which
 * is the real cost of pushing it past 1, and the reason to check a fold-down
 * afterwards.
 */
export const stereoWidth = (
    left: Channel,
    right: Channel,
    width: Channel = 1.0,
): ChannelList =>
    chans([0.0, 1.0].map((c) => new Ugen("StereoWidth", [left, right, width, c])));

/**
 * Places a mono `signal` on a **ring** of `numchans` channels. `pos` spans
 * the whole ring over `[-1, 1]`, so −1 and 1 are the same place.
 *
 * Each channel gets a raised sine lobe `width` channels wide, centred on the
 * source: at the default width of two, neighbouring channels hold equal power
 * between them and a source parked on a channel is exactly unity there.
 * Narrower leaves gaps, wider spreads into more channels at once.
 * `orientation` turns the ring itself — 0.5, the default, puts the origin
 * between two channels, which is what an even ring wants; use 0 to put a
 * channel at the front.
 *
 * Returns `numchans` channels: `out(0, panAz(4, sig, pos))`.
 */
export function panAz(
    numchans: number,
    signal: Channel,
    pos: Channel = 0.0,
    level: Channel = 1.0,
    width: Channel = 2.0,
    orientation: Channel = 0.5,
): ChannelList {
    if (!Number.isInteger(numchans) || numchans < 1) {
        throw new TypeError(
            `panAz needs a channel count of at least 1, got ${String(numchans)}`,
        );
    }
    return new ChannelList(
        Array.from(
            { length: numchans },
            (_unused, c) =>
                new Ugen("PanAz", [
                    signal, pos, level, width, orientation, numchans, c,
                ]),
        ),
    );
}

/** Equal-power crossfade between two signals: −1 is all `a`, 1 is all `b`. */
export const xfade2 = (
    a: Channel,
    b: Channel,
    pan: Channel = 0.0,
    level: Channel = 1.0,
): Ugen => new Ugen("XFade2", [a, b, pan, level]);

/**
 * Crossfade with the constant-amplitude law — the right one for correlated
 * sources.
 */
export const linXfade2 = (
    a: Channel,
    b: Channel,
    pan: Channel = 0.0,
    level: Channel = 1.0,
): Ugen => new Ugen("LinXFade2", [a, b, pan, level]);

/**
 * @internal — the sources of a selector, given as arguments or as one list.
 * Exported for `demand`'s `dswitch1`, which takes them the same way, as the
 * Python package shares its own underscored helper between the two families.
 */
export function sources(
    list: readonly Channel[] | [ChannelList | readonly Channel[]],
): Channel[] {
    const items = list.length === 1 && isList(list[0])
        ? new ChannelList(list[0]).items
        : (list as readonly Channel[]);
    if (items.length === 0) throw new TypeError("a selector needs at least one source");
    return [...items];
}

/**
 * Outputs one of `sources`, chosen by the `which` index (truncated, and
 * clamped to the ends rather than wrapping). Every source runs whether or
 * not it is selected — they are UGens in the graph, not branches — so this
 * picks what is *heard*, never what is computed.
 */
export const select = (
    which: Channel,
    ...items: readonly Channel[] | [ChannelList | readonly Channel[]]
): Ugen => new Ugen("Select", [which, ...sources(items)]);

/**
 * `select` with the index's fraction crossfading to the next source, at
 * equal power: `which = 0.5` is halfway between the first two.
 */
export const selectX = (
    which: Channel,
    ...items: readonly Channel[] | [ChannelList | readonly Channel[]]
): Ugen => new Ugen("SelectX", [which, ...sources(items)]);

/**
 * Spreads `signals` evenly across the stereo field and mixes them down to
 * two channels — one `pan2` per signal, summed. A client-side convenience,
 * not a UGen; unlike sclang's, it does not normalize behind your back.
 */
export function splay(
    signals: ChannelList | readonly Channel[],
    spread = 1.0,
    level: Channel = 1.0,
    center = 0.0,
): ChannelList {
    const items = new ChannelList(signals).items;
    const n = items.length;
    const span = n === 1
        ? [0.0]
        : Array.from({ length: n }, (_unused, i) => (i / (n - 1)) * 2.0 - 1.0);
    const panned = items.map((s, i) => pan2(s, center + span[i]! * spread, level));
    // Mix each side down separately, so the fold uses the fused sums instead
    // of an Add chain per channel.
    return chans([
        mix(panned.map((p) => p.at(0))),
        mix(panned.map((p) => p.at(1))),
    ]);
}
