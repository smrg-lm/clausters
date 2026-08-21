// Putting an engraved page on screen (mirrors `clausters/gui/notation/view.py`).
//
// A helper over a display list the engraver already produced: `scoreView` wraps
// it in a `scroll` sized to the page, ready to drop into a window.
//
// Two helpers over a display list the engraver already produced: `scoreView`
// wraps it in a `scroll` sized to the page, and `transport` hands back the
// shared `Transport` with the page's own unit filled in — a `score` widget
// places its cursor in **score milliseconds**, not samples, and that conversion
// is the only thing a page needs on top of the transport the timeline views
// already use.

import { score, scroll } from "../guidef.ts";
import type { GuiNode } from "../guidef.ts";
import { Transport } from "../transport.ts";
import type { TransportOptions } from "../transport.ts";
import type { GuiHost } from "../host.ts";

/** What {@link scoreView} takes past the page itself. */
export interface ScoreViewOptions {
    /** Name the two widgets by hand; left out, the host assigns them. */
    scrollId?: number;
    scoreId?: number;
    /** Tags the inner `score` so a driver can address it by name. */
    name?: string;
    /** The content width, in the host's units. */
    width?: number;
    /** Cursor-anchored zoom, which also decides the pan axes. */
    zoom?: boolean;
    /** The rate the playback cursor reads the engine clock through. */
    sampleRate?: number;
    /** Opt the page into pitch editing. */
    editable?: boolean;
}

/**
 * Wrap an engraved display list in a `scroll` sized to the page, ready to drop
 * into a window. The content area is `width` wide and as tall as the page's
 * aspect needs, so a multi-system score scrolls down the systems.
 *
 * `zoom` enables cursor-anchored zoom to read a dense passage, and it also
 * decides the pan axes: **zoomed in, the page is wider than the view**, so x has
 * to pan too (`axis: "both"`); without zoom the page always fits the width and
 * only y can move (`axis: "y"`, a plain vertical scroll view).
 *
 * `editable` opts the page into pitch editing: left off, a drag does nothing and
 * the view is read-only, which is what a plain plot of a score wants; a driver
 * that applies the `"transpose"` round trip passes `editable: true`.
 */
export function scoreView(
    displayList: Record<string, unknown>,
    {
        scrollId,
        scoreId,
        name,
        width = 1000.0,
        zoom = true,
        sampleRate,
        editable,
    }: ScoreViewOptions = {},
): GuiNode {
    const vb = (displayList.vb as number[] | undefined) ?? [1.0, 1.0];
    const w = vb[0] ?? 1.0;
    const aspect = w ? (vb[1] ?? 1.0) / w : 1.0;
    const height = Math.round(width * aspect * 10) / 10;
    return scroll(
        {
            id: scrollId,
            axis: zoom ? "both" : "y",
            zoom,
            contentW: width,
            contentH: height,
        },
        score({
            id: scoreId,
            name,
            displayList,
            sampleRate,
            editable,
            x: 0.0,
            y: 0.0,
            w: width,
            h: height,
        }),
    );
}

/**
 * A {@link Transport} driving a `score` widget's playback cursor — play, pause,
 * stop and locate, with the cursor following the sound.
 *
 * The same transport the timeline views use; what a page needs on top is only
 * its unit: a `score` widget places its static cursor in **score
 * milliseconds**, not samples, so this fills in that conversion (a beat is
 * `1000 / tempo` ms) and leaves the rest as it is — `source(at)` starts a pass
 * at beat `at` and answers the playing `Playhead`, `extent()` gives the piece's
 * length in beats.
 *
 * The engraving is what makes both easy to write: a page hands back the notes
 * with their onsets and lengths, so the timeline a pass plays and the end it
 * stops at are read off the page itself.
 */
export function transport(
    host: GuiHost | null,
    scoreId: number,
    options: Omit<TransportOptions, "toUnits">,
): Transport {
    const tempo = Number(options.tempo);
    return new Transport(host, scoreId, {
        ...options,
        toUnits: (beats) => (beats * 1000.0) / tempo,
    });
}
