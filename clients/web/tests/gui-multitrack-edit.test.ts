// `MultitrackEditor`: the handler of the multitrack the GUI host holds, over
// a saved session or over a multitrack handed to it whole, with the same
// cases as the Python client's `tests/test_gui_multitrack_edit.py`.
//
// What is checked is this client's half: which call each member makes
// through the application door (`GuiHost.openApp`, `GuiHost.callApp`), how
// it reads the answer, and the page's multitrack forwarding its changes to
// the window's while it is open. What the application does with a call is
// the host's and is tested there (`clients/gui/src/host/audio_editor.rs`).

import assert from "node:assert/strict";
import test from "node:test";

import { MultitrackEditor, edit } from "../src/gui/editing/index.ts";
import type { GuiHost } from "../src/gui/host.ts";
import { Multitrack, Region, Session as SavedSession } from "../src/multitrack.ts";
import { loadCore } from "../src/base/core.ts";

await loadCore();

type Call = Record<string, any>;

/**
 * A host that answers the door from a table, and records the calls. Its
 * window holds a multitrack of its own, which a `handle` call reads and edits
 * as the application's does.
 */
class FakeHost {
    calls: (["open", string, Record<string, unknown>] | ["call", number, Call])[] = [];
    verbs: [number, string][] = [];
    open = new Set<number>();
    held: Multitrack | null = null;
    path: string | null = null;
    snapshot: unknown = null;
    recorded: [string | undefined, unknown[]][] = [];
    closedHandler: (() => void) | null = null;
    left: unknown = null;
    answers: Record<string, unknown> = {
        playing: true,
        save: { destination: "/songs/song.json" },
        output: { tracks: { "1": [0.5] }, starved: 0 },
        history: { canUndo: true, canRedo: false, undoLabel: "rename", redoLabel: null },
    };

    constructor(answers: Record<string, unknown> = {}) {
        Object.assign(this.answers, answers);
    }

    async openApp(app: string, options: Record<string, unknown> = {}): Promise<number> {
        this.calls.push(["open", app, { ...options }]);
        const snapshot = options.snapshot as Call | undefined;
        this.bind(snapshot !== undefined ? snapshot.session.multitrack ?? {} :
            "path" in options || "multitrack" in options ? options.multitrack ?? {} : null,
            snapshot?.path ?? options.path ?? null);
        this.open.add(9);
        return 9;
    }

    async callApp(window: number | { id: number }, call: Call): Promise<unknown> {
        const id = typeof window === "number" ? window : window.id;
        this.calls.push(["call", id, call]);
        if (call.query !== undefined) return this.answers[call.query];
        const doing = call.do as Call;
        if (doing.verb === "handle") {
            const { verb, ...args } = doing.call as Call;
            if (verb === "record") {
                this.recorded.push([doing.label, args.legs as unknown[]]);
                return true;
            }
            const answer = this.held!.call(verb, args);
            if (verb === "apply" && doing.record !== false) this.recorded.push([doing.label, [args.intent]]);
            return answer;
        }
        if (doing.verb === "close") {
            this.closeWindow();
            return false;
        }
        if (doing.verb === "new_file" || doing.verb === "close_file" || (doing.verb === "open" && doing.session !== undefined)) {
            // the window lets go of its session, as the host says it
            const path = (doing.session as string | undefined) ?? null;
            this.release(doing.verb === "close_file" ? null : (new Multitrack().write() as Call), path);
            return null;
        }
        return this.answers[doing.verb];
    }

    /** The session the window shows, as the application holds it. */
    bind(written: Call | null, path: unknown = null): void {
        this.path = path === null ? null : String(path);
        this.held = written === null ? null : Multitrack.read(written);
    }

    /** What the window hands back of its session: a `/gui_app_open` snapshot. */
    snapshotOf(): unknown {
        if (this.held === null) return null;
        return { session: { format: new SavedSession().format, multitrack: this.held.write(), sources: {} }, path: this.path, saved: null };
    }

    /** The window goes, as the host says it: what it left the multitrack as rides with the close. */
    closeWindow(): void {
        this.left = this.held?.write() ?? null;
        this.snapshot = this.snapshotOf();
        this.open.delete(9);
        this.closedHandler?.();
    }

    /** The hand put another session in the window (`/gui_app_released`): its opener is handed the first, and the window goes on. */
    release(written: Call | null = null, path: string | null = null): void {
        const snapshot = this.snapshotOf();
        this.bind(written, path);
        for (const listen of [...this.listeners]) listen({ addr: "/gui_app_released", args: [9, JSON.stringify(snapshot)] });
    }

    closedSnapshot(): unknown {
        const snapshot = this.snapshot;
        this.snapshot = null;
        return snapshot;
    }

    closedWith(): unknown {
        const left = this.left;
        this.left = null;
        return left;
    }

    verb(window: number | { id: number }, verb: string): void {
        this.verbs.push([typeof window === "number" ? window : window.id, verb]);
    }

    isOpen(id: number): boolean {
        return this.open.has(id);
    }

    async waitWhile(pred: () => boolean): Promise<boolean> {
        return !pred();
    }

