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
import { existsSync } from "node:fs";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import {
    addSpanner,
    concat,
    del,
    fromNotes,
    insert,
    header,
    insertMeasures,
    interpretation,
    marks,
    measures,
    moveSteps,
    ops,
    pitch,
    removeMeasures,
    retrograde,
    setBarline,
    setBreak,
    setDur,
    setHeader,
    setMarks,
    setMeter,
    setPitches,
    sheetFromMei,
    sheetFromVoice,
    silence,
    stack,
    stretch,
    tie,
    toMei,
    toNotes,
    toTimeline,
    toVoice,
    transpose,
} from "../src/gui/notation/index.ts";
import type { Sheet } from "../src/gui/notation/index.ts";
import { Event } from "../src/seq/event.ts";
import { rest } from "../src/seq/event.ts";
import * as notation from "../src/gui/notation/index.ts";
import { setEngraverUrl } from "../src/gui/notation/index.ts";

await loadCore();

/**
 * The engraver is fetched on demand in a page, so a script has to say where it
 * is. Only the one test that opens a *typed* score needs it; everything else
 * here is the model, which is pure data and needs no engraver at all.
 */
const engraver = new URL("../vendor/verovio/verovio.js", new URL(".", import.meta.url));
const engraved = existsSync(engraver);
if (engraved) setEngraverUrl(engraver.href);

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

