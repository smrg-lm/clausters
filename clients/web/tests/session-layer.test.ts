// `clausters/session`: the session layer, importable where the facade is
// not. These suites run under node, with no document, which is exactly the
// program the subpath is for -- the facade would fail at its first line here
// (it defines the custom elements). What it exports is the facade's own
// objects, from the same modules, not copies of them.

import assert from "node:assert/strict";
import test from "node:test";

import * as layer from "../src/session-layer.ts";
import { Session } from "../src/session.ts";
import { main } from "../src/base/main.ts";
import { play } from "../src/play.ts";
import { panic } from "../src/panic.ts";
import { plot } from "../src/plot.ts";
import { scope } from "../src/scope.ts";
import { Routine } from "../src/base/stream.ts";

test("it imports with no document, and exports the session and its verbs", () => {
    assert.equal(typeof document, "undefined");
    assert.equal(layer.Session, Session);
    assert.equal(layer.main, main);
    assert.equal(layer.play, play);
    assert.equal(layer.panic, panic);
    assert.equal(layer.Routine, Routine);
    assert.equal(layer.plot, plot);
    assert.equal(layer.scope, scope);
    assert.deepEqual(
        Object.keys(layer).sort(),
        [
            "Environment", "PatchWindow", "PlotWindow", "RandomContext", "Routine",
            "ScopeWindow", "Session", "defaultSession", "main", "panic", "play", "plot", "scope",
        ],
    );
});

test("an offline session made through it plays and panics", async () => {
    const session = await layer.Session.nrt();
    try {
        session.clock.play(
            new Routine(function* () {
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
