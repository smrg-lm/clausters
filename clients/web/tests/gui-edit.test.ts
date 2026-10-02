// `edit(x)` over the three fundamental structures.
//
// One verb, three editors, and no multitrack anywhere: a curve a page built, a
// timeline it filled, a buffer it holds. What is checked is the acceptance the
// track was opened with -- two windows over one structure share one stack, an
// edit read back is the edit that was drawn, and a window composing two
// structures undoes across both in the order the edits were made.
//
// The Python client's twin is `tests/test_gui_edit.py`, case for case.
//
// Run with `npm test`; this suite needs the core staged (`./build.sh`).

import assert from "node:assert/strict";
import { TempoMap } from "../src/base/time.ts";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { Editing, NotesEditor, PointsEditor, edit, watch }
    from "../src/gui/editing/index.ts";
import { unwatch } from "../src/base/log.ts";
import { Bpf } from "../src/defs/ugens/index.ts";
import { Event } from "../src/seq/event.ts";
import { OscItem, Timeline } from "../src/seq/timeline.ts";
import { EventSequence } from "../src/seq/sequence.ts";
import { Server } from "../src/defs/server/index.ts";
import { OscNrtInterface } from "../src/base/connection.ts";
import type { GuiHost, PropValue } from "../src/gui/host.ts";
import type { GuiNode } from "../src/gui/guidef.ts";

await loadCore();

const SR = 48_000;
const TEMPO = 2.0;
const BEAT = SR / TEMPO;

/** What the host is told, so an answer can be read. */
class FakeHost {
    acks: [number, [number, Record<string, PropValue>][], string | undefined][] = [];
    trees: GuiNode[] = [];
    /** What `headClock` was told: `[id, which, transport]`. */
    clocks: [number, string, number][] = [];
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
    /** A live set, kept per widget so a query answers it -- as the host answers what a set wrote. */
    props = new Map<number, Record<string, PropValue>>();
    set(id: number, props: Record<string, PropValue>): void {
        this.props.set(id, { ...(this.props.get(id) ?? {}), ...props });
    }
    async query(id: number): Promise<{ type: string; props: Record<string, PropValue> }> {
        return { type: "notes", props: { ...(this.props.get(id) ?? {}) } };
    }
    headClock(id: { id: number }, which: string, transport = 0): void {
        this.clocks.push([id.id, which, transport]);
    }
    onMessage(): () => void {
        return () => {};
    }
    ack(
        seq: number,
        _docVersion = 0,
        _generations: readonly (readonly [number, number])[] = [],
        reason?: string,
    ): void {
        this.acks.push([seq, [], reason]);
    }
    push(
        seq: number,
        sets: readonly (readonly [number, Record<string, PropValue>])[],
        _docVersion = 0,
        _generations: readonly (readonly [number, number])[] = [],
        reason?: string,
    ): void {
        this.acks.push([seq, sets.map((s) => [s[0], s[1]]), reason]);
    }
}

const asHost = (host: FakeHost): GuiHost => host as unknown as GuiHost;

const aCurve = (): Bpf => new Bpf([[0.0, 200.0, "exp"], [2.0, 900.0]]);

const aTimeline = (): Timeline =>
    new Timeline([
        [0.0, new Event({ midinote: 60, dur: 1.0 })],
        [1.0, new Event({ midinote: 64, dur: 1.0 })],
    ]);


async function opened(editor: { open: (h: GuiHost) => Promise<unknown> }) {
    const host = new FakeHost();
    await editor.open(asHost(host));
    const tree = host.trees[0] as GuiNode;
    return { host, wid: (tree.children as GuiNode[])[0]?.id as number };
}

const blob = (values: number[]): Uint8Array => new Uint8Array(Float32Array.from(values).buffer);

// ---- the verb ----

test("the verb opens the editor the structure asks for", async () => {
    const off = { open: false } as const;
    assert.ok((await edit(aCurve(), { sampleRate: SR, ...off })) instanceof PointsEditor);
    assert.ok((await edit(aTimeline(), { sampleRate: SR, ...off })) instanceof NotesEditor);
});

