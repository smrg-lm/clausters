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

import { score, scroll, Source } from "../guidef.ts";
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
    /** Tags the `scroll`, so a driver can grow its content with the page. */
    scrollName?: string;
    /** The content width, in the host's units. */
    width?: number;
    /** Cursor-anchored zoom, which also decides the pan axes. */
    zoom?: boolean;
    /** The rate the playback cursor reads the engine clock through. */
    sampleRate?: number;
    /** Opt the page into pitch editing. */
    editable?: boolean;
    /** Opt into note entry: a press on blank paper reports where it landed. */
    entry?: boolean;
}

/**
 * Wrap an engraved display list in a `scroll` sized to the page, ready to drop
 * into a window. The page is an object, or a {@link Source} holding one so a
 * re-engrave reaches the definition and every window at once
 * (`source(undefined, { displayList })`). The content area is `width` wide and
 * as tall as the page's aspect needs, so a multi-system score scrolls down the
 * systems.
 *
 * `zoom` enables cursor-anchored zoom to read a dense passage, and it also
 * decides the pan axes: **zoomed in, the page is wider than the view**, so x has
 * to pan too (`axis: "both"`); without zoom the page always fits the width and
 * only y can move (`axis: "y"`, a plain vertical scroll view).
 *
 * `editable` opts the page into pitch editing: left off, a drag does nothing and
 * the view is read-only, which is what a plain plot of a score wants; a driver
 * that applies the `"transpose"` round trip passes `editable: true`. `entry`
 * opts it into **note entry**: a press on blank paper inside a staff reports
 * `"insert" <after-xml:id> <position> <staff>` — a place, not a note, since the
 * pitch needs the clef and the key and the duration is nobody's until a driver
 * chooses one.
 *
 * `scrollName` tags the **scroll**, which a driver needs for one thing: the page
 * is drawn to fit the box it is given, so an edit that adds a system would
 * shrink the whole engraving to keep it inside. Growing the box with the page
 * instead keeps the drawn size fixed and lets the scroll do what it is for:
 *
 * ```ts
 * const h = Math.round((width * vb[1]) / vb[0] * 10) / 10;
 * win.widget(name).set({ h });
 * win.widget(scrollName).set({ contentH: h });
 * ```
 *
 * Left out, the view fits whatever it is sent, which is what a page that is
 * never edited wants.
 */
export function scoreView(
    displayList: Record<string, unknown> | Source,
    {
        scrollId,
        scoreId,
        name,
        scrollName,
        width = 1000.0,
        zoom = true,
        sampleRate,
        editable,
        entry,
    }: ScoreViewOptions = {},
): GuiNode {
    // The scroll is sized from the page, so the size has to be readable here
    // whether the page arrived as an object or as a `Source` holding one — the
    // source's own expansion is what a definition carries.
    const page = displayList instanceof Source ? displayList.props() : displayList;
    const vb = (page.vb as number[] | undefined) ?? [1.0, 1.0];
    const w = vb[0] ?? 1.0;
    const aspect = w ? (vb[1] ?? 1.0) / w : 1.0;
    const height = Math.round(width * aspect * 10) / 10;
    return scroll(
        {
            id: scrollId,
            name: scrollName,
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
            entry,
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
 * milliseconds**, not samples, so this fills in that conversion and leaves the
 * rest as it is — `source(at)` starts a pass at beat `at` and answers the
 * playing `Playhead`, `extent()` gives the piece's length in beats.
 *
 * The conversion goes through the piece's time map like every other one, not
 * through a division of its own: a page is engraved on the beat axis, and the
 * millisecond a beat is drawn at is the second it falls on. Pass `tempoMap`
 * (the clock's, `TempoClock.map`) when the tempo changes along the piece;
 * `tempo` alone is that tempo as a single segment.
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
    const tr = new Transport(host, scoreId, options);
    tr.toUnits = (beats) => tr.tempoMap.secsAt(Number(beats)) * 1000.0;
    return tr;
}
