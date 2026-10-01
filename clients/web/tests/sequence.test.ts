// An event sequence: events as concrete data, held by the document, each with an
// identity of its own. The handle edits the Rust structure in place, and a page
// reads and writes it through objects -- a `SeqEvent` per event, an
// `Automation` per curve -- that these tests read back.
//
// The Python client's `tests/test_sequence.py` is the same suite; keep them
// reading alike.

import assert from "node:assert/strict";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { TempoMap } from "../src/base/time.ts";
import { Event } from "../src/seq/event.ts";
import { Automation } from "../src/multitrack.ts";
import { EventSequence } from "../src/seq/sequence.ts";

await loadCore();

const sequence = () =>
    new EventSequence([
        [0.0, new Event({ midinote: 60 })],
        [1.0, new Event({ midinote: 62 })],
        [2.0, new Event({ midinote: 64 })],
    ]);

test("events are held in beat order", () => {
    const seq = new EventSequence([[2.0, new Event({ midinote: 64 })], [0.0, new Event({ midinote: 60 })]]);
    assert.equal(seq.length, 2);
    assert.deepEqual([...seq].map(([beat, event]) => [beat, event.midinote()]), [[0, 60], [2, 64]]);
    assert.deepEqual([...seq.events].map((event) => event.at), [0, 2]);
});

test("removing an event leaves its neighbours theirs", () => {
    const seq = sequence();
    const [first, second, third] = seq.events;
    first.remove();
    second.at = 3.0;
    assert.ok(second.at === 3.0 && second.get("midinote") === 62);
    assert.ok(third.get("midinote") === 64);
    assert.deepEqual([...seq.events], [third, second]);
    assert.equal(first.sequence, null);
    assert.throws(() => { first.at = 1.0; }, /no longer holds/);
});

test("an add answers the event it made", () => {
    const seq = sequence();
    const added = seq.events.add(5.0, { midinote: 70 });
    assert.ok(added === seq.events.item(-1) && added.at === 5.0 && added.get("midinote") === 70);
    const again = seq.events.add(5.0, added); // its keys, copied
    assert.ok(again !== added);
    assert.deepEqual(seq.events.at(5.0), [added, again]);
});

test("a key is written with its family's coherence", () => {
    const seq = new EventSequence([[0.0, new Event({ freq: 440.0, midinote: 69 })]]);
    const [event] = seq.events;
    event.set("midinote", 72);
    assert.ok(Math.abs(event.event.freq() - 523.2511306) < 1e-6);
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
    assert.equal(EventSequence.fromData([{ at: 1.0, data: { midinote: 60 } }]).events.item(0).at, 1);
});

test("the events are objects, and one event is one object", () => {
    const seq = new EventSequence([
        [0.0, new Event({ midinote: 60 })],
        [1.0, new Event({ midinote: 62 })],
        [1.0, new Event({ midinote: 64 })],
        [3.0, new Event({ midinote: 67 })],
    ]);
    const first = seq.events.item(0);
    assert.ok(first === seq.events.item(0) && first === [...seq.events][0], "the identity map");
    assert.ok(first.at === 0 && first.get("midinote") === 60 && first.has("midinote"));
    assert.ok(first.event instanceof Event && first.event.get("midinote") === 60);
    assert.deepEqual(seq.events.at(1.0).map((e) => e.get("midinote")), [62, 64]);
    assert.deepEqual(seq.events.range(0.5, 3.0).map((e) => e.get("midinote")), [62, 64], "half-open");
    assert.ok(seq.events.item(-1).get("midinote") === 67 && seq.events.length === 4);
    assert.equal(new Map([[first, "a key"]]).get(seq.events.item(0)), "a key");
});

test("a read sees what the sequence holds now", () => {
    const seq = sequence();
    const second = seq.events.item(1);
    seq.applyIntent({ intent: "move", id: 2, at: 5.0 }); // as a hand on the roll
    assert.ok(second.at === 5.0 && seq.events.item(-1) === second);
    seq.applyIntent({ intent: "remove", id: 2 });
    assert.equal(second.sequence, null);
    assert.throws(() => second.at, /no longer holds/);
});

test("the curves read as automation, with their holder", () => {
    const seq = sequence();
    seq.applyIntent({ intent: "automation", automation: {
        id: 0, target: { cc: 74 }, name: "brightness",
        points: [{ at: 0.0, value: 0.0 }, { at: 2.0, value: 127.0 }],
    } });
    seq.applyIntent({ intent: "eventautomation", id: 1, automation: {
        id: 0, target: { bend: true }, points: [{ at: 0.0, value: 1.0 }],
    } });
    const [brightness] = seq.automation;
    assert.ok(brightness.name === "brightness" && brightness.held);
    assert.deepEqual(brightness.target, { cc: 74 });
    assert.deepEqual(brightness.toPoints().slice(0, 2), [0, 0]);
    const bend = seq.events.item(0).automation.item(0);
    assert.equal(bend, seq.events.item(0).automation.item(0));
    assert.deepEqual(bend.target, { bend: true });
    assert.equal(seq.events.item(1).automation.length, 0);
});

