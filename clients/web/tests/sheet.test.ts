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
    addSpanner,
    concat,
    del,
    fromNotes,
    insert,
    insertMeasures,
    marks,
    measures,
    ops,
    pitch,
    removeMeasures,
    retrograde,
    setDur,
    setMarks,
    setMeter,
    setPitches,
    sheetFromVoice,
    silence,
    stack,
    stretch,
    tie,
    toMei,
    toVoice,
    transpose,
} from "../src/gui/notation/index.ts";
import type { Sheet } from "../src/gui/notation/index.ts";
import { Event } from "../src/seq/event.ts";
import { rest } from "../src/seq/event.ts";
import * as notation from "../src/gui/notation/index.ts";

await loadCore();

/**
 * The catalog's `snake_case` verb under this language's spelling.
 *
 * Casing is idiom and nothing else -- the call is the same call. The one
 * exception is written out rather than special-cased silently: `delete` is a
 * reserved word here, so the verb is `del`, which is the only place the two
 * clients' spellings diverge at all.
 */
function helperFor(verb: string): string {
    if (verb === "delete") return "del";
    return verb.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase());
}

test("the catalog and this shell name the same verbs", () => {
    // This is the test the binding table cannot be: operations ride inside a
    // payload through one symbol, so a verb that reached only one client would
    // drift silently -- the same structural blindness the props manifest has.
    const shell = notation as unknown as Record<string, unknown>;
    const missing = ops()
        .map((spec) => spec.op)
        .filter((verb) => typeof shell[helperFor(verb)] !== "function");
    assert.deepEqual(missing, [], "the core knows a verb this shell has no helper for");
    // and the catalog is not empty, which is how this test would pass by
    // checking nothing at all
    assert.ok(ops().length >= 20, `only ${ops().length} verbs in the catalog`);
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

test("the algebra rearranges a score and composes", () => {
    const four = sheetFromVoice(Array.from({ length: 4 }, () => ({ midis: [60], ticks: 8 })));
    const items = (s: Sheet) =>
        (s.staves[0] as { voices: { items: Record<string, any>[] }[] }).voices[0].items;

    // one score after another
    assert.equal(items(concat(four, four)).length, 8);
    // and at the same time, as voices or as staves
    assert.equal((stack(four, four).staves[0] as { voices: unknown[] }).voices.length, 2);
    assert.equal(stack(four, four, { asStaff: true }).staves.length, 2);
    // a stretch does not move a barline: four quarters doubled is two bars
    assert.deepEqual(items(stretch(four, 2))[0].dur, [1, 2]);
    assert.equal(toMei(stretch(four, 2)).match(/<measure/g)?.length, 2);
    // reversing keeps the length
    assert.equal(items(retrograde(four)).length, 4);
    // and composing two operations is the operation on the composed score
    const up = (s: Sheet) => transpose(s, 2);
    assert.equal(toMei(up(concat(four, four))), toMei(concat(up(four), up(four))));
});

test("the grid opens and closes, and the music moves with it", () => {
    const eight = sheetFromVoice(Array.from({ length: 8 }, () => ({ midis: [60], ticks: 8 })));
    assert.equal(toMei(insertMeasures(eight, 2, 1)).match(/<measure/g)?.length, 3);
    assert.equal(toMei(removeMeasures(eight, 2, 2)).match(/<measure/g)?.length, 1);
    // changing the meter rewrites no note
    const remetered = setMeter(eight, 2, 3, 4);
    const items = (s: Sheet) =>
        (s.staves[0] as { voices: { items: Record<string, any>[] }[] }).voices[0].items;
    assert.deepEqual(items(remetered).map((i) => i.dur), items(eight).map((i) => i.dur));
});

test("an edit names its item, and deleting is not silencing", () => {
    const three = sheetFromVoice(Array.from({ length: 3 }, () => ({ midis: [60], ticks: 8 })));
    const items = (s: Sheet) =>
        (s.staves[0] as { voices: { items: Record<string, any>[] }[] }).voices[0].items;
    const id = items(three)[1].id as number;

    assert.equal(items(del(three, id)).length, 2);
    assert.equal(items(silence(three, id)).length, 3);
    assert.equal(items(silence(three, id))[1].id, id, "silencing keeps the item");
    assert.deepEqual(items(setDur(three, id, [1, 2]))[1].dur, [1, 2]);
    assert.equal(items(insert(three, [1, 8], { after: id })).length, 4);
    assert.equal(items(setPitches(three, id, [pitch("b", 3, 1)]))[1].pitches[0].step, "b");
    assert.equal(items(tie(three, id))[1].tie, true);
    assert.equal((toVoice(three, [id], 1).staves[0] as { voices: unknown[] }).voices.length, 2);
    // and every verb refuses an item that is not there, saying which
    assert.throws(() => del(three, 999), /999/);
});

test("polyphony, tuplets and marks reach the page", () => {
    const items = (s: Sheet) =>
        (s.staves[0] as { voices: { items: Record<string, any>[] }[] }).voices[0].items;

    // two voices on one staff, and two staves under a brace
    let duo = sheetFromVoice([{ midis: [60], ticks: 16 }, { midis: [60], ticks: 16 }]);
    duo = stack(duo, transpose(duo, -12));
    assert.equal(toMei(duo).match(/<layer/g)?.length, 2);
    const grand = stack(
        sheetFromVoice([{ midis: [60], ticks: 32 }]),
        sheetFromVoice([{ midis: [48], ticks: 32 }]),
        { asStaff: true },
    );
    assert.match(toMei(grand), /symbol="brace"/);

    // three in the time of two, which no grid of 32nds can hold
    const tup = sheetFromVoice([{ midis: [60], ticks: 24 }]);
    (tup.staves[0] as any).voices[0].items = [1, 2, 3].map((id) => ({
        kind: "note", id, pitches: [{ step: "c", octave: 4 }], dur: [1, 12],
    })).concat([{ kind: "rest", id: 4, dur: [3, 4] } as any]);
    assert.match(toMei(tup), /num="3" numbase="2"/);

    // the marks a note carries, and what is written between two notes
    let s = sheetFromVoice([
        { midis: [60], ticks: 8 }, { midis: [64], ticks: 8 }, { midis: [67], ticks: 16 },
    ]);
    const ids = items(s).map((i) => i.id as number);
    s = setMarks(s, ids[0], marks({ articulations: ["stacc"], dynamic: "mf", sounding: [1, 8] }));
    s = addSpanner(s, "crescendo", ids[0], ids[2]);
    const mei = toMei(s);
    assert.match(mei, /<artic artic="stacc"\/>/);
    assert.ok(mei.includes("<dynam") && mei.includes('form="cres"'));
    // A sounding length stays in the score and is deliberately not written: an
    // engraver reads one as the note's real duration and advances its own clock
    // by it, which pulls every attack after it earlier.
    assert.ok(mei.includes('dur="4"') && !mei.includes("dur.ges"));
    // and a spanner naming a note that is not there is refused, not dropped
    assert.throws(() => addSpanner(s, "slur", ids[0], 999), /999/);
});

test("an accidental is printed only where it is needed", () => {
    // a scale in B flat prints no flat its armature already implies
    const flats = sheetFromVoice([{ midis: [70], ticks: 8 }, { midis: [70], ticks: 8 }],
        { key: "Bb" });
    assert.ok(!toMei(flats).includes('<accid accid="f"/>'));
    assert.match(toMei(flats), /accid\.ges="f"/);
    // a chromatic note prints its own, and does not restate it in the same bar
    const sharp = sheetFromVoice([{ midis: [66], ticks: 8 }, { midis: [66], ticks: 8 }]);
    assert.equal(toMei(sharp).match(/<accid accid="s"\/>/g)?.length, 1);
    // and a natural in a key that alters that step is a *sign*, not silence
    const natural = sheetFromVoice([{ midis: [60], ticks: 8 }], { key: "F#" });
    assert.match(toMei(natural), /<accid accid="n"\/>/);
});

test("a rest that fills a measure is written as one", () => {
    // MEI has an element for it and an engraver draws it centred in the bar,
    // which is where a reader looks; a decomposed whole rest hangs at the start
    // and reads as a rest on the downbeat with something after it.
    const duo = stack(
        sheetFromVoice([{ midis: [60], ticks: 32 }, { midis: [60], ticks: 32 }]),
        sheetFromVoice([{ midis: [60], ticks: 32 }]),
    );
    const mei = toMei(duo);
    assert.match(mei, /<mRest\/>/);
    assert.ok(!mei.includes('<rest dur="1"'));
});