    /** The page's subscribers, which the host's own messages reach. */
    listeners = new Set<(msg: { addr: string; args: unknown[] }) => void>();

    onMessage(handler: (msg: { addr: string; args: unknown[] }) => void): () => void {
        this.listeners.add(handler);
        return () => this.listeners.delete(handler);
    }

    /** What the host tells the page a hand did in its window. */
    handChanged(): void {
        for (const listen of [...this.listeners]) listen({ addr: "/gui_app_changed", args: [9, 2n] });
    }

    setClosedHandler(_id: number, handler: (() => void) | null): void {
        this.closedHandler = handler;
    }

    last(): Call {
        return this.calls[this.calls.length - 1]![2] as Call;
    }
}

function aSession(path = "/songs/song.json"): SavedSession {
    const session = new SavedSession();
    session.path = path;
    return session;
}

test("edit opens a saved session in the multitrack application", async () => {
    const host = new FakeHost();
    const editor = await edit(aSession(), { host: host as unknown as GuiHost });
    assert.ok(editor instanceof MultitrackEditor);
    assert.deepEqual(host.calls[0], ["open", "multitrack", { title: "song.json", path: "/songs/song.json" }]);
    assert.ok(editor.window !== null && !editor.closed);
    assert.equal(await edit(aSession(), { host: host as unknown as GuiHost }), editor, "a second edit finds it");
});

test("a session with no file is refused", () => {
    assert.throws(() => new MultitrackEditor(new SavedSession()), /save it first/);
});

test("a multitrack opens as an untitled session", async () => {
    const host = new FakeHost();
    const multitrack = new Multitrack();
    multitrack.tracks.add("drums");
    const editor = await edit(multitrack, { host: host as unknown as GuiHost });
    assert.ok(editor instanceof MultitrackEditor);
    const [, app, options] = host.calls[0] as ["open", string, Record<string, any>];
    assert.equal(app, "multitrack");
    assert.equal(options.title, "Untitled");
    assert.equal(options.multitrack.tracks[0].name, "drums");
    assert.equal(editor.path, null);
    assert.equal(editor.session, null);
    assert.equal(await edit(multitrack, { host: host as unknown as GuiHost }), editor, "a second edit finds it");
});

test("the page's multitrack is the window's while it is open", async () => {
    const host = new FakeHost();
    const multitrack = new Multitrack();
    const track = multitrack.tracks.add("a");
    const editor = await edit(multitrack, { host: host as unknown as GuiHost });
    // a change is an edit of the window's, recorded under its label
    track.name = "b";
    assert.equal(track.name, "b", "the page's copy holds it at once");
    await editor.output();
    assert.equal((host.held!.call("track", { id: track.ident }) as Call).name, "b");
    assert.notEqual(host.recorded[host.recorded.length - 1]![0], undefined);
    // a block is one entry, recorded when it ends
    multitrack.history.entry("tidy", () => {
        track.muted = true;
        track.name = "c";
    });
    await editor.output();
    const [label, legs] = host.recorded[host.recorded.length - 1]!;
    assert.equal(label, "tidy");
    assert.equal(legs.length, 2);
    const history = multitrack.history as { undoLabel(): Promise<string | null>; canUndo(): Promise<boolean> };
    assert.equal(await history.undoLabel(), "rename");
    assert.equal(await history.canUndo(), true);
    // the window goes, and the multitrack holds what it left it as
    const tracks = (host.held!.write().tracks as Call[]).map((t) => ({ ...t, name: "last" }));
    host.held!.call("apply", { intent: { intent: "settracks", tracks } });
    host.closeWindow();
    assert.ok(editor.closed);
    assert.equal(track.name, "last");
    track.name = "mine again";
    assert.equal(track.name, "mine again", "and it is the page's again");
});

test("what a hand does in the window reaches the page's multitrack", async () => {
    const host = new FakeHost();
    const multitrack = new Multitrack();
    const track = multitrack.tracks.add("a");
    await edit(multitrack, { host: host as unknown as GuiHost });
    // a hand renames the track in the window
    const tracks = (host.held!.write().tracks as Call[]).map((t) => ({ ...t, name: "by hand" }));
    host.held!.call("apply", { intent: { intent: "settracks", tracks } });
    assert.equal(track.name, "a", "the page's copy until it is told");
    host.handChanged();
    const editor = (await edit(multitrack, { host: host as unknown as GuiHost })) as MultitrackEditor;
    await editor.output();
    assert.equal(track.name, "by hand", "told, the copy follows the window");
    host.closeWindow();
    assert.equal(host.listeners.size, 0, "a closed window is not listened to");
});

