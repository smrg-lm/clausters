// The areas, and what arming one means for the others -- `base/log.ts`.
//
// The Python client's twin is `tests/test_log.py`. The module is the same one in
// two languages: same three areas, same `watch`/`unwatch`, same environment
// spelling, and the same rule that an ancestor answers for its children without
// printing a line twice.
//
// Run with `npm test`.

import assert from "node:assert/strict";
import test from "node:test";

import { area, unwatch, watch } from "../src/base/log.ts";

/** A sink that only records. */
function recorder(): { debug: (m: string) => void; warn: (m: string) => void; lines: string[] } {
    const lines: string[] = [];
    return { lines, debug: (m) => lines.push(m), warn: (m) => lines.push(m) };
}

test("an area is silent until it is watched", () => {
    const sink = recorder();
    area("gui").debug("nobody asked");
    assert.deepEqual(sink.lines, []);
    assert.equal(area("gui").watched, false);
});

test("watching an area prints it and leaves the others quiet", () => {
    const sink = recorder();
    watch("gui", sink);
    try {
        area("gui").debug("a host command");
        area("server").debug("not this one");
    } finally {
        unwatch();
    }
    assert.deepEqual(sink.lines, ["clausters.gui: a host command"]);
});

test("an ancestor answers for its children, and arming both prints once", () => {
    // The rule Python's `_already_watched` is for: a record propagates up, so a
    // handler at each level prints every line of the narrower area twice -- which
    // is what `CLAUSTERS_LOG=gui,gui.editing` did, and a doubled log is one a
    // reader stops trusting.
    const sink = recorder();
    watch("gui", sink);
    watch("gui.editing", sink);
    try {
        area("gui.editing").debug("a joint");
    } finally {
        unwatch();
    }
    assert.deepEqual(sink.lines, ["clausters.gui.editing: a joint"]);
});

test("watching the root catches every area", () => {
    const sink = recorder();
    watch("", sink);
    try {
        area("server").debug("one");
        area("gui.editing").debug("two");
    } finally {
        unwatch();
    }
    assert.equal(sink.lines.length, 2);
});
