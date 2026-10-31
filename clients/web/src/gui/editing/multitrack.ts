/**
 * The **multitrack**: its tracks read from their files and played by the
 * application the GUI host holds, and this page its handler.
 *
 * The multitrack is one program, and it is not this one. The GUI host opens
 * it ({@link GuiHost.openApp}) in a window of the host's, on an engine of the
 * application's own: over a saved session, whose takes are read where they
 * are -- their headers when it opens, their samples as the transport reads
 * them, their peaks, kept in the session's `peaks/` folder, as the window
 * draws them -- or over a {@link Multitrack} handed to it whole, an
 * **untitled session** with no file behind it until it is saved. What a
 * gesture does, the history, the playback and the save all run in the host's
 * process. Every member below is one call to the application
 * ({@link GuiHost.callApp}), and what it answers is read from there each time
 * rather than kept here. A read is therefore a promise here where the
 * reference client's is a property.
 *
 * **The page's multitrack is the window's while it is open.** A change made
 * through the {@link Multitrack} it was opened over -- the session's, or the
 * one handed over -- is an edit of the window's, undone by its Ctrl+Z, in the
 * order the page made them, and `multitrack.history` is the window's
 * history. When the window goes, the multitrack holds what the window left
 * it as. **The multitrack's objects answer at once from a copy this page
 * holds**, since a call to the host is a promise: the copy is brought to the
 * window's after each of the page's own changes and steps, and after what a
 * hand does in the window, which the host tells the page
 * (`/gui_app_changed`). The reference client reads the window's at every
 * read.
 *
 * A box of notes plays the sequence the session holds for it, and a double
 * click on one opens its roll in the same application, one history with the
 * multitrack.
 *
 * @module
 */

import { Multitrack, Region, Session } from "../../multitrack.ts";
import type { Extra, WindowHistory, WindowOver } from "../../multitrack.ts";
import type { GuiHost, Stage } from "../host.ts";
import type { WindowHandle } from "../handle.ts";
import { Marking, marking } from "./marking.ts";

/** **The multitrack's transport**, as a script plays it: each verb one call to the application, in the multitrack's seconds. */
export class MultitrackTransport {
    private readonly editor: MultitrackEditor;

    constructor(editor: MultitrackEditor) {
        this.editor = editor;
    }

    /** Whether the transport rolls. */
    async playing(): Promise<boolean> {
        return Boolean(await this.editor.act("playing"));
    }

    /**
     * **Roll**: over `start`..`end` seconds when both are given (and over and
     * over with `looping`), else from where it stands.
     */
    async play(start: number | null = null, end: number | null = null, { looping = false }: { looping?: boolean } = {}): Promise<this> {
        const args: Record<string, unknown> = { looping: Boolean(looping) };
        if (start !== null && end !== null) args.range = [Number(start), Number(end)];
        await this.editor.act("play", args);
        return this;
    }

    /** Hold where it is; a `play` goes on from there. */
    async pause(): Promise<this> {
        await this.editor.act("pause");
        return this;
    }

    /** Stop, and go back to `mark` seconds. */
    async stop(mark = 0): Promise<this> {
        await this.editor.act("stop", { mark: Number(mark) });
        return this;
    }

    /** Stand the transport at `at` seconds. */
    async locate(at: number): Promise<this> {
        await this.editor.act("locate", { at: Number(at) });
        return this;
    }

    /** Resolves when the transport stops, or on `timeout` seconds: whether it stopped. */
    async wait(timeout?: number): Promise<boolean> {
        const deadline = timeout === undefined ? null : Date.now() + timeout * 1000;
        while (await this.playing()) {
            if (deadline !== null && Date.now() >= deadline) return false;
            await new Promise((resolve) => setTimeout(resolve, 50));
        }
        return true;
    }
}

/** What a {@link MultitrackEditor} opens over by its file: a saved session. */
export interface SavedSession {
    /** The file it was opened from or saved to, or `null` for none. */
    path: string | null;
    /** Its multitrack. */
    multitrack: Multitrack;
}

