// `edit(x)` over the three fundamental structures.
//
// One verb, three editors, and no composition anywhere: a curve a page built, a
// timeline it filled, a buffer it holds. What is checked is the acceptance the
// track was opened with — two windows over one structure share one stack, an
// edit read back is the edit that was drawn, and a window composing two
// structures undoes across both in the order the edits were made.
//
// The Python client's twin is `tests/test_gui_edit.py`, case for case.
//
// Run with `npm test`; this suite needs the core staged (`./build.sh`).

import assert from "node:assert/strict";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { Editing, NotesEditor, PointsEditor, SamplesEditor, edit } from "../src/gui/editing/index.ts";
import { Automation } from "../src/seq/automation.ts";
import { Event as SeqEvent } from "../src/seq/event.ts";
import { OscItem, Timeline } from "../src/seq/timeline.ts";
import type { GuiHost, PropValue } from "../src/gui/host.ts";
import type { GuiNode } from "../src/gui/guidef.ts";

await loadCore();

const SR = 48_000;
const TEMPO = 2.0;
const BEAT = SR / TEMPO;

/** What the host is told, so an answer can be read. */
class FakeHost {
    acks: [number, [number, Record<string, PropValue>][]][] = [];
    trees: GuiNode[] = [];
    private next = 20_000;

    allocId(): number {
        return this.next++;
    }
    open(tree: GuiNode): { id: number } {
        this.trees.push(tree);
        return { id: 900 + this.trees.length };
    }
    define(id: number, tree: GuiNode): { id: number } {
        this.trees.push(tree);
        return { id };
    }
    set(): void {}
    onMessage(): () => void {
        return () => {};
    }
    ack(seq: number): void {
        this.acks.push([seq, []]);
    }
    push(seq: number, sets: readonly (readonly [number, Record<string, PropValue>])[]): void {
        this.acks.push([seq, sets.map((s) => [s[0], s[1]])]);
    }
}

const asHost = (host: FakeHost): GuiHost => host as unknown as GuiHost;

const aCurve = (): Automation =>
    Automation.fromPoints([[0.0, 200.0, 2, 0.0], [2.0, 900.0, 1, 0.0]], null, { name: "cutoff" });

const aTimeline = (): Timeline =>
    new Timeline([
        [0.0, new SeqEvent({ midinote: 60, dur: 1.0 })],
        [1.0, new SeqEvent({ midinote: 64, dur: 1.0 })],
    ]);

/**
 * A server buffer, as the samples domain touches one: a number, a shape, and the
 * two calls that read and write its frames.
 */
class FakeBuffer {
    bufnum = 7;
    frames: number;
    channels: number;
    sampleRate = SR;
    data: number[];

    constructor(frames = 16, channels = 1) {
        this.frames = frames;
        this.channels = channels;
        this.data = new Array(frames * channels).fill(0);
    }

    getSamples({ start = 0, count = -1 }: { start?: number; count?: number } = {}) {
        const end = count < 0 ? this.data.length : start + count;
        return Promise.resolve(Float32Array.from(this.data.slice(start, end)));
    }

    setSamples(samples: ArrayLike<number>, { start = 0 }: { start?: number } = {}) {
        for (let i = 0; i < samples.length; i += 1) this.data[start + i] = Number(samples[i]);
        return Promise.resolve();
    }
}

async function opened(editor: { open: (h: GuiHost) => Promise<unknown> }) {
    const host = new FakeHost();
    await editor.open(asHost(host));
    const tree = host.trees[0] as GuiNode;
    return { host, wid: (tree.children as GuiNode[])[0]?.id as number };
}

const blob = (values: number[]): Uint8Array => new Uint8Array(Float32Array.from(values).buffer);

// ---- the verb ----

test("the verb opens the editor the structure asks for", () => {
    assert.ok(edit(aCurve(), { sampleRate: SR }) instanceof PointsEditor);
    assert.ok(edit(aTimeline(), { sampleRate: SR }) instanceof NotesEditor);
    assert.ok(edit(new FakeBuffer()) instanceof SamplesEditor);
});

