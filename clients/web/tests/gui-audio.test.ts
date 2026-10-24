// `AudioEditor`: a take edited as a list of parts over files it never writes.
//
// What is checked is this page's half -- the steps the crate answers are walked
// against the take's server, the renders and moves are carried out on the
// disk, and the renders the history lets go of are deleted. What each gesture
// does to the list is the crate's and is tested there.
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
import { CommandError } from "../src/errors.ts";
import { disk as endpoint } from "../src/steps.ts";
import type { RenderStep } from "../src/steps.ts";

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
    props = new Map<number, Record<string, unknown>>();
    set(id: number, props: Record<string, unknown>): void {
        this.props.set(id, { ...this.props.get(id), ...props });
    }
    clip = "";
    clipboard(document: string): void { this.clip = document; }
    verbs: string[] = [];
    verb(_window: unknown, name: string): void { this.verbs.push(name); }
    close(): void {}
    headClock(id: number | { readonly id: number }, name: string, _transport = 0): void {
        this.clock = name;
        this.clockOf = typeof id === "number" ? id : id.id;
    }
    onMessage(): () => void {
        return () => {};
    }
    ack(seq: number, _docVersion = 0, _generations: readonly (readonly [number, number])[] = [], reason?: string): void {
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

/**
 * **What a run does on the disk**, recorded rather than done: the renders
 * (what one writes is the offline engine's, tested where it is,
 * `file-render.test.ts`), the moves and the files deleted. Emptied before
 * each test.
 */
const disk = {
    renders: [] as RenderStep[],
    moves: [] as [string, string][],
    deleted: [] as string[],
    refuse: null as ((path: string) => boolean) | null,
};
endpoint.render = async (render) => {
    if (disk.refuse !== null && disk.refuse(render.path)) throw new Error("quota exceeded");
    disk.renders.push(render);
};
endpoint.move = async (from, to) => { disk.moves.push([from, to]); };
endpoint.remove = async (path) => { disk.deleted.push(path); };
test.beforeEach(() => {
    disk.renders = [];
    disk.moves = [];
    disk.deleted = [];
    disk.refuse = null;
});

/**
 * The take's list as `[file stem, start, frames]`: which file each part reads,
 * and where.
 */
const stems = (editor: AudioEditor): [string, number, number][] =>
    editor.parts.map((p) => [
        String(p.path).split("/").pop()!.replace(/\.[^.]*$/, ""),
        Number(p.start),
        Number(p.frames),
    ]);

/** Every step in flight, carried out. */
const settle = async (): Promise<void> => {
    for (let i = 0; i < 20; i++) await new Promise((resolve) => setTimeout(resolve, 0));
};

async function opened(
    take: FakeBuffer,
    options: {
        workspace?: string; file?: SoundFile; sharedHistory?: boolean; title?: string;
    } = {},
): Promise<[AudioEditor, FakeHost, number, Editing]> {
    const context = new Editing();
    // a folder of no disk: what is done there is recorded (`disk`)
    const editor = new AudioEditor(take as never, { sampleRate: SR, context, workspace: "/work", ...options });
    const host = new FakeHost();
    await editor.open(host as unknown as GuiHost);
    await settle();
    return [editor, host, Number(picture(host.trees[0]).id), context];
}

test("a buffer is written to the working folder and the join reads it", async () => {
    const take = new FakeBuffer();
    const [editor, host] = await opened(take);
    const display = editor.buffer.bufnum;
    const write = take.server.sent.find(([addr]) => addr === "/buffer_write")![1];
    assert.ok(write[0] === take.bufnum && String(write[1]).endsWith("-opened.w64"), "the buffer, out");
    assert.ok(String(write[1]).startsWith("/work/"));
    const stitched = take.server.sent.filter(([addr]) => addr === "/buffer_stitch");
    assert.equal(stitched[0]![1][0], display);
    assert.equal(stitched[0]![1][3], write[1], "the join reads the file");
    assert.equal(picture(host.trees[0]).segments.parts[0].path, write[1]);
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

test("a stroke renders a file and the join reads it", async () => {
    const take = new FakeBuffer();
    const [editor, , wid] = await opened(take);
    take.server.sent = [];
    assert.equal(editor.apply("/gui_event", [wid, 1, 0, "draw", 0, 40, [0.5, -0.5], [0.0, 0.0]]), true);
    await settle();
    assert.ok(disk.renders.at(-1)!.path.endsWith("-stroke.w64"), "the stroke, rendered");
    assert.deepEqual(take.server.addrs(), ["/buffer_stitch"]);
    const openedStem = stems(editor)[0]![0];
    assert.deepEqual(stems(editor), [[openedStem, 0, 40], ["1-2-stroke", 0, 2], [openedStem, 42, 58]]);
});

test("a render the history cannot reach is deleted", async () => {
    const take = new FakeBuffer();
    const [editor, , wid, context] = await opened(take);
    editor.apply("/gui_event", [wid, 1, 0, "draw", 0, 10, [0.5], [0.0]]);
    await settle();
    const first = disk.renders.at(-1)!.path;
    assert.equal(editor.undo(), true);
    await settle();
    assert.ok(!disk.deleted.includes(first), "a redo can still find it");
    editor.apply("/gui_event", [wid, 2, context.version, "draw", 0, 30, [0.5], [0.0]]);
    await settle();
    assert.ok(disk.deleted.includes(first));
});

test("reverse renders the selection backwards", async () => {
    const [editor, host, wid] = await opened(new FakeBuffer());
    editor.apply("/gui_event", [wid, 1, 0, "selection", 10, 20]);
    await settle();
    assert.equal(editor.reverse(), true);
    assert.deepEqual(host.verbs, ["reverse"], "the host performs the window's verb");
    // and the window's event comes back
    editor.apply("/gui_event", [Number((editor as unknown as { windowId: number }).windowId), 2, 0, "reverse"]);
    await settle();
    assert.ok(disk.renders.at(-1)!.path.endsWith("-reversed.w64"));
    assert.deepEqual(stems(editor).map(([, , frames]) => frames), [10, 20, 70]);
    assert.equal(editor.undo(), true);
    await settle();
    assert.equal(stems(editor).length, 1);
});

test("a render's progress is on the window while it runs", async () => {
    const seen: Record<string, unknown>[] = [];
    const [editor, host, wid] = await opened(new FakeBuffer());
    const window = Number((editor as unknown as { windowId: number }).windowId);
    const recorded = endpoint.render;
    endpoint.render = async (_render, onProgress) => {
        onProgress?.(50, 100);
        seen.push({ ...host.props.get(window) });
    };
    try {
        editor.apply("/gui_event", [wid, 1, 0, "draw", 0, 10, [0.5], [0.0]]);
        await settle();
    } finally {
        endpoint.render = recorded;
    }
    assert.equal(seen[0]!.progress, 0.5);
    assert.equal(seen[0]!.progress_label, "rendering 1-2-stroke.w64");
    assert.equal(host.props.get(window)!.progress, -1, "taken away once it has ended");
});

test("two renders over two files run at once and land in either order", async () => {
    const [editor, , wid, context] = await opened(new FakeBuffer());
    const originalOpen = SoundFile.open;
    SoundFile.open = async (path: string) => new SoundFile(path, 200, 1, SR, "wav", "float");
    const recorded = endpoint.render;
    const gates = new Map<string, () => void>();
    const started: string[] = [];
    endpoint.render = (render) => {
        if (!render.path) return Promise.resolve(); // a file's peaks, measured as it opens
        started.push(render.path);
        return new Promise<void>((resolve) => gates.set(render.path, resolve));
    };
    const stroked = async (index: number): Promise<boolean> => {
        await editor.focusFile(index);
        return stems(editor).some(([stem]) => stem.endsWith("-stroke"));
    };
    try {
        await editor.openFile("/takes/b.wav");
        await settle();
        // a stroke in the second file, then one in the first while it renders
        editor.apply("/gui_event", [wid, 1, context.version, "draw", 0, 10, [0.5], [0.0]]);
        await settle();
        await editor.focusFile(0);
        editor.apply("/gui_event", [wid, 2, context.version, "draw", 0, 10, [0.5], [0.0]]);
        await settle();
        assert.equal(started.length, 2, "both render at once");
        gates.get(started[1]!)!();
        await settle();
        assert.equal(await stroked(0), true, "the second landed");
        assert.equal(await stroked(1), false, "the first is still rendering");
        gates.get(started[0]!)!();
        await settle();
        assert.equal(await stroked(1), true, "and then the first");
        assert.equal(editor.canUndo, true, "each stroke is its file's edit");
    } finally {
        for (const release of gates.values()) release();
        endpoint.render = recorded;
        SoundFile.open = originalOpen;
    }
});

test("a file changed on disk is said when the window comes back", async () => {
    const { mkdtemp, utimes } = await import("node:fs/promises");
    const { tmpdir } = await import("node:os");
    const folder = await mkdtemp(`${tmpdir()}/clausters-stat-`);
    const path = `${folder}/take.wav`;
    const file = await SoundFile.write(path, new Float32Array(100), 1, SR, { sampleFormat: "float" });
    const [editor, host] = await opened(new FakeBuffer(), { file });
    const window = Number((editor as unknown as { windowId: number }).windowId);
    editor.apply("/gui_event", [window, 1, 0, "activated", 1]);
    await settle();
    assert.equal(host.acks.at(-1)![2], undefined, "the first look takes the file as it is");
    await utimes(path, 1_000_000, 1_000_000);
    editor.apply("/gui_event", [window, 2, 0, "activated", 1]);
    await settle();
    assert.equal(host.acks.at(-1)![2], "take.wav changed on disk");
    editor.apply("/gui_event", [window, 3, 0, "activated", 1]);
    await settle();
    assert.equal(host.acks.at(-1)![2], undefined, "said once");
});

test("a cut copies the span's parts and renders nothing", async () => {
    const take = new FakeBuffer();
    const [editor, host, wid] = await opened(take);
    take.server.sent = [];
    const rendered = disk.renders.length;
    editor.apply("/gui_event", [wid, 1, 0, "cut", 10.0, 20.0]);
    await settle();
    assert.equal(take.server.addrs()[0], "/buffer_stitch");
    assert.equal(disk.renders.length, rendered, "nothing is made");
    assert.equal(JSON.parse(host.clip).content.kind, "segments");
    assert.equal(editor.buffer.frames, 80);
    assert.equal(editor.undo(), true);
    await settle();
    assert.equal(editor.buffer.frames, 100);
});

test("a delete removes samples without copying them", async () => {
    const take = new FakeBuffer();
    const [editor, host, wid] = await opened(take);
    take.server.sent = [];
    editor.apply("/gui_event", [wid, 1, 0, "delete", 10.0, 20.0]);
    await settle();
    assert.equal(take.server.addrs()[0], "/buffer_stitch", "no block was copied out");
    assert.equal(host.clip, "", "and nothing went on the clipboard");
    assert.equal(editor.buffer.frames, 80);
    assert.equal(editor.undo(), true);
    await settle();
    assert.equal(editor.buffer.frames, 100);
});

test("a save renders the take over its file or as another", async () => {
    const file = new SoundFile("/takes/a.wav", 100, 1, SR, "wav", "float");
    const take = new FakeBuffer();
    const [editor, , wid] = await opened(take, { file });
    assert.deepEqual(stems(editor), [["a", 0, 100]], "the file, read where it is");
    assert.ok(!take.server.addrs().includes("/buffer_write"));
    editor.apply("/gui_event", [wid, 1, 0, "cut", 10.0, 20.0]);
    await settle();
    assert.equal(await editor.save(), "/takes/a.wav");
    await settle();
    // the file the take reads is moved aside, never written while it is read
    const [[aside, held], [fresh, back]] = disk.moves as [[string, string], [string, string]];
    assert.ok(aside === "/takes/a.wav" && held.startsWith("/work/"));
    assert.ok(fresh === disk.renders.at(-1)!.path && back === "/takes/a.wav");
    assert.equal(await editor.save({ path: "/takes/b.wav", sampleFormat: "int24" }), "/takes/b.wav");
    await settle();
    assert.deepEqual([disk.renders.at(-1)!.path, disk.renders.at(-1)!.format], ["/takes/b.wav", "int24"]);
    assert.equal(disk.moves.length, 2, "a file nothing reads is rendered in place");
    assert.equal(await editor.save(), "/takes/b.wav");
});

test("a save over a buffer reads a render into it or into a new one", async () => {
    const take = new FakeBuffer();
    const [editor, , wid] = await opened(take);
    editor.apply("/gui_event", [wid, 1, 0, "cut", 10.0, 20.0]);
    await settle();
    take.server.sent = [];
    const saved = (await editor.save()) as { bufnum: number; frames: number };
    assert.deepEqual([saved.bufnum, saved.frames], [take.bufnum, 80]);
    const read = take.server.sent.filter(([addr]) => addr === "/buffer_allocRead").map(([, args]) => args);
    assert.ok(read[0]![0] === take.bufnum && read[0]![1] === disk.renders.at(-1)!.path);
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
    const editor = new AudioEditor(take as never, { sampleRate: SR, title: "take", context, workspace: "/work" });
    const host = new FakeHost();
    await editor.open(host as unknown as GuiHost);
    await settle();
    const tree = host.trees[0]!;
    assert.deepEqual([tree.type, tree.title], ["window", "take"]);
    assert.equal(tree.children![1]!.flow, "row", "the take, and its level beside it");
    const taken = picture(tree);
    const wid = Number(taken.id);
    assert.equal(taken.type, "signal");
    assert.ok(taken.channels === 2 && taken.buffer === undefined);
    assert.ok(String(taken.segments.parts[0].path).endsWith("-opened.w64"));
    assert.equal(taken.measure, "peak rms");
    assert.deepEqual(taken.gestures, { drag: "select", alt: "draw", ctrl: "sample" });
    // the list, and the band a history step may have moved onto the take
    assert.deepEqual(Object.keys(editor.view!.props(editor as never, wid)).sort(), [
        "cursor",
        "segments",
        "sel_len",
        "sel_start",
    ]);
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
    // rolling, the music goes on from the cursor
    const moved = take.server.sent.filter(([addr]) => addr === "/transport_locateSample");
    assert.deepEqual(moved[moved.length - 1][1].map(Number), [1, 60]);
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
    editor.apply("/gui_event", [wid, 5, 0, "selection", 40, 30, true]);
    await settle();
    assert.deepEqual(sent("/transport_locateSample").at(-1), [1, 40], "let go: into the span");

    // Not looping, a release moves the music to the span's start too.
    editor.apply("/gui_event", [window, 6, 0, "loop", 0]);
    await settle();
    server.sent = [];
    editor.apply("/gui_event", [wid, 7, 0, "selection", 5, 10]);
    editor.apply("/gui_event", [wid, 8, 0, "selection", 5, 10, true]);
    await settle();
    assert.deepEqual(sent("/transport_locateSample").at(-1), [1, 5], "let go: the music goes there");
    assert.deepEqual(sent("/transport_loop").at(-1), [1], "and the pass loops no more than it did");
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
    assert.ok(disk.renders.at(-1)!.path.endsWith("-gain.w64"), "the take, rendered through the curve");
    assert.equal(take.server.addrs().includes("/buffer_gain"), false);
    assert.deepEqual(stems(editor).map(([stem]) => stem.split("-").at(-1)), ["gain"]);
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

/** What the coordinator asks this page to read, instead of reading it. */
function asked(editor: AudioEditor): unknown[] {
    const asked: unknown[] = [];
    (editor as unknown as { shape: (request: Record<string, unknown>) => Promise<unknown> }).shape = (request) => {
        asked.push(request.shape ?? request.path);
        return Promise.reject(new Error("not read here"));
    };
    return asked;
}

test("a new file asked for in the window is delivered to the handler", async () => {
    const [editor] = await opened(new FakeBuffer(), { file: SoundFile.empty(1, 100, SR) });
    const read = asked(editor);
    const window = Number((editor as unknown as { windowId: number }).windowId);
    editor.apply("/gui_event", [window, 1, 0, "new_file"]);
    const dialog = (name: string) => editor.windowWidget("dialog", name);
    editor.apply("/gui_event", [dialog("new:channels"), 2, 0, "3"]);
    editor.apply("/gui_event", [dialog("new:ok"), 3, 0, "click"]);
    await settle();
    assert.equal(read.length, 1);
    const shape = read[0] as Record<string, number>;
    assert.equal(shape.channels, 3);
    assert.deepEqual(Object.keys(shape).sort(), ["channels", "frames", "rate"]);
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

test("files asked for are read one at a time", async () => {
    const [editor] = await opened(new FakeBuffer());
    const read = asked(editor);
    await assert.rejects(editor.openFile(["/takes/b.wav", "/takes/c.wav"]), /not read here/);
    assert.deepEqual(read, ["/takes/b.wav"], "the second is read once the first is added");
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
        assert.equal(editor.closed, false, "closing the last file leaves the editor open");
        assert.deepEqual(editor.files, []);
        assert.equal(editor.file, null);
    } finally {
        SoundFile.prototype.toBuffer = originalRead;
        SoundFile.open = originalOpen;
        endpoint.render = async (render) => {
            if (disk.refuse !== null && disk.refuse(render.path)) throw new Error("quota exceeded");
            disk.renders.push(render);
        };
    }
});

test("playing a file while viewing another silences the viewed meter", async () => {
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
            const playback = (editor as unknown as { playback: { state(): Promise<Record<string, any>> } }).playback;
            const state = await playback.state();
            assert.equal(state.playing, first);
            assert.equal(state.meters.channels, channels);
            assert.equal(state.meters.bus >= 0, editor.coreCall("binding").id === first);
            const wid = Number(picture(host.trees.at(-1)).id);
            const expected = editor.coreCall("binding").id === first ? 0 : -1;
            assert.equal(props.get(wid)?.playhead_at, expected);
            await editor.hideFiles();
            const pane = editor.windowWidget("pane");
            const findPane = (node: any): any => node.id === pane ? node :
                (node.children ?? []).map(findPane).find((found: unknown) => found);
            assert.equal(findPane(host.trees.at(-1)).axes.x.playhead_at, expected);
            await driver.stop();
            take.server.playing = false;
            assert.deepEqual((await playback.state()).meters, { bus: -channels!, channels });
            await driver.play();
            take.server.playing = true;
            first = editor.coreCall("binding").id;
            assert.equal((await playback.state()).meters.channels, channels);
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
    assert.throws(() => editor.close(), /previous server instance/);
    assert.equal(take.server.sent.length, sent);
    assert.notEqual(previousIds, take.server.ids);
});


test("the window reads its actions from the file in focus", async () => {
    const [editor, host, wid, context] = await opened(new FakeBuffer());
    const window = (editor as any).windowId as number;
    const drawn = host.trees[0] as unknown as { actions: Record<string, { enabled: boolean }> };
    assert.equal(drawn.actions.undo.enabled, false, "drawn with its actions");
    assert.equal(editor.actions.copy.enabled, false);
    assert.equal(editor.actions.copy.reason, "nothing is selected");
    assert.equal(editor.actions.view_wave.checked, true);
    editor.apply("/gui_event", [wid, 1, context.version, "selection", 10, 20]);
    await settle();
    const published = () => host.props.get(window)!.actions as Record<string, any>;
    assert.equal(published().copy.enabled, true, "the sweep is read");
    editor.apply("/gui_event", [wid, 2, context.version, "draw", 0, 10, [0.5], [0.0]]);
    await settle();
    const undo = published().undo;
    assert.equal(undo.enabled, true);
    assert.deepEqual(undo.target, {
        kind: "document", member: (editor as any).member, object: editor.coreCall("binding").id,
    });
    editor.close();
});

test("a second editor never walks the first one's edits", async () => {
    const [first, , wid, context] = await opened(new FakeBuffer());
    first.apply("/gui_event", [wid, 1, context.version, "draw", 0, 10, [0.5], [0.0]]);
    await settle();
    const before = first.parts;
    const second = new AudioEditor(new FakeBuffer() as never, { sampleRate: SR, context });
    await second.open(new FakeHost() as unknown as GuiHost);
    await settle();
    assert.equal(second.canUndo, false);
    assert.equal(second.actions.undo.enabled, false);
    assert.equal(second.undo(), false);
    assert.deepEqual(first.parts, before);
    assert.equal(first.canUndo, true);
    first.close();
    second.close();
});

test("a shared undo from one window tells it the version the step's commit left", async () => {
    // The step restores the other window's file, on that editor's queue; told
    // where the step left the context before it committed, this window would
    // name an old version and its next gesture would be refused as stale.
    class Told extends FakeHost {
        versions: number[] = [];
        override ack(seq: number, version = 0): void {
            this.versions.push(version);
            super.ack(seq);
        }
        override push(
            seq: number,
            sets: readonly (readonly [number, Record<string, PropValue>])[],
            version = 0,
            generations: readonly (readonly [number, number])[] = [],
            reason?: string,
        ): void {
            this.versions.push(version);
            super.push(seq, sets, version, generations, reason);
        }
    }
    const context = new Editing();
    const take = new FakeBuffer();
    const other = new FakeBuffer();
    other.bufnum = 8;
    other.server = take.server;
    const first = new AudioEditor(take as never, { sampleRate: SR, context, sharedHistory: true });
    const second = new AudioEditor(other as never, { sampleRate: SR, context, sharedHistory: true });
    const one = new Told();
    const two = new Told();
    await first.open(one as unknown as GuiHost);
    await second.open(two as unknown as GuiHost);
    await settle();
    const wid = Number(picture(two.trees.at(-1)).id);
    second.apply("/gui_event", [wid, 1, context.version, "draw", 0, 10, [0.5], [0.0]]);
    await settle();
    assert.equal(first.undo(), true);
    await settle();
    assert.equal(one.versions.at(-1), context.version);
    first.close();
    second.close();
    await settle();
});

test("shared undo updates an unfocused file without moving focus", async () => {
    const [editor, , , context] = await opened(new FakeBuffer(), { sharedHistory: true });
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


test("a copy outlives the editor it was made in and pastes elsewhere", async () => {
    const take = new FakeBuffer();
    const [first, host, wid, context] = await opened(take);
    first.apply("/gui_event", [wid, 1, context.version, "copy", 10, 20]);
    await settle();
    const copied = host.clip;
    const named = String(JSON.parse(copied).content.parts[0].path);
    assert.ok(named.startsWith("/work/"));
    first.close();
    await settle();
    assert.ok(!disk.deleted.includes(named), "the clipboard still names it");
    const [second, otherHost, otherWid, otherContext] = await opened(new FakeBuffer());
    second.apply("/gui_event", [otherWid, 1, otherContext.version, "paste", 50, "segments", copied]);
    await settle();
    assert.equal(second.buffer.frames, 120);
    assert.equal(second.parts[1]!.path, named);
    second.apply("/gui_event", [otherWid, 2, otherContext.version, "mix", 0, "segments", copied]);
    await settle();
    assert.equal(second.buffer.frames, 120);
    assert.ok(disk.renders.at(-1)!.path.endsWith("-mixed.w64"));
    second.apply("/gui_event", [otherWid, 3, otherContext.version, "copy", 0, 3]);
    await settle();
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

test("a paste of text puts nothing in and clears the copy", async () => {
    const take = new FakeBuffer();
    const [editor, host, wid, context] = await opened(take);
    editor.apply("/gui_event", [wid, 1, context.version, "copy", 0, 4]);
    await settle();
    const copied = host.clip;
    editor.apply("/gui_event", [wid, 2, context.version, "paste", 0, "text", JSON.stringify({ content: { kind: "text", text: "external" } })]);
    await settle();
    assert.equal(editor.buffer.frames, 100);
    assert.equal(editor.canUndo, false);
    editor.apply("/gui_event", [wid, 3, context.version, "paste", 0, "segments", copied]);
    await settle();
    assert.equal(editor.buffer.frames, 104, "a copy pasted again is still the copy");
    editor.close();
    await settle();
});

test("a failed cut keeps the take", async () => {
    const take = new FakeBuffer();
    const [editor, , wid, context] = await opened(take);
    const request = take.server.request.bind(take.server);
    take.server.request = (addr, args) => addr === "/buffer_stitch"
        ? Promise.resolve({ addr: "/fail", args: [addr, "stitch failed"] })
        : request(addr, args);
    editor.apply("/gui_event", [wid, 2, context.version, "cut", 10, 20]);
    await settle();
    assert.equal(editor.buffer.frames, 100);
    assert.equal(editor.canUndo, false);
    take.server.request = request;
    // the join over what was there is stitched again before anything else
    await editor.retryPending();
    editor.apply("/gui_event", [wid, 3, context.version, "cut", 10, 20]);
    await settle();
    assert.equal(editor.buffer.frames, 80);
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
    disk.refuse = (path) => path === "/takes/refused.wav";
    await assert.rejects(editor.save({ path: "/takes/refused.wav" }), /quota/);
    assert.equal(editor.file!.path, "/takes/saved.wav");
    assert.equal(editor.dirty, true);
    disk.refuse = null;
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

test("a new file whose render fails is not added and its buffer returns", async () => {
    const take = new FakeBuffer();
    const [editor] = await opened(take);
    disk.refuse = (path) => path.endsWith("-new.w64");
    const before = editor.files.length;
    await assert.rejects(editor.newFile(1, 50, SR), /quota/);
    await settle();
    assert.equal(editor.files.length, before);
    assert.equal(take.server.buffers.freed.length, new Set(take.server.buffers.freed).size);
    disk.refuse = null;
    await editor.newFile(1, 50, SR);
    assert.equal(editor.files.length, before + 1);
    editor.close();
    await settle();
});

test("failed repair is discoverable and retryable without repeating the edit", async () => {
    const take = new FakeBuffer();
    const [editor, host, wid, context] = await opened(take);
    const before = stems(editor);
    const version = context.version;
    const request = take.server.request.bind(take.server);
    take.server.request = (addr, args) => addr === "/buffer_stitch"
        ? Promise.resolve({ addr: "/fail", args: [addr, "temporary failure"] }) : request(addr, args);
    // Execute through the same operation adapter without the GUI's error sink.
    await assert.rejects(editor.operation({ kind: "edit", event: {
        addr: "/gui_event", args: [wid, 1, version, "draw", 0, 10, [0.5], [0.0]],
    } }), /recovery is pending/);
    const pending = context.serviceCall<{ repairs: { running: boolean }[] }>("operation", { action: "pendingRepairs", member: (editor as any).member }).repairs;
    assert.equal(pending.length, 1);
    assert.equal(pending[0]!.running, false);
    await assert.rejects(editor.retryPending(), /recovery is pending/);
    assert.deepEqual(stems(editor), before);
    assert.equal(context.version, version);
    const echoes = host.acks.length;
    take.server.request = request;
    await editor.retryPending();
    await settle();
    assert.equal(host.acks.length, echoes);
    assert.deepEqual(stems(editor), before);
    assert.equal(context.version, version);
    assert.equal(editor.canUndo, false);
    assert.deepEqual(context.serviceCall("operation", { action: "pendingRepairs", member: (editor as any).member }).repairs, []);
    const freed = [...take.server.buffers.freed];
    await editor.retryPending();
    await settle();
    assert.deepEqual(take.server.buffers.freed, freed);
    assert.equal(freed.length, new Set(freed).size);
    editor.apply("/gui_event", [wid, 2, version, "draw", 0, 10, [0.5], [0.0]]);
    await settle();
    assert.equal(await editor.save({ path: "/takes/recovered.wav" }), "/takes/recovered.wav");
    await editor.closeFile();
    await settle();
});

test("envelope visibility changed during an edit survives confirmation and history", async () => {
    const take = new FakeBuffer();
    const [editor, _host, wid, context] = await opened(take);
    const request = take.server.request.bind(take.server);
    let shown = false;
    take.server.request = (addr, args) => {
        if (addr === "/buffer_stitch" && !shown) {
            shown = true;
            editor.showEnvelope();
        }
        return request(addr, args);
    };
    const pane = (node: any): any => node.type === "pane" ? node
        : (node.children ?? []).map(pane).find((found: unknown) => found != null);
    editor.apply("/gui_event", [wid, 1, context.version, "draw", 0, 10, [0.5], [0.0]]);
    await settle();
    for (const step of [null, () => editor.undo(), () => editor.redo()]) {
        if (step !== null) {
            assert.equal(step(), true);
            await settle();
        }
        assert.equal(pane(editor.draw()).hidden, "");
        assert.equal(editor.layer, "points");
    }
    editor.close();
    await settle();
});

test("an editor opens with no file and its first file comes into focus", async () => {
    const server = new FakeServer();
    const editor = new AudioEditor(null, { server: server as never, context: new Editing() });
    const host = new FakeHost();
    await editor.open(host as unknown as GuiHost);
    await settle();
    const tree = host.trees.at(-1)!;
    assert.equal(picture(tree).type, "label", "where the take would be, how to open one");
    assert.ok("menu" in tree, "the window keeps its chrome");
    assert.equal(editor.file, null);
    assert.deepEqual(editor.files, []);
    assert.throws(() => editor.parts, /no file is open/);
    await assert.rejects(editor.save({ path: "/takes/x.wav" }), /no file is open/);
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
        await settle();
        assert.equal(editor.file!.path, "/takes/b.wav");
        assert.equal(picture(host.trees.at(-1)).type, "signal", "the file is drawn");
        assert.equal(editor.parts.length, 1);
        await editor.closeFile();
        await settle();
        assert.equal(editor.closed, false);
        assert.equal(editor.file, null);
        assert.equal(picture(host.trees.at(-1)).type, "label", "drawn again, empty");
    } finally {
        SoundFile.prototype.toBuffer = originalRead;
        SoundFile.open = originalOpen;
    }
});

test("closing a file keeps the meter and corrects only what is drawn", async () => {
    const nodes = (node: GuiNode): GuiNode[] => [node, ...(node.children ?? []).flatMap(nodes)];
    const meters = (tree: GuiNode) => nodes(tree).filter((n) => n.type === "meter");
    class Watch extends FakeHost {
        absent: [number, Record<string, unknown>][] = [];
        override set(id: number, props: Record<string, unknown>): void {
            // the window itself is the host's, everything else is drawn
            if (id < 900 && !nodes(this.trees.at(-1)!).some((n) => n.id === id)) {
                this.absent.push([id, props]);
            }
            super.set(id, props);
        }
    }
    const editor = new AudioEditor(new FakeBuffer() as never, { sampleRate: SR, context: new Editing() });
    const host = new Watch();
    await editor.open(host as unknown as GuiHost);
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
        await editor.openFile(["/takes/a.wav", "/takes/b.wav"]);
        await settle();
        assert.ok(meters(host.trees.at(-1)!).length > 0);
        await editor.closeFile();
        await settle();
        assert.ok(meters(host.trees.at(-1)!).length > 0, "the meter follows the file viewed now");
        await editor.closeFile();
        await editor.closeFile();
        await settle();
        assert.equal(meters(host.trees.at(-1)!).length, 0, "an empty window draws no meter");
        await editor.openFile("/takes/c.wav");
        await settle();
        assert.ok(meters(host.trees.at(-1)!).length > 0);
        assert.deepEqual(host.absent, [], "nothing addressed to a widget the window does not draw");
    } finally {
        SoundFile.prototype.toBuffer = originalRead;
        SoundFile.open = originalOpen;
    }
});

test("the shared trace is what the coordinator answers this page", async () => {
    const { readFileSync } = await import("node:fs");
    const expected = (JSON.parse(readFileSync(new URL("./audio-trace-vectors.json", import.meta.url), "utf8")) as {
        trace: [string, string[]][];
    }).trace;
    const originalRead = SoundFile.prototype.toBuffer;
    const originalOpen = SoundFile.open;
    SoundFile.prototype.toBuffer = async function({ server } = {}) {
        const endpoint = server as unknown as FakeServer;
        return { bufnum: endpoint.buffers.alloc(), frames: this.frames, channels: this.channels,
            sampleRate: this.sampleRate, server: endpoint, path: this.path } as never;
    };
    SoundFile.open = async (path: string) => new SoundFile(path, 200, 2, SR);
    try {
        const take = new FakeBuffer(100, 1);
        const editor = new AudioEditor(take as never, { sampleRate: SR, context: new Editing() });
        await settle();
        const trace: [string, string[]][] = [];
        const inner = editor as unknown as {
            effect(entry: Record<string, unknown>, held?: unknown): Promise<void>;
            answerAwait(entry: Record<string, unknown>): Promise<Record<string, unknown>>;
        };
        const effect = inner.effect.bind(editor);
        const wait = inner.answerAwait.bind(editor);
        inner.effect = (entry, held) => {
            trace.at(-1)![1].push(String(entry.kind));
            return effect(entry, held);
        };
        inner.answerAwait = (entry) => {
            trace.at(-1)![1].push(`await:${String(entry.kind)}` + (entry.via !== undefined ? `:${String(entry.via)}` : ""));
            return wait(entry);
        };
        const host = new FakeHost();
        const step = async (name: string, act: () => unknown) => {
            trace.push([name, []]);
            await act();
            await settle();
        };
        await step("open", () => editor.open(host as unknown as GuiHost));
        const window = Number((editor as unknown as { windowId: number }).windowId);
        await step("space bar, stopped", () => editor.apply("/gui_event", [window, 1, 0, "play", 0]));
        take.server.playing = true;
        await step("space bar, rolling", () => editor.apply("/gui_event", [window, 2, 0, "play", 0]));
        take.server.playing = false;
        const driver = (editor.transport as unknown as { driver: { setSpan(s: [number, number]): Promise<void>; setLooping(on: boolean): Promise<void> } }).driver;
        await step("span", () => driver.setSpan([10 / SR, 40 / SR]));
        await step("loop", () => driver.setLooping(true));
        const wid = Number(picture(host.trees.at(-1)).id);
        await step("cut", () => editor.apply("/gui_event", [wid, 3, (editor as unknown as { editing: Editing }).editing.version, "cut", 10.0, 20.0]));
        await step("undo from the window", () => editor.apply("/gui_event", [window, 4, 0, "undo"]));
        await step("save", () => editor.save({ path: "/takes/a.wav" }));
        await step("open a file", () => editor.openFile(["/takes/b.wav"]));
        await step("focus the first", () => editor.focusFile(0));
        await step("close the focused", () => editor.closeFile());
        await step("both views", () => editor.viewBoth());
        await step("close", () => editor.close());
        assert.deepEqual(trace, expected,
            "the two clients drive the coordinator differently (or regenerate gen-audio-trace-vectors.py)");
    } finally {
        SoundFile.prototype.toBuffer = originalRead;
        SoundFile.open = originalOpen;
    }
});

/** The first node of a tree `wanted` says yes to. */
function findNode(node: any, wanted: (node: any) => boolean): any {
    if (wanted(node)) return node;
    for (const child of node.children ?? []) {
        const found = findNode(child, wanted);
        if (found !== undefined) return found;
    }
    return undefined;
}

/**
 * **A server that refuses, and one that is slow**: each command named in
 * `fail` is answered once with a `/fail`, and while the first command named in
 * `slow` is out, its action runs before the reply comes -- what a hand does in
 * the window while a write is in flight. `"render"` names an offline render,
 * which {@link FaultyServer.render} carries out.
 */
class FaultyServer extends FakeServer {
    fail = new Map<string, string>();
    slow = new Map<string, () => unknown>();

    render = async (_render: RenderStep): Promise<void> => {
        const refused = this.fail.get("render");
        if (refused !== undefined) {
            this.fail.delete("render");
            throw new Error(refused);
        }
        const meanwhile = this.slow.get("render");
        if (meanwhile !== undefined) {
            this.slow.delete("render");
            meanwhile();
        }
    };

    override request(addr: string, args: unknown[]): Promise<{ addr: string; args: unknown[] }> {
        const refused = this.fail.get(addr);
        if (refused !== undefined) {
            this.fail.delete(addr);
            this.sendMsg(addr, ...args);
            return Promise.resolve({ addr: "/fail", args: [addr, refused] });
        }
        const meanwhile = this.slow.get(addr);
        if (meanwhile !== undefined) {
            this.slow.delete(addr);
            this.sendMsg(addr, ...args);
            meanwhile();
            const first = Array.isArray(args[0]) ? Number(args[0][1]) : Number(args[0]);
            return Promise.resolve({ addr: "/done", args: [addr, first] });
        }
        return super.request(addr, args);
    }
}

test("the shared fault trace is what the coordinator answers this page", async () => {
    const { readFileSync } = await import("node:fs");
    const expected = (JSON.parse(readFileSync(new URL("./audio-trace-vectors.json", import.meta.url), "utf8")) as {
        faults: [string, string[] | null, Record<string, unknown>][];
    }).faults;
    const originalRead = SoundFile.prototype.toBuffer;
    const originalOpen = SoundFile.open;
    SoundFile.prototype.toBuffer = async function({ server } = {}) {
        const endpoint = server as unknown as FakeServer;
        return { bufnum: endpoint.buffers.alloc(), frames: this.frames, channels: this.channels,
            sampleRate: this.sampleRate, server: endpoint, path: this.path } as never;
    };
    SoundFile.open = async (path: string) => new SoundFile(path, 200, 2, SR);
    try {
        const take = new FakeBuffer(100, 1);
        take.path = "/takes/a.wav";
        const server = new FaultyServer();
        take.server = server;
        endpoint.render = server.render;
        const editor = new AudioEditor(take as never, {
            sampleRate: SR, context: new Editing(), file: new SoundFile("/takes/a.wav", 100, 1, SR),
        });
        await settle();
        const trace: [string, string[] | null, Record<string, unknown>][] = [];
        const inner = editor as unknown as {
            effect(entry: Record<string, unknown>, held?: unknown): Promise<void>;
            answerAwait(entry: Record<string, unknown>): Promise<Record<string, unknown>>;
            windowId: number;
            editing: Editing;
        };
        const effect = inner.effect.bind(editor);
        const wait = inner.answerAwait.bind(editor);
        inner.effect = (entry, held) => {
            trace.at(-1)![1]?.push(String(entry.kind));
            return effect(entry, held);
        };
        inner.answerAwait = (entry) => {
            trace.at(-1)![1]?.push(`await:${String(entry.kind)}` + (entry.via !== undefined ? `:${String(entry.via)}` : ""));
            return wait(entry);
        };
        const host = new FakeHost();
        const state = (): Record<string, unknown> => {
            const file = editor.file;
            return {
                path: file === null ? null : file.path, frames: file === null ? null : file.frames,
                dirty: editor.dirty, files: editor.files.length, undo: editor.canUndo, closed: editor.closed,
            };
        };
        const category = (error: unknown): string =>
            error instanceof CommandError ? "command" : error instanceof RangeError ? "value" : "runtime";
        const step = async (name: string, act: () => unknown, concurrent = false) => {
            trace.push([name, concurrent ? null : [], {}]);
            let raised: string | null = null;
            try {
                await act();
                // a hand's gesture fails in the work it queued, which a page awaits
                await editor.idle();
            } catch (error) {
                raised = category(error);
            }
            await settle();
            trace.at(-1)![2] = { ...state(), raised };
        };
        const cut = (at = 10.0) => {
            const wid = Number(picture(host.trees.at(-1)).id);
            editor.apply("/gui_event", [wid, trace.length, inner.editing.version, "cut", at, 20.0]);
        };
        const window = (verb: string) => editor.apply("/gui_event", [inner.windowId, trace.length, 0, verb]);
        /** A file chosen in the window's list, by the hand. */
        const pick = (index: number) => {
            const listed = findNode(host.trees.at(-1), (node) => node.rows !== undefined);
            editor.apply("/gui_event", [Number(listed.id), trace.length, 0, "select", index]);
        };
        const slow = (addr: string, meanwhile: () => unknown, act: () => unknown) => {
            server.slow.set(addr, meanwhile);
            return act();
        };
        await step("open", () => editor.open(host as unknown as GuiHost));
        await step("cut", () => cut());
        server.fail.set("render", "no space left on device");
        await step("a refused save", () => editor.save());
        await step("the save retried", () => editor.save());
        await step("a cut while the write is out", () => slow("render", () => cut(), () => editor.save()), true);
        await step("undo to what was written", () => window("undo"));
        server.fail.set("/buffer_stitch", "stitch refused");
        await step("a cut whose stitch is refused", () => cut());
        await step("open another file", () => editor.openFile(["/takes/b.wav"]));
        await step("back to the first", () => editor.focusFile(0));
        await step("the second cut while the first is written",
            () => slow("render", () => { pick(1); cut(); }, () => editor.save()), true);
        await step("back to the first again", () => editor.focusFile(0));
        await step("the window closed while a write is out",
            () => slow("render", () => editor.close(), () => editor.save({ path: "/takes/c.wav" })), true);
        assert.deepEqual(trace, expected,
            "the two clients drive the coordinator differently (or regenerate gen-audio-trace-vectors.py)");
    } finally {
        SoundFile.prototype.toBuffer = originalRead;
        SoundFile.open = originalOpen;
    }
});
