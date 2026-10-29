/**
 * The **multitrack over a saved session**: its tracks read from their files
 * and played by the application the GUI host holds.
 *
 * The multitrack is one program, and it is not this one. The GUI host opens it
 * over a session file ({@link GuiHost.openApp}), in a window of the host's, on
 * an engine of the application's own: the session's takes are read where they
 * are -- their headers when it opens, their samples as the transport reads
 * them, their peaks, kept in the session's `peaks/` folder, as the window
 * draws them -- and what a gesture does, the history, the playback and the
 * save all run in the host's process. This object is its **handler**: every
 * member below is one call to the application ({@link GuiHost.callApp}), and
 * what it answers is read from there each time rather than kept here. A read
 * is therefore a promise here where the reference client's is a property.
 *
 * A box of notes plays the sequence the session holds for it, and a double
 * click on one opens its roll in the same application, one history with the
 * multitrack.
 *
 * @module
 */

import type { GuiHost, Stage } from "../host.ts";
import type { WindowHandle } from "../handle.ts";

/** **The multitrack's transport**, as a script plays it: each verb one call to the application, in the multitrack's seconds. */
export class SessionTransport {
    private readonly editor: SessionEditor;

    constructor(editor: SessionEditor) {
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

/** What a {@link SessionEditor} opens over: a saved session, by its file. */
export interface SavedSession {
    /** The file it was opened from or saved to, or `null` for none. */
    path: string | null;
}

/** How a {@link SessionEditor} opens. */
export interface SessionEditorOptions {
    /** The window's title; the session file's name when not given. */
    title?: string;
}

/**
 * **The multitrack the GUI host holds, over a saved session, and this script
 * its handler.**
 *
 * `edit` over a multitrack `Session` that has a file answers one, open. Each
 * member is a call to the application in the host's process, answered by it.
 * A session with no file is refused with a `RangeError`: save it first.
 */
export class SessionEditor {
    /** The window's title. */
    readonly title: string;
    /** The session file it opens. */
    readonly path: string;
    /** The session it was opened over, as it was then. */
    readonly session: SavedSession;
    private hostOf: GuiHost | null = null;
    private handle: WindowHandle | null = null;

    constructor(session: SavedSession, { title }: SessionEditorOptions = {}) {
        if (session.path === null || session.path === undefined) {
            throw new RangeError(
                "the multitrack opens a session from its file, and this one has " +
                    "none: save it first (`session.save(path)`)",
            );
        }
        this.session = session;
        this.path = String(session.path);
        this.title = title !== undefined ? String(title) : this.path.split("/").pop()!;
    }

    // ---- the window ----

    /**
     * **Ask the host to open the multitrack**, on `host` or the ambient one,
     * and answer its window. One already open answers the window it has.
     */
    async open(host?: GuiHost, { stage }: { stage?: Stage | null } = {}): Promise<WindowHandle> {
        if (this.handle !== null) return this.handle;
        const resolved = host ?? (await (await import("../../plot.ts")).resolveHost());
        const window = await resolved.openApp("multitrack", { title: this.title, path: this.path }, 5.0, { element: stage });
        const { WindowHandle } = await import("../handle.ts");
        this.hostOf = resolved;
        this.handle = new WindowHandle(resolved, window, new Map());
        return this.handle;
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
    wait(timeout?: number): Promise<boolean> {
        return this.handle === null ? Promise.resolve(true) : this.handle.wait(timeout);
    }

    /** Call `handler()` when the window is closed. `null` clears it. */
    onClosed(handler: (() => void) | null): this {
        this.opened().onClosed(handler);
        return this;
    }

    /** Close the window, and what plays it. */
    async close(): Promise<this> {
        if (!this.closed) await this.act("close");
        return this;
    }

    private opened(): WindowHandle {
        if (this.handle === null) throw new Error("open() the multitrack first");
        return this.handle;
    }

    // ---- the door ----

    /** One call to the application, and its answer. */
    async call(call: Record<string, unknown>, timeout: number | null = null): Promise<any> {
        return (this.hostOf as GuiHost).callApp(this.opened(), call, timeout);
    }

    /** One of its readings: `transport` or `output`. */
    async query(name: string): Promise<Record<string, any>> {
        return ((await this.call({ query: name })) ?? {}) as Record<string, any>;
    }

    /** One action of the multitrack, carried to its end, and its result. */
    async act(verb: string, args: Record<string, unknown> = {}): Promise<any> {
        return this.call({ do: { verb, ...args } });
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

    /** Step the history back one edit ({@link SessionEditor.verb}). */
    undo(): boolean {
        return this.verb("undo");
    }

    /** Step the history forward again ({@link SessionEditor.verb}). */
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

    // ---- the transport, and what is heard ----

    /** **The multitrack's transport** ({@link SessionTransport}). */
    get transport(): SessionTransport {
        return new SessionTransport(this);
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
     * over. Every file inside the session's folder is written relative to it.
     * Resolves to the path written.
     */
    async save(path: string | null = null): Promise<string> {
        const answer = ((await this.act("save", path === null ? {} : { path: String(path) })) ?? {}) as Record<string, any>;
        return String(answer.destination ?? this.path);
    }
}
