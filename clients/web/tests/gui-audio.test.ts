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

await loadCore();

const SR = 48_000;

/** What the host is told, so an answer can be read. */
class FakeHost {
    acks: [number, [number, Record<string, PropValue>][], string | undefined][] = [];
    trees: GuiNode[] = [];
    clock = "device";
    clockOf = -1;
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
    ids = new IdSpaces(8192, 1024, 2, 16384, 4096, 0, 1);
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
    transportAt(): { transportState: () => Promise<{ playing: boolean }> } {
        return { transportState: () => Promise.resolve({ playing: this.playing }) };
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
    options: { historyBytes?: number; residentBytes?: number; scratch?: string } = {},
): Promise<[AudioEditor, FakeHost, number, Editing]> {
    const context = new Editing();
    const editor = new AudioEditor(take as never, { sampleRate: SR, context, ...options });
    const host = new FakeHost();
    await editor.open(host as unknown as GuiHost);
    await settle();
    return [editor, host, Number(host.trees[0]!.children![0]!.id), context];
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
    const take = host.trees[0]!.children![0]! as unknown as {
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

test("a cut moves no samples", async () => {
    const take = new FakeBuffer();
    const [editor, , wid] = await opened(take);
    take.server.sent = [];
    editor.apply("/gui_event", [wid, 1, 0, "cut", 10.0, 20.0]);
    await settle();
    assert.deepEqual(
        take.server.addrs(),
        ["/buffer_stitch", "/node_set"],
        "the join stitched, and the readers' window set to its new length",
    );
    assert.equal(editor.buffer.frames, 80);
    assert.equal(editor.undo(), true);
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
            ([addr, args]) => addr === "/buffer_write" && args[1] === `/scratch/take-${first}.wav`,
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
    assert.deepEqual(take.server.sent, [
        ["/buffer_write", [editor.buffer.bufnum, "/takes/a.wav", "wav", "float"]],
    ]);
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
    assert.deepEqual(take.server.addrs(), ["/buffer_alloc", "/server_sync", "/buffer_gen"]);
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
    assert.deepEqual(
        [tree.type, tree.title, tree.flow],
        ["window", "take", "row"],
        "the take, and its level beside it",
    );
    const picture = tree.children![0]! as Record<string, unknown>;
    const wid = Number(picture.id);
    assert.equal(picture.type, "signal");
    assert.deepEqual([picture.buffer, picture.channels], [editor.buffer.bufnum, 2]);
    assert.equal(picture.measure, "peak rms");
    assert.deepEqual(picture.gestures, { drag: "select", alt: "draw", ctrl: "sample" });
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
    const meter = (window.children ?? [])[1] as Record<string, unknown>;
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
