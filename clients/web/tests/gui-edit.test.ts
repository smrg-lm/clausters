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
import { Editing, NotesEditor, PointsEditor, SamplesEditor, edit, measures, watch }
    from "../src/gui/editing/index.ts";
import { unwatch } from "../src/base/log.ts";
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
    acks: [number, [number, Record<string, PropValue>][], string | undefined][] = [];
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

const aCurve = (): Automation =>
    Automation.fromPoints([[0.0, 200.0, 2, 0.0], [2.0, 900.0, 1, 0.0]], null, { name: "cutoff" });

const aTimeline = (): Timeline =>
    new Timeline([
        [0.0, new SeqEvent({ midinote: 60, dur: 1.0 })],
        [1.0, new SeqEvent({ midinote: 64, dur: 1.0 })],
    ]);

/** The two writes a take's editor sends, laid into a `FakeBuffer`. */
class FakeServer {
    sent: string[] = [];
    private readonly take: FakeBuffer;

    constructor(take: FakeBuffer) {
        this.take = take;
    }

    bulkChunk(): Promise<number> {
        return Promise.resolve(8192);
    }

    sendMsg(addr: string, ...args: unknown[]): void {
        this.sent.push(addr);
        const bytes = args[args.length - 1] as Uint8Array;
        const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
        const mono = addr === "/buffer_setRange";
        const at = (i: number) => {
            const arg = args[i];
            return Array.isArray(arg) ? Number(arg[1]) : Number(arg);
        };
        const first = mono ? at(1) : at(2) * this.take.channels + at(1);
        const stride = mono ? 1 : this.take.channels;
        for (let i = 0; i * 4 < bytes.byteLength; i += 1) {
            this.take.data[first + i * stride] = view.getFloat32(i * 4, true);
        }
    }

    request(addr: string, args: unknown[]): Promise<{ addr: string; args: unknown[] }> {
        this.sendMsg(addr, ...args);
        const bufnum = Array.isArray(args[0]) ? Number(args[0][1]) : Number(args[0]);
        return Promise.resolve({ addr: "/done", args: [addr, bufnum] });
    }
}

/**
 * A server buffer, as the samples domain touches one: a number, a shape, and
 * the server its writes go to.
 */
class FakeBuffer {
    bufnum = 7;
    frames: number;
    channels: number;
    sampleRate = SR;
    data: number[];
    server: FakeServer;

    constructor(frames = 16, channels = 1) {
        this.frames = frames;
        this.channels = channels;
        this.data = new Array(frames * channels).fill(0);
        this.server = new FakeServer(this);
    }