/** How a {@link MultitrackEditor} opens. */
export interface MultitrackEditorOptions {
    /** The window's title; the session file's name when not given, `"Untitled"` for a multitrack. */
    title?: string;
}

/** One leg of a page's block: the change, and the edit that puts it back. */
interface Leg {
    forward: unknown;
    backward: unknown;
}

interface Retained {
    session: Extra;
    path: string | null;
    saved: Extra | null;
}
const retained = new WeakMap<Multitrack, Retained>();

/** One session's stable object address, independent of the window binding. */
class SessionObject implements WindowOver {
    pending = 0;
    attached = true;
    private depth = 0;
    private legs: Leg[] | null = null;
    private blockLabel = "edit";

    readonly owner: MultitrackEditor;
    readonly identity: number;
    readonly multitrack: Multitrack;
    constructor(owner: MultitrackEditor, identity: number, multitrack: Multitrack) {
        this.owner = owner;
        this.identity = identity;
        this.multitrack = multitrack;
        multitrack.window = this;
    }
    get closed(): boolean { return !this.attached || this.owner.closed; }
    async query(name: string): Promise<Record<string, any>> {
        if (this.closed) throw new Error("the session view is closed");
        return (await this.owner.callFor(this, { query: name, object: this.identity })) ?? {};
    }
    async act(verb: string, args: Extra = {}): Promise<any> {
        if (this.closed) throw new Error("the session view is closed");
        return this.owner.callFor(this, { do: { verb, object: this.identity, ...args } });
    }
    forward(intent: Extra, label: string, current: unknown): void {
        if (this.legs !== null) {
            this.legs.push({ forward: intent, backward: current });
            this.owner.send(this, { verb: "apply", intent }, label, false);
        } else {
            this.owner.send(this, { verb: "apply", intent }, label, true);
        }
    }
    async follow(): Promise<void> {
        if (this.pending > 0 || this.closed) return;
        const state = await this.owner.raw({ do: { verb: "handle", object: this.identity, call: { verb: "state" } } });
        if (this.pending === 0 && !this.closed && state !== null && typeof state === "object") this.multitrack.holdWritten(state as Extra);
    }
    private async step(verb: "undo" | "redo"): Promise<boolean> {
        const stepped = Boolean(await this.act(verb));
        await this.follow();
        return stepped;
    }
    get history(): WindowHistory {
        return {
            entry: <T>(label: string, run: () => T): T => {
                this.depth += 1;
                if (this.depth === 1) { this.legs = []; this.blockLabel = String(label); }
                try { return run(); }
                finally {
                    this.depth -= 1;
                    if (this.depth === 0) {
                        const legs = this.legs ?? [];
                        this.legs = null;
                        if (legs.length > 0 && !this.closed) this.owner.send(this, { verb: "record", legs }, this.blockLabel, true);
                    }
                }
            },
            undo: () => this.step("undo"),
            redo: () => this.step("redo"),
            canUndo: async () => Boolean((await this.query("history")).canUndo),
            canRedo: async () => Boolean((await this.query("history")).canRedo),
            undoLabel: async () => ((await this.query("history")).undoLabel ?? null) as string | null,
            redoLabel: async () => ((await this.query("history")).redoLabel ?? null) as string | null,
        };
    }
}

/**
 * **The multitrack the GUI host holds, and this page its handler.**
 *
 * `edit` over a multitrack `Session` that has a file, or over a
 * {@link Multitrack}, answers one, open. Each member is a call to the
 * application in the host's process, answered by it. A session with no file
 * is refused with a `RangeError`: save it first.
 *
 * **What is marked** (`selected`, `select`, `unselect`) is the boxes the hand
 * holds -- a click, Alt+click or a marquee -- as the {@link Region} objects of
 * the multitrack, in the order the boxes are drawn. The window's, not the
 * multitrack's: it enters no history, and nothing is marked with no window
 * open. (The time range a sweep leaves is the transport's.)
 */
