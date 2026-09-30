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

test("a sequence goes to a MIDI file and back", () => {
    const seq = new EventSequence([
        [0.0, new Event({ midinote: 60, velocity: 100, sustain: 1.0 })],
        [1.5, new Event({ midinote: 64, velocity: 80, sustain: 0.5, channel: 2 })],
    ], { tempoMap: new TempoMap(4.0) });
    const data = seq.toSmf(96);
    assert.equal(new TextDecoder().decode(data.slice(0, 4)), "MThd");
    const back = EventSequence.fromSmf(data);
    assert.deepEqual(
        [...back].map(([beat, e]) =>
            [beat, e.get("midinote"), e.get("velocity"), e.get("sustain"), e.get("channel")]),
        [[0, 60, 100, 1, undefined], [1.5, 64, 80, 0.5, 2]],
    );
    assert.ok(Math.abs(back.tempoMap!.secsAt(4.0) - 1.0) < 1e-9);
    assert.throws(() => EventSequence.fromSmf(new TextEncoder().encode("nope")), /not a MIDI file/);
});

test("a timeline renders into the events it plays", async () => {
    const { OscItem, Timeline } = await import("../src/seq/timeline.ts");
    const { Pbind, Pseq } = await import("../src/seq/pattern.ts");
    const child = new Timeline([[0.0, new Event({ midinote: 72, dur: 1.0, legato: 1.0 })]], { tempo: 4.0 });
    const tl = new Timeline([
        [0.0, new Event({ degree: 0, dur: 1.0, legato: 0.5 })],
        [1.0, new Pbind({ midinote: new Pseq([62, 64]), dur: 0.5 })],
        [2.0, OscItem("/cue", 1)],
        [3.0, child],
    ], { tempo: 2.0 });
    const seq = await tl.renderEvents();
    assert.deepEqual(
        [...seq].map(([beat, e]) => [beat, e.get("type") ?? "note", e.get("midinote"), e.get("addr")]),
        [[0, "note", 60, undefined], [1, "note", 62, undefined], [1.5, "note", 64, undefined],
            [2, "osc", undefined, "/cue"], [3, "note", 72, undefined]],
    );
    // The child's note lasts one of its beats: half a beat of the parent's.
    const last = seq.entries().at(-1)!;
    assert.ok(Math.abs(Number(last[2].get("sustain")) - 0.5) < 1e-9);
    assert.ok(Math.abs(seq.tempoMap!.secsAt(2.0) - 1.0) < 1e-9, "the timeline's map rides along");
    assert.equal((await tl.renderEvents(1.2)).length, 2);
});

test("a pattern renders into the events it plays", async () => {
    const { Pbind, Pn, Pseq } = await import("../src/seq/pattern.ts");
    const seq = await new Pbind({ degree: new Pseq([0, 2, 4]), dur: 0.5 }).renderEvents();
    assert.deepEqual([...seq].map(([beat, e]) => [beat, e.get("degree")]), [[0, 0], [0.5, 2], [1, 4]]);
    assert.equal((await new Pbind({ degree: new Pn(0) }).renderEvents(3.5)).length, 4);
});

test("a score reads into a sequence and a sequence engraves", async () => {
    const { OscItem } = await import("../src/seq/timeline.ts");
    const { sheetFromTimeline, toSequence } = await import("../src/gui/notation/mei.ts");
    const seq = new EventSequence([
        [0.0, new Event({ midinote: 60, dur: 1.0 })],
        [1.0, new Event({ midinote: 64, dur: 1.0 })],
        [1.5, OscItem("/cue")],
    ]);
    const back = toSequence(sheetFromTimeline(seq));
    assert.deepEqual(
        [...back].map(([beat, e]) => [beat, e.get("midinote"), e.get("dur")]),
        [[0, 60, 1], [1, 64, 1]],
        "the osc event has no pitch, so no note on the page",
    );
});

test("a lane and a note's expression are curves the sequence holds", () => {
    const seq = sequence();
    const first = seq.entries()[0][0];
    const lane = seq.addLane({ cc: 74 }, { points: [[0.0, 0.0], [2.0, 127.0]], name: "brightness" });
    const bend = seq.addExpression(first, { bend: true }, { points: [[0.0, 0.0], [0.5, 1.0]] });
    let data = seq.data() as {
        lanes?: { id: number; name: string; points: unknown[] }[];
        events: { id: number; expression?: { id: number }[] }[];
    };
    assert.deepEqual(data.lanes?.map((c) => [c.id, c.name, c.points.length]), [[lane, "brightness", 2]]);
    const held = data.events.find((e) => e.id === first);
    assert.deepEqual(held?.expression?.map((c) => c.id), [bend]);
    assert.ok(lane !== bend && bend !== first, "one counter for events and curves");
    seq.removeExpression(first, bend);
    seq.removeLane(lane);
    data = seq.data() as typeof data;
    assert.ok(!data.lanes?.length);
    assert.ok(!data.events.find((e) => e.id === first)?.expression?.length);
});

