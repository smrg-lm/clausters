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
import { existsSync } from "node:fs";
import { TempoMap } from "../src/base/time.ts";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import {
    AudioEditor, Domain, Editing, Editor, Marking, NotesEditor, PointsEditor,
    View, edit, watch,
} from "../src/gui/editing/index.ts";
import { unwatch } from "../src/base/log.ts";
import { POINTS } from "../src/document.ts";
import { Bpf, Env } from "../src/defs/ugens/index.ts";
import { Automation } from "../src/multitrack.ts";
import { Event } from "../src/seq/event.ts";
import { OscItem, Timeline } from "../src/seq/timeline.ts";
import { EventSequence } from "../src/seq/sequence.ts";
import { Server } from "../src/defs/server/index.ts";
import { OscNrtInterface } from "../src/base/connection.ts";
import type { GuiHost, PropValue } from "../src/gui/host.ts";
import type { GuiNode } from "../src/gui/guidef.ts";
import { button } from "../src/gui/guidef.ts";

/**
 * The view a window edits: the node its `main` names, which stands under the
 * toolbar of a window with chrome -- or, in one with none, its first.
 */
function picture(tree: unknown): Record<string, any> {
    const root = tree as { main?: number; children?: unknown[] };
    const find = (node: any): any =>
        node.id === root.main ? node : (node.children ?? []).map(find).find((f: unknown) => f);
    return (root.main !== undefined && find(root)) || (root.children as any[])[0];
}


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
    close(): void {}
    onMessage(): () => void {
        return () => {};
    }
    /** What `/gui_verb` was asked for: `[window, verb]`. */
    verbs: [number, string][] = [];
    verb(id: number | { id: number }, verb: string): void {
        this.verbs.push([typeof id === "number" ? id : id.id, verb]);
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
    return { host, wid: picture(tree).id as number };
}

const blob = (values: number[]): Uint8Array => new Uint8Array(Float32Array.from(values).buffer);

// ---- the verb ----

test("the verb opens the editor the structure asks for", async () => {
    const off = { open: false } as const;
    assert.ok((await edit(aCurve(), { sampleRate: SR, ...off })) instanceof PointsEditor);
    assert.ok((await edit(aTimeline(), { sampleRate: SR, ...off })) instanceof NotesEditor);
});