export class MultitrackEditor {
    /** The window's title. */
    readonly title: string;
    private readonly initialSession: SavedSession | null;
    private activeMultitrack: Multitrack | null;
    private objects = new Map<number, SessionObject>();
    private active: number | null = null;
    private snapshot: Retained | null = null;
    private settled = false;
    private filed: string | null;
    private hostOf: GuiHost | null = null;
    private handle: WindowHandle | null = null;
    private closedHandler: (() => void) | null = null;
    /** The page's changes, forwarded in the order they were made. */
    private queue: Promise<unknown> = Promise.resolve();
    /** Stops listening for what a hand does in the window. */
    private unlisten: (() => void) | null = null;
    constructor(structure: SavedSession | Multitrack | null = null, { title }: MultitrackEditorOptions = {}) {
        if (structure === null) {
            this.initialSession = null;
            this.activeMultitrack = null;
            this.filed = null;
            this.title = title !== undefined ? String(title) : "Multitrack";
        } else if (structure instanceof Multitrack) {
            this.initialSession = null;
            this.activeMultitrack = structure;
            this.filed = null;
            this.title = title !== undefined ? String(title) : "Untitled";
        } else {
            if (structure.path === null || structure.path === undefined) {
                throw new RangeError("the multitrack opens a session from its file, and this one has " +
                    "none: save it first (`session.save(path)`)");
            }
            this.initialSession = structure;
            this.activeMultitrack = structure.multitrack;
            this.filed = String(structure.path);
            this.title = title !== undefined ? String(title) : this.filed.split("/").pop()!;
        }
        this.snapshot = this.activeMultitrack === null ? null : retained.get(this.activeMultitrack) ?? null;
        if (this.snapshot !== null) this.filed = this.snapshot.path;
    }

    /** The active multitrack, or `null` with no session. Earlier objects retain their contents and history. */
    get multitrack(): Multitrack | null { return this.activeMultitrack; }

    /** The session supplied by the page, while it is the active object; otherwise `null`. */
    get session(): SavedSession | null {
        return this.initialSession?.multitrack === this.activeMultitrack ? this.initialSession : null;
    }

    // ---- the window ----

    /**
     * **Ask the host to open the multitrack**, on `host` or the ambient one,
     * and answer its window. One already open answers the window it has. From
     * then on the multitrack it was opened over is the window's (the module's
     * documentation).
     */
    async open(host?: GuiHost, { stage }: { stage?: Stage | null } = {}): Promise<WindowHandle> {
        if (this.handle !== null && !this.closed) return this.handle;
        if (this.handle !== null) { this.settle(); await this.queue; }
        const resolved = host ?? (await (await import("../../plot.ts")).resolveHost());
        const options: Record<string, unknown> = { title: this.title };
        if (this.snapshot !== null && this.activeMultitrack !== null) {
            options.snapshot = { ...this.snapshot, session: { ...this.snapshot.session, multitrack: this.activeMultitrack.write() } };
        } else if (this.filed !== null) options.path = this.filed;
        else if (this.activeMultitrack !== null) options.multitrack = this.activeMultitrack.write();
        const window = await resolved.openApp("multitrack", options, 5.0, { element: stage });
        const { WindowHandle } = await import("../handle.ts");
        this.objects = new Map();
        this.active = null;
        this.settled = false;
        this.hostOf = resolved;
        this.handle = new WindowHandle(resolved, window, new Map());
        this.handle.onClosed(() => this.closedNow());
        // **What a hand does in the window** is told (`/gui_app_changed`),
        // and the copy the page holds follows it after the page's own changes
        this.unlisten = resolved.onMessage((msg) => {
            if (msg.addr !== "/gui_app_changed" || Number(msg.args[0]) !== window) return;
            this.queue = this.queue.then(async () => {
                await this.refreshBinding();
                for (const object of this.objects.values()) await object.follow();
            });
        });
        // the page's multitrack is the window's from now on
        await this.refreshBinding(this.activeMultitrack);
        return this.handle;
    }

