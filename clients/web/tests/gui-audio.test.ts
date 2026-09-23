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
import { AudioEditor, Editing } from "../src/gui/editing/index.ts";
import type { GuiHost, PropValue } from "../src/gui/host.ts";
import type { GuiNode } from "../src/gui/guidef.ts";

await loadCore();

const SR = 48_000;

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

type Sent = [string, number[]];

/**
 * Every message a take's steps send, in order, each answered the way the server
 * answers it.
 */
class FakeServer {
    sent: Sent[] = [];
    buffers = new FakeAllocator();

    bulkChunk(): Promise<number> {
        return Promise.resolve(8192);
    }
    sendMsg(addr: string, ...args: unknown[]): void {
        this.sent.push([addr, args.map((a) => (Array.isArray(a) ? Number(a[1]) : Number.NaN))]);
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
    options: { historyBytes?: number } = {},
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

test("the window draws a join stitched over the whole take", async () => {
    const take = new FakeBuffer();
    const [editor] = await opened(take);
    const display = editor.buffer.bufnum;
    const stitched = take.server.sent.filter(([addr]) => addr === "/buffer_stitch");
    assert.equal(stitched[0]![1][0], display);
    assert.equal(stitched[0]![1][3], take.bufnum, "it reads the take");
    assert.equal(editor.buffer.frames, 100);
    assert.ok(!take.server.addrs().includes("/buffer_setRange"), "the take is never written");
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
    assert.deepEqual(spans(editor), [[7, 0], [fresh, 0], [7, 42]]);
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
    assert.deepEqual(take.server.addrs(), ["/buffer_stitch"]);
    assert.equal(editor.buffer.frames, 80);
    assert.equal(editor.undo(), true);
    assert.equal(editor.buffer.frames, 100);
});