test("a refused edit says why", () => {
    const seq = sequence();
    seq.setMidi("1.0");
    assert.throws(() => seq.events.item(0).automation.add({ bend: true }, { points: [[0.0, 0.0]] }), /MIDI 1\.0/);
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
    assert.ok(Math.abs(Number(seq.events.item(-1).get("sustain")) - 0.5) < 1e-9);
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

test("a sequence curve and a note's curve are curves the sequence holds", () => {
    const seq = sequence();
    const first = seq.events.item(0);
    const curve = seq.automation.add({ cc: 74 }, { points: [[0.0, 0.0], [2.0, 127.0]], name: "brightness" });
    const bend = first.automation.add({ bend: true }, { points: [[0.0, 0.0], [0.5, 1.0]] });
    assert.deepEqual([...seq.automation], [curve]);
    assert.deepEqual([...first.automation], [bend]);
    assert.deepEqual([curve.name, curve.points.length], ["brightness", 2]);
    curve.points = [[0.0, 10.0], [4.0, 20.0]]; // written back whole
    const written = seq.data() as { automation: { points: unknown[] }[] };
    assert.deepEqual(written.automation[0].points[1], { at: 4.0, value: 20.0 });
    assert.equal(curve, seq.automation.item(0), "it keeps its id, so it is the same object");
    bend.remove();
    curve.remove();
    assert.ok(!curve.held && seq.automation.length === 0 && first.automation.length === 0);
});

test("a free curve becomes the view once added", () => {
    const level = new Automation({ target: { control: "amp" }, points: [[0.0, 0.1], [4.0, 0.5]], name: "level" });
    assert.ok(!level.held && level.id === 0);
    assert.ok(sequence().automation.add(level) === level && level.held);
    level.name = "loudness";
    assert.ok(level.write().name === "loudness" && level.id !== 0);
    assert.throws(() => sequence().automation.add(level), /held already/);
});

test("a MIDI spec admits the curves it can say", () => {
    const seq = sequence();
    const first = seq.events.item(0);
    assert.equal(seq.midi, null, "a sequence for the server");
    seq.setMidi("1.0");
    assert.equal(seq.midi, "1.0");
    first.automation.add({ pressure: true }, { points: [[0.0, 0.5]] });
    assert.throws(() => first.automation.add({ bend: true }, { points: [[0.0, 0.0]] }), /MIDI 1\.0/);
    seq.setMidi("mpe", { members: 7 });
    assert.equal(seq.midi, "mpe");
    assert.deepEqual(seq.data().midi, { mpe: { upper: false, members: 7 } });
    first.automation.add({ bend: true }, { points: [[0.0, 0.0]] });
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
    seq.automation.add({ cc: 7 }, { points: [[0.0, 100.0]] });
    seq.events.item(0).automation.add({ bend: true }, { points: [[0.0, 6.0]] });
    const messages = seq.midiMessages();
    assert.deepEqual([messages[0][0], [...messages[0][1]]], [0, [0xb0, 101, 0]], "the zone first");
    const back = EventSequence.fromSmf(seq.toSmf());
    assert.equal(back.midi, "mpe");
    assert.deepEqual([...back.automation].map((curve) => curve.target), [{ cc: 7 }], "the master's: the zone's");
    const bent = [...back.events].find((e) => e.automation.length > 0)!;
    assert.deepEqual(bent.automation.item(0).target, { bend: true });
    assert.ok(Math.abs(Number(bent.automation.item(0).points[0].value) - 6.0) < 1e-9);
});

test("a sequence goes to a MIDI 2.0 clip and back", () => {
    const seq = new EventSequence([[0.0, new Event({ midinote: 60, velocity: 100, sustain: 1.0 })]]);
    seq.setMidi("2.0");
    const first = seq.events.item(0);
    first.automation.add({ bend: true }, { points: [[0.0, 12.0]] });
    first.automation.add({ cc: 1 }, { points: [[0.0, 64.0]] });
    const data = seq.toClip();
    assert.equal(new TextDecoder().decode(data.slice(0, 8)), "SMF2CLIP");
    const back = EventSequence.fromClip(data);
    assert.equal(back.midi, "2.0");
    const values = Object.fromEntries([...back.events.item(0).automation].map(
        (c) => [Object.keys(c.target as Record<string, unknown>)[0], Number(c.points[0].value)]));
    assert.ok(Math.abs(values.bend - 12.0) < 1e-6);
    assert.ok(Math.abs(values.cc - 64.0) < 1e-3);
    assert.throws(() => EventSequence.fromClip(new TextEncoder().encode("MThd")), /SMF2CLIP/);
});

test("a sequence curve goes to its notes and a chord gives it back", () => {
    const seq = new EventSequence([60, 64, 67].map((m) => [1.0, new Event({ midinote: m, sustain: 2.0 })]));
    const curve = seq.automation.add({ bend: true, channel: 0 }, { points: [[0.0, 0.0], [4.0, 4.0]] });
    seq.automation.toEvents(curve);
    assert.ok(seq.automation.length === 0 && !curve.held);
    for (const event of seq.events) {
        assert.deepEqual(event.automation.item(0).points.map((p) => [p.at, p.value]), [[0, 1], [2, 3]]);
    }
    const back = seq.automation.fromEvents({ bend: true });
    assert.deepEqual([...seq.automation], [back]);
    assert.deepEqual(back.target, { bend: true, channel: 0 });
    const [first, second] = seq.events;
    first.automation.add({ pressure: true }, { points: [[0.0, 0.0], [2.0, 1.0]] });
    second.automation.add({ pressure: true }, { points: [[0.0, 0.0], [2.0, 0.5]] });
    assert.throws(() => seq.automation.fromEvents({ pressure: true }), /cannot say both/);
});