test("something none of the three reads says what they are", async () => {
    await assert.rejects(() => edit({}), /edit` opens a SoundFile/);
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

test("marking is taken by the editors that act on what is marked", async () => {
    // `selected`, `select` and `unselect` are a capability an editor takes: the
    // roll and the audio editor have it, and the points editor, where no
    // operation acts on a group of points, has none of it -- nor a time range
    // of its own.
    for (const word of ["selected", "select", "unselect"] as const) {
        assert.equal(NotesEditor.prototype[word], Marking.prototype[word]);
    }
    // the audio editor is the host's application, and its handler marks
    // through the application's transport
    for (const word of ["selected", "select", "unselect"] as const) {
        assert.equal(typeof AudioEditor.prototype[word], "function");
    }
    const editor = await edit(aCurve(), { sampleRate: SR, open: false });
    assert.ok(editor instanceof PointsEditor);
    for (const word of ["selected", "select", "unselect", "span"]) {
        assert.equal(word in editor, false);
    }
});

test("a normalized envelope is kept inside its ranges", async () => {
    // Time and value both from 0 to 1: a point a hand reports outside is kept
    // inside, and the curve the page holds is written that way.
    const env = new Env([0.0, 1.0, 0.0], [0.5, 0.5]);
    const editor = (await edit(env, {
        sampleRate: SR, open: false, min: 0.0, max: 1.0, start: 0.0, end: 1.0,
    })) as unknown as PointsEditor;
    const { wid } = await opened(editor);
    assert.deepEqual(editor.rules, { values: [0, 1], time: [0, 1] });
    assert.equal(
        editor.apply("/gui_event", [wid, 1, 0, "points",
            0.0, -0.2, 1, 0.0, 0.4, 1.5, 1, 0.0, 1.6, 0.0, 1, 0.0]),
        true,
    );
    assert.deepEqual(env.toPoints(), [0.0, 0.0, 1, 0.0, 0.4, 1.0, 1, 0.0, 1.0, 0.0, 1, 0.0]);
});

test("an automation is kept in the range of what it automates", async () => {
    // With nothing declared, a curve over a parameter is kept in that
    // parameter's range -- the one the roll and the multitrack draw it over.
    const cutoff = new Automation({ target: { cc: 74 }, points: [[0.0, 10.0], [1.0, 100.0]] });
    const editor = (await edit(cutoff, { sampleRate: SR, open: false })) as unknown as PointsEditor;
    const { wid } = await opened(editor);
    assert.deepEqual(editor.rules, { values: [0, 127], time: null });
    editor.apply("/gui_event", [wid, 1, 0, "points", 0.0, 10.0, 1, 0.0, 1.0, 200.0, 1, 0.0]);
    assert.equal(cutoff.toPoints()[5], 127);
});

test("a selected segment takes its shape from the chooser under the curve", async () => {
    // A click on a segment selects it; the chooser in the row under the curve sets
    // its shape, as an edit the curve's history takes back. The page's own
    // widgets go in that row, beside the chooser.
    const curve = aCurve();
    const editor = await edit(curve, {
        sampleRate: SR, open: false, extra: [button({ name: "play" })],
    });
    const host = new FakeHost();
    await editor.open(asHost(host));
    const tree = host.trees[0] as GuiNode;
    const wid = (tree.children as GuiNode[])[0]?.id as number;
    const column = ((tree.children as GuiNode[])[1]!.children as GuiNode[])[0]!;
    const shape = (column.children as GuiNode[])[0]?.id as number;
    assert.equal((column.children as GuiNode[])[1]?.name, "play");
    assert.equal(editor.apply("/gui_event", [wid, 1, 0, "segment", 0]), false);
    assert.equal(editor.apply("/gui_event", [shape, 2, 0, 3]), true);
    assert.equal(curve.toPoints()[2], 3, "the segment is sine");
    assert.equal(editor.undo(), true);
    assert.equal(curve.toPoints()[2], 2, "and exponential again");
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












// ---- the acceptance the track was opened with ----

test("edit over a structure with its editor open hands that one back", async () => {
    // A second window of one role over one structure would ask for the same
    // widgets as the first: `edit` hands back the editor that is open instead,
    // and opens nothing. Another role is another window -- a roll in hertz
    // beside one in MIDI notes -- and a closed editor is not handed back.
    const fake = new FakeHost();
    const host = asHost(fake);
    const curve = aCurve();
    const first = await edit(curve, { sampleRate: SR, host });
    assert.equal(await edit(curve, { sampleRate: SR, host, title: "again" }), first);
    assert.equal(fake.trees.length, 1, "nothing else was opened");
    // another curve is another window
    assert.notEqual(await edit(aCurve(), { sampleRate: SR, host }), first);
    first.close();
    assert.notEqual(await edit(curve, { sampleRate: SR, host }), first);
});

test("two editors over one structure share one stack", async () => {
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


// ---- a page's change is a turn ----

test("edit takes the ambient server's rate", async () => {
    // With no sampleRate, the roll is laid out at the rate of the server a play
    // would resolve -- and at 48 kHz with none.
    const { main } = await import("../src/base/main.ts");
    const held = main.server;
    try {
        main.server = null;
        assert.equal((await edit(aCurve(), { open: false })).sampleRate, 48_000, "no server anywhere");
        main.server = { queryInfo: async () => ({ nominalSampleRate: 96_000 }) } as never;
        assert.equal((await edit(aCurve(), { open: false })).sampleRate, 96_000);
        assert.equal((await edit(aCurve(), { sampleRate: 44_100, open: false })).sampleRate, 44_100, "a rate given wins");
    } finally {
        main.server = held;
    }
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



/** A number somebody edits. The whole structure. */
class Dial {
    value = 0.0;
}

/**
 * `Dial`'s vocabulary: one verb, and the state it replaces -- the twin of the
 * Python suite's, for the one test that reads the generic editor's path, which
 * no application in this package takes any more.
 */
class DialDomain extends Domain<Dial> {
    override readonly name = POINTS;

    override payload(_structure: Dial, tag: string, values: readonly unknown[]): unknown {
        if (tag !== "dial") return null;
        return { intent: "setpoints", points: [{ at: 0.0, value: Number(values[0]) }] };
    }

    current(structure: Dial, _payload: unknown): unknown {
        return { intent: "setpoints", points: [{ at: 0.0, value: structure.value }] };
    }

    project(structure: Dial, payload: unknown): boolean {
        const value = Number((payload as { points: { value: number }[] }).points[0]!.value);
        if (value === structure.value) return false;
        structure.value = value;
        return true;
    }
}

/** One widget drawing one number. */
class DialView extends View<Dial> {
    build(editor: Editor<Dial>): GuiNode {
        const wid = this.widget(editor, "dial", editor.structure);
        return { type: "window", children: [{ id: wid, type: "number", value: editor.structure.value }] };
    }

    override props(editor: Editor<Dial>): Record<string, PropValue> {
        return { value: editor.structure.value };
    }
}

test("the editing trace is silent until it is watched", async () => {
    // The five joints, and the fact that they cost nothing unarmed. A window in
    // front of a person fails in ways nothing else sees, so the path says what
    // it did -- but a library that printed by default would make every importer
    // pay for the formatting.
    //
    // The Python twin is
    // `test_gui_editing.py::test_the_editing_trace_is_silent_until_it_is_watched`.
    const editor = new Editor(new Dial(), {
        sampleRate: SR, domain: new DialDomain(), view: new DialView(),
    });
    const host = new FakeHost();
    await editor.open(asHost(host));
    const wid = ((host.trees[0] as GuiNode).children as GuiNode[])[0]?.id as number;

    const quiet: string[] = [];
    assert.equal(editor.apply("/gui_event", [wid, 1, 0, "dial", 0.25]), true);
    assert.equal(quiet.length, 0, "silent unless asked");

    const said: string[] = [];
    watch({ debug: (line) => said.push(line), warn: (line) => said.push(line) });
    try {
        assert.equal(editor.apply("/gui_event", [wid, 2, 0, "dial", 0.75]), true);
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


/** The transport a recorded `/transport_*` command names: its first argument. */
function transportOf(args: unknown[]): number {
    const first = args[0];
    return Number(Array.isArray(first) ? first[1] : first);
}

test("two sequences play together and a free gives the transport back", async () => {
    // Each `play(sequence)` takes a transport, so two sound at once; with none
    // left the play fails saying so, and a `free` gives one back.
    const server = new PlayingServer();
    const seqs = Array.from({ length: 16 }, (_, i) =>
        new EventSequence([[0.0, new Event({ midinote: 60 + i, dur: 1.0 })]], { tempoMap: new TempoMap(TEMPO) }));
    const transports = [];
    for (const seq of seqs.slice(0, 15)) transports.push(await seq.play({ server: server as never }));
    const ids = transports.map((t) => t.id);
    assert.equal(new Set(ids).size, 15);
    assert.equal(Math.min(...ids), 1, "sixteen transports, above the one addressed by number");
    await assert.rejects(seqs[15]!.play({ server: server as never }), /--transports/);

    server.sent.length = 0;
    const freed = await transports[0]!.free();
    const addrs = server.sent.map(([addr]) => addr);
    assert.ok(addrs.includes("/lane_free") && addrs.includes("/node_free"));
    assert.equal(server.ids.inUse("transports"), 14);
    assert.ok(freed.span === null && freed.driver === null, "nothing loaded on it now");
    assert.equal((await seqs[15]!.play({ server: server as never })).id, freed.id, "the one given back");
});

test("a page takes a transport of its own and gives it back", async () => {
    // `transportNew` takes a free transport, never one that something played
    // holds, and `free` gives it back.
    const server = new PlayingServer();
    const seq = new EventSequence([[0.0, new Event({ midinote: 60, dur: 1.0 })]], { tempoMap: new TempoMap(TEMPO) });
    const played = await seq.play({ server: server as never });
    const own = server.transportNew();
    assert.ok(own.id !== 0 && own.id !== played.id && own === server.transportAt(own.id));
    assert.equal(server.ids.inUse("transports"), 2);
    await own.free();
    assert.equal(server.ids.inUse("transports"), 1);
    await own.free(); // twice is nothing
    assert.equal((await server.transportAt(0).free()).id, 0, "nothing to free by number");
    assert.equal(server.ids.inUse("transports"), 1);
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








// ---- the score editor plays ----

const scoreEngraver = new URL("../vendor/verovio/verovio.js", import.meta.url);


/**
 * A gesture of two edits: the dial to the first value, then the second. A
 * second value of `null` is an edit this domain cannot invert, and one of
 * `"boom"` is one it fails to write.
 */
class TwoTurnDomain extends DialDomain {
    override payloads(_structure: Dial, tag: string, values: readonly unknown[]): unknown[] {
        if (tag !== "two") return [];
        return values.map((value) => ({ intent: "setpoints", points: [{ at: 0.0, value }] }));
    }

    override current(structure: Dial, payload: unknown): unknown {
        if ((payload as { points: { value: unknown }[] }).points[0]!.value === null) return null;
        return super.current(structure, payload);
    }

    override project(structure: Dial, payload: unknown): boolean {
        if ((payload as { points: { value: unknown }[] }).points[0]!.value === "boom") {
            throw new Error("the dial would not turn");
        }
        return super.project(structure, payload);
    }
}


test("a window verb is asked of the host by name or by member", async () => {
    // The Python twin is `test_gui_editing.py::
    // test_a_window_verb_is_asked_of_the_host_by_name_or_by_member`.
    const editor = new Editor(new Dial(), {
        sampleRate: SR, domain: new DialDomain(), view: new DialView(),
    });
    assert.equal(editor.verb("split"), false, "no window, nothing to act on");
    const host = new FakeHost();
    await editor.open(asHost(host));
    const window = 900 + host.trees.length;
    assert.equal(editor.verb("view_all"), true);
    assert.equal(editor.selectAll(), true);
    assert.equal(editor.paste(), true);
    assert.deepEqual(host.verbs, [[window, "view_all"], [window, "select_all"], [window, "paste"]]);
});

test("a curve window has its menu and says what it does not do", async () => {
    // A curve's window carries the bar that holds the window's verbs and the
    // history, and its readings refuse the rest with why; `chrome: false`
    // draws the curve alone. It reads them from the moment it opens.
    const host = new FakeHost();
    const editor = (await edit(aCurve(), { sampleRate: SR, host: host as unknown as GuiHost })) as unknown as PointsEditor;
    const tree = host.trees[host.trees.length - 1] as unknown as { menu: { label: string }[] };
    assert.deepEqual(tree.menu.map((title) => title.label), ["File", "Edit", "View", "Help"]);
    const read = host.props.get(editor.id!)!.actions as unknown as Record<string, { enabled: boolean }>;
    assert.equal(read.undo.enabled, false);
    const actions = editor.actions as Record<string, { reason?: string }>;
    assert.ok(actions.paste.reason?.includes("clipboard"));
    assert.ok(actions.play.reason?.includes("does not play"));
    const bare = new PointsEditor(aCurve(), { sampleRate: SR, chrome: false });
    assert.equal((bare.draw() as unknown as { menu?: unknown }).menu, undefined);
});
