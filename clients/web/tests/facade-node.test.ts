// The package facade, `clausters`, under node: imported with no document, as
// a worker or a script imports it, it exports the session and its verbs and
// registers no element -- the Python client's `from clausters import Session,
// play, Routine`, in this client.

import assert from "node:assert/strict";
import test from "node:test";

import * as clausters from "../src/index.ts";
import { Routine } from "../src/base/stream.ts";

test("it imports with no document, and exports the session and its verbs", () => {
    assert.equal(typeof document, "undefined");
    for (const name of ["Session", "play", "panic", "plot", "scope", "edit", "openApp", "Routine", "defaultSession"]) {
        assert.ok(name in clausters, `the facade exports ${name}`);
    }
    assert.equal(clausters.Routine, Routine);
});

test("an element of a page is refused where there is no page", () => {
    assert.throws(() => clausters.page.defineComponent("fm-voice", "file:///x/"), /element of a page/);
});

test("an offline session made through it plays a routine and panics", async () => {
    const session = await clausters.Session.nrt();
    try {
        session.clock.play(
            new clausters.Routine(function* () {
                for (;;) yield 1;
            }),
        );
        assert.equal(session.clock.queued, 1);
        session.panic();
        assert.equal(session.clock.queued, 0);
    } finally {
        session.close();
    }
});