    /** The window went: the multitrack holds what it left it as. */
    private closedNow(): void {
        this.unlisten?.();
        this.unlisten = null;
        this.settle();
        this.closedHandler?.();
    }

    /** Read the owner's binding; a new identity creates one wrapper, never retargets an old one. */
    private async refreshBinding(seed: Multitrack | null = null): Promise<void> {
        if (this.closed) return;
        const file = (await this.raw({ query: "file" })) as Record<string, any>;
        this.filed = file.path ?? null;
        if (file.empty) { this.active = null; this.activeMultitrack = null; return; }
        const identity = Number(file.object);
        if (!this.objects.has(identity)) {
            const state = seed ?? Multitrack.read(await this.raw({ do: { verb: "handle", object: identity, call: { verb: "state" } } }));
            this.objects.set(identity, new SessionObject(this, identity, state));
        }
        this.active = identity;
        this.activeMultitrack = this.objects.get(identity)!.multitrack;
    }

    /** Return every retained object to its own snapshot when the window goes. */
    private settle(): void {
        if (this.settled || this.handle === null) return;
        const host = this.hostOf as GuiHost;
        const metadata = host.closedObjects(this.handle.id) as { active: number | null; objects: Record<string, Retained> } | null;
        let snapshots: Record<string, Retained> = {};
        if (metadata !== null) {
            snapshots = metadata.objects;
            this.active = metadata.active;
        } else {
            const left = host.closedWith(this.handle.id);
            if (left !== null && this.active !== null) snapshots[String(this.active)] = { session: { multitrack: left }, path: this.filed, saved: null };
        }
        for (const [identity, written] of Object.entries(snapshots)) {
            const id = Number(identity);
            if (!this.objects.has(id)) this.objects.set(id, new SessionObject(this, id, Multitrack.read((written.session.multitrack ?? {}) as Extra)));
        }
        for (const [identity, held] of this.objects) {
            held.attached = false;
            held.multitrack.window = null;
            const written = snapshots[String(identity)];
            if (written === undefined) continue;
            held.multitrack.holdWritten((written.session.multitrack ?? {}) as Extra);
            retained.set(held.multitrack, written);
            if (this.initialSession?.multitrack === held.multitrack) {
                Object.assign(this.initialSession, Session.read(written.session), { multitrack: held.multitrack, path: written.path });
            }
        }
        this.activeMultitrack = this.active === null ? null : this.objects.get(this.active)!.multitrack;
        this.snapshot = snapshots[String(this.active)] ?? null;
        this.filed = this.snapshot?.path ?? null;
        this.settled = true;
    }

    /** The session file it opened, or `null` for an untitled one. */
    get path(): string | null {
        return this.filed;
    }

    /** The {@link GuiHost} it is open on, or `null`. */
    get host(): GuiHost | null {
        return this.hostOf;
    }

    /** The open window's handle, or `null`. */
    get window(): WindowHandle | null {
        return this.handle;
    }

    /** Whether the window is gone. */
    get closed(): boolean {
        return this.handle === null || this.handle.closed;
    }

    /** Resolves when the window is closed, or on `timeout` seconds: `true` for the first. */
    async wait(timeout?: number): Promise<boolean> {
        if (this.handle === null) return true;
        const done = await this.handle.wait(timeout);
        if (done) this.settle();
        return done;
    }

    /** Call `handler()` when the window is closed. `null` clears it. */
    onClosed(handler: (() => void) | null): this {
        this.opened();
        this.closedHandler = handler;
        return this;
    }

    /** Close the window, and what plays it. */
    async close(): Promise<this> {
        if (!this.closed) {
            await this.act("close");
            await this.wait(5);
        }
        this.settle();
        return this;
    }