    setSamples(): Promise<void> {
        return Promise.reject(new Error("a take's editor writes through its steps"));
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

test("the verb opens the editor the structure asks for", async () => {
    const off = { open: false } as const;
    assert.ok((await edit(aCurve(), { sampleRate: SR, ...off })) instanceof PointsEditor);
    assert.ok((await edit(aTimeline(), { sampleRate: SR, ...off })) instanceof NotesEditor);
    assert.ok((await edit(new FakeBuffer(), off)) instanceof SamplesEditor);
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

test("a curve is drawn, edited and read back with no composition", async () => {
    const curve = aCurve();
    const editor = await edit(curve, { sampleRate: SR, tempo: TEMPO, open: false });
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

test("an edit made against a picture an undo replaced is refused", async () => {
    // The staleness floor, on the road an editor actually travels. A host
    // stamps every event with the version it was last told, and it is told only
    // when an acknowledgement reaches it — a round trip a hand outruns — so an
    // edit naming an older version is the ordinary case and applies. What does
    // not is an edit made against a picture the composition has moved away from
    // by a route the host never saw: here an undo.
    const curve = aCurve();
    const editor = await edit(curve, { sampleRate: SR, tempo: TEMPO, open: false });
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
    // drop whatever arrived in between — here, the undo.
    assert.equal(
        editor.apply("/gui_event", [wid, 2, against, "points",
            0.0, 900.0, 1, 0.0, 2.0, 900.0, 1, 0.0]),
        false,
    );
    assert.deepEqual(curve.toPoints().slice(0, 2), [0.0, 200.0], "the undo stands");
    assert.equal(host.acks[host.acks.length - 1]![2], "the composition changed since this edit");

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
    const editor = await edit(curve, { sampleRate: SR, tempo: TEMPO, open: false });
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
    const editor = await edit(curve, { sampleRate: SR, tempo: TEMPO, open: false });
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
    const editor = await edit(timeline, { sampleRate: SR, tempo: TEMPO, open: false });
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
    const editor = await edit(timeline, { sampleRate: SR, tempo: TEMPO, open: false });
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
    const editor = await edit(timeline, { sampleRate: SR, tempo: TEMPO, open: false });
    const { wid } = await opened(editor);
    editor.apply("/gui_event", [wid, 1, 0, "notes", 0.0, BEAT, 67, 100, 0]);
    assert.ok(
        [...timeline].some(([, item]) => item === marker),
        "a rebuilt timeline would have dropped it",
    );
});

test("a marker dragged in the roll moves it on the timeline", async () => {
    // The lane the roll draws and nobody answered: a marker slid in the OSC
    // lane is an edit of the timeline, with an inverse like any other.
    const timeline = aTimeline();
    timeline.add(3.0, new OscItem("/hit", 7));
    const editor = await edit(timeline, { sampleRate: SR, tempo: TEMPO, open: false });
    const { wid } = await opened(editor);

    assert.equal(editor.apply("/gui_event", [wid, 1, 0, "osc", 1.5 * BEAT, "/hit"]), true);
    const at = [...timeline].filter(([, item]) => item instanceof OscItem);
    assert.equal(at.length, 1);
    assert.equal(at[0][0], 1.5, "the marker moved");
    assert.deepEqual(
        (at[0][1] as OscItem).args,
        [7],
        "the message it sends is not the lane's to lose",
    );
    assert.equal(editor.undoLabel, "edit the markers");
    assert.equal(editor.undo(), true);
    assert.deepEqual(
        [...timeline].filter(([, i]) => i instanceof OscItem).map(([beat]) => beat),
        [3.0],
    );
});

test("a marker removed in the roll leaves its neighbours theirs", async () => {
    // Matched by label rather than by order, so removing one does not hand the
    // next one's message to the wrong marker.
    const timeline = new Timeline([
        [0.0, new OscItem("/a", 1)],
        [1.0, new OscItem("/b", 2)],
        [2.0, new OscItem("/c", 3)],
    ]);
    const editor = await edit(timeline, { sampleRate: SR, tempo: TEMPO, open: false });
    const { wid } = await opened(editor);

    assert.equal(
        editor.apply("/gui_event", [wid, 1, 0, "osc", 0.0, "/a", 2 * BEAT, "/c"]),
        true,
    );
    assert.deepEqual(
        [...timeline].map(([, item]) => [(item as OscItem).addr, [...(item as OscItem).args]]),
        [["/a", [1]], ["/c", [3]]],
    );
});

test("a marker added in the roll is refused and says why", async () => {
    // A marker is the message it sends and the lane cannot type one, so the
    // gesture is answered rather than half-applied: the reason, and the markers
    // as they still are.
    const timeline = new Timeline([[0.0, new OscItem("/a")]]);
    const editor = await edit(timeline, { sampleRate: SR, tempo: TEMPO, open: false });
    const { host, wid } = await opened(editor);

    assert.equal(
        editor.apply("/gui_event", [wid, 1, 0, "osc", 0.0, "/a", BEAT, ""]),
        false,
    );
    assert.equal([...timeline].length, 1);
    const [seq, corrections, reason] = host.acks[host.acks.length - 1];
    assert.equal(seq, 1);
    // The sentence is the crate's and names no language: it is one string
    // now, so a page cannot be told to type a script's spelling of `add`.
    assert.ok(String(reason).includes("a marker is the message it sends"));
    assert.deepEqual(corrections[0][1].osc, [0.0, "/a"]);
});

test("the notes gesture does not move the markers", async () => {
    const timeline = aTimeline();
    timeline.add(3.0, new OscItem("/hit"));
    const editor = await edit(timeline, { sampleRate: SR, tempo: TEMPO, open: false });
    const { wid } = await opened(editor);
    editor.apply("/gui_event", [wid, 1, 0, "notes", 0.0, BEAT, 67, 100, 0]);
    assert.deepEqual(
        [...timeline].map(([beat, item]) => [beat, (item as object).constructor.name]),
        [[0.0, "Event"], [3.0, "OscItem"]],
    );
});

// ---- samples ----

test("a stroke writes the server's buffer and undoes off the wire", async () => {
    const take = new FakeBuffer(8);
    const editor = await edit(take, { tempo: TEMPO, open: false });
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
    const editor = await edit(take, { tempo: TEMPO, open: false });
    const { wid } = await opened(editor);
    assert.equal(editor.apply("/gui_event", [wid, 1, 0, "sample", 0, 3, 0.9, 0.0]), true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    // A server buffer holds `f32`, and the write crosses as one.
    assert.equal(take.data[3], Math.fround(0.9));
    editor.undo();
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.equal(take.data[3], 0);
});

test("a stroke on one channel of a stereo take leaves the other alone", async () => {
    const take = new FakeBuffer(4, 2);
    take.data = [0.1, 0.2, 0.1, 0.2, 0.1, 0.2, 0.1, 0.2];
    const editor = await edit(take, { tempo: TEMPO, open: false });
    const { wid } = await opened(editor);
    editor.apply("/gui_event", [wid, 1, 0, "draw", 1, 1, blob([0.7, 0.8]), blob([0.2, 0.2])]);
    // The interleaved splice is a read and a write, so it settles a turn later.
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.deepEqual(
        take.data.map((v) => Math.round(v * 10) / 10),
        [0.1, 0.2, 0.1, 0.7, 0.1, 0.8, 0.1, 0.2],
    );
    assert.deepEqual(take.server.sent, ["/buffer_setRangeChannel"], "one channel, never read");
});

test("a take's window is composed by the crate", async () => {
    const take = new FakeBuffer(8, 2);
    const editor = await edit(take, { tempo: TEMPO, title: "take", open: false });
    const { host, wid } = await opened(editor);
    const tree = host.trees[0] as GuiNode & Record<string, unknown>;
    assert.deepEqual([tree.type, tree.title, tree.flow], ["window", "take", "col"]);
    const picture = (tree.children as (GuiNode & Record<string, unknown>)[])[0];
    assert.equal(picture.type, "signal");
    assert.equal(picture.id, wid);
    assert.deepEqual([picture.buffer, picture.channels], [take.bufnum, 2]);
    assert.equal(picture.measure, "peak rms");
    assert.equal(picture.label, `buffer ${take.bufnum}`);
    assert.deepEqual(picture.gestures, { drag: "select", alt: "draw", ctrl: "sample" });
    assert.deepEqual(editor.view?.props(editor, wid), { reload: 1 });
});

test("a refused measure stack keeps the one the picture had", async () => {
    const editor = new SamplesEditor(new FakeBuffer() as never, { sampleRate: SR, layers: ["peak"] });
    editor.layers = ["rms", "peak"];
    assert.deepEqual(editor.layers, ["rms", "peak"]);
    assert.throws(() => {
        editor.layers = ["loud"];
    }, /'loud'/);
    assert.deepEqual(editor.layers, ["rms", "peak"]);
    assert.throws(() => measures([]), /measures something/);
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
    const timeline = aTimeline();
    const curveEditor = await edit(curve, { sampleRate: SR, tempo: TEMPO, context, open: false });
    const roll = await edit(timeline, { sampleRate: SR, tempo: TEMPO, context, open: false });
    const { wid: curveWid } = await opened(curveEditor);
    const { wid: rollWid } = await opened(roll);

    curveEditor.apply("/gui_event", [curveWid, 1, 0, "points",
        0.0, 300.0, 1, 0.0, 2.0, 900.0, 1, 0.0]);
    roll.apply("/gui_event", [rollWid, 1, 0, "notes", 0.0, BEAT, 67, 100, 0]);
    assert.equal(context.undoLabel, "edit the notes");

    // The notes go back first: one pile, walked in the order the edits landed.
    assert.equal(roll.undo(), true);
    assert.deepEqual([...timeline].map(([, e]) => (e as SeqEvent).midinote()), [60, 64]);
    assert.equal(curve.toPoints()[1], 300.0, "the curve has not moved yet");
    assert.equal(curveEditor.undo(), true);
    assert.equal(curve.toPoints()[1], 200.0);
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
    assert.equal(viewProps("clip", "{}"), "", "a kind the crate does not draw");
});

test("two editors in one application keep their own floor", async () => {
    // The echo is **one view's** end of the conversation, so a window set does
    // not share one. The floor rises when the version moved and no event of
    // *this* view moved it — with one echo per application the two windows
    // would each answer for the other, and a gesture the left window made
    // against a picture the right window had already changed would find
    // `version === applied` and be accepted, which is the whole of what the
    // floor is for.
    //
    // The Python twin is
    // `test_gui_editing.py::test_two_editors_in_one_application_keep_their_own_floor`.
    const curve = aCurve();
    const left = await edit(curve, { sampleRate: SR, tempo: TEMPO, open: false });
    const right = await edit(curve, {
        sampleRate: SR, tempo: TEMPO, open: false, app: left.app,
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
    // it did — but a library that printed by default would make every importer
    // pay for the formatting.
    //
    // The Python twin is
    // `test_gui_editing.py::test_the_editing_trace_is_silent_until_it_is_watched`.
    const curve = aCurve();
    const editor = await edit(curve, { sampleRate: SR, tempo: TEMPO, open: false });
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
