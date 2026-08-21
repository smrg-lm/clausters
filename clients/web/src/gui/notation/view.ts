// Putting an engraved page on screen (mirrors `clausters/gui/notation/view.py`).
//
// A helper over a display list the engraver already produced: `scoreView` wraps
// it in a `scroll` sized to the page, ready to drop into a window.
//
// **The other half of the reference module is not here yet.** Its `transport`
// hands back the shared `clausters.gui.transport.Transport` with the page's own
// unit filled in — a `score` widget places its cursor in score milliseconds, not
// samples — and that transport is part of the arrangement/editor port, which
// this client does not have. It lands with it, not before; what is missing is
// the transport, not a decision about it.

import { score, scroll } from "../guidef.ts";
import type { GuiNode } from "../guidef.ts";

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