    private opened(): WindowHandle {
        if (this.handle === null) throw new Error("open() the multitrack first");
        return this.handle;
    }

    // ---- the door ----

    /** One call to the application, and its answer: after the page's changes it is behind. */
    async call(call: Record<string, unknown>, timeout: number | null = null): Promise<any> {
        await this.queue;
        const answer = await (this.hostOf as GuiHost).callApp(this.opened(), call, timeout);
        const verb = (call.do as Extra | undefined)?.verb;
        if (verb === "new_file" || verb === "close_file" || verb === "open") await this.refreshBinding();
        return answer;
    }

    /** One of its readings: `transport`, `output` or `history`. */
    async query(name: string): Promise<Record<string, any>> {
        return ((await this.call({ query: name })) ?? {}) as Record<string, any>;
    }

    /** One action of the multitrack, carried to its end, and its result. */
    async act(verb: string, args: Record<string, unknown> = {}): Promise<any> {
        return this.call({ do: { verb, ...args } });
    }

    /** An internal call without waiting on the page's own queue. @internal */
    async raw(call: Record<string, unknown>): Promise<any> {
        return (this.hostOf as GuiHost).callApp(this.opened(), call, null);
    }

    /** A retained-object call checks its lifetime again after earlier edits settle. @internal */
    async callFor(object: SessionObject, call: Record<string, unknown>): Promise<any> {
        await this.queue;
        if (object.closed) throw new Error("the session view is closed");
        return this.raw(call);
    }

    /** Forward a retained object's edit in the page's order. @internal */
    send(object: SessionObject, call: Extra, label: string, record: boolean): void {
        object.pending += 1;
        this.queue = this.queue
            .then(() => object.closed ? undefined : this.raw({ do: { verb: "handle", object: object.identity, call, label, ...(record ? {} : { record: false }) } }))
            .catch((error: unknown) => console.warn(`clausters: the window refused a change: ${String(error)}`))
            .then(async () => { object.pending -= 1; await object.follow(); });
    }

    // ---- the window's verbs ----

    /**
     * **Perform the window's verb `name`**, as its key, its menu entry or its
     * tool would (`docs/verbs.md`). Answers whether it was asked: `false` with
     * no window open.
     */
    verb(name: string): boolean {
        if (this.closed) return false;
        (this.hostOf as GuiHost).verb(this.opened(), String(name));
        return true;
    }

    /**
     * Ask the window to undo ({@link MultitrackEditor.verb}); whether the verb was sent.
     * The result does not confirm completion. A script waiting for the history step
     * awaits `multitrack.history.undo()`.
     */
    undo(): boolean {
        return this.verb("undo");
    }

    /**
     * Ask the window to redo ({@link MultitrackEditor.verb}); whether the verb was sent.
     * The result does not confirm completion. A script waiting for the history step
     * awaits `multitrack.history.redo()`.
     */
    redo(): boolean {
        return this.verb("redo");
    }

    /** Split the box under the cursor: the window's `split`. */
    split(): boolean {
        return this.verb("split");
    }

    /** Join the selected boxes: the window's `join`. */
    join(): boolean {
        return this.verb("join");
    }

    /** Move the selected boxes onto the grid: the window's `quantize`. */
    quantize(): boolean {
        return this.verb("quantize");
    }

    /** Switch whether a pass stops where the contents end: the window's `stop_at_end`. */
    stopAtEnd(): boolean {
        return this.verb("stop_at_end");
    }

    /** Give every track its default height back: the window's `reset_heights`. */
    resetHeights(): boolean {
        return this.verb("reset_heights");
    }

    /** Fit every track into the window: the window's `compact_tracks`. */
    compactTracks(): boolean {
        return this.verb("compact_tracks");
    }

    /** Draw every box of samples as its take's waveform, as the audio editor draws a take: the window's `view_wave`. */
    viewWave(): boolean {
        return this.verb("view_wave");
    }