test("the transport and the save are calls to the application", async () => {
    const host = new FakeHost();
    const editor = new MultitrackEditor(aSession("/songs/other.json"), { title: "mix" });
    await editor.open(host as unknown as GuiHost);
    await editor.transport.play(1.0, 2.0, { looping: true });
    assert.deepEqual(host.last(), { do: { verb: "play", looping: true, range: [1.0, 2.0] } });
    await editor.transport.stop(0.5);
    assert.deepEqual(host.last(), { do: { verb: "stop", mark: 0.5 } });
    await editor.transport.locate(3.0);
    assert.deepEqual(host.last(), { do: { verb: "locate", at: 3.0 } });
    assert.equal(await editor.transport.playing(), true);
    assert.equal((await editor.output()).starved, 0);
    assert.equal(await editor.save("/songs/song.json"), "/songs/song.json");
    assert.deepEqual(host.last(), { do: { verb: "save", path: "/songs/song.json" } });
    assert.ok(editor.undo());
    assert.deepEqual(host.verbs[host.verbs.length - 1], [9, "undo"]);
    assert.ok(editor.split());
    assert.deepEqual(host.verbs[host.verbs.length - 1], [9, "split"]);
});

test("the grid is the window's", async () => {
    const host = new FakeHost({ verb: { grid: 0.5, snap: true, view: "roll", set: true } });
    const editor = await edit(aSession(), { host: host as unknown as GuiHost });
    assert.equal(await editor.grid(), 0.5);
    assert.equal(await editor.snapToGrid(), true);
    await editor.setGrid(0.25);
    assert.deepEqual(host.last(), { do: { verb: "verb", call: { verb: "sync", grid: 0.25 } } });
    await editor.setSnapToGrid(false);
    assert.deepEqual(host.last(), { do: { verb: "verb", call: { verb: "sync", snap: false } } });
    assert.equal(await editor.notesView(), "roll");
    await editor.setNotesView("score");
    assert.deepEqual(host.last(), { do: { verb: "verb", call: { verb: "notesView", view: "score" } } });
});

test("what the hand holds is the window's", async () => {
    const host = new FakeHost({ selected: { selected: [] } });
    const multitrack = new Multitrack();
    const editor = await edit(multitrack, { host: host as unknown as GuiHost });
    assert.deepEqual(await editor.selected(), []);
    host.answers.selected = { selected: [7] };
    const [region] = await editor.selected();
    assert.ok(region instanceof Region && region.ident === 7);
    assert.deepEqual(await editor.selected(), [region]);
    assert.deepEqual(host.last(), { query: "selected" });
    await editor.select([region!]);
    assert.deepEqual(host.last(), { do: { verb: "select", regions: [7] } });
    await editor.unselect();
    assert.deepEqual(host.last(), { do: { verb: "select", regions: [] } });
    host.closeWindow();
    assert.deepEqual(await editor.selected(), [], "nothing is marked with no window open");
});


test("a window that lets go of the session hands the multitrack back", async () => {
    const host = new FakeHost();
    const original = new Multitrack();
    const track = original.tracks.add("original");
    const editor = await edit(original, { host: host as unknown as GuiHost });
    track.name = "edited in the window";
    await editor.output();
    const replacement = new Multitrack();
    replacement.tracks.add("replacement");
    host.release(replacement.write(), "/songs/next.json");
    assert.ok(editor.closed, "the handler is done with the window");
    assert.equal(editor.multitrack, original);
    assert.equal(track.name, "edited in the window", "what the window left it as");
    const asked = host.calls.length;
    track.name = "local again";
    await Promise.resolve();
    assert.equal(host.calls.length, asked, "the multitrack is the page's again");
    assert.equal([...host.held!.tracks][0]!.name, "replacement");
});

test("an editor can open without a session", async () => {
    const host = new FakeHost();
    const editor = new MultitrackEditor();
    await editor.open(host as unknown as GuiHost);
    assert.deepEqual(host.calls[0], ["open", "multitrack", { title: "Multitrack" }]);
    assert.equal(editor.multitrack, null);
    assert.equal(editor.path, null);
    host.closeWindow();
    assert.ok(editor.closed);
    assert.equal(editor.multitrack, null);
});

test("reopening keeps the same object and its local changes", async () => {
    const host = new FakeHost();
    const original = new Multitrack();
    const track = original.tracks.add("original");
    const editor = await edit(original, { host: host as unknown as GuiHost });
    host.closeWindow();
    track.name = "after close";
    await editor.open(host as unknown as GuiHost);
    assert.equal(editor.multitrack, original);
    assert.equal(track.name, "after close");
    assert.ok(host.calls.filter(call => call[0] === "open").at(-1)![2].snapshot);
    host.closeWindow();
    assert.equal(track.name, "after close");
});

test("the window's file verbs hand the session back", async () => {
    const host = new FakeHost();
    const original = new Multitrack();
    original.tracks.add("original");
    const editor = (await edit(original, { host: host as unknown as GuiHost })) as MultitrackEditor;
    await editor.newFile();
    assert.deepEqual(host.last(), { do: { verb: "new_file" } });
    assert.ok(editor.closed);
    assert.equal(editor.multitrack, original);
    await editor.open(host as unknown as GuiHost);
    await editor.openFile("/songs/next.json");
    assert.deepEqual(host.last(), { do: { verb: "open", session: "/songs/next.json" } });
    assert.ok(editor.closed);
    await editor.open(host as unknown as GuiHost);
    await editor.closeFile();
    assert.deepEqual(host.last(), { do: { verb: "close_file" } });
    assert.ok(editor.closed);
    assert.equal(host.held, null);
});
