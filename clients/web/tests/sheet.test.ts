// The score model in this client, against the catalog and against the Python
// shell (mirrors the score-model tests in `tests/test_gui_notation.py`).
//
// The model crosses as data and so do the operations, which buys a new verb for
// no ABI at all and costs the one thing the binding table used to give free: it
// sees one symbol and no verbs. So the catalog is what parity is read against,
// and this file is that reading for the web client — the same assertions the
// Python one makes, in the same order.
//
// Needs `./build.sh` (the core wasm). Run with `npm test`.

import assert from "node:assert/strict";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import {
    fromNotes,
    measures,
    ops,
    sheetFromVoice,
    toMei,
    transpose,
} from "../src/gui/notation/index.ts";
import { Event } from "../src/seq/event.ts";
import { rest } from "../src/seq/event.ts";
import * as notation from "../src/gui/notation/index.ts";

await loadCore();

test("the catalog and this shell name the same verbs", () => {
    // This is the test the binding table cannot be: operations ride inside a
    // payload through one symbol, so a verb that reached only one client would
    // drift silently -- the same structural blindness the props manifest has.
    const catalogued = new Set(ops().map((spec) => spec.op));
    const exposed = new Set(
        [...catalogued].filter((name) => typeof (notation as Record<string, unknown>)[name] === "function"),
    );
    assert.deepEqual(
        [...catalogued].sort(),
        [...exposed].sort(),
        "the core knows a verb this shell has no helper for",
    );
    for (const spec of ops()) {
        for (const name of spec.required) {
            assert.ok(["semitones", "steps", "span"].includes(name), name);
        }
    }
});

test("a voice lifts into the model and writes the same bytes", () => {
    const sheet = sheetFromVoice(
        [{ midis: [60], ticks: 8 }, { ticks: 8 }, { midis: [64, 67], ticks: 16 }],
        { meter: "4/4", clef: "G2", key: "C" },
    );
    // ticks became exact rationals, and MIDI numbers became spelled pitches
    const items = (sheet.staves[0] as { voices: { items: Record<string, any>[] }[] })
        .voices[0].items;
    assert.deepEqual(items[0].dur, [1, 4]);
    assert.equal(items[0].pitches[0].step, "c");
    assert.equal(items[1].kind, "rest");
    assert.equal(items[2].pitches.length, 2);
    // and writing a monophonic one out is byte for byte what `fromNotes`
    // produces, because `fromNotes` now travels this same road
    const mono = sheetFromVoice([
        { midis: [60], ticks: 8 },
        { ticks: 8 },
        { midis: [64], ticks: 16 },
    ]);
    assert.equal(
        toMei(mono),
        fromNotes(
            [
                new Event({ midinote: 60, dur: 1.0 }),
                rest(1.0),
                new Event({ midinote: 64, dur: 2.0 }),
            ],
            { meter: "4/4" },
        ),
    );
});

test("transposing keeps the spelling the interval implies", () => {
    const sheet = sheetFromVoice([{ midis: [60], ticks: 8 }]);
    const pitchOf = (s: typeof sheet) =>
        (s.staves[0] as { voices: { items: Record<string, any>[] }[] }).voices[0].items[0]
            .pitches[0];
    // a major third up from C is E natural, not F flat
    assert.deepEqual(pitchOf(transpose(sheet, 4)), { step: "e", alter: 0, octave: 4 });
    // a minor third is the same two steps with the alteration doing the work
    assert.equal(pitchOf(transpose(sheet, 3)).alter, -1);
    // and the sheet that was sent is untouched, because it crossed by value
    assert.equal(pitchOf(sheet).step, "c");
});

test("a measure span is resolved by the core against the grid", () => {
    // eight quarters; in 4/4 measure 2 starts at the fifth
    const voice = Array.from({ length: 8 }, () => ({ midis: [60], ticks: 8 }));
    const octaves = (s: { staves: unknown[] }) =>
        (s.staves[0] as { voices: { items: Record<string, any>[] }[] }).voices[0].items.map(
            (i) => i.pitches[0].octave,
        );
    assert.deepEqual(
        octaves(transpose(sheetFromVoice(voice, { meter: "4/4" }), 12, { span: measures(2, 2) })),
        [4, 4, 4, 4, 5, 5, 5, 5],
    );
    // in 3/4 the same span names different notes -- the arithmetic no client does
    assert.deepEqual(
        octaves(transpose(sheetFromVoice(voice, { meter: "3/4" }), 12, { span: measures(2, 2) })),
        [4, 4, 4, 5, 5, 5, 4, 4],
    );
});

test("what is refused says why and changes nothing", () => {
    const sheet = sheetFromVoice([{ midis: [60], ticks: 8 }]);
    assert.throws(() => transpose(sheet, 1, { span: measures(4, 2) }), /backwards/);
    assert.throws(() => transpose(sheet, 1, { span: measures(0, 1) }), /from 1/);
    // and what the model can hold but MEI cannot yet be written for
    const tuplet = JSON.parse(JSON.stringify(sheet));
    tuplet.staves[0].voices[0].items[0].dur = [1, 12];
    assert.throws(() => toMei(tuplet), /tuplet/);
});