    /** Draw every box of samples as its take's spectrogram, as the audio editor draws a take: the window's `view_spectrogram`. */
    viewSpectrogram(): boolean {
        return this.verb("view_spectrogram");
    }

    // ---- the grid, and how a box of notes is drawn ----

    /** One verb of the window's editor (`grid`, `sync`, `notesView`), and its answer. */
    private async editorVerb(call: Record<string, unknown>): Promise<Record<string, any>> {
        return ((await this.act("verb", { call })) ?? {}) as Record<string, any>;
    }

    /** The grid a box snaps to and `quantize` moves it onto, in beats. */
    async grid(): Promise<number> {
        return Number((await this.editorVerb({ verb: "grid" })).grid ?? 0);
    }

    /** Sets the grid, in beats. */
    async setGrid(beats: number): Promise<void> {
        await this.editorVerb({ verb: "sync", grid: Number(beats) });
    }

    /** Whether a box the hand moves snaps to the grid. */
    async snapToGrid(): Promise<boolean> {
        return Boolean((await this.editorVerb({ verb: "grid" })).snap);
    }

    /** Sets whether a box the hand moves snaps to the grid. */
    async setSnapToGrid(on: boolean): Promise<void> {
        await this.editorVerb({ verb: "sync", snap: Boolean(on) });
    }

    /** How a box of notes is drawn: `"roll"` or `"score"`. */
    async notesView(): Promise<string> {
        return String((await this.editorVerb({ verb: "notesView" })).view);
    }

    /** Sets how a box of notes is drawn; a `RangeError` for a view there is not. */
    async setNotesView(view: string): Promise<void> {
        if (!(await this.editorVerb({ verb: "notesView", view: String(view) })).set) {
            throw new RangeError(`no notes view "${view}": a box of notes is drawn as a "roll" or a "score"`);
        }
    }

    // ---- what the hand holds ----

    /** The regions of the boxes the window holds. @internal */
    async marked(): Promise<Region[]> {
        if (this.closed) return [];
        await this.queue;
        await this.refreshBinding();
        const held = ((await this.query("selected")).selected ?? []) as number[];
        return this.multitrack === null ? [] : held.map((region) => this.multitrack!.viewOf(Region, Number(region)));
    }

    /** Holds the boxes of `regions` in the window. @internal */
    async mark(regions: Iterable<Region> | null): Promise<void> {
        if (this.closed) return;
        await this.act("select", { regions: [...(regions ?? [])].map((region) => region.ident) });
    }

    // ---- the transport, and what is heard ----

    /** **The multitrack's transport** ({@link MultitrackTransport}). */
    get transport(): MultitrackTransport {
        return new MultitrackTransport(this);
    }

    /**
     * **What the engine plays**: each track's meter levels by track id
     * (`tracks`), and the samples played as silence for want of a frame read
     * in time since the engine opened (`starved`).
     */
    async output(): Promise<Record<string, any>> {
        return this.query("output");
    }

    // ---- saving ----

    /**
     * **Write the session as the edits have left it**: over the file it was
     * opened from, or, given a `path`, there, which a later `save` then writes
     * over. An untitled one needs a `path` the first time. Every file inside
     * the session's folder is written relative to it. Resolves to the path
     * written.
     */
    async save(path: string | null = null): Promise<string> {
        const answer = ((await this.act("save", path === null ? {} : { path: String(path) })) ?? {}) as Record<string, any>;
        const written = String(answer.destination ?? path ?? this.filed);
        this.filed = written;
        return written;
    }
}

/** The marking surface, merged so `editor.selected()` types as its own. */
export interface MultitrackEditor extends Marking<Region[], Iterable<Region>> {}
marking(MultitrackEditor);

/** Whether `structure` is a {@link Multitrack}. */
export function isMultitrack(structure: unknown): structure is Multitrack {
    return structure instanceof Multitrack;
}
