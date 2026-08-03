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

function sources(list: readonly Channel[] | [ChannelList | readonly Channel[]]) {
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
