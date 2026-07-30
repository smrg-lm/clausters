// The display-scale watch: the browser's answer to `ScaleFactorChanged`.
//
// A `ResizeObserver` cannot see this change — browser zoom, or a drag onto a
// monitor of another density, moves `devicePixelRatio` while the CSS box stays
// exactly as it was — so `onScaleChange` watches a media query on the *current*
// ratio and has to **re-arm on the new one** each time it fires. That re-arming
// is the part worth a test: get it wrong and the scale is reported once and
// never again.
//
// No DOM here: `matchMedia` and `devicePixelRatio` are stubbed, which is all the
// function reads.

import assert from "node:assert/strict";
import test from "node:test";

import { onScaleChange } from "../src/gui/page.ts";

/** One stubbed media query, with the listeners the code attached to it. */
interface Query {
    media: string;
    listeners: Set<() => void>;
}

/** Installs the stubs, returning the queries created and a restore function. */
function stubMatchMedia(): { queries: Query[]; restore: () => void } {
    const scope = globalThis as unknown as {
        matchMedia?: unknown;
        devicePixelRatio?: number;
    };
    const before = { matchMedia: scope.matchMedia, ratio: scope.devicePixelRatio };
    const queries: Query[] = [];
    scope.matchMedia = (media: string) => {
        const q: Query = { media, listeners: new Set() };
        queries.push(q);
        return {
            media,
            matches: true,
            addEventListener: (_: string, fn: () => void) => q.listeners.add(fn),
            removeEventListener: (_: string, fn: () => void) => q.listeners.delete(fn),
        };
    };
    return {
        queries,
        restore: () => {
            scope.matchMedia = before.matchMedia;
            scope.devicePixelRatio = before.ratio;
        },
    };
}

/** Moves the stubbed ratio and fires the query armed on the old one. */
function moveRatio(queries: Query[], to: number): void {
    (globalThis as unknown as { devicePixelRatio: number }).devicePixelRatio = to;
    const armed = queries[queries.length - 1];
    for (const fn of [...armed.listeners]) fn();
}

test("the scale watch re-arms on the ratio it moved to", () => {
    const { queries, restore } = stubMatchMedia();
    (globalThis as unknown as { devicePixelRatio: number }).devicePixelRatio = 1;
    let reported = 0;
    const stop = onScaleChange(() => reported++);
    try {
        assert.equal(queries.length, 1);
        assert.equal(queries[0].media, "(resolution: 1dppx)", "armed on the ratio now");
        assert.equal(reported, 0, "arming is not a report");

        moveRatio(queries, 2);
        assert.equal(reported, 1, "the page re-measures");
        assert.equal(queries[1].media, "(resolution: 2dppx)", "re-armed on the new ratio");
        assert.equal(queries[0].listeners.size, 0, "the old query is let go");

        // A second move only works if the re-arm did: this is the failure the
        // test exists for.
        moveRatio(queries, 3);
        assert.equal(reported, 2);
        assert.equal(queries[2].media, "(resolution: 3dppx)");

        stop();
        assert.equal(queries[2].listeners.size, 0, "the disposer detaches");
        moveRatio(queries, 1);
        assert.equal(reported, 2, "nothing is reported after stopping");
    } finally {
        stop();
        restore();
    }
});

test("a run time without matchMedia watches nothing and fails at nothing", () => {
    const scope = globalThis as unknown as { matchMedia?: unknown };
    const before = scope.matchMedia;
    scope.matchMedia = undefined;
    try {
        const stop = onScaleChange(() => assert.fail("nothing can fire"));
        stop();
    } finally {
        scope.matchMedia = before;
    }
});