test("something none of the three reads says what they are", async () => {
    await assert.rejects(() => edit({}), /edit` opens a Buffer/);
});

test("the verb opens on the host it is given", async () => {
    // `edit(x)` is one call: the window is up and listening when it resolves,
    // so nothing has to be opened afterwards.
    const host = new FakeHost();
    const curve = aCurve();
    const editor = await edit(curve, { sampleRate: SR, host: asHost(host) });
    assert.equal(host.trees.length, 1, "one window, opened by the verb");
    assert.equal(editor.closed, false);
    assert.equal(typeof editor.id, "number");
});

// ---- a curve ----

test("a curve is drawn, edited and read back with no multitrack", async () => {
    const curve = aCurve();
    const editor = await edit(curve, { sampleRate: SR, open: false });
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

test("a sweep over a curve is kept and is no edit", async () => {
    // The points editor keeps the range a sweep leaves, with its value band;
    // the three applications keep theirs as `selected` and a transport's span,
    // so the base editor keeps none.
    const editor = (await edit(aCurve(), { sampleRate: SR, open: false })) as unknown as PointsEditor;
    const { wid } = await opened(editor);
    assert.equal(editor.apply("/gui_event", [wid, 1, 0, "selection", SR, SR, -0.5, 0.25]), false);
    assert.deepEqual(editor.selection, { start: 1.0, len: 1.0, value: { min: -0.5, max: 0.25 } });
    assert.equal(editor.canUndo, false);
    const roll = new NotesEditor(new EventSequence(), { sampleRate: SR });
    assert.ok(!("selection" in roll), "an application keeps no selection dict");
});

test("an edit made against a picture an undo replaced is refused", async () => {
    // The staleness floor, on the road an editor actually travels. A host
    // stamps every event with the version it was last told, and it is told only
    // when an acknowledgement reaches it -- a round trip a hand outruns -- so an
    // edit naming an older version is the ordinary case and applies. What does
    // not is an edit made against a picture the data has moved away from
    // by a route the host never saw: here an undo.
    const curve = aCurve();
    const editor = await edit(curve, { sampleRate: SR, open: false });
    const { host, wid } = await opened(editor);

    assert.equal(
        editor.apply("/gui_event", [wid, 1, 0, "points",
            0.0, 300.0, 1, 0.0, 2.0, 100.0, 1, 0.0]),
        true,
    );
    // The version the host has just been told it is drawing, read where it
    // lives: the version is the editing **context's**, not this view's.
    const against = Editing.of(curve).version;

    assert.equal(editor.undo(), true);
    assert.deepEqual(curve.toPoints().slice(0, 2), [0.0, 200.0]);

    // The event the hand had already sent, naming the picture it was made
    // against. Refused rather than applied: an edit-back payload is absolute
    // *and* whole, so applying one made against an older picture would silently
    // drop whatever arrived in between -- here, the undo.
    assert.equal(
        editor.apply("/gui_event", [wid, 2, against, "points",
            0.0, 900.0, 1, 0.0, 2.0, 900.0, 1, 0.0]),
        false,
    );
    assert.deepEqual(curve.toPoints().slice(0, 2), [0.0, 200.0], "the undo stands");
    assert.equal(host.acks[host.acks.length - 1]![2], "the data changed since this edit");

    // And a gesture made against the picture that now holds applies.
    assert.equal(
        editor.apply("/gui_event", [wid, 3, Editing.of(curve).version, "points",
            0.0, 600.0, 1, 0.0, 2.0, 600.0, 1, 0.0]),
        true,
    );
    assert.deepEqual(curve.toPoints().slice(0, 2), [0.0, 600.0]);
});

test("a segment's shape survives the round trip", async () => {
    // The crate carries a point's `data` and reads none of it, which is what
    // keeps an undo from putting the curve back straight.
    const curve = aCurve();
    const editor = await edit(curve, { sampleRate: SR, open: false });
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
    const editor = await edit(curve, { sampleRate: SR, open: false });
    const { wid } = await opened(editor);
    assert.equal(
        editor.apply("/gui_event", [wid, 1, 0, "points", ...curve.toPoints()]),
        false,
    );
    assert.equal(editor.canUndo, false);
});

// ---- an event sequence, and a timeline rendered into one ----

/** A timeline at `tempo`, opened: its rendered sequence is what the roll edits. */
async function aRoll(tempo = TEMPO): Promise<{ timeline: Timeline; editor: NotesEditor }> {
    const timeline = aTimeline();
    timeline.map = new TempoMap(tempo);
    const editor = (await edit(timeline, { sampleRate: SR, open: false })) as unknown as NotesEditor;
    return { timeline, editor };
}

const midinotes = (seq: EventSequence): [number, unknown][] =>
    [...seq].map(([beat, e]) => [beat, e.get("midinote")]);

test("a timeline opens as the events it renders and is left as it was", async () => {
    const { timeline, editor } = await aRoll();
    const { wid } = await opened(editor);
    assert.equal(editor.apply("/gui_event", [wid, 1, 0, "notes",
        1, 0.0, BEAT, 67, 13, 0,
        2, 2 * BEAT, BEAT, 72, 13, 0]), true);
    assert.deepEqual(midinotes(editor.sequence), [[0, 67], [2, 72]]);
    // The timeline is code, and the roll edited what it produced.
    assert.deepEqual([...timeline].map(([beat, e]) => [beat, (e as Event).midinote()]), [[0, 60], [1, 64]]);
    assert.equal(editor.undo(), true);
    assert.deepEqual(midinotes(editor.sequence), [[0, 60], [1, 64]]);
});

test("a sequence is edited in place by id", async () => {
    const seq = new EventSequence([
        [0.0, new Event({ midinote: 60, dur: 1.0, instrument: "bell" })],
        [1.0, new Event({ midinote: 64, dur: 1.0 })],
    ], { tempoMap: new TempoMap(TEMPO) });
    const editor = await edit(seq, { sampleRate: SR, open: false });
    const { wid } = await opened(editor);
    const [first, second] = seq.events;
    // Note 1 is gone and note 2 moved: order is no identity, so note 2 keeps
    // its own keys and the one removed is the one named.
    editor.apply("/gui_event", [wid, 1, 0, "notes", 2, 2 * BEAT, BEAT, 65, 13, 0]);
    assert.deepEqual([...seq.events], [second]);
    assert.equal(first.sequence, null);
    assert.deepEqual([second.at, second.get("midinote")], [2, 65]);
});

test("a roll's ruler reads the sequence's own map", async () => {
    const { editor } = await aRoll();
    const roll = (editor.view!.build(editor).children as GuiNode[])[0] as unknown as Record<string, any>;
    assert.deepEqual(JSON.parse(roll.axes.x.tempo_map), JSON.parse(new TempoMap(TEMPO).dump()));
    assert.deepEqual(roll.note_ids, [1, 2]);
    assert.deepEqual(roll.notes.slice(0, 5), [0, BEAT * 0.8, 60, 13, 0]);
});

test("a note keeps what the roll cannot draw", async () => {
    const seq = new EventSequence([[0.0, new Event({ midinote: 60, dur: 1.0, instrument: "bell" })]],
        { tempoMap: new TempoMap(TEMPO) });
    const editor = await edit(seq, { sampleRate: SR, open: false });
    const { wid } = await opened(editor);
    editor.apply("/gui_event", [wid, 1, 0, "notes", 1, 0.0, BEAT, 65, 13, 0]);
    const [[, event]] = [...seq];
    assert.equal(event.get("instrument"), "bell");
    assert.equal(event.midinote(), 65);
});

test("a note the hand made gets an id and the roll is told", async () => {
    const { editor } = await aRoll();
    const { host, wid } = await opened(editor);
    editor.apply("/gui_event", [wid, 1, 0, "notes",
        1, 0.0, BEAT * 0.8, 60, 13, 0,
        2, BEAT, BEAT * 0.8, 64, 13, 0,
        0, 3 * BEAT, BEAT, 67, 90, 0]);
    assert.deepEqual([...editor.sequence.events].map((e) => e.at), [0, 1, 3]);
    const [, corrections] = host.acks.at(-1)!;
    assert.deepEqual(corrections[0]![1].note_ids, [1, 2, 3]);
});

test("what the roll does not draw is kept", async () => {
    const timeline = aTimeline();
    timeline.add(3.0, OscItem("/mark"));
    timeline.map = new TempoMap(TEMPO);
    const editor = (await edit(timeline, { sampleRate: SR, open: false })) as unknown as NotesEditor;
    const { wid } = await opened(editor);
    editor.apply("/gui_event", [wid, 1, 0, "notes", 1, 0.0, BEAT, 67, 13, 0]);
    assert.deepEqual([...editor.sequence].map(([, e]) => e.get("type") ?? "note"), ["note", "osc"]);
});

test("a marker dragged in the roll moves it", async () => {
    const timeline = aTimeline();
    timeline.add(3.0, OscItem("/hit", 7));
    timeline.map = new TempoMap(TEMPO);
    const editor = (await edit(timeline, { sampleRate: SR, open: false })) as unknown as NotesEditor;
    const { wid } = await opened(editor);
    assert.equal(editor.apply("/gui_event", [wid, 1, 0, "osc", 1.5 * BEAT, "/hit"]), true);
    const at = [...editor.sequence].filter(([, e]) => e.get("type") === "osc");
    assert.equal(at[0]![0], 1.5);
    assert.deepEqual(at[0]![1].get("args"), [7], "the message it sends is not the lane's to lose");
    assert.equal(editor.undoLabel, "edit the markers");
    assert.equal(editor.undo(), true);
    assert.deepEqual([...editor.sequence].filter(([, e]) => e.get("type") === "osc").map(([b]) => b), [3]);
});

test("a marker removed in the roll leaves its neighbours theirs", async () => {
    const seq = new EventSequence([
        [0.0, OscItem("/a", 1)], [1.0, OscItem("/b", 2)], [2.0, OscItem("/c", 3)],
    ], { tempoMap: new TempoMap(TEMPO) });
    const editor = await edit(seq, { sampleRate: SR, open: false });
    const { wid } = await opened(editor);
    assert.equal(editor.apply("/gui_event", [wid, 1, 0, "osc", 0.0, "/a", 2 * BEAT, "/c"]), true);
    assert.deepEqual([...seq].map(([, e]) => [e.get("addr"), e.get("args")]), [["/a", [1]], ["/c", [3]]]);
});

test("a marker added in the roll is refused and says why", async () => {
    const seq = new EventSequence([[0.0, OscItem("/a")]], { tempoMap: new TempoMap(TEMPO) });
    const editor = await edit(seq, { sampleRate: SR, open: false });
    const { host, wid } = await opened(editor);
    assert.equal(editor.apply("/gui_event", [wid, 1, 0, "osc", 0.0, "/a", BEAT, ""]), false);
    assert.equal(seq.length, 1);
    const [stamp, corrections, reason] = host.acks.at(-1)!;
    assert.equal(stamp, 1);
    assert.match(reason ?? "", /a marker is the message it sends/);
    assert.deepEqual(corrections[0]![1].osc, [0, "/a"]);
});

// ---- the acceptance the track was opened with ----

test("edit called twice gives two windows and one stack", async () => {
    const curve = aCurve();
    const left = await edit(curve, { sampleRate: SR, open: false });
    const right = await edit(curve, { sampleRate: SR, open: false });
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
    const seq = new EventSequence([[0.0, new Event({ midinote: 60, dur: 1.0 })]],
        { tempoMap: new TempoMap(TEMPO) });
    const curveEditor = await edit(curve, { sampleRate: SR, context, open: false });
    const roll = await edit(seq, { sampleRate: SR, context, open: false });
    const { wid: curveWid } = await opened(curveEditor);
    const { wid: rollWid } = await opened(roll);

    curveEditor.apply("/gui_event", [curveWid, 1, 0, "points",
        0.0, 300.0, 1, 0.0, 2.0, 900.0, 1, 0.0]);
    roll.apply("/gui_event", [rollWid, 1, 0, "notes", 1, 0.0, BEAT, 67, 13, 0]);
    assert.equal(context.undoLabel, "edit the notes");

    // The notes go back first: one pile, walked in the order the edits landed.
    assert.equal(roll.undo(), true);
    assert.deepEqual([...seq].map(([, e]) => e.midinote()), [60]);
    assert.equal(curve.toPoints()[1], 300.0, "the curve has not moved yet");
    assert.equal(curveEditor.undo(), true);
    assert.equal(curve.toPoints()[1], 200.0);
});

// ---- a page's change is a turn ----

test("edit takes the ambient server's rate", async () => {
    // With no sampleRate, the roll is laid out at the rate of the server a play
    // would resolve -- and at 48 kHz with none.
    const { main } = await import("../src/base/main.ts");
    const seq = new EventSequence([[0.0, new Event({ midinote: 60, dur: 1.0 })]]);
    const held = main.server;
    try {
        main.server = null;
        assert.equal((await edit(seq, { open: false })).sampleRate, 48_000, "no server anywhere");
        main.server = { queryInfo: async () => ({ nominalSampleRate: 96_000 }) } as never;
        assert.equal((await edit(seq, { open: false })).sampleRate, 96_000);
        assert.equal((await edit(seq, { sampleRate: 44_100, open: false })).sampleRate, 44_100, "a rate given wins");
    } finally {
        main.server = held;
    }
});

test("a page's change with the roll open redraws it and is undone there", async () => {
    const seq = new EventSequence([
        [0.0, new Event({ midinote: 60, dur: 1.0 })],
        [1.0, new Event({ midinote: 64, dur: 1.0 })],
    ], { tempoMap: new TempoMap(TEMPO) });
    const roll = await edit(seq, { sampleRate: SR, open: false });
    const { host } = await opened(roll);
    const told: boolean[] = [];
    roll.onChange = () => told.push(true);
    const [first, second] = seq.events;
    const drawn = host.acks.length;
    second.set("midinote", 67);
    const [, corrections] = host.acks.at(-1)!;
    assert.equal(host.acks.length, drawn + 1);
    assert.equal((corrections[0]![1].notes as number[])[7], 67,
        "the roll is redrawn with the note where the page put it");
    assert.ok(told.length === 1 && roll.undoLabel === "set midinote");

    seq.history.entry("humanize", () => {
        for (const event of seq.events) event.at += 0.25;
    });
    assert.equal(host.acks.length, drawn + 2, "one block, one redraw");
    assert.equal(roll.undoLabel, "humanize");
    assert.equal(roll.undo(), true, "the window's Ctrl+Z takes the whole block back");
    assert.deepEqual([...seq.events].map((event) => event.at), [0, 1]);
    assert.ok(roll.undo() && second.get("midinote") === 64);
    assert.ok(seq.history.redo() && second.get("midinote") === 67);
    assert.equal(first, seq.events.item(0), "one event, one object, across the walk");
});

test("a roll opened in a context it was handed claims the sequence", async () => {
    const context = new Editing();
    const seq = new EventSequence([[0.0, new Event({ midinote: 60, dur: 1.0 })]],
        { tempoMap: new TempoMap(TEMPO) });
    const roll = await edit(seq, { sampleRate: SR, context, open: false });
    await opened(roll);
    seq.events.item(0).at = 2.0;
    assert.equal(context.undoLabel, "move an event", "the page's change is that context's");
    assert.equal(Editing.of(seq), context);
});

test("a window over a held curve and the roll are one order", async () => {
    // A note's curve opened on its own is the sequence's, so its window joins
    // the roll's history: one gesture is one entry, the roll redraws it, and
    // either window takes it back.
    const seq = new EventSequence([[0.0, new Event({ midinote: 60, dur: 1.0 })]],
        { tempoMap: new TempoMap(TEMPO) });
    const bend = seq.events.item(0).automation.add({ bend: true }, { points: [[0.0, 0.0], [1.0, 2.0]] });
    const roll = await edit(seq, { sampleRate: SR, open: false });
    const { host: rollHost } = await opened(roll);
    const curve = await edit(bend, { sampleRate: SR, open: false });
    const { wid } = await opened(curve);
    const values = () => bend.points.map((p) => p.value);
    const drawn = rollHost.acks.length;
    curve.apply("/gui_event", [wid, 1, 0, "points", 0.0, 0.0, 1, 0.0, 1.0, 5.0, 1, 0.0]);
    assert.deepEqual(values(), [0, 5]);
    assert.ok(rollHost.acks.length > drawn, "the roll is redrawn");
    assert.ok(roll.undo() && !roll.canUndo, "one gesture, one entry");
    assert.deepEqual(values(), [0, 2]);
    assert.ok(curve.redo());
    assert.deepEqual(values(), [0, 5]);
});

test("a sequence nobody asked a history of records nothing", async () => {
    const { contexts } = await import("../src/history.ts");
    const seq = new EventSequence([[0.0, new Event({ midinote: 60, dur: 1.0 })]]);
    seq.events.item(0).at = 2.0;
    seq.events.add(3.0, { midinote: 62 });
    assert.equal(contexts.get(seq), undefined);
    assert.equal(seq.history.canUndo, false, "asking makes one, empty");
});

test("the routing table is the crate's and not this module's", async () => {
    // It was eight strings here and eight in the Python client's editor, and
    // nothing kept the two agreeing. Now both read the one list the document
    // crate holds beside the presentation rule it states.
    await loadCore();
    const { notAnEdit } = await import("../src/gui/editing/index.ts");
    assert.ok(notAnEdit().includes("selection"));
    assert.ok(!notAnEdit().includes("notes"), "an edit is not screen state");
    assert.equal(notAnEdit(), notAnEdit(), "read once and kept");
});

test("a catalogue view is described by the crate and not by this client", async () => {
    // Which widget draws a take, and the three gestures it offers, were written
    // here, in the Python client and in the standalone host. One answer now.
    await loadCore();
    const { viewProps } = await import("../src/core/clausters_core_web.js");
    const said = JSON.parse(
        viewProps("waveform", JSON.stringify({ buffer: 3, channels: 1, ruler: "time", sample_rate: SR })),
    ) as Record<string, unknown>;
    assert.equal(said.type, "signal");
    assert.equal(said.view, "trace");
    assert.deepEqual(said.gestures, { drag: "select", alt: "draw", ctrl: "sample" });
    assert.equal(said.id, undefined, "which number a widget gets is the caller's");

    // And the roll's pitch window, the other rule that travelled with the
    // picture: one note is its own window, padded.
    const roll = JSON.parse(
        viewProps("pianoroll", JSON.stringify({ notes: [0.0, 1.0, 60.0, 100.0, 0.0] })),
    ) as { axes: { y: unknown } };
    assert.deepEqual(roll.axes.y, { min: 56.0, max: 64.0 });
    assert.match(
        (JSON.parse(viewProps("clip", "{}")) as { error: string }).error,
        /"clip"/,
        "a kind the crate does not draw says so",
    );
    assert.match(
        (JSON.parse(viewProps("pianoroll", JSON.stringify({ notes: "sixty" }))) as { error: string }).error,
        /cannot be drawn/,
    );
});

test("a sequence with a marker still draws its notes", async () => {
    // The OSC markers are `time label` pairs, and a label is text: typed as
    // numbers alone, one marker refused the whole roll and the window opened
    // with nothing on it.
    await loadCore();
    const seq = new EventSequence([
        [0.0, new Event({ midinote: 60, dur: 1.0 })],
        [3.0, OscItem("/mark", 1, "cue")],
    ]);
    const editor = new NotesEditor(seq, { sampleRate: SR });
    const roll = (editor.view!.build(editor).children as GuiNode[])[0] as unknown as Record<string, unknown>;
    assert.equal(roll.type, "notes");
    assert.equal((roll.notes as number[]).length, 5, "the one note is on the roll");
    assert.equal((roll.osc as unknown[])[1], "/mark", "and the marker beside it, by its label");
});

test("two editors in one application keep their own floor", async () => {
    // The echo is **one view's** end of the conversation, so a window set does
    // not share one. The floor rises when the version moved and no event of
    // *this* view moved it -- with one echo per application the two windows
    // would each answer for the other, and a gesture the left window made
    // against a picture the right window had already changed would find
    // `version === applied` and be accepted, which is the whole of what the
    // floor is for.
    //
    // The Python twin is
    // `test_gui_editing.py::test_two_editors_in_one_application_keep_their_own_floor`.
    const curve = aCurve();
    const left = await edit(curve, { sampleRate: SR, open: false });
    const right = await edit(curve, {
        sampleRate: SR, open: false, app: left.app,
    });
    assert.notEqual(left.echo, right.echo, "an echo is a view's, not a window set's");
    assert.equal(left.app, right.app, "and the window set is still one");

    const before = left.echo.state.applied;
    const { wid } = await opened(right);
    assert.equal(
        right.apply("/gui_event", [wid, 1, 0, "points",
            0.0, 300.0, 1, 0.0, 1.0, 500.0, 2, 0.0]),
        true,
    );
    const version = (left as unknown as { editing: { version: number } }).editing.version;
    assert.ok(version > before, "the neighbour moved the version");
    assert.equal(
        left.echo.state.applied,
        before,
        "the neighbour's edit answered for this window's conversation",
    );
});

test("the editing trace is silent until it is watched", async () => {
    // The five joints, and the fact that they cost nothing unarmed. A window in
    // front of a person fails in ways nothing else sees, so the path says what
    // it did -- but a library that printed by default would make every importer
    // pay for the formatting.
    //
    // The Python twin is
    // `test_gui_editing.py::test_the_editing_trace_is_silent_until_it_is_watched`.
    const curve = aCurve();
    const editor = await edit(curve, { sampleRate: SR, open: false });
    const { wid } = await opened(editor);

    const quiet: string[] = [];
    assert.equal(
        editor.apply("/gui_event", [wid, 1, 0, "points", 0.0, 250.0, 1, 0.0, 1.0, 500.0, 2, 0.0]),
        true,
    );
    assert.equal(quiet.length, 0, "silent unless asked");

    const said: string[] = [];
    watch({ debug: (line) => said.push(line), warn: (line) => said.push(line) });
    try {
        assert.equal(
            editor.apply("/gui_event",
                [wid, 2, 0, "points", 0.0, 350.0, 1, 0.0, 1.0, 500.0, 2, 0.0]),
            true,
        );
    } finally {
        unwatch();
    }
    const printed = said.join("\n");
    assert.ok(printed.includes("event "), printed);
    assert.ok(printed.includes("record ["), printed);
    assert.ok(printed.includes("ack "), printed);
});

// ---- the notes editor plays ----

/** A server whose transport answers and whose commands are recorded. */
class PlayingServer extends Server {
    sent: [string, unknown[]][] = [];
    state = { playing: false };

    constructor() {
        super({ connection: new OscNrtInterface() });
        this.latency = 0.1;
    }
    override async transportState(): Promise<never> {
        return { ...this.state } as never;
    }
    override async queryInfo(): Promise<never> {
        return { nominalSampleRate: 100 } as never;
    }
    override async notify(): Promise<void> {}
    override sendMsg(addr: string, ...args: unknown[]): void {
        this.sent.push([addr, args]);
        if (addr === "/transport_play") this.state.playing = true;
    }
    override async request(addr: string, args: unknown[] = []): Promise<never> {
        this.sendMsg(addr, ...args);
        return { addr: "/done", args: [addr] } as never;
    }
    /** The note starts of the last `/lane_set`, in samples. */
    lane(): number[] | null {
        const sets = this.sent.filter(([addr]) => addr === "/lane_set");
        if (sets.length === 0) return null;
        const args = sets.at(-1)![1];
        const json = Array.isArray(args[1]) ? (args[1] as [string, string])[1] : String(args[1]);
        return (JSON.parse(json).notes as number[][]).map((note) => note[0]!);
    }
}

test("the notes editor plays on its own transport and hears an edit", async () => {
    const server = new PlayingServer();
    const seq = new EventSequence([
        [0.0, new Event({ midinote: 60, dur: 1.0 })],
        [2.0, new Event({ midinote: 64, dur: 1.0 })],
    ], { tempoMap: new TempoMap(TEMPO) });
    const editor = new NotesEditor(seq, { sampleRate: SR, server });
    const { wid } = await opened(editor);
    await editor.play();
    const addrs = server.sent.map(([addr]) => addr);
    assert.ok(addrs.includes("/transport_group") && addrs.includes("/lane_new"));
    assert.equal(addrs.at(-1), "/transport_play");
    assert.ok(!addrs.includes("/sched_atTransport"), "the transport plays it, nothing is stamped");
    // The two notes as the lane's data, at 100 samples a second and two beats
    // a second: beat 0 at 0, beat 2 at 100.
    assert.deepEqual(server.lane(), [0, 100]);
    assert.equal(await editor.playing(), true);

    // An edit is the lane's new data, and no clock is asked for.
    server.sent = [];
    editor.apply("/gui_event", [wid, 1, 0, "notes",
        1, 0.0, BEAT * 0.8, 60, 13, 0,
        2, 3 * BEAT, BEAT * 0.8, 67, 13, 0]);
    await editor.settled();
    assert.equal(server.sent[0]![0], "/lane_set");
    assert.deepEqual(server.lane(), [0, 150], "the note moved to beat 3 is heard where it lands");
    await editor.stop();
    assert.ok(server.sent.map(([addr]) => addr).includes("/transport_locateSample"));
});

test("play answers the transport the sequence plays on", async () => {
    // No window: play(sequence) loads the lane on the server's notes transport
    // and answers that transport, whose verbs speak the sequence's beats; a
    // change made through the sequence's objects is heard, and the pass ends
    // where the contents do.
    const { play } = await import("../src/play.ts");
    const { Transport } = await import("../src/defs/server/transport.ts");
    const server = new PlayingServer();
    const seq = new EventSequence([
        [0.0, new Event({ midinote: 60, dur: 1.0 })],
        [2.0, new Event({ midinote: 64, dur: 1.0 })],
    ], { tempoMap: new TempoMap(TEMPO) });
    const transport = await (play(seq, { server: server as never }) as Promise<InstanceType<typeof Transport>>);
    assert.ok(transport instanceof Transport && await transport.playing());
    assert.equal(transport, server.transportAt(transport.id), "one transport, one object");
    const addrs = server.sent.map(([addr]) => addr);
    assert.ok(addrs.includes("/lane_new") && addrs.includes("/transport_end"), "it ends with its contents");
    assert.equal(addrs.at(-1), "/transport_play");
    assert.deepEqual(server.lane(), [0, 100]);

    server.sent = [];
    seq.events.item(1).at = 3.0; // heard from where the position is
    await transport.playing(); // after the calls before it
    assert.equal(server.sent.filter(([addr]) => addr === "/lane_set").length, 1);
    assert.deepEqual(server.lane(), [0, 150]);

    assert.equal(await transport.end(), "contents");
    await transport.setEnd(4.0); // an end marker, in its beats
    assert.equal(await transport.end(), 4.0);
    server.sent = [];
    await transport.pause();
    await transport.locate(1.0);
    await transport.loop(0.0, 2.0);
    await transport.stop();
    assert.ok(server.sent.length > 0, "each verb is the playback's");
    server.state.playing = false; // the pass reached its end
    assert.equal(await transport.wait(1.0), true);
});

test("a loop asked while stopped is kept for the next play", async () => {
    // A short pass ends before a page asks for its loop: the loop is the
    // playback's state, kept stopped, and the next play loops the span.
    const server = new PlayingServer();
    const seq = new EventSequence([
        [0.0, new Event({ midinote: 60, dur: 1.0 })],
        [2.0, new Event({ midinote: 64, dur: 1.0 })],
    ], { tempoMap: new TempoMap(TEMPO) });
    const transport = await seq.play({ server: server as never });
    server.state.playing = false; // the pass reached its end
    await transport.loop(0.0, 2.0);
    assert.deepEqual(transport.span, [0, 2]);
    assert.ok(transport.looping);
    server.sent = [];
    await transport.play();
    const loops = server.sent.filter(([addr, args]) => addr === "/transport_loop" && args.length > 1);
    const span = loops.at(-1)![1].slice(1).map((a) => Number(Array.isArray(a) ? a[1] : a));
    assert.deepEqual(span, [0, 100], "beats [0, 2) at two beats a second and 100 samples a second");
    await transport.unloop();
    assert.ok(!transport.looping);
    assert.deepEqual(transport.span, [0, 2], "the span stays");
});

test("the span is drawn on the roll, and a sweep is the span", async () => {
    const server = new PlayingServer();
    const seq = new EventSequence([
        [0.0, new Event({ midinote: 60, dur: 1.0 })],
        [2.0, new Event({ midinote: 64, dur: 1.0 })],
    ], { tempoMap: new TempoMap(TEMPO) });
    const roll = new NotesEditor(seq, { sampleRate: SR, server: server as never });
    const { host, wid } = await opened(roll);
    const transport = await seq.play({ server: server as never });
    await transport.setSpan([1.0, 2.0]);
    const [, corrections] = host.acks.at(-1)!;
    const props = new Map(corrections).get(wid)!;
    assert.deepEqual([props.sel_start, props.sel_len], [BEAT, BEAT], "the band a sweep leaves");
    roll.apply("/gui_event", [wid, 1, 0, "selection", 0.0, 2 * BEAT]);
    await roll.settled();
    assert.deepEqual(transport.span, [0, 2], "a sweep is the transport's span");
});

test("what the roll marks is the events themselves", async () => {
    const seq = new EventSequence([
        [0.0, new Event({ midinote: 60, dur: 1.0 })],
        [1.0, new Event({ midinote: 62, dur: 1.0 })],
        [2.0, new Event({ midinote: 64, dur: 1.0 })],
    ], { tempoMap: new TempoMap(TEMPO) });
    const roll = await edit(seq, { sampleRate: SR, open: false }) as unknown as NotesEditor;
    await opened(roll);
    const [first, second, third] = seq.events;
    assert.deepEqual(await roll.selected(), []);
    roll.select(seq.events.between(1.0, 3.0));
    assert.deepEqual(await roll.selected(), [second, third]);
    for (const event of await roll.selected()) event.set("velocity", 90);
    assert.ok(second.get("velocity") === 90 && first.get("velocity") === undefined);
    roll.unselect();
    assert.deepEqual(await roll.selected(), []);
});

test("two rolls over one sequence send the lane one change once", async () => {
    // Every roll over a sequence is told of a change -- the one that made it,
    // and the other adopting it -- and the lane they share takes it once: the
    // same for a gesture and for a page's change. Twice, the two updates'
    // steps interleaved on the shared runner, and one waited on a step the
    // other sent.
    const server = new PlayingServer();
    const seq = new EventSequence([
        [0.0, new Event({ midinote: 60, dur: 1.0 })],
        [2.0, new Event({ midinote: 64, dur: 1.0 })],
    ], { tempoMap: new TempoMap(TEMPO) });
    const keys = new NotesEditor(seq, { sampleRate: SR, server });
    const hertz = new NotesEditor(seq, { sampleRate: SR, server, yAxis: "hz" });
    const { wid } = await opened(keys);
    await opened(hertz);
    await keys.play();
    const laneSets = () => server.sent.filter(([addr]) => addr === "/lane_set").length;

    server.sent = [];
    keys.apply("/gui_event", [wid, 1, 0, "notes",
        1, 0.0, BEAT * 0.8, 60, 13, 0,
        2, 3 * BEAT, BEAT * 0.8, 67, 13, 0]);
    await keys.settled();
    await hertz.settled();
    assert.equal(laneSets(), 1);
    assert.deepEqual(server.lane(), [0, 150]);
    server.sent = [];
    seq.events.item(0).at = 1.0;
    await keys.settled();
    await hertz.settled();
    assert.equal(laneSets(), 1);
    assert.deepEqual(server.lane(), [50, 150]);
});

test("the roll draws its play cursor from its transport and a locate cues it", async () => {
    const server = new PlayingServer();
    const seq = new EventSequence([[0.0, new Event({ midinote: 60, dur: 1.0 })]],
        { tempoMap: new TempoMap(TEMPO) });
    const editor = new NotesEditor(seq, { sampleRate: SR, server });
    const { host, wid } = await opened(editor);
    assert.equal(host.clocks.length, 1);
    assert.deepEqual(host.clocks[0]!.slice(0, 2), [901, "transport"]);
    const roll = (host.trees[0]!.children as GuiNode[])[0] as unknown as { axes: { x: { playhead_at: number } } };
    assert.equal(roll.axes.x.playhead_at, 0);
    const placed: number[] = [];
    editor.onLocate = (beat) => placed.push(beat);
    server.sent = [];
    // One beat of the roll's axis.
    editor.apply("/gui_event", [wid, 1, 0, "locate", BEAT]);
    await editor.settled();
    assert.equal(editor.cursor, 1);
    assert.deepEqual(placed, [1]);
    assert.deepEqual(server.sent.map(([addr]) => addr), ["/transport_locateSample"]);
});

test("a roll in hertz draws and edits frequencies", async () => {
    const seq = new EventSequence([[0.0, new Event({ midinote: 60, dur: 1.0 })]],
        { tempoMap: new TempoMap(TEMPO) });
    const editor = new NotesEditor(seq, { sampleRate: SR, yAxis: "hz" });
    const { host, wid } = await opened(editor);
    const roll = (host.trees[0]!.children as GuiNode[])[0] as unknown as {
        axes: { y: { unit: string } };
        notes: number[];
    };
    assert.equal(roll.axes.y.unit, "hz");
    assert.ok(Math.abs(roll.notes[2]! - 261.6256) < 1e-3, "middle C, in hertz");
    editor.apply("/gui_event", [wid, 1, 0, "notes", 1, 0.0, BEAT * 0.8, 300.0, 100, 0]);
    const moved = [...seq][0]![1];
    assert.ok(Math.abs(Number(moved.get("freq")) - 300.0) < 1e-3);
    assert.ok(Math.abs(Number(moved.get("midinote")) - 62.37) < 0.01, "the MIDI note follows the frequency");
});

test("the space bar plays and stops the roll, and its end is the transport's", async () => {
    const server = new PlayingServer();
    const seq = new EventSequence([
        [0.0, new Event({ midinote: 60, dur: 1.0 })],
        [2.0, new Event({ midinote: 64, dur: 1.0 })],
    ], { tempoMap: new TempoMap(TEMPO) });
    const editor = new NotesEditor(seq, { sampleRate: SR, server });
    const { host } = await opened(editor);
    const window = 900 + host.trees.length;
    editor.end = "contents";
    editor.apply("/gui_event", [window, 1, 0, "play", 0]);
    await editor.settled();
    assert.equal(server.sent.at(-1)![0], "/transport_play");
    // Two beats a second at 100 samples a second: the last note sounds 0.8 of
    // its beat (the default legato), so it ends on beat 2.8 -- sample 140.
    const ends = server.sent.filter(([addr]) => addr === "/transport_end");
    const args = ends.at(-1)![1] as [string, number | bigint][];
    assert.deepEqual(args.map(([, v]) => Number(v)), [3, 140, 0], "its transport, the end, the return");
    server.sent = [];
    editor.apply("/gui_event", [window, 2, 0, "play", 0]);
    await editor.settled();
    const addrs = server.sent.map(([addr]) => addr);
    assert.ok(addrs.includes("/transport_stop") && addrs.includes("/transport_locateSample"),
        "a stop, back to the position cursor");
});

test("the space bar plays the time range a sweep left", async () => {
    const server = new PlayingServer();
    const seq = new EventSequence([
        [0.0, new Event({ midinote: 60, dur: 1.0 })],
        [2.0, new Event({ midinote: 64, dur: 1.0 })],
    ], { tempoMap: new TempoMap(TEMPO) });
    const editor = new NotesEditor(seq, { sampleRate: SR, server });
    const { host, wid } = await opened(editor);
    const window = 900 + host.trees.length;
    // A sweep from beat 1 to beat 2, on the roll's axis.
    editor.apply("/gui_event", [wid, 1, 0, "selection", BEAT, BEAT]);
    editor.apply("/gui_event", [window, 2, 0, "play", 0]);
    await editor.settled();
    const sent = server.sent
        .filter(([addr]) => addr === "/transport_end" || addr === "/transport_locateSample")
        .map(([addr, args]) => [addr, (args as [string, number | bigint][]).slice(1).map(([, v]) => Number(v))]);
    // 100 samples a second, two beats a second: beat 1 is sample 50, beat 2 is 100.
    assert.ok(sent.some(([a, v]) => a === "/transport_end" && JSON.stringify(v) === "[100,0]"), JSON.stringify(sent));
    assert.deepEqual(sent.at(-1), ["/transport_locateSample", [50]], "from the range's start");
});