test("a MIDI spec admits the curves it can say", () => {
    const seq = sequence();
    const first = seq.entries()[0][0];
    assert.equal(seq.midi, null, "a sequence for the server");
    seq.setMidi("1.0");
    assert.equal(seq.midi, "1.0");
    seq.addExpression(first, { pressure: true }, { points: [[0.0, 0.5]] });
    assert.throws(() => seq.addExpression(first, { bend: true }, { points: [[0.0, 0.0]] }), /MIDI 1\.0/);
    seq.setMidi("mpe", { members: 7 });
    assert.equal(seq.midi, "mpe");
    assert.deepEqual(seq.data().midi, { mpe: { upper: false, members: 7 } });
    seq.addExpression(first, { bend: true }, { points: [[0.0, 0.0]] });
    assert.throws(() => seq.setMidi("1.0"), /bend/);
    seq.setMidi(null);
    assert.equal(seq.midi, null);
});

test("a sequence's curves go to a MIDI file and back", () => {
    const seq = new EventSequence([
        [0.0, new Event({ midinote: 60, velocity: 100, sustain: 1.0 })],
        [0.0, new Event({ midinote: 64, velocity: 100, sustain: 1.0 })],
    ]);
    seq.setMidi("mpe");
    const first = seq.entries()[0][0];
    seq.addLane({ cc: 7 }, { points: [[0.0, 100.0]] });
    seq.addExpression(first, { bend: true }, { points: [[0.0, 6.0]] });
    const messages = seq.midiMessages();
    assert.deepEqual([messages[0][0], [...messages[0][1]]], [0, [0xb0, 101, 0]], "the zone first");
    const back = EventSequence.fromSmf(seq.toSmf());
    assert.equal(back.midi, "mpe");
    const data = back.data() as {
        lanes: { target: unknown }[];
        events: { expression?: { target: unknown; points: { value: number }[] }[] }[];
    };
    assert.deepEqual(data.lanes.map((l) => l.target), [{ cc: 7 }], "the master's: the zone's");
    const bent = data.events.find((e) => e.expression !== undefined);
    assert.deepEqual(bent?.expression?.[0].target, { bend: true });
    assert.ok(Math.abs((bent?.expression?.[0].points[0].value ?? 0) - 6.0) < 1e-9);
});

test("a sequence goes to a MIDI 2.0 clip and back", () => {
    const seq = new EventSequence([[0.0, new Event({ midinote: 60, velocity: 100, sustain: 1.0 })]]);
    seq.setMidi("2.0");
    const first = seq.entries()[0][0];
    seq.addExpression(first, { bend: true }, { points: [[0.0, 12.0]] });
    seq.addExpression(first, { cc: 1 }, { points: [[0.0, 64.0]] });
    const data = seq.toClip();
    assert.equal(new TextDecoder().decode(data.slice(0, 8)), "SMF2CLIP");
    const back = EventSequence.fromClip(data);
    assert.equal(back.midi, "2.0");
    const held = (back.data() as {
        events: { expression: { target: Record<string, unknown>; points: { value: number }[] }[] }[];
    }).events[0];
    const values = Object.fromEntries(held.expression.map((c) => [Object.keys(c.target)[0], c.points[0].value]));
    assert.ok(Math.abs(values.bend - 12.0) < 1e-6);
    assert.ok(Math.abs(values.cc - 64.0) < 1e-3);
    assert.throws(() => EventSequence.fromClip(new TextEncoder().encode("MThd")), /SMF2CLIP/);
});

test("a lane goes to its notes and a chord gives it back", () => {
    const seq = new EventSequence([60, 64, 67].map((m) => [1.0, new Event({ midinote: m, sustain: 2.0 })]));
    const lane = seq.addLane({ bend: true, channel: 0 }, { points: [[0.0, 0.0], [4.0, 4.0]] });
    seq.laneToExpression(lane);
    let data = seq.data() as {
        lanes?: { id: number; target: unknown }[];
        events: { expression: { points: { at: number; value: number }[] }[] }[];
    };
    assert.ok(data.lanes === undefined || data.lanes.length === 0);
    for (const held of data.events) {
        assert.deepEqual(held.expression[0].points.map((p) => [p.at, p.value]), [[0, 1], [2, 3]]);
    }
    const back = seq.expressionToLane({ bend: true });
    data = seq.data() as typeof data;
    assert.equal(data.lanes?.[0].id, back);
    assert.deepEqual(data.lanes?.[0].target, { bend: true, channel: 0 });
    const [first, second] = seq.entries().map(([id]) => id);
    seq.addExpression(first, { pressure: true }, { points: [[0.0, 0.0], [2.0, 1.0]] });
    seq.addExpression(second, { pressure: true }, { points: [[0.0, 0.0], [2.0, 0.5]] });
    assert.throws(() => seq.expressionToLane({ pressure: true }), /cannot say both/);
});