test("the operators rearrange a score and compose", () => {
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

// -- the interpreter: what the page means, read back into sound ---------------

function quarters(n: number, options = {}): Sheet {
    return sheetFromVoice(Array.from({ length: n }, () => ({ midis: [60], ticks: 8 })), options);
}

function idsOf(sheet: Sheet): number[] {
    const staves = sheet.staves as { voices: { items: { id: number }[] }[] }[];
    return staves[0].voices[0].items.map((item) => item.id);
}

test("the interpretation is data and comes from the core", () => {
    const reading = interpretation();
    // every number the reading depends on, in one value a caller can edit --
    // and none of them written down in this client
    assert.ok(reading.dynamics.mf > reading.dynamics.p);
    assert.equal((reading.articulations as Record<string, { factor: number }>).stacc.factor, 0.5);
    assert.equal(reading.beat_unit, 4);
    // the downbeat, and nothing else: one and three in a 4/4 is a style
    assert.deepEqual((reading.accents as { at: number[] }[]).map((a) => a.at), [[0, 1]]);
});

test("a staccato shortens the sound and moves no attack", () => {
    let sheet = quarters(4);
    const ids = idsOf(sheet);
    sheet = setMarks(sheet, ids[1], marks({ articulations: ["stacc"] }));
    const notes = toNotes(sheet);
    // the written value and the heard one are two numbers, and only one moved
    assert.equal(notes[1].dur, 1.0);
    assert.equal(notes[1].sustain, 0.5);
    assert.deepEqual(notes.map((n) => n.t), [0.0, 1.0, 2.0, 3.0]);
});

test("a dynamic governs until the next one and a hairpin shapes a span", () => {
    let sheet = quarters(8);
    const ids = idsOf(sheet);
    sheet = setMarks(sheet, ids[1], marks({ dynamic: "p" }));
    sheet = addSpanner(sheet, "crescendo", ids[1], ids[4]);
    const amps = toNotes(sheet).map((n) => n.amp);
    // the mark is on one note and governs every note after it
    assert.ok(amps[1] < amps[0]);
    // the hairpin rises across its span...
    assert.ok(amps[2] > amps[1] && amps[3] > amps[2] && amps[4] > amps[3]);
    // ...and past its far end nothing of it is left
    assert.equal(amps[5], amps[6]);
});

test("a tie is one sound and a tuplet needs no rule", () => {
    const tied = tie(quarters(3), 1, true);
    const notes = toNotes(tied);
    assert.equal(notes.length, 2, "the second note does not attack again");
    assert.equal(notes[0].dur, 2.0);
    assert.equal(notes[1].t, 2.0);
    // a triplet's division is already exact in the rational the item holds
    const triplet = stretch(quarters(3), [1, 3]);
    assert.ok(Math.abs(toNotes(triplet)[2].t - 2 / 3) < 1e-12);
});

test("an interpretation is overridden without editing the core", () => {
    const style = interpretation();
    (style.accents as unknown[]).push({ at: [1, 2], gain: 1.1, meter: "4/4" });
    style.detach = 0.6;
    const notes = toNotes(quarters(4), style);
    // a stress this reader believes in and the default does not
    assert.ok(Math.abs(notes[2].amp / notes[1].amp - 1.1) < 1e-12);
    // and a player who detaches by habit, which the default deliberately is not
    assert.equal(notes[1].sustain, 0.6);
    assert.equal(toNotes(quarters(4))[1].sustain, 1.0);
});

test("a staff names itself and never what plays it", () => {
    const duo = stack(quarters(2), quarters(2), { asStaff: true });
    const notes = toNotes(duo);
    assert.deepEqual([...new Set(notes.map((n) => n.staff))].sort(), [0, 1]);
    // the binding is made where the score is rendered, explicitly
    const timeline = toTimeline(duo, { instruments: { 0: "flute", 1: "cello" } });
    const played = new Set([...timeline].map(([, event]) => (event as Event).get("instrument")));
    assert.deepEqual([...played].sort(), ["cello", "flute"]);
});

test("a timeline carries both lengths onto the event", () => {
    let sheet = quarters(2);
    sheet = setMarks(sheet, idsOf(sheet)[0], marks({ articulations: ["stacc"] }));
    const [, first] = [...toTimeline(sheet)][0] as [number, Event];
    assert.equal(first.get("dur"), 1.0);
    assert.equal(first.sustain(), 0.5);
    assert.equal(first.midinote(), 60);
});

test("a hairpin written to a note that is gone is refused by name", () => {
    const sheet = quarters(2);
    sheet.spanners = [{ kind: "crescendo", from: 1, to: 99 }];
    assert.throws(() => toNotes(sheet), /crescendo/);
});

// -- the reader: a document back into the model -------------------------------

test("a score written, read and written again is the same bytes", () => {
    const sheet = concat(quarters(4), quarters(4));
    const once = toMei(sheet);
    assert.equal(toMei(sheetFromMei(once)), once);
});

test("a typed score opens into the model and its verbs can touch it", {
    skip: engraved ? false : "run third_party/build-verovio-wasm.sh",
}, async () => {
    // ABC is what a reader types; verovio normalizes whatever it loaded to MEI,
    // so there is one input format here rather than four.
    const phrase = "X:1\nT:Six bars\nC:Anon.\nM:4/4\nL:1/4\nK:G\n"
        + "C D E F | G/A/G/F/ E D | [CEG] G C2 |\n";
    const score = await notation.Score.open(phrase);
    const sheet = sheetFromMei(score.mei());
    const head = sheet.header as { title: string; composer: string };
    assert.equal(head.title, "Six bars");
    assert.equal(head.composer, "Anon.");
    assert.equal(sheet.key, "G");
    const staves = sheet.staves as { voices: { items: Record<string, unknown>[] }[] }[];
    const items = staves[0].voices[0].items;
    assert.equal((items[10].pitches as unknown[]).length, 3, "the chord came back a chord");
    assert.deepEqual(items[4].dur, [1, 8]);
    // and every verb the model has now works on it, which is the whole point
    const up = transpose(sheet, 2);
    const moved = (up.staves as { voices: { items: { pitches: { step: string }[] }[] }[] }[]);
    assert.equal(moved[0].voices[0].items[0].pitches[0].step, "d");
});

test("the emitter's own padding does not come back as music", () => {
    // A voice is written into whole measures, so a short one is padded. Reading
    // that back would grow the score by a rest every time it was saved.
    const duo = stack(quarters(8), quarters(4), { asStaff: true });
    const back = sheetFromMei(toMei(duo));
    const staves = back.staves as { voices: { items: unknown[] }[] }[];
    assert.equal(staves[1].voices[0].items.length, 4, "four quarters, not padding");
});

test("the header, the barlines and the breaks are edited and survive", () => {
    let sheet = concat(quarters(4), quarters(4));
    sheet = setHeader(sheet, header({ title: "Study", composer: "A. Composer" }));
    sheet = setBarline(sheet, 1, "rptend");
    sheet = setBreak(sheet, 2, "system");
    const back = sheetFromMei(toMei(sheet));
    assert.deepEqual(back.header, { title: "Study", composer: "A. Composer" });
    const grid = back.grid as { barlines: unknown[]; breaks: unknown[] };
    assert.deepEqual(grid.barlines, [[0, "rptend"]]);
    assert.deepEqual(grid.breaks, [[1, "system"]]);
    // and taking one back removes it rather than storing "ordinary"
    const plain = setBarline(sheet, 1, "single");
    assert.deepEqual((plain.grid as { barlines?: unknown[] }).barlines ?? [], []);
});

test("a beam somebody chose is a spanner like any other", () => {
    let sheet = sheetFromVoice(Array.from({ length: 4 }, () => ({ midis: [60], ticks: 4 })));
    const ids = idsOf(sheet);
    sheet = addSpanner(sheet, "beam", ids[0], ids[3]);
    const mei = toMei(sheet);
    assert.match(mei, /<beam>/);
    const back = sheetFromMei(mei);
    assert.deepEqual(
        (back.spanners as unknown[]).find((s) => (s as { kind: string }).kind === "beam"),
        { kind: "beam", from: ids[0], to: ids[3] },
    );
});

test("what is not a score says so", () => {
    assert.throws(() => sheetFromMei("<not xml"), /XML/);
    assert.throws(() => sheetFromMei("<mei><music/></mei>"), /score/);
});

// -- the score's edit path, on the model --------------------------------------

test("an open score carries the model and is edited through its verbs", {
    skip: engraved ? false : "run third_party/build-verovio-wasm.sh",
}, async () => {
    const score = await notation.Score.open("X:1\nT:t\nM:4/4\nL:1/4\nK:G\nC D E F |\n");
    const items = (s: notation.Sheet) =>
        (s.staves as { voices: { items: { id: number; pitches: { step: string }[] }[] }[] }[])[0]
            .voices[0].items;
    assert.equal(items(score.sheet()).length, 4);
    // the same payload the sheet verbs build: an edit to an open score and an
    // edit to a sheet in hand are one operation through one implementation
    assert.ok(score.apply({ op: "transpose", semitones: 2 }));
    assert.deepEqual(items(score.sheet()).map((i) => i.pitches[0].step), ["d", "e", "f", "g"]);
    // and undo puts the model back, not only the page
    assert.ok(score.undo());
    assert.equal(items(score.sheet())[0].pitches[0].step, "c");
});

test("a note dragged on the page moves the model's item", {
    skip: engraved ? false : "run third_party/build-verovio-wasm.sh",
}, async () => {
    const score = await notation.Score.open("X:1\nT:t\nM:4/4\nL:1/4\nK:Eb\nA A A A |\n");
    const items = (s: notation.Sheet) =>
        (s.staves as { voices: { items: { id: number; pitches: { step: string; alter: number }[] }[] }[] }[])[0]
            .voices[0].items;
    const first = items(score.sheet())[0].id;
    assert.ok(score.transpose(`n${first}`, 1));
    const moved = items(score.sheet())[0].pitches[0];
    // in E flat, dragging a note onto B gives B flat: reading in a key is what
    // the arrival means, and nobody has to say so
    assert.equal(moved.step, "b");
    assert.equal(moved.alter, -1);
});

test("a refused operation leaves the page and the model alone", {
    skip: engraved ? false : "run third_party/build-verovio-wasm.sh",
}, async () => {
    const score = await notation.Score.open("X:1\nT:t\nM:4/4\nL:1/4\nK:C\nC D E F |\n");
    const before = score.mei();
    assert.ok(!score.apply({ op: "move_steps", id: 9999, steps: 1 }));
    assert.equal(score.mei(), before);
    assert.ok(!score.canUndo);
});

// -- note entry: the page names a place, the model turns it into a note --------

test("a place on the staff becomes a pitch the clef and the key agree on", () => {
    // What the page's `"insert"` gesture reports is a staff position, because a
    // renderer can measure a place and not a pitch. The model reads it.
    const sheet = sheetFromVoice([{ midis: [60], ticks: 8 }], { clef: "G2" });
    const pitchesOf = (s: Sheet, i: number) =>
        (s.staves as { voices: { items: { pitches: unknown[] }[] }[] }[])[0]
            .voices[0].items[i].pitches;
    // the top line of a treble staff is F5
    const written = insert(sheet, [1, 4], { after: idsOf(sheet)[0], position: 0 });
    assert.deepEqual(pitchesOf(written, 1), [{ step: "f", alter: 0, octave: 5 }]);

    // the same place on a bass staff is A3, which is why this is not a client's
    // arithmetic to do
    const bass = sheetFromVoice([{ midis: [48], ticks: 8 }], { clef: "F4" });
    const low = insert(bass, [1, 4], { after: idsOf(bass)[0], position: 0 });
    assert.equal((pitchesOf(low, 1)[0] as { step: string }).step, "a");
});

test("a place clicked in a key arrives in that key", () => {
    const sheet = sheetFromVoice([{ midis: [60], ticks: 8 }], { key: "Eb" });
    // four steps below the top line F5 is B4, and in E flat that is a B flat
    const written = insert(sheet, [1, 4], { after: idsOf(sheet)[0], position: -4 });
    const added = (written.staves as { voices: { items: { pitches: unknown[] }[] }[] }[])[0]
        .voices[0].items[1].pitches;
    assert.deepEqual(added, [{ step: "b", alter: -1, octave: 4 }]);
});

test("a page can ask for note entry and says so on the wire", () => {
    // The gesture is opt-in and its own flag: on every other page a press on
    // blank paper clears the selection, and a page that never asked for note
    // entry must keep doing exactly that.
    const page = { vb: [10, 10], glyphs: {}, prims: [] };
    const kids = (n: unknown) => (n as { children: Record<string, unknown>[] }).children;
    assert.ok(!("entry" in kids(notation.scoreView(page, { name: "s" }))[0]));
    assert.equal(kids(notation.scoreView(page, { name: "s", entry: true }))[0].entry, true);
});
