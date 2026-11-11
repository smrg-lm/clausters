/**
 * **A handler of an application the GUI host holds**: what a page keeps while a
 * window of the host's has what it opened (mirrors
 * `clausters/gui/editing/hosted.py`).
 *
 * The roll and the score editor are applications of the host, as the audio
 * editor and the multitrack are: the window, what a gesture does, the history,
 * the engraver and the playback all run in the host's process
 * ({@link GuiHost.openApp}). An editor here is the page's handle: each member is
 * one call to the application ({@link GuiHost.callApp}), and what it answers is
 * read from there rather than kept here. A read is therefore a promise here where
 * the reference client's is a property.
 *
 * **The page's structure is the window's while one is open.** An
 * `EventSequence` or a `Score` keeps its identity, and a change made through it
 * is an edit of the window's, undone by its Ctrl+Z, in the order the page made
 * them. When the last window over it goes, it holds what that window left it as.
 * **Its objects answer at once from a copy this page holds**, since a call to the
 * host is a promise: the copy is brought to the window's after each of the page's
 * own changes and steps, and after what a hand does in the window, which the host
 * tells the page (`/gui_app_changed`). The reference client reads the window's at
 * every read.
 *
 * @module
 */

import type { GuiHost, Stage, WindowActivity, WindowProfile } from "../host.ts";
import type { WindowHandle } from "../handle.ts";

/** One leg of a page's block: the change, and the edit that puts it back. */
export interface Leg {
    forward: unknown;
    backward: unknown;
}

/** **The multitrack's transport**, as a script plays it: each verb one call to the application, in the multitrack's seconds. */
export class MultitrackTransport {
    private readonly editor: { act(verb: string, args?: Record<string, unknown>): Promise<any> };

