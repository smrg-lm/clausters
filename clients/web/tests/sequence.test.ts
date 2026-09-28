// An event sequence: events as concrete data, held by the document, each with an
// id. The handle edits the Rust structure in place; these tests read it back
// through the same handle.
//
// The Python client's `tests/test_sequence.py` is the same suite; keep them
// reading alike.

import assert from "node:assert/strict";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { TempoMap } from "../src/base/time.ts";
import { Event } from "../src/seq/event.ts";
import { EventSequence } from "../src/seq/sequence.ts";

await loadCore();

const sequence = () =>
    new EventSequence([
        [0.0, new Event({ midinote: 60 })],
        [1.0, new Event({ midinote: 62 })],
        [2.0, new Event({ midinote: 64 })],
    ]);

test("events are held in beat order, each with an id", () => {
    const seq = new EventSequence([[2.0, new Event({ midinote: 64 })], [0.0, new Event({ midinote: 60 })]]);
    assert.equal(seq.length, 2);
    assert.deepEqual([...seq].map(([beat, event]) => [beat, event.midinote()]), [[0, 60], [2, 64]]);
    assert.deepEqual(seq.entries().map(([id]) => id).sort(), [1, 2]);
});

test("removing an event leaves its neighbours theirs", () => {
    const seq = sequence();
    const [first, second, third] = seq.entries().map(([id]) => id);
    seq.remove(first);
    seq.move(second, 3.0);
    assert.equal(seq.get(second)[0], 3.0);
    assert.equal(seq.get(second)[1].get("midinote"), 62);
    assert.equal(seq.get(third)[1].get("midinote"), 64);
    assert.throws(() => seq.get(first), RangeError);
});

test("an add answers its id and an edit its inverse", () => {
    const seq = sequence();
    const answer = seq.apply({ intent: "add", event: { at: 5.0, data: { midinote: 70 } } });
    assert.ok(answer.applied);
    assert.equal(answer.id, 4);
    seq.apply(answer.current as Record<string, unknown>); // undo
    assert.equal(seq.length, 3);
    assert.equal(seq.add(4.0, { midinote: 67 }), 5, "an id is never handed out twice");
});

test("a key is written with its family's coherence", () => {
    const seq = new EventSequence([[0.0, new Event({ freq: 440.0, midinote: 69 })]]);
    const [[id]] = seq.entries();
    seq.set(id, "midinote", 72);
    assert.ok(Math.abs(seq.get(id)[1].freq() - 523.2511306) < 1e-6);
});

test("the tempo map travels with the events", () => {
    const seq = new EventSequence([[0.0, { midinote: 60, sustain: 2.0 }]], {
        tempoMap: new TempoMap(2.0),
    });
    assert.ok(Math.abs(seq.tempoMap!.secsAt(4.0) - 2.0) < 1e-9);
    seq.tempoMap = null;
    assert.equal(seq.tempoMap, null);
    assert.equal(seq.duration(), 2.0);
});

test("the data round-trips, ids and all", () => {
    const seq = sequence();
    const back = EventSequence.fromData(seq.data());
    assert.deepEqual(back.data(), seq.data());
    assert.equal(EventSequence.fromData([{ at: 1.0, data: { midinote: 60 } }]).entries()[0][0], 1);
});

test("a refused edit says why", () => {
    assert.throws(() => sequence().remove(9), /no event 9/);
});
