// `AudioEditor`: a take edited as a list of parts over takes it never writes.
//
// What is checked is this page's half -- the steps the crate answers are walked
// against the take's server, the buffers a turn needs are handed over before
// it, and the takes the history lets go of are freed. What each gesture does to
// the list is the crate's and is tested there.
//
// The Python client's twin is `tests/test_gui_audio.py`, case for case.
//
// Run with `npm test`; this suite needs the core staged (`./build.sh`).

import assert from "node:assert/strict";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { IdSpaces } from "../src/core/clausters_core_web.js";
import { AudioEditor, Editing, edit, measures } from "../src/gui/editing/index.ts";
import type { GuiHost, PropValue } from "../src/gui/host.ts";
import type { GuiNode } from "../src/gui/guidef.ts";
import { Transport } from "../src/defs/server/transport.ts";
import { GuiIdAllocator } from "../src/gui/ids.ts";
import { SoundFile } from "../src/soundfile.ts";

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

/** What the host is told, so an answer can be read. */
class FakeHost {
    acks: [number, [number, Record<string, PropValue>][], string | undefined][] = [];
    trees: GuiNode[] = [];
    clock = "device";
    clockOf = -1;
    readonly ids = new GuiIdAllocator(20_000);

    allocId(): number {
        return this.ids.alloc();
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
    clip = "";
    clipboard(document: string): void { this.clip = document; }
    close(): void {}
    headClock(id: number | { readonly id: number }, name: string, _transport = 0): void {
        this.clock = name;
        this.clockOf = typeof id === "number" ? id : id.id;
    }
    onMessage(): () => void {
        return () => {};
    }
    ack(seq: number): void {
        this.acks.push([seq, [], undefined]);
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

/** Buffer numbers, handed out from 100 and taken back. */
class FakeAllocator {
    private next = 100;
    freed: number[] = [];

    alloc(): number {
        this.next += 1;
        return this.next;
    }
    free(bufnum: number): void {
        this.freed.push(bufnum);
    }
}

type Sent = [string, (number | string)[]];

/**
 * Every message a take's steps send, in order, each answered the way the server
 * answers it.
 */
class FakeServer {
    sent: Sent[] = [];
    buffers = new FakeAllocator();
    ids = new IdSpaces(8192, 1024, 2, 16384, 4096, 16, 64, 0, 1);
    /** Whether the transport rolls, as a `/transport_query` answers. */
    playing = false;

    bulkChunk(): Promise<number> {
        return Promise.resolve(8192);
    }
    notify(): Promise<void> {
        return Promise.resolve();
    }
    queryInfo(): Promise<{ nominalSampleRate: number }> {
        return Promise.resolve({ nominalSampleRate: SR });
    }
    transportAt(): { state: () => Promise<{ playing: boolean }> } {
        return { state: () => Promise.resolve({ playing: this.playing }) };
    }
    sendMsg(addr: string, ...args: unknown[]): void {
        this.sent.push([
            addr,
            args.map((a) => (Array.isArray(a) ? (a[1] as number | string) : typeof a === "string" ? a : Number.NaN)),
        ]);
    }
    request(addr: string, args: unknown[]): Promise<{ addr: string; args: unknown[] }> {
        this.sendMsg(addr, ...args);
        const first = Array.isArray(args[0]) ? Number(args[0][1]) : Number(args[0]);
        if (addr === "/server_sync") return Promise.resolve({ addr: "/server_sync.reply", args: [first] });
        return Promise.resolve({ addr: "/done", args: [addr, first] });
    }
    addrs(): string[] {
        return this.sent.map(([addr]) => addr);
    }
}

/** The take: a number, a shape, and the server it is on. */
class FakeBuffer {
    bufnum = 7;
    frames: number;
    channels: number;
    sampleRate = SR;
    path: string | undefined = undefined;
    server = new FakeServer();

    constructor(frames = 100, channels = 1) {
        this.frames = frames;
        this.channels = channels;
    }
    setSamples(): Promise<void> {
        return Promise.reject(new Error("an audio editor never writes the take"));
    }
}

/** Every step in flight, carried out. */
const settle = async (): Promise<void> => {
    for (let i = 0; i < 20; i++) await new Promise((resolve) => setTimeout(resolve, 0));
};

async function opened(
    take: FakeBuffer,
    options: {
        historyBytes?: number; residentBytes?: number; scratch?: string; file?: SoundFile;
    } = {},
): Promise<[AudioEditor, FakeHost, number, Editing]> {
    const context = new Editing();
    const editor = new AudioEditor(take as never, { sampleRate: SR, context, ...options });
    const host = new FakeHost();
    await editor.open(host as unknown as GuiHost);
    await settle();
    return [editor, host, Number(picture(host.trees[0]).id), context];
}

const spans = (editor: AudioEditor): [number, number][] =>
    editor.parts.map((p) => {
        const source = p.source as { source: number; range: { start: number } };
        return [source.source, source.range.start];
    });

test("the window draws a join over a private copy of the take", async () => {
    const take = new FakeBuffer();
    const [editor] = await opened(take);
    const display = editor.buffer.bufnum;
    const copy = take.server.sent.find(([addr]) => addr === "/buffer_gen")![1];
    assert.deepEqual([copy[1], copy[3]], ["copy", take.bufnum], "copied out of the take");
    const stitched = take.server.sent.filter(([addr]) => addr === "/buffer_stitch");
    assert.equal(stitched[0]![1][0], display);
    assert.equal(stitched[0]![1][3], copy[0], "it reads the copy");
    assert.equal(editor.buffer.frames, 100);
    assert.ok(!take.server.addrs().includes("/buffer_setRange"), "the take is never written");
});

test("the window opens with both cursors on the transport clock", async () => {
    const [editor, host] = await opened(new FakeBuffer());
    const take = host.trees[0]!.children![1]!.children![1]!.children![0]! as unknown as {
        axes: { x: Record<string, unknown> };
    };
    assert.equal(take.axes.x.cursor, 0, "the position cursor, placed");
    assert.equal(take.axes.x.playhead_at, 0, "the play cursor, anchored");
    assert.equal(host.clock, "transport", "drawn from the transport's position");
    assert.equal(host.clockOf, editor.window!.id, "the editor's own window");
});

test("a stroke writes a new take and the join reads it", async () => {
    const take = new FakeBuffer();
    const [editor, , wid] = await opened(take);
    take.server.sent = [];
    assert.equal(editor.apply("/gui_event", [wid, 1, 0, "draw", 0, 40, [0.5, -0.5], [0.0, 0.0]]), true);
    await settle();
    assert.deepEqual(take.server.addrs(), [
        "/buffer_alloc",
        "/server_sync",
        "/buffer_setRange",
        "/buffer_stitch",
    ]);
    const fresh = take.server.sent[0]![1][0]!;
    const copy = spans(editor)[0]![0];
    assert.deepEqual(spans(editor), [[copy, 0], [fresh, 0], [copy, 42]]);
});

test("a take the history cannot reach is freed", async () => {
    const take = new FakeBuffer();
    const [editor, , wid, context] = await opened(take);
    editor.apply("/gui_event", [wid, 1, 0, "draw", 0, 10, [0.5], [0.0]]);
    await settle();
    const first = spans(editor)[1]![0];
    assert.equal(editor.undo(), true);
    await settle();
    assert.ok(!take.server.buffers.freed.includes(first), "a redo can still find it");
    editor.apply("/gui_event", [wid, 2, context.version, "draw", 0, 30, [0.5], [0.0]]);
    await settle();
    assert.ok(take.server.buffers.freed.includes(first));
    assert.ok(take.server.sent.some(([addr, args]) => addr === "/buffer_free" && args[0] === first));
});

test("the history is held to its byte limit", async () => {
    const take = new FakeBuffer();
    const [editor, , wid, context] = await opened(take, { historyBytes: 4 });
    for (const seq of [1, 2, 3]) {
        editor.apply("/gui_event", [wid, seq, context.version, "draw", 0, 10, [0.5], [0.0]]);
        await settle();
    }
    assert.equal(take.server.buffers.freed.length, 1, "the oldest take only the history held");
    assert.equal(editor.canUndo, true);
});

test("a cut copies before removing samples", async () => {
    const take = new FakeBuffer();
    const [editor, , wid] = await opened(take);
    take.server.sent = [];
    editor.apply("/gui_event", [wid, 1, 0, "cut", 10.0, 20.0]);
    await settle();
    assert.deepEqual(
        take.server.addrs().slice(0, 4),
        ["/buffer_alloc", "/server_sync", "/buffer_gen", "/buffer_stitch"],
        "the join stitched, the readers' window set to its new length, and the transport cued to where the span was",
    );
    assert.equal(editor.buffer.frames, 80);
    assert.equal(editor.undo(), true);
    await settle();
    assert.equal(editor.buffer.frames, 100);
});

test("a take past the resident budget goes to disk and comes back", async () => {
    const take = new FakeBuffer();
    const [editor, , wid, context] = await opened(take, { residentBytes: 0, scratch: "/scratch" });
    take.server.sent = [];
    for (const seq of [1, 2]) {
        editor.apply("/gui_event", [wid, seq, context.version, "draw", 0, 10, [0.5], [0.0]]);
        await settle();
    }
    const first = take.server.sent[take.server.addrs().indexOf("/buffer_alloc")]![1][0];
    assert.ok(
        take.server.sent.some(
            ([addr, args]) => addr === "/buffer_write" && args[0] === first && String(args[1]).startsWith("/scratch/take-") && String(args[1]).endsWith(`-${first}.wav`),
        ),
    );
    take.server.sent = [];
    assert.equal(editor.undo(), true);
    await settle();
    assert.deepEqual(take.server.addrs().slice(0, 2), ["/buffer_allocRead", "/buffer_stitch"]);
});

test("a save writes the edited take over its file or as another", async () => {
    const take = new FakeBuffer();
    take.path = "/takes/a.wav";
    const [editor] = await opened(take);
    take.server.sent = [];
    assert.equal(await editor.save(), "/takes/a.wav");
    await settle();
    const write = take.server.sent.find(([addr]) => addr === "/buffer_write")![1];
    const snapshot = write[0];
    assert.notEqual(snapshot, editor.buffer.bufnum);
    assert.deepEqual(write.slice(1), ["/takes/a.wav", "wav", "float"]);
    assert.ok(take.server.sent.some(([addr, args]) => addr === "/buffer_gen" && args[0] === snapshot && args[3] === editor.buffer.bufnum));
    assert.ok(take.server.sent.some(([addr, args]) => addr === "/buffer_free" && args[0] === snapshot));
    assert.equal(await editor.save({ path: "/takes/b.wav", sampleFormat: "int24" }), "/takes/b.wav");
    assert.equal(await editor.save(), "/takes/b.wav");
});

test("a save over a buffer rewrites it or writes a new one", async () => {
    const take = new FakeBuffer();
    const [editor, , wid] = await opened(take);
    editor.apply("/gui_event", [wid, 1, 0, "cut", 10.0, 20.0]);
    await settle();
    take.server.sent = [];
    const saved = (await editor.save()) as { bufnum: number; frames: number };
    assert.deepEqual([saved.bufnum, saved.frames], [take.bufnum, 80]);
    const copies = take.server.sent.filter(([addr]) => addr === "/buffer_gen").map(([, args]) => args);
    assert.equal(copies[0]![3], editor.buffer.bufnum);
    assert.equal(copies.at(-1)![0], take.bufnum);
    assert.equal(copies.at(-1)![3], copies[0]![0]);
    const fresh = (await editor.save({ buffer: true })) as { bufnum: number };
    assert.ok(![take.bufnum, editor.buffer.bufnum].includes(fresh.bufnum));
    assert.equal(((await editor.save()) as { bufnum: number }).bufnum, fresh.bufnum, "a later save writes there");
});

test("edit opens a buffer in the audio editor", async () => {
    const editor = await edit(new FakeBuffer() as never, { open: false, context: new Editing() });
    assert.ok(editor instanceof AudioEditor);
});

test("a take's window is composed by the crate", async () => {
    const take = new FakeBuffer(8, 2);
    const context = new Editing();
    const editor = new AudioEditor(take as never, { sampleRate: SR, title: "take", context });
    const host = new FakeHost();
    await editor.open(host as unknown as GuiHost);
    const tree = host.trees[0]!;
    assert.deepEqual([tree.type, tree.title], ["window", "take"]);
    assert.equal(tree.children![1]!.flow, "row", "the take, and its level beside it");
    const taken = picture(tree);
    const wid = Number(taken.id);
    assert.equal(taken.type, "signal");
    assert.deepEqual([taken.buffer, taken.channels], [editor.buffer.bufnum, 2]);
    assert.equal(taken.measure, "peak rms");
    assert.deepEqual(taken.gestures, { drag: "select", alt: "draw", ctrl: "sample" });
    assert.deepEqual(editor.view!.props(editor as never, wid), { reload: 1 });
});

test("a refused measure stack keeps the one the picture had", () => {
    const editor = new AudioEditor(new FakeBuffer() as never, {
        sampleRate: SR, layers: ["peak"], context: new Editing(),
    });
    editor.layers = ["rms", "peak"];
    assert.deepEqual(editor.layers, ["rms", "peak"]);
    assert.throws(() => (editor.layers = ["loud"]), /'loud'/);
    assert.deepEqual(editor.layers, ["rms", "peak"]);
    assert.throws(() => measures([]), /measures something/);
});

test("the take sounds through the editor's own nodes", async () => {
    const take = new FakeBuffer(100, 2);
    const [, host] = await opened(take);
    const sent = take.server.addrs();
    for (const addr of ["/transport_follow", "/transport_group", "/transport_fade"]) {
        assert.ok(sent.includes(addr), addr);
    }
    const readers = take.server.sent.filter(([addr]) => addr === "/graph_addSlot");
    assert.equal(readers.length, 2, "a reader per channel");
    assert.equal(host.clock, "transport");
    const window = host.trees[0] as GuiNode & { plays?: boolean };
    assert.equal(window.plays, true, "the space bar is the editor's");
    const meter = (((window.children ?? [])[1]!.children ?? [])[1]!.children ?? []).find(
        (c) => c.type === "meter",
    ) as Record<string, unknown>;
    assert.equal(meter.type, "meter");
    assert.equal(meter.rate, "control");
    assert.equal(meter.channels, 2);
});

test("the space bar plays and a second press stops back at the cursor", async () => {
    const take = new FakeBuffer();
    const [editor, host, wid] = await opened(take);
    const window = host.trees.length + 900;
    take.server.sent = [];
    editor.apply("/gui_event", [wid, 1, 0, "locate", 40]);
    editor.apply("/gui_event", [window, 2, 0, "play", 0]);
    await settle();
    const end = take.server.sent.filter(([addr]) => addr === "/transport_end");
    assert.deepEqual(end[end.length - 1][1].map(Number), [1, 100, 40]);
    assert.equal(take.server.addrs()[take.server.addrs().length - 1], "/transport_play");
    take.server.sent = [];
    take.server.playing = true;
    editor.apply("/gui_event", [window, 3, 0, "play", 0]);
    await settle();
    assert.deepEqual(take.server.addrs().slice(0, 2), ["/transport_stop", "/transport_locateSample"]);
});

test("closing the window frees the editor's nodes", async () => {
    const take = new FakeBuffer();
    const [editor] = await opened(take);
    take.server.sent = [];
    editor.close();
    await settle();
    assert.ok(take.server.addrs().includes("/node_free"));
});

test("placing the cursor cues a stopped transport there", async () => {
    const take = new FakeBuffer();
    const [editor, , wid] = await opened(take);
    take.server.sent = [];
    editor.apply("/gui_event", [wid, 1, 0, "locate", 40]);
    await settle();
    const located = take.server.sent.filter(([addr]) => addr === "/transport_locateSample");
    assert.deepEqual(located[located.length - 1][1].map(Number), [1, 40]);
    take.server.sent = [];
    take.server.playing = true;
    editor.apply("/gui_event", [wid, 2, 0, "locate", 60]);
    await settle();
    assert.ok(!take.server.addrs().includes("/transport_locateSample"));
});

test("a loop follows the selection redrawn while it plays", async () => {
    const take = new FakeBuffer();
    const [editor, host, wid] = await opened(take);
    const window = host.trees.length + 900;
    const server = take.server;
    const sent = (addr: string): number[][] =>
        server.sent.filter(([at]) => at === addr).map(([, args]) => args.map(Number));

    editor.apply("/gui_event", [wid, 1, 0, "selection", 10, 20]);
    editor.apply("/gui_event", [window, 2, 0, "play", 1]);
    await settle();
    server.playing = true;
    server.sent = [];
    editor.apply("/gui_event", [wid, 3, 0, "selection", 40, 0]);
    editor.apply("/gui_event", [wid, 4, 0, "selection", 40, 30]);
    await settle();
    assert.deepEqual(sent("/transport_loop").at(-1), [1, 40, 70], "the span follows the hand");
    assert.equal(sent("/transport_locateSample").length, 0, "and the head does not");
    editor.apply("/gui_event", [wid, 5, 0, "locate", 40]);
    await settle();
    assert.deepEqual(sent("/transport_locateSample").at(-1), [1, 40], "let go: into the span");

    // Not looping, the pass keeps the end it was played with.
    editor.apply("/gui_event", [window, 6, 0, "loop", 0]);
    await settle();
    server.sent = [];
    editor.apply("/gui_event", [wid, 7, 0, "selection", 5, 10]);
    editor.apply("/gui_event", [wid, 8, 0, "locate", 5]);
    await settle();
    assert.deepEqual(server.addrs().filter((addr) => addr.startsWith("/transport_")), []);
});

test("the take's transport plays its span and keeps the switch", async () => {
    const take = new FakeBuffer();
    const server = take.server;
    const held = new Map<number, Transport>();
    (server as unknown as { transportAt(n: number): Transport }).transportAt = (n: number) => {
        if (!held.has(n)) {
            const t = new Transport(server as never, n);
            (t as unknown as { state(): Promise<{ playing: boolean }> }).state = () =>
                Promise.resolve({ playing: server.playing });
            held.set(n, t);
        }
        return held.get(n)!;
    };
    const [editor, host, wid] = await opened(take);
    const props = new Map<number, Record<string, unknown>>();
    host.set = ((w: number, p: Record<string, unknown>) => {
        props.set(w, { ...props.get(w), ...p });
    }) as never;
    const transport = editor.transport!;
    assert.equal(transport, editor.transport, "one transport, one object");
    await transport.loop(10 / SR, 30 / SR);
    assert.deepEqual(transport.span, [10 / SR, 30 / SR]);
    assert.ok(transport.looping);
    assert.deepEqual([props.get(wid)?.sel_start, props.get(wid)?.sel_len], [10, 20]);
    assert.equal(props.get(wid)?.looping, 1, "the window's L switch");
    server.sent.length = 0;
    await transport.play();
    const loops = server.sent.filter(([addr]) => addr === "/transport_loop");
    assert.deepEqual(loops.at(-1)![1].slice(-2).map(Number), [10, 30], "the span, looped");
    assert.equal((await editor.selected())!.start, 10, "what is marked is the range");
    await editor.unselect();
    assert.equal(transport.span, null);
    assert.equal(await editor.selected(), null);
    editor.apply("/gui_event", [wid, 9, 0, "selection", 20, 40]);
    assert.deepEqual(transport.span, [20 / SR, 60 / SR], "a sweep is the transport's span");
});



test("visual layers use shared outcomes for calls and menu events", async () => {
    const editor = new AudioEditor(new FakeBuffer() as never, { sampleRate: SR, context: new Editing() });
    const host = new FakeHost();
    await editor.open(host as never);
    editor.hideLayer("peak");
    assert.equal((editor.layers[0] as {visible: boolean}).visible, false);
    editor.showLayer("peak");
    editor.layerAlpha("peak", 0.3);
    editor.soloLayer("peak");
    assert.equal((editor.layers[0] as {alpha: number}).alpha, 0.3);
    assert.equal((editor.layers[0] as {solo: boolean}).solo, true);
    editor.soloLayer("peak");
    editor.viewSpectrogram();
    assert.deepEqual(editor.layers, ["spectrogram"]);
    const before = editor.layers;
    assert.throws(() => editor.showLayer("peak"), /different domains/);
    assert.deepEqual(editor.layers, before);
    editor.viewWave();
    assert.deepEqual(editor.layers, ["peak", "rms"]);
    assert.equal(host.trees.length, 1, "an alternative is a prop, not a new window");
});

test("both views stack the spectrogram in a lane under the take", async () => {
    const editor = new AudioEditor(new FakeBuffer() as never, { sampleRate: SR, context: new Editing() });
    const host = new FakeHost();
    await editor.open(host as never);
    const wid = [...editor.view!.widgets.keys()][0]!;
    editor.apply("/gui_event", [wid, 1, 0, "view_both"]);
    assert.deepEqual(editor.layers, ["peak", "rms"]);
    assert.equal(host.trees.length, 2, "the window is drawn again with a second lane");
    const lanes = (host.trees.at(-1)!.children![1]!.children![1] as GuiNode).children![0] as GuiNode;
    assert.equal(lanes.type, "layout");
    const [pane, lane] = lanes.children as GuiNode[];
    assert.equal(pane!.type, "pane");
    assert.equal(lane!.measure, "spectrogram");
    assert.equal(lane!.link, pane!.id);
    editor.viewWave();
    assert.equal(host.trees.length, 3);
    assert.equal(((host.trees.at(-1)!.children![1]!.children![1] as GuiNode).children![0] as GuiNode).type, "pane");
});

test("the edit layer is the pane body and not the visual stack", async () => {
    const editor = new AudioEditor(new FakeBuffer() as never, { sampleRate: SR, context: new Editing() });
    const host = new FakeHost();
    await editor.open(host as unknown as GuiHost);
    const pane = host.trees[0]!.children![1]!.children![1]!.children![0]!;
    assert.equal(pane.type, "pane");
    assert.equal(pane.children![0]!.id, picture(host.trees[0]).id);
    assert.equal((pane.axes as any).x.unit, "time");
    assert.equal(editor.layer, "take");
    editor.layer = "placement";
    assert.equal(editor.layer, "placement");
    await settle();
    assert.deepEqual(host.acks.at(-1)![1], [[pane.id, { layer: "placement" }]]);
    editor.layer = "take";
    assert.deepEqual(editor.layers, ["peak", "rms"]);
    assert.throws(() => { editor.layer = "points:1"; }, /no 'points:1' body/);
    assert.equal(editor.layer, "take");
});

test("gain curve edits and apply share the take history", async () => {
    const take = new FakeBuffer();
    const [editor, host, , context] = await opened(take);
    const original = editor.parts;
    const duration = 100 / SR;
    const points = [0, 0.25, 5, -3, duration, 0.75, 1, 0];
    editor.showEnvelope();
    assert.equal(editor.layer, "points");
    await settle();
    take.server.sent.length = 0;
    assert.equal(editor.setEnvelope(points), true);
    assert.deepEqual(editor.envelope, points);
    await settle();
    assert.equal(take.server.addrs().includes("/buffer_gain"), false);
    assert.equal(take.server.addrs().includes("/buffer_setRange"), false);
    assert.equal(editor.applyEnvelope(), true);
    await settle();
    const gains = take.server.sent.filter(([addr]) => addr === "/buffer_gain").map(([, args]) => args);
    assert.ok(gains.length);
    assert.ok(gains[0][0] !== take.bufnum && gains[0][0] !== editor.buffer.bufnum);
    assert.deepEqual(gains[0].slice(1, 7), [0, 100, 0.25, 0.75, 5, -3]);
    assert.equal(editor.envelope[1], 1);
    assert.equal(editor.undo(), true);
    await settle();
    assert.deepEqual(editor.parts, original);
    assert.deepEqual(editor.envelope, points);
    assert.equal(editor.undo(), true);
    await settle();
    assert.equal(editor.envelope[1], 1);
    assert.equal(editor.redo(), true);
    await settle();
    assert.equal(editor.redo(), true);
    await settle();
    assert.equal(editor.envelope[1], 1);
    assert.ok(context.version > 1);
    assert.ok(host.acks.length);
});

test("invalid gain quads leave history and samples unchanged", async () => {
    const [editor, , , context] = await opened(new FakeBuffer());
    const before = editor.envelope;
    const version = context.version;
    assert.throws(() => editor.setEnvelope([0, 1, 1]), /complete/);
    assert.deepEqual(editor.envelope, before);
    assert.equal(context.version, version);
    assert.equal(editor.applyEnvelope(), false);
    await settle();
});


test("preview switches keep samples and history and release the insert", async () => {
    const take = new FakeBuffer();
    const [editor, , , context] = await opened(take);
    editor.setEnvelope([0, 0.25, 1, 0, 100 / SR, 0.75, 1, 0]);
    await settle();
    const before = editor.parts;
    const version = context.version;
    take.server.sent.length = 0;
    editor.previewOn();
    await settle();
    assert.deepEqual(editor.parts,before);
    assert.equal(context.version,version);
    assert.ok(take.server.addrs().includes("/buffer_gain"));
    assert.ok(take.server.addrs().includes("/synth_new"));
    take.server.sent.length = 0;
    editor.previewOff();
    await settle();
    assert.equal(take.server.addrs().includes("/buffer_alloc"),false);
    editor.previewOn();
    await settle();
    assert.equal(take.server.addrs().includes("/buffer_alloc"),false);
    editor.removeEffect();
    await settle();
    assert.equal(take.server.addrs().filter(addr => addr === "/buffer_free").length,2);
    assert.deepEqual(editor.parts,before);
    assert.equal(context.version,version);
    assert.equal(editor.envelope[1],0.25);
});

test("an empty sound file opens untitled and asks for a path", async () => {
    const [editor, host] = await opened(new FakeBuffer(4800, 2), { file: SoundFile.empty(2, 4800, SR) });
    assert.equal(editor.buffer.frames, 4800);
    const file = editor.file!;
    assert.deepEqual([file.path, file.frames, file.channels], [null, 4800, 2]);
    await assert.rejects(editor.save(), /path/);
    assert.equal(await editor.save({ path: "/takes/new.wav" }), "/takes/new.wav");
    assert.equal(editor.file!.path, "/takes/new.wav");
    assert.equal(editor.file!.sampleFormat, "float");
    // the window numbers the dialogs and holds their stack
    assert.ok(JSON.stringify(host.trees[0]).includes("Save as"));
});

test("a new file asked for in the window is delivered to the handler", async () => {
    const [editor] = await opened(new FakeBuffer(), { file: SoundFile.empty(1, 100, SR) });
    const asked: unknown[] = [];
    (editor as unknown as { newFileShape: (shape: unknown) => Promise<void> }).newFileShape = (shape) => {
        asked.push(shape);
        return Promise.resolve();
    };
    (editor as unknown as { take: (outcome: unknown) => boolean }).take({
        turn: "route", newFile: { channels: 3, frames: 10, rate: SR },
    });
    await settle();
    assert.deepEqual(asked, [{ channels: 3, frames: 10, rate: SR }]);
});

test("a take opened over a buffer is no file", async () => {
    const [editor] = await opened(new FakeBuffer());
    assert.equal(editor.file, null);
});

test("channels fold through the crate and correct the take", async () => {
    const [editor, host, wid] = await opened(new FakeBuffer(100, 4));
    editor.foldChannel(1);
    assert.deepEqual(editor.folded, [1]);
    await settle();
    const pushed = host.acks[host.acks.length - 1]![1];
    assert.ok(pushed.some(([w, p]) => w === wid && JSON.stringify(p.folded) === "[1]"), JSON.stringify(pushed));
    editor.soloChannel(2);
    assert.deepEqual(editor.folded, [0, 1, 3]);
    editor.showAllChannels();
    assert.deepEqual(editor.folded, []);
    assert.throws(() => editor.foldChannel(7), /channel 7/);
});

test("files asked for in the window are delivered to the handler", async () => {
    const [editor] = await opened(new FakeBuffer());
    const asked: string[][] = [];
    (editor as unknown as { openPaths: (path: string[]) => Promise<void> }).openPaths = (path) => {
        asked.push(path);
        return Promise.resolve();
    };
    (editor as unknown as { take: (outcome: unknown) => boolean }).take({
        turn: "route", openFile: { paths: ["/takes/b.wav", "/takes/c.wav"] },
    });
    await settle();
    assert.deepEqual(asked, [["/takes/b.wav", "/takes/c.wav"]]);
});


test("files share one handler and transport but keep state and history", async () => {
    const [editor, host, wid] = await opened(new FakeBuffer());
    const transportId = editor.transport!.id;
    const originalRead = SoundFile.prototype.toBuffer;
    const originalOpen = SoundFile.open;
    SoundFile.prototype.toBuffer = async function({ server } = {}) {
        const endpoint = server as unknown as FakeServer;
        return { bufnum: endpoint.buffers.alloc(), frames: this.frames, channels: this.channels,
            sampleRate: this.sampleRate, server: endpoint, path: this.path } as never;
    };
    SoundFile.open = async (path: string) => new SoundFile(path, 200, 2, SR);
    try {
        editor.setEnvelope([0, 1, 1, 0, 100 / SR, 0.5, 1, 0]);
        editor.coreCall("locate", { at: 20 / SR });
        editor.coreCall("span", { span: [10 / SR, 40 / SR] });
        const envelope = editor.envelope;
        const listed = () => host.trees.at(-1)!.children![1]!.children![0]!.id;
        const filesList = listed();
        await editor.openFile(["/takes/b.wav", "/takes/c.wav"]);
        assert.equal(listed(), filesList, "the list is the window's: a click in flight still finds it");
        assert.equal(editor.files.length, 3);
        assert.equal(picture(host.trees.at(-1)).id, wid);
        assert.equal(editor.file!.path, "/takes/c.wav");
        assert.equal(editor.transport!.id, transportId);
        assert.equal(editor.canUndo, false);
        await editor.focusFile(0);
        assert.equal(editor.cursor, 20 / SR);
        assert.deepEqual(editor.envelope, envelope);
        assert.equal(editor.undo(), true);
        await settle();
        assert.equal(editor.canRedo, true);
        await editor.focusFile(1);
        assert.equal(editor.canUndo, false);
        assert.equal(editor.canRedo, false);
        await editor.focusFile(0);
        assert.equal(editor.canRedo, true);
        editor.hideFiles();
        assert.equal(editor.coreCall("files").visible, false);
        editor.showFiles();
        assert.equal(host.trees.at(-1)!.children![1]!.children![0]!.type, "table");
        await editor.closeFile(2);
        assert.equal(editor.files.length, 2);
        assert.equal(editor.canRedo, true);
        await assert.rejects(editor.focusFile(-1), /index/);
        await editor.closeFile();
        assert.equal(editor.file!.path, "/takes/b.wav");
        await editor.closeFile();
        assert.equal(editor.closed, true);
        assert.deepEqual(editor.files, []);
    } finally {
        SoundFile.prototype.toBuffer = originalRead;
        SoundFile.open = originalOpen;
    }
});

test("playing a file while viewing another keeps the signal and meter", async () => {
    const take = new FakeBuffer(100, 1);
    const [editor, host] = await opened(take);
    const props = new Map<number, Record<string, unknown>>();
    host.set = ((id: number, values: Record<string, unknown>) => {
        props.set(id, { ...props.get(id), ...values });
    }) as typeof host.set;
    const originalRead = SoundFile.prototype.toBuffer;
    const originalOpen = SoundFile.open;
    SoundFile.prototype.toBuffer = async function({ server } = {}) {
        const endpoint = server as unknown as FakeServer;
        return { bufnum: endpoint.buffers.alloc(), frames: this.frames, channels: this.channels,
            sampleRate: this.sampleRate, server: endpoint, path: this.path } as never;
    };
    SoundFile.open = async (path: string) => new SoundFile(path, 200, path.includes("six") ? 6 : 2, SR);
    const driver = (editor.transport as unknown as { driver: { play(): Promise<void>; stop(): Promise<void> } }).driver;
    try {
        let first = editor.coreCall("binding").id;
        await driver.play();
        take.server.playing = true;
        await editor.openFile(["/takes/stereo.wav", "/takes/six.wav"]);
        for (const [index, channels] of [[2, 6], [1, 2], [0, 1], [1, 2], [2, 6]]) {
            await editor.focusFile(index!);
            const playback = (editor as unknown as { playback: { call(v: string): Promise<Record<string, any>> } }).playback;
            const state = await playback.call("state");
            assert.equal(state.playing, first);
            const wid = Number(picture(host.trees.at(-1)).id);
            assert.equal(props.get(wid)?.playhead_at, editor.coreCall("binding").id === first ? 0 : -1);
            await driver.stop();
            take.server.playing = false;
            assert.deepEqual((await playback.call("state")).meters, { bus: -channels!, channels });
            await driver.play();
            take.server.playing = true;
            first = editor.coreCall("binding").id;
            assert.equal((await playback.call("state")).meters.channels, channels);
        }
        editor.close();
    } finally {
        SoundFile.prototype.toBuffer = originalRead;
        SoundFile.open = originalOpen;
    }
});


test("two audio windows keep distinct component bindings on one host", async () => {
    const take = new FakeBuffer();
    const [first, host, wid, context] = await opened(take);
    const second = new AudioEditor(take as never, { sampleRate: SR, context });
    await second.open(host as unknown as GuiHost);
    const otherIds = [...second.view!.widgets.keys()];
    for (const id of first.view!.widgets.keys()) assert.ok(!otherIds.includes(id));
    first.viewBoth();
    await settle();
    assert.equal(picture(host.trees.at(-1)).id, wid);
    assert.deepEqual([...second.view!.widgets.keys()], otherIds);
    first.close();
    assert.equal(second.closed, false);
    second.close();
});


test("reconnecting refuses stale editor resources before sending", async () => {
    const take = new FakeBuffer();
    const [editor] = await opened(take);
    await settle();
    const previousIds = take.server.ids;
    const sent = take.server.sent.length;
    take.server.ids = new IdSpaces(8192, 1024, 2, 16384, 4096, 16, 64, 0, 1);
    assert.throws(() => editor.parts, /previous server instance/);
    const playback = (editor as unknown as { playback: { call(v: string): Promise<unknown> } }).playback;
    await assert.rejects(playback.call("close"), /previous server instance/);
    assert.equal(take.server.sent.length, sent);
    assert.notEqual(previousIds, take.server.ids);
});


test("shared undo updates an unfocused file without moving focus", async () => {
    const [editor, , , context] = await opened(new FakeBuffer());
    const original = editor.envelope;
    editor.setEnvelope([0, 0.5, 1, 0, 100 / SR, 0.25, 1, 0]);
    await settle();
    const originalRead = SoundFile.prototype.toBuffer;
    const originalOpen = SoundFile.open;
    SoundFile.prototype.toBuffer = async function({ server } = {}) {
        const endpoint = server as unknown as FakeServer;
        return { bufnum: endpoint.buffers.alloc(), frames: this.frames, channels: this.channels,
            sampleRate: this.sampleRate, server: endpoint, path: this.path } as never;
    };
    SoundFile.open = async (path: string) => new SoundFile(path, 200, 2, SR);
    try {
        await editor.openFile("/takes/b.wav");
        const binding = editor.coreCall("binding");
        const stepped = context.step("undo");
        assert.equal(stepped.stepped, true);
        assert.notEqual(stepped.effects?.[0]?.file?.id, binding.id);
        context.carry(stepped);
        await settle();
        assert.equal(editor.coreCall("binding").id, binding.id);
        assert.equal(editor.file!.path, "/takes/b.wav");
        await editor.focusFile(0);
        assert.deepEqual(editor.envelope, original);
        editor.close();
    } finally {
        SoundFile.prototype.toBuffer = originalRead;
        SoundFile.open = originalOpen;
    }
});


test("clipboard replacement does not free a previous server incarnations id", async () => {
    const take = new FakeBuffer();
    const [first] = await opened(take);
    const publish = (editor: AudioEditor, buffer: number) =>
        (editor as unknown as { clipboard(doc: string, buffer: number): void }).clipboard(JSON.stringify({ content: { kind: "buffer", channels: 1, frames: 4, sample_rate: SR, buffer } }), buffer);
    publish(first, 700);
    take.server.ids = new IdSpaces(8192, 1024, 2, 16384, 4096, 16, 64, 0, 1);
    const second = new AudioEditor(take as never, { context: new Editing(), sampleRate: SR });
    await settle();
    take.server.sent = [];
    publish(second, 701);
    assert.ok(!take.server.sent.some(([addr, args]) => addr === "/buffer_free" && Number(args[0]) === 700));
    publish(second, 702);
    await settle();
    assert.ok(take.server.sent.some(([addr, args]) => addr === "/buffer_free" && Number(args[0]) === 701));
    second.close();
});

test("clipboard copy survives origin close and paste owns its samples", async () => {
    const take = new FakeBuffer();
    const [first, host, wid, context] = await opened(take);
    first.apply("/gui_event", [wid, 1, context.version, "copy", 10, 20]);
    await settle();
    const copied = host.clip;
    const source = JSON.parse(copied).content.buffer as number;
    assert.ok(JSON.parse(copied).content.location.publication >= 0);
    first.close();
    await settle();
    assert.ok(!take.server.buffers.freed.includes(source));
    const [second, otherHost, otherWid, otherContext] = await opened(take);
    second.apply("/gui_event", [otherWid, 1, otherContext.version, "paste", 50, "buffer", copied]);
    await settle();
    assert.equal(second.buffer.frames, 120);
    assert.ok(second.parts.every((p) => (p.source as { source: number }).source !== source));
    second.apply("/gui_event", [otherWid, 2, otherContext.version, "mix", 0, "buffer", copied]);
    await settle();
    assert.equal(second.buffer.frames, 120);
    assert.ok(take.server.addrs().includes("/buffer_mix"));
    second.apply("/gui_event", [otherWid, 3, otherContext.version, "copy", 0, 3]);
    await settle();
    assert.ok(take.server.buffers.freed.includes(source));
    assert.notEqual(otherHost.clip, copied);
    second.undo();
    await settle();
    assert.equal(second.buffer.frames, 120);
    second.undo();
    await settle();
    assert.equal(second.buffer.frames, 100);
    second.close();
    await settle();
});

test("paste refuses other servers stale publications and external text", async () => {
    const take = new FakeBuffer();
    const [editor, host, wid, context] = await opened(take);
    editor.apply("/gui_event", [wid, 1, context.version, "copy", 0, 4]);
    await settle();
    const copied = host.clip;
    const [other, , otherWid, otherContext] = await opened(new FakeBuffer());
    other.apply("/gui_event", [otherWid, 1, otherContext.version, "paste", 0, "buffer", copied]);
    await settle();
    assert.equal(other.buffer.frames, 100);
    assert.equal(other.canUndo, false);
    editor.apply("/gui_event", [wid, 2, context.version, "copy", 10, 4]);
    await settle();
    editor.apply("/gui_event", [wid, 3, context.version, "paste", 0, "buffer", copied]);
    await settle();
    assert.equal(editor.buffer.frames, 100);
    const current = host.clip;
    editor.apply("/gui_event", [wid, 4, context.version, "paste", 0, "text", JSON.stringify({ content: { kind: "text", text: "external" } })]);
    await settle();
    assert.ok(take.server.buffers.freed.includes(Number(JSON.parse(current).content.buffer)));
    assert.equal(editor.buffer.frames, 100);
    other.close();
    editor.close();
    await settle();
});

test("failed cut copy keeps contents and previous publication", async () => {
    const take = new FakeBuffer();
    const [editor, host, wid, context] = await opened(take);
    editor.apply("/gui_event", [wid, 1, context.version, "copy", 0, 4]);
    await settle();
    const previous = host.clip;
    const request = take.server.request.bind(take.server);
    take.server.request = (addr, args) => addr === "/buffer_gen"
        ? Promise.resolve({ addr: "/fail", args: [addr, "copy failed"] })
        : request(addr, args);
    editor.apply("/gui_event", [wid, 2, context.version, "cut", 10, 20]);
    await settle();
    assert.equal(editor.buffer.frames, 100);
    assert.equal(editor.canUndo, false);
    assert.equal(host.clip, previous);
    take.server.request = request;
    editor.apply("/gui_event", [wid, 3, context.version, "cut", 10, 20]);
    await settle();
    assert.equal(editor.buffer.frames, 80);
    assert.notEqual(host.clip, previous);
    editor.close();
    await settle();
});


test("undo redo correct only composed views while optional ids stay stable", async () => {
    const take = new FakeBuffer();
    const context = new Editing();
    const editor = new AudioEditor(take as never, { sampleRate: SR, context });
    const host = new FakeHost();
    await editor.open(host as never);
    const wid = [...editor.view!.widgets.keys()][0]!;
    const treeIds = (node: GuiNode): Set<number> => {
        const result = new Set<number>(node.id === undefined ? [] : [node.id]);
        for (const child of node.children ?? []) {
            for (const id of treeIds(child)) result.add(id);
        }
        return result;
    };
    editor.apply("/gui_event", [wid, 1, context.version, "draw", 0, 10, [0.5], [0.0]]);
    await settle();
    const lane = editor.windowWidget("lane");
    for (const configure of [() => editor.viewWave(), () => editor.viewBoth(),
        () => editor.viewSpectrogram(), () => editor.viewBoth(), () => editor.viewWave()]) {
        configure();
        await settle();
        const present = treeIds(host.trees.at(-1)!);
        assert.ok([...editor.view!.widgets.keys()].every((id) => present.has(id)));
        assert.equal(editor.view!.widgets.has(lane), present.has(lane));
        assert.equal(editor.windowWidget("lane"), lane);
        for (const walk of [() => editor.undo(), () => editor.redo()]) {
            host.acks.length = 0;
            assert.equal(walk(), true);
            await settle();
            const corrections = host.acks.flatMap((ack) => ack[1]);
            assert.ok(corrections.length > 0);
            assert.ok(corrections.every(([id]) => present.has(id)));
            assert.ok(corrections.some(([id]) => id === wid));
        }
    }
    editor.close();
});

test("failed save keeps destination and undo to saved revision is clean", async () => {
    const take = new FakeBuffer();
    const [editor, _host, wid, context] = await opened(take);
    editor.apply("/gui_event", [wid, 1, context.version, "draw", 0, 10, [0.5], [0.0]]);
    await settle();
    await editor.save({ path: "/takes/saved.wav" });
    editor.apply("/gui_event", [wid, 2, context.version, "draw", 0, 20, [0.25], [0.0]]);
    await settle();
    const request = take.server.request.bind(take.server);
    take.server.request = (addr, args) => addr === "/buffer_write"
        ? Promise.resolve({ addr: "/fail", args: [addr, "quota exceeded"] }) : request(addr, args);
    await assert.rejects(editor.save({ path: "/takes/refused.wav" }), /quota/);
    assert.equal(editor.file!.path, "/takes/saved.wav");
    assert.equal(editor.dirty, true);
    take.server.request = request;
    assert.equal(editor.undo(), true);
    await settle();
    assert.equal(editor.dirty, false);
    assert.equal(editor.redo(), true);
    await settle();
    assert.equal(editor.dirty, true);
    assert.equal(await editor.save(), "/takes/saved.wav");
    assert.equal(editor.dirty, false);
    editor.close();
    await settle();
});

test("failed file open keeps existing files and returns reserved ids", async () => {
    const take = new FakeBuffer();
    const [editor] = await opened(take);
    const allocated: number[] = [];
    const originalRead = SoundFile.prototype.toBuffer;
    const request = take.server.request.bind(take.server);
    SoundFile.prototype.toBuffer = async function({ server } = {}) {
        const endpoint = server as unknown as FakeServer;
        const number = endpoint.buffers.alloc();
        allocated.push(number);
        return { bufnum: number, frames: this.frames, channels: this.channels, sampleRate: this.sampleRate,
            server, path: this.path } as never;
    };
    try {
        take.server.request = (addr, args) => addr === "/buffer_gen"
            ? Promise.resolve({ addr: "/fail", args: [addr, "copy refused"] }) : request(addr, args);
        const before = editor.files.length;
        await assert.rejects(editor.newFile(1, 50, SR), /copy refused/);
        await settle();
        assert.equal(editor.files.length, before);
        assert.ok(take.server.buffers.freed.includes(allocated[0]!));
        assert.equal(take.server.buffers.freed.length, new Set(take.server.buffers.freed).size);
        take.server.request = request;
        await editor.newFile(1, 50, SR);
        assert.equal(editor.files.length, before + 1);
    } finally {
        SoundFile.prototype.toBuffer = originalRead;
        take.server.request = request;
        editor.close();
        await settle();
    }
});

test("failed cleanup keeps buffer id reserved until successful retry", async () => {
    const take = new FakeBuffer();
    const [editor, _host, wid, context] = await opened(take);
    editor.apply("/gui_event", [wid, 1, 0, "draw", 0, 10, [0.5], [0.0]]);
    await settle();
    const first = spans(editor)[1]![0];
    assert.equal(editor.undo(), true);
    await settle();
    const request = take.server.request.bind(take.server);
    take.server.request = (addr, args) => addr === "/buffer_free"
        ? Promise.resolve({ addr: "/fail", args: [addr, "free refused"] }) : request(addr, args);
    editor.apply("/gui_event", [wid, 2, context.version, "draw", 0, 30, [0.5], [0.0]]);
    await settle();
    assert.equal(take.server.buffers.freed.includes(first), false);
    const pending = context.serviceCall<{ tickets: { ticket: { resource: { buffer: number } }; reason: string }[] }>("cleanup", { action: "pending" }).tickets;
    assert.equal(pending.length, 1);
    assert.equal(pending[0]!.ticket.resource.buffer, first);
    assert.match(pending[0]!.reason, /free refused/);
    take.server.request = request;
    await editor.releaseBuffers([first]);
    await editor.releaseBuffers([first]);
    assert.equal(take.server.buffers.freed.filter((id) => id === first).length, 1);
    assert.deepEqual(context.serviceCall("cleanup", { action: "pending" }).tickets, []);
    editor.close();
    await settle();
});