test("something none of the three reads says what they are", () => {
    assert.throws(() => edit({}), /edit` opens a Buffer/);
});

// ---- a curve ----

test("a curve is drawn, edited and read back with no composition", async () => {
    const curve = aCurve();
    const editor = edit(curve, { sampleRate: SR, tempo: TEMPO });
    const { wid } = await opened(editor);

    assert.equal(
        editor.apply("/gui_event", [wid, 1, 0, "points",
            0.0, 300.0, 1, 0.0, 1.0, 500.0, 2, 0.0, 2.0, 100.0, 1, 0.0]),
        true,
    );
    // Read back through the object the caller already holds: no handing back.
    assert.deepEqual(curve.toPoints().slice(0, 2), [0.0, 300.0]);
    assert.deepEqual(curve.toPoints().slice(4, 6), [1.0, 500.0]);
    assert.equal(editor.canUndo, true);
    assert.equal(editor.undoLabel, "draw the curve");

    assert.equal(editor.undo(), true);
    assert.deepEqual(curve.toPoints().slice(0, 2), [0.0, 200.0]);
});

test("a segment's shape survives the round trip", async () => {
    // The crate carries a point's `data` and reads none of it, which is what
    // keeps an undo from putting the curve back straight.
    const curve = aCurve();
    const editor = edit(curve, { sampleRate: SR, tempo: TEMPO });
    const { wid } = await opened(editor);
    editor.apply("/gui_event", [wid, 1, 0, "points", 0.0, 300.0, 5, -4.0, 2.0, 900.0, 1, 0.0]);
    assert.deepEqual(curve.toPoints().slice(2, 4), [5, -4.0], "the shape the hand drew");
    editor.undo();
    assert.deepEqual(
        curve.toPoints().slice(2, 4),
        [2, 0.0],
        "and the shape it had before (exponential), not a straight line",
    );
});

test("a resend of the curve is not an edit", async () => {
    const curve = aCurve();
    const editor = edit(curve, { sampleRate: SR, tempo: TEMPO });
    const { wid } = await opened(editor);
    assert.equal(
        editor.apply("/gui_event", [wid, 1, 0, "points", ...curve.toPoints()]),
        false,
    );
    assert.equal(editor.canUndo, false);
});

// ---- a timeline ----

test("a roll edits the timeline the caller holds", async () => {
    const timeline = aTimeline();
    const editor = edit(timeline, { sampleRate: SR, tempo: TEMPO });
    const { wid } = await opened(editor);

    assert.equal(
        editor.apply("/gui_event", [wid, 1, 0, "notes",
            0.0, BEAT, 67, 100, 0, 2 * BEAT, BEAT, 72, 100, 0]),
        true,
    );
    const played = [...timeline].map(([beat, event]) => [beat, (event as SeqEvent).midinote()]);
    assert.deepEqual(played, [[0.0, 67], [2.0, 72]]);
    assert.equal(editor.undo(), true);
    assert.deepEqual(
        [...timeline].map(([beat, event]) => [beat, (event as SeqEvent).midinote()]),
        [[0.0, 60], [1.0, 64]],
    );
});

test("a note keeps what the roll cannot draw", async () => {
    // Order is the only identity the payload carries, so the i-th note's own
    // event is edited rather than rebuilt from the five numbers.
    const timeline = new Timeline([
        [0.0, new SeqEvent({ midinote: 60, dur: 1.0, instrument: "bell" })],
    ]);
    const editor = edit(timeline, { sampleRate: SR, tempo: TEMPO });
    const { wid } = await opened(editor);
    editor.apply("/gui_event", [wid, 1, 0, "notes", 0.0, BEAT, 65, 100, 0]);
    const [, event] = [...timeline][0] as [number, SeqEvent];
    assert.equal(event.get("instrument"), "bell");
    assert.equal(event.midinote(), 65);
});

test("what the roll does not draw is kept", async () => {
    const timeline = aTimeline();
    const marker = new OscItem("/mark");
    timeline.add(3.0, marker);
    const editor = edit(timeline, { sampleRate: SR, tempo: TEMPO });
    const { wid } = await opened(editor);
    editor.apply("/gui_event", [wid, 1, 0, "notes", 0.0, BEAT, 67, 100, 0]);
    assert.ok(
        [...timeline].some(([, item]) => item === marker),
        "a rebuilt timeline would have dropped it",
    );
});

// ---- samples ----

test("a stroke writes the server's buffer and undoes off the wire", async () => {
    const take = new FakeBuffer(8);
    const editor = edit(take, { tempo: TEMPO });
    const { wid } = await opened(editor);

    assert.equal(
        editor.apply("/gui_event", [wid, 1, 0, "draw", 0, 2, blob([0.5, -0.5]), blob([0, 0])]),
        true,
    );
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.deepEqual(take.data.slice(2, 4), [0.5, -0.5]);
    assert.equal(editor.canUndo, true);
    assert.equal(editor.undoLabel, "draw the samples");
    // The inverse rode on the wire: nothing was read back to invert it.
    assert.equal(editor.undo(), true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.deepEqual(take.data.slice(2, 4), [0, 0]);
});

test("one dragged sample is the same edit one frame wide", async () => {
    const take = new FakeBuffer(8);
    const editor = edit(take, { tempo: TEMPO });
    const { wid } = await opened(editor);
    assert.equal(editor.apply("/gui_event", [wid, 1, 0, "sample", 0, 3, 0.9, 0.0]), true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.equal(take.data[3], 0.9);
    editor.undo();
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.equal(take.data[3], 0);
});

test("a stroke on one channel of a stereo take leaves the other alone", async () => {
    const take = new FakeBuffer(4, 2);
    take.data = [0.1, 0.2, 0.1, 0.2, 0.1, 0.2, 0.1, 0.2];
    const editor = edit(take, { tempo: TEMPO });
    const { wid } = await opened(editor);
    editor.apply("/gui_event", [wid, 1, 0, "draw", 1, 1, blob([0.7, 0.8]), blob([0.2, 0.2])]);
    // The interleaved splice is a read and a write, so it settles a turn later.
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.deepEqual(
        take.data.map((v) => Math.round(v * 10) / 10),
        [0.1, 0.2, 0.1, 0.7, 0.1, 0.8, 0.1, 0.2],
    );
});

// ---- the acceptance the track was opened with ----

test("edit called twice gives two windows and one stack", async () => {
    const curve = aCurve();
    const left = edit(curve, { sampleRate: SR });
    const right = edit(curve, { sampleRate: SR });
    const { wid } = await opened(left);
    const { host: rightHost } = await opened(right);
    rightHost.acks.length = 0;

    left.apply("/gui_event", [wid, 1, 0, "points", 0.0, 400.0, 1, 0.0, 2.0, 900.0, 1, 0.0]);
    assert.equal(right.canUndo, true, "one pile, whichever window made the edit");
    assert.ok(rightHost.acks.length > 0, "and the other window is told what to draw");

    // An undo in *either* updates both, which is the whole claim.
    assert.equal(right.undo(), true);
    assert.deepEqual(curve.toPoints().slice(0, 2), [0.0, 200.0]);
});

test("a window over a curve and a roll undoes across both in order", async () => {
    // The composed case: two structures, one editing context, one order.
    const context = new Editing();
    const curve = aCurve();
    const timeline = aTimeline();
    const curveEditor = edit(curve, { sampleRate: SR, tempo: TEMPO, context });
    const roll = edit(timeline, { sampleRate: SR, tempo: TEMPO, context });
    const { wid: curveWid } = await opened(curveEditor);
    const { wid: rollWid } = await opened(roll);

    curveEditor.apply("/gui_event", [curveWid, 1, 0, "points",
        0.0, 300.0, 1, 0.0, 2.0, 900.0, 1, 0.0]);
    roll.apply("/gui_event", [rollWid, 1, 0, "notes", 0.0, BEAT, 67, 100, 0]);
    assert.equal(context.history.undoLabel, "edit the notes");

    // The notes go back first: one pile, walked in the order the edits landed.
    assert.equal(roll.undo(), true);
    assert.deepEqual([...timeline].map(([, e]) => (e as SeqEvent).midinote()), [60, 64]);
    assert.equal(curve.toPoints()[1], 300.0, "the curve has not moved yet");
    assert.equal(curveEditor.undo(), true);
    assert.equal(curve.toPoints()[1], 200.0);
});