    constructor(editor: { act(verb: string, args?: Record<string, unknown>): Promise<any> }) {
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

/**
 * **A structure while windows have it**: the editors over it, the first still
 * open answering for it.
 *
 * @internal
 */
export class Held {
    editors: HostedEditor[] = [];

    /** The editor whose window answers for the structure, or `null`. */
    live(): HostedEditor | null {
        return this.editors.find((editor) => !editor.closed) ?? null;
    }
}

/** How a hosted editor opens. */
export interface HostedEditorOptions {
    /** The window's title. */
    title?: string;
    /** Its width, in logical pixels; left out, the application's. */
    width?: number;
    /** Its height, in logical pixels; left out, the application's. */
    height?: number;
    /** `false` opens what is edited with none of the application's chrome around it, the keys kept. */
    chrome?: boolean;
}

/**
 * **The page's handle of a window the host holds**: its opening, its door, its
 * history and its closing, which every application's handler shares (the audio
 * editor, the multitrack, a roll, a page). An editor says which application it
 * is, how it opens, what is the window's once it opens, what it takes back when
 * a window goes, and how the copy this page holds follows the window's.
 */
export abstract class HostedEditor {
    /** The window's title. */
    readonly title: string;
    /** Whether the window carries the application's chrome. */
    readonly chrome: boolean;
    /** Called after any gesture that **changed the data**, in this window or another over the same structure. */
    onChange: (() => void) | null = null;
    private readonly size: [number | undefined, number | undefined];
    private hostOf: GuiHost | null = null;
    private handle: WindowHandle | null = null;
    private closedHandler: (() => void) | null = null;
    private settled = true;
    /** The window let go of what this handler opened, and goes on as the host's own. @internal */
    protected released = false;
    /** The page's changes, forwarded in the order they were made. */
    private queue: Promise<unknown> = Promise.resolve();
    /** The page's changes not yet followed by the copy it holds. @internal */
    protected pending = 0;
    private unlisten: (() => void) | null = null;
    private depth = 0;
    /** The legs of the page's block under way, or `null` outside one. @internal */
    protected legs: Leg[] | null = null;
    private blockLabel = "edit";

    protected constructor(fallback: string, { title, width, height, chrome = true }: HostedEditorOptions) {
        this.title = title !== undefined ? String(title) : fallback;
        this.size = [width, height];
        this.chrome = Boolean(chrome);
    }

    /** Which application it is. */
    protected abstract readonly app: string;
    /** What the application opens with, `beside` the window it opens next to when another already has the structure. */
    protected abstract openingOptions(beside: HostedEditor | null): Record<string, unknown>;
    /** What says who has this editor's structure. */
    protected abstract heldBy(): Held;
    /** The structure is the window's from now on. */
    protected taken(): void {}
    /** The window went, leaving `left`; with `last` it was the last over the structure, which is the page's again. */
    protected abstract settle(left: Record<string, unknown> | null, last: boolean): void;
    /** Brings the copy the page holds to the window's. */
    protected abstract follow(): Promise<void>;
    /** A hand moved what the window holds. */
    protected changedByHand(): void {}

    // ---- the window ----

    /**
     * **Ask the host to open this editor's window**, on `host` or the ambient
     * one, and answer it. One already open answers the window it has. From then
     * on the structure it was opened over is the window's (the module's
     * documentation).
     */
    async open(host?: GuiHost, { stage }: { stage?: Stage | null } = {}): Promise<WindowHandle> {
        if (this.handle !== null && !this.closed) return this.handle;
        if (this.handle !== null) {
            // a window that went before its close was heard: what it left is
            // taken back first
            this.leave();
            await this.queue;
        }
        const resolved = host ?? (await (await import("../../plot.ts")).resolveHost());
        const held = this.heldBy();
        const beside = held.live();
        if (beside !== null && beside.host !== resolved) {
            throw new RangeError("this structure is open in a window of another host: close it there first");
        }
        const options: Record<string, unknown> = { title: this.title, ...this.openingOptions(beside) };
        if (this.size[0] !== undefined) options.w = Math.trunc(this.size[0]);
        if (this.size[1] !== undefined) options.h = Math.trunc(this.size[1]);
        if (!this.chrome) options.chrome = false;
        if (beside !== null) options.over = beside.id;
        const window = await resolved.openApp(this.app, options, 5.0, { element: stage });
        return this.hold(resolved, window);
    }

    /**
     * **The handler of `window`** from now on, a window of `host`'s this
     * editor's application holds, opened by {@link open} or found by another
     * editor (a box's roll). The structure is the window's unless another
     * window of the page's has it already.
     *
     * @internal
     */
    async hold(host: GuiHost, window: number): Promise<WindowHandle> {
        const resolved = host;
        const held = this.heldBy();
        const take = held.live() === null;
        const { WindowHandle } = await import("../handle.ts");
        this.hostOf = resolved;
        this.handle = new WindowHandle(resolved, window, new Map());
        this.settled = false;
        this.released = false;
        this.handle.onClosed(() => this.closedNow());
        // **What a hand does in the window** is told (`/gui_app_changed`), and
        // the copy the page holds follows it after the page's own changes; **a
        // document let go of** hands this one back (`/gui_app_released`), and
        // the window goes on as the host's own
        this.unlisten = resolved.onMessage((msg) => {
            if (Number(msg.args[0]) !== window) return;
            if (msg.addr === "/gui_app_changed") {
                this.changedByHand();
                this.pending += 1;
                this.queue = this.queue.then(() => this.followed()).then(() => this.onChange?.());
            } else if (msg.addr === "/gui_app_released" && msg.args.length > 1) {
                // the window's own close, later, is nothing of this handler's
                resolved.setClosedHandler(window, null);
                this.released = true;
                this.leave(JSON.parse(String(msg.args[1])) as Record<string, unknown>);
                this.closedHandler?.();
            }
        });
        held.editors.push(this);
        if (take) this.taken();
        this.pending += 1;
        this.queue = this.queue.then(() => this.followed());
        await this.queue;
        return this.handle;
    }

    private closedNow(): void {
        this.leave();
        this.closedHandler?.();
    }

    private leave(given: Record<string, unknown> | null = null): void {
        if (this.settled || this.handle === null) return;
        this.settled = true;
        this.unlisten?.();
        this.unlisten = null;
        const left = given ?? ((this.hostOf as GuiHost).closedWith(this.handle.id) as Record<string, unknown> | null);
        const held = this.heldBy();
        held.editors = held.editors.filter((editor) => editor !== this);
        this.settle(left ?? null, held.live() === null);
    }

    /** The {@link GuiHost} it is open on, or `null`. */
    get host(): GuiHost | null {
        return this.hostOf;
    }

    /** The open window's handle, or `null`. */
    get window(): WindowHandle | null {
        return this.handle;
    }

    /** The open window's id, or `null`. */
    get id(): number | null {
        return this.handle === null ? null : this.handle.id;
    }

    /** Whether it is done with its window: the window is gone, or it let go of what this handler opened (the window goes on as the host's own). */
    get closed(): boolean {
        return this.handle === null || this.handle.closed || this.released;
    }

    /** Resolves when the window is closed, or on `timeout` seconds: `true` for the first. */
    async wait(timeout?: number): Promise<boolean> {
        if (this.handle === null || this.released) return true;
        const done = await this.handle.wait(timeout);
        if (done) this.leave();
        return done;
    }

    /** Call `handler()` when its window goes. `null` clears it. */
    onClosed(handler: (() => void) | null): this {
        this.opened();
        this.closedHandler = handler;
        return this;
    }

    /** Close the window, and what plays it, without asking: what it edits is the page's, and is left as the window has it. */
    async close(): Promise<this> {
        if (!this.closed) {
            await this.act("close");
            await this.wait(5);
        }
        this.leave();
        return this;
    }

    protected opened(): WindowHandle {
        if (this.handle === null) throw new Error(`open() the ${this.app} editor first`);
        return this.handle;
    }

    // ---- the door ----

    /** One call to the application, and its answer: after the page's changes it is behind. */
    async call(call: Record<string, unknown>, timeout: number | null = null): Promise<any> {
        await this.queue;
        const window = this.opened();
        return (this.hostOf as GuiHost).callApp(window, call, timeout);
    }

    /** One verb of the window's editor, read; it changes nothing the window draws. */
    async coreCall(verb: string, args: Record<string, unknown> = {}): Promise<Record<string, any>> {
        return ((await this.call({ member: { verb, ...args } })) ?? {}) as Record<string, any>;
    }

    /** A setting of the window's, which then redraws it. */
    protected async setting(args: Record<string, unknown>): Promise<void> {
        await this.act("set", { call: { verb: "sync", ...args } });
    }

    /** One of its readings: `transport`, `actions`, `selected` or `history`. */
    async query(name: string): Promise<Record<string, any>> {
        return ((await this.call({ query: name })) ?? {}) as Record<string, any>;
    }

    /** One action of the application, carried to its end, and its result. */
    async act(verb: string, args: Record<string, unknown> = {}): Promise<any> {
        return this.call({ do: { verb, ...args } });
    }

    /** One action asked now, ahead of what waits: what brings the copy to the window's, which the page's changes wait on. */
    protected asked(verb: string, args: Record<string, unknown> = {}): Promise<any> {
        return (this.hostOf as GuiHost).callApp(this.opened(), { do: { verb, ...args } }, null);
    }

    /** A call the page made, in its order, and the copy brought to the window's once nothing else waits. */
    protected send(verb: string, args: Record<string, unknown>): void {
        this.pending += 1;
        const host = this.hostOf as GuiHost;
        const window = this.opened();
        this.queue = this.queue
            .then(() => host.callApp(window, { do: { verb, ...args } }, null))
            .catch((error: unknown) => console.warn(`clausters: the window refused a change: ${String(error)}`))
            .then(() => this.followed());
    }

    /**
     * A change the page's sequence made, `current` the edit that puts it back:
     * an edit of the window's under `label`, sent after the ones before it --
     * or, inside an {@link WindowHistory.entry}, kept for the entry's end.
     *
     * @internal
     */
    forward(intent: Record<string, unknown>, label: string, current: unknown, applied: Record<string, unknown> = intent): void {
        this.send("handle", { call: { verb: "apply", intent: applied, inverse: true } });
        const leg = { forward: intent, backward: current };
        if (this.legs !== null) {
            this.legs.push(leg);
            return;
        }
        this.send("handle", { label, call: { verb: "record", legs: [leg] } });
    }

    private async followed(): Promise<void> {
        this.pending -= 1;
        if (this.pending > 0 || this.closed) return;
        await this.follow();
    }

    /** A step of the window's history, and the copy the page holds brought to it. */
    private async step(verb: "undo" | "redo"): Promise<boolean> {
        const stepped = Boolean(await this.act(verb));
        this.pending += 1;
        await this.followed();
        return stepped;
    }

    /** **The window's history**, as the page's structure reads it while the window is open. @internal */
    get history() {
        return {
            entry: <T>(label: string, run: () => T): T => {
                this.depth += 1;
                if (this.depth === 1) {
                    this.legs = [];
                    this.blockLabel = String(label);
                }
                try {
                    return run();
                } finally {
                    this.depth -= 1;
                    if (this.depth === 0) {
                        const legs = this.legs ?? [];
                        this.legs = null;
                        if (legs.length > 0 && !this.closed) {
                            this.send("handle", { label: this.blockLabel, call: { verb: "record", legs } });
                        }
                    }
                }
            },
            undo: () => this.step("undo"),
            redo: () => this.step("redo"),
            canUndo: () => this.canUndo(),
            canRedo: () => this.canRedo(),
            undoLabel: () => this.undoLabel(),
            redoLabel: () => this.redoLabel(),
        };
    }

    /** Step back over the last entry of the history its window walks; whether anything moved. */
    undo(): Promise<boolean> {
        return this.step("undo");
    }

    /** Step forward again after `undo`; whether anything moved. */
    redo(): Promise<boolean> {
        return this.step("redo");
    }

    /** Whether there is an edit to step back over. */
    async canUndo(): Promise<boolean> {
        return Boolean((await this.query("history")).canUndo);
    }

    /** Whether there is an undone edit to step forward into. */
    async canRedo(): Promise<boolean> {
        return Boolean((await this.query("history")).canRedo);
    }

    /** What an undo would be called, for a menu item. */
    async undoLabel(): Promise<string | null> {
        return ((await this.query("history")).undoLabel ?? null) as string | null;
    }

    /** What a redo would be called, for a menu item. */
    async redoLabel(): Promise<string | null> {
        return ((await this.query("history")).redoLabel ?? null) as string | null;
    }

    /** **The window's actions as they read now**, by verb. */
    actions(): Promise<Record<string, any>> {
        return this.query("actions");
    }

    /** The unit the window's time ruler counts in, read off its actions. @internal */
    protected async rulerUnit(): Promise<string> {
        const actions = await this.actions();
        for (const unit of ["beats", "time", "samples"]) {
            if (actions[`ruler_${unit}`]?.checked) return unit;
        }
        return "";
    }

    /** Asks the window's ruler verb for `unit`; a `RangeError` for no unit. @internal */
    protected chooseRuler(unit: "beats" | "time" | "samples"): void {
        if (unit !== "beats" && unit !== "time" && unit !== "samples") {
            throw new RangeError(`a ruler counts in "beats", "time" or "samples", not ${JSON.stringify(unit)}`);
        }
        this.verb(`ruler_${unit}`);
    }

    /** Nothing to bring in step: what a page changes is the window's already, and every window over it is redrawn when it does. */
    adopt(): void {}

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

    /** **A hand played to the window**, at `(x, y)` in its device pixels; whether it was sent. */
    pointer(kind: "press" | "move" | "release", x: number, y: number, mods = ""): boolean {
        if (this.closed) return false;
        (this.hostOf as GuiHost).pointer(this.opened(), kind, x, y, mods);
        return true;
    }

    /** A press and a release at `(x, y)`, with `mods` held. */
    click(x: number, y: number, mods = ""): boolean {
        return this.pointer("press", x, y, mods) && this.pointer("release", x, y, mods);
    }

    /** **A drag from `start` to `end`**, as a hand makes it. */
    drag(start: [number, number], end: [number, number], mods = "", steps = 8): boolean {
        const [x0, y0] = start;
        const [x1, y1] = end;
        if (!this.pointer("press", x0, y0, mods)) return false;
        const n = Math.max(Math.trunc(steps), 1);
        for (let i = 1; i <= n; i++) {
            const t = i / n;
            this.pointer("move", x0 + (x1 - x0) * t, y0 + (y1 - y0) * t, mods);
        }
        return this.pointer("release", x1, y1, mods);
    }

    /** **A key pressed on the window**, with `mods` held; whether it was sent. */
    pressKey(key: string, mods = ""): boolean {
        if (this.closed) return false;
        (this.hostOf as GuiHost).pressKey(this.opened(), key, mods);
        return true;
    }

    /** **Text typed into the window**, a key per character; whether it was sent. */
    typeText(text: string): boolean {
        if (this.closed) return false;
        (this.hostOf as GuiHost).typeText(this.opened(), text);
        return true;
    }

    /** **Write the window's picture to `path`** as a PNG, and answer the path. */
    async capture(path: string, timeout = 5.0): Promise<string> {
        return (this.hostOf as GuiHost).capture(this.opened(), path, timeout);
    }

    /** **What the window costs, and why.** */
    async activity(timeout = 1.0): Promise<WindowActivity> {
        return (this.hostOf as GuiHost).activity(this.opened(), timeout);
    }

    /** **What the window costs over the next `seconds`.** */
    async profile(seconds = 5.0, timeout = 1.0): Promise<WindowProfile> {
        return (this.hostOf as GuiHost).profile(this.opened(), seconds, timeout);
    }

    /** The whole of what the window's view holds, in view: its `view_all`. */
    viewAll(): boolean {
        return this.verb("view_all");
    }

    /** Cut what is selected: the window's `cut`. */
    cut(): boolean {
        return this.verb("cut");
    }

    /** Copy what is selected: the window's `copy`. */
    copy(): boolean {
        return this.verb("copy");
    }

    /** Paste at the cursor: the window's `paste`. */
    paste(): boolean {
        return this.verb("paste");
    }

    /** Select everything the view holds: the window's `select_all`. */
    selectAll(): boolean {
        return this.verb("select_all");
    }

    /** The position cursor to the start: the window's `to_start`. */
    toStart(): boolean {
        return this.verb("to_start");
    }

    /** The position cursor to the end: the window's `to_end`. */
    toEnd(): boolean {
        return this.verb("to_end");
    }

    /** The loop switch, on or off: the window's `loop`. */
    loop(): boolean {
        return this.verb("loop");
    }
}

/** **The transport a roll or a page plays on**, as a script plays it, in beats: its window's own, each verb one call to the application. */
export class HostedTransport extends MultitrackTransport {
    private readonly hosted: HostedEditor & { showSpan(span: readonly [number, number] | null): Promise<void> };

    constructor(editor: HostedEditor & { showSpan(span: readonly [number, number] | null): Promise<void> }) {
        super(editor);
        this.hosted = editor;
    }

    /** The time range the space bar plays, `[start, end]` in beats, which an Alt+drag on a roll sweeps; `null` with none. */
    async span(): Promise<[number, number] | null> {
        const written = (await this.hosted.coreCall("range")).range;
        return Array.isArray(written) ? [Number(written[0]), Number(written[1])] : null;
    }

    /** Draws a time range, or none. */
    async setSpan(span: readonly [number, number] | null): Promise<void> {
        await this.hosted.showSpan(span);
    }

    /** The loop switch: over `start`..`end` beats when both are given, else over the span or every note. */
    async loop(start: number | null = null, end: number | null = null, { on = true }: { on?: boolean } = {}): Promise<this> {
        const args: Record<string, unknown> = { looping: Boolean(on) };
        if (start !== null && end !== null) args.range = [Number(start), Number(end)];
        await this.hosted.act("loop", args);
        return this;
    }
}
