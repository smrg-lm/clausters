// A tempo map as a value: shared between clocks, written out, read back.
//
// The map is not a field of anything — it is a value on the beat axis, the
// peer of a `Timeline`, and a clock is the process that moves over it. These
// are the three facts that follow, and `clients/python/tests/test_tempomap.py`
// asserts the same ones in the same order.

import assert from "node:assert/strict";
import test from "node:test";

import { TempoClock } from "../src/base/clock.ts";
import { loadCore } from "../src/base/core.ts";
import { TempoMap } from "../src/base/time.ts";

await loadCore();

test("every clock builds its own map, so nothing has to be passed", () => {
    const a = new TempoClock(2.0);
    const b = new TempoClock(2.0);
    a.setTempo(4.0);
    assert.equal(a.tempo, 4.0);
    assert.equal(b.tempo, 2.0);
});

test("two clocks handed one map are reading one piece", () => {
    const piece = new TempoMap(1.0);
    piece.push(4.0, 2.0); // written ahead of any clock: the NRT half
    const lead = new TempoClock(1.0, { tempoMap: piece });
    const second = new TempoClock(1.0, { tempoMap: piece });
    assert.equal(lead.beats2secs(8.0), 6.0);
    assert.equal(second.beats2secs(8.0), 6.0);

    // ...and a live gesture on one is on both: the RT half, on the same map.
    lead.setTempo(3.0, { over: 4.0, unit: "seconds", curve: "exponential" });
    assert.equal(lead.map.version, second.map.version);
    assert.equal(lead.beats2secs(20.0), second.beats2secs(20.0));
    assert.equal(lead.beats2secs(0.0), 0.0); // the past is untouched
});

test("a fork stops the two being one", () => {
    const piece = new TempoMap(1.0);
    const own = new TempoClock(1.0, { tempoMap: piece.copy() });
    own.setTempo(9.0);
    assert.equal(own.tempo, 9.0);
    assert.equal(piece.tempoAt(0.0), 1.0);
});

test("a live gesture lands on a map written ahead of the clock", () => {
    // The append-only rule is the map's and stays: push refuses to go
    // backwards. Saying "from here on" is the gesture's job.
    const piece = new TempoMap(1.0);
    piece.push(4.0, 2.0);
    assert.equal(piece.push(1.0, 3.0), false); // the wasm push answers, it does not throw
    const clock = new TempoClock(1.0, { tempoMap: piece });
    clock.setTempo(3.0); // at beat 0, under the breakpoint at 4
    assert.equal(clock.tempo, 3.0);
    assert.equal(clock.beats2secs(8.0), 8.0 / 3.0); // the plan after it is gone
});

test("a map round trips through its breakpoints", () => {
    const map = new TempoMap(1.0);
    map.shaped(2.0, 6.0, 1.0, 2.0, 2, 0.0); // exponential
    const json = map.dump();
    assert.ok(!json.includes("secs"), json); // the integral is derived
    const back = TempoMap.load(json);
    assert.ok(back !== undefined);
    for (const b of [-1.0, 0.0, 2.0, 4.5, 9.0]) {
        assert.equal(back.secsAt(b), map.secsAt(b));
    }
    assert.equal(back.version, 1); // a loaded map has had no edits
});

test("a stored map is checked by the door that reads it", () => {
    for (const json of ["[]", '[{"beats":0.0,"tempo":0.0}]', "not json"]) {
        assert.equal(TempoMap.load(json), undefined, json);
    }
});
