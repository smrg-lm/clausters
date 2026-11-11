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
import type { EventSequence } from "../../seq/sequence.ts";
import { NotesEditor } from "./events.ts";
import { Held, HostedEditor, MultitrackTransport } from "./hosted.ts";
import { Marking, marking } from "./marking.ts";

export { MultitrackTransport };

/** What a {@link MultitrackEditor} opens over by its file: a saved session. */
export interface SavedSession {
    /** The file it was opened from or saved to, or `null` for none. */
    path: string | null;
    /** Its multitrack. */
    multitrack: Multitrack;
    /** What its regions are windows onto, by source id: a box of notes reads its `sequence`. */
    sources?: Map<number, { sequence?: EventSequence | null }>;
}

/** How a {@link MultitrackEditor} opens. */
export interface MultitrackEditorOptions {
    /** The window's title; the session file's name when not given, `"Untitled"` for a multitrack. */
    title?: string;
}

/** A session as a window hands it back, and as `/gui_app_open` takes it again: its contents, its file, its last save. */
interface Snapshot {
    session: Extra;
    path: string | null;
    saved: Extra | null;
}

/**
 * **What a window left each multitrack as**, by the multitrack, so that
 * opening it again, from this handler or from a later `edit`, reads none of
 * it from the file over changes the window left behind.
 */
const snapshots = new WeakMap<Multitrack, Snapshot>();

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
export class MultitrackEditor extends HostedEditor implements WindowOver {
    /** Whether a hand has changed the data since this was last cleared. */
    dirty = false;

    protected override changedByHand(): void {
        this.dirty = true;
    }

    /** The session it was opened over, or `null` for an untitled one. */
    readonly session: SavedSession | null;
    /** The multitrack it was opened over: the window's while it is open, `null` for one opened over nothing. */
    readonly multitrack: Multitrack | null;
    protected readonly app = "multitrack";
    private filed: string | null;
    private snapshot: Snapshot | null = null;
    private readonly holders = new Held();

    constructor(structure: SavedSession | Multitrack | null = null, { title }: MultitrackEditorOptions = {}) {
        if (structure !== null && !(structure instanceof Multitrack)
            && (structure.path === null || structure.path === undefined)) {
            throw new RangeError(
                "the multitrack opens a session from its file, and this one has " +
                    "none: save it first (`session.save(path)`)",
            );
        }
        const filed = structure === null || structure instanceof Multitrack ? null : String(structure.path);
        const fallback = structure === null ? "Multitrack"
            : filed === null ? "Untitled" : filed.split("/").pop()!;
        super(fallback, { title });
        this.filed = filed;
        if (structure === null) {
            this.session = null;
            this.multitrack = null;
            return;
        }
        if (structure instanceof Multitrack) {
            this.session = null;
            this.multitrack = structure;
        } else {
            this.session = structure;
            this.multitrack = structure.multitrack;
        }
        // what a window left this multitrack as, if one did: it is opened over that
        this.snapshot = snapshots.get(this.multitrack) ?? null;
        if (this.snapshot !== null) this.filed = this.snapshot.path;
    }

    // ---- the window ----

    protected heldBy(): Held {
        return this.holders;
    }

    /** Opened again after its window went, it is opened over what that window left it as (the module's documentation). */
    protected openingOptions(): Record<string, unknown> {
        if (this.snapshot !== null && this.multitrack !== null) {
            return { snapshot: { ...this.snapshot, session: { ...this.snapshot.session, multitrack: this.multitrack.write() } } };
        }
        if (this.filed !== null) return { path: this.filed };
        if (this.multitrack !== null) return { multitrack: this.multitrack.write() };
        return {};
    }

    /** The page's multitrack is the window's from now on. */
    protected taken(): void {
        if (this.multitrack !== null) this.multitrack.window = this;
    }

    /**
     * Hand the page's multitrack back, once the window is done with it: what
     * the window left it as, and the session it was opened over brought to
     * the same. A window that let go hands its snapshot over as `left`; one
     * that closed, the multitrack, with the snapshot beside it.
     */
    protected settle(given: Record<string, unknown> | null): void {
        const host = this.host as GuiHost;
        const id = this.opened().id;
        let snapshot: Snapshot | null;
        let left: unknown;
        if (this.released) {
            snapshot = given as Snapshot | null;
            left = host.closedWith(id);
        } else {
            snapshot = host.closedSnapshot(id) as Snapshot | null;
            left = given;
        }
        const multitrack = this.multitrack;
        if (multitrack === null || multitrack.window !== this) return;
        multitrack.window = null;
        if (snapshot !== null) left = snapshot.session.multitrack ?? left;
        if (left !== null && typeof left === "object") multitrack.holdWritten(left as Extra);
        if (snapshot === null) return;
        snapshots.set(multitrack, snapshot);
        this.snapshot = snapshot;
        this.filed = snapshot.path;
        if (this.session !== null) {
            Object.assign(this.session, Session.read(snapshot.session), { multitrack, path: snapshot.path });
        }
    }

    /** Brings the copy the page holds to the window's, when no change of the page's is still waiting. */
    protected async follow(): Promise<void> {
        if (this.multitrack === null) return;
        const state = await this.asked("handle", { call: { verb: "state" } });
        if (this.pending === 0 && state !== null && typeof state === "object") this.multitrack?.holdWritten(state as Extra);
    }

    /** The session file it opened, or `null` for an untitled one. */
    get path(): string | null {
        return this.filed;
    }

    // ---- the door ----

    /**
     * A change the page's multitrack made, `current` the edit that puts it
     * back: an edit of the window's under `label`, sent after the ones
     * before it -- or, inside an {@link WindowHistory.entry}, kept for the
     * entry's end.
     *
     * @internal
     */
    forward(intent: Extra, label: string, current: unknown): void {
        if (this.legs !== null) {
            this.legs.push({ forward: intent, backward: current });
            this.send("handle", { call: { verb: "apply", intent }, label, record: false });
            return;
        }
        this.send("handle", { call: { verb: "apply", intent }, label });
    }

    // ---- the window's verbs ----

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

    // ---- the grid ----

    /** One verb of the window's editor (`grid`, `sync`), and its answer. */
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

    /**
     * **What the window's time ruler counts in**: `"beats"` (bars and beats,
     * through the tempo map), `"time"` (minutes and seconds) or `"samples"` --
     * the View menu's Ruler, the window's own and no edit. The multitrack's ruler is over its tracks, bars and beats
     * until another is chosen.
     */
    async ruler(): Promise<string> {
        return this.rulerUnit();
    }

    /** Sets what the window's time ruler counts in: no edit. */
    setRuler(unit: "beats" | "time" | "samples"): void {
        this.chooseRuler(unit);
    }

    // ---- what the hand holds ----

    /** The regions of the boxes the window holds. @internal */
    async marked(): Promise<Region[]> {
        if (this.closed) return [];
        const held = ((await this.query("selected")).selected ?? []) as number[];
        const multitrack = this.multitrack;
        return multitrack === null ? [] : held.map((region) => multitrack.viewOf(Region, Number(region)));
    }

    /** Holds the boxes of `regions` in the window. @internal */
    async mark(regions: Iterable<Region> | null): Promise<void> {
        if (this.closed) return;
        await this.act("select", { regions: [...(regions ?? [])].map((region) => region.ident) });
    }

    // ---- a box's roll ----

    /**
     * **The roll of a box of notes**, as a double click on the box opens it:
     * `region`, a {@link Region} whose box reads a sequence of events, edited
     * in a window of its own, opened if none is open over it, in the
     * multitrack's history -- an edit in either window is undone from both, and
     * the roll goes when the multitrack does.
     *
     * Answers the roll's {@link NotesEditor}, the one this handler answered
     * before while that window is open. Its `structure` is the session's
     * `EventSequence` for the box, the window's while it is open as a notes
     * editor's is.
     *
     * Rejects with a `RangeError` for a box that holds no notes.
     */
    async openRoll(region: Region): Promise<NotesEditor> {
        const named = (region.content.window ?? {}).source as Extra | undefined;
        const id = typeof named?.source === "number" ? named.source : null;
        const sequence = this.session === null || id === null ? undefined : this.session.sources?.get(id)?.sequence;
        if (sequence == null) throw new RangeError("the box holds no notes");
        const answer = (await this.call({ roll: region.ident })) as { window: number };
        const window = Number(answer.window);
        this.#rolls = this.#rolls.filter((roll) => !roll.closed);
        const open = this.#rolls.find((roll) => roll.id === window);
        if (open !== undefined) return open;
        const roll = new NotesEditor(sequence, { title: `notes ${id}` });
        // the host opened it for this page without a `/gui_app_open`
        const host = this.host as GuiHost;
        host.holdOpen(window);
        await roll.hold(host, window);
        this.#rolls.push(roll);
        return roll;
    }

    /** The rolls this handler answered. */
    #rolls: NotesEditor[] = [];

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

    // ---- the window's session ----

    /**
     * **A new, untitled session in the window**, as its File > New puts one
     * there. The window lets go of the session it was opened over: this
     * multitrack holds what the window left it as, as a close leaves it, and
     * the window goes on as the host's own.
     */
    async newFile(): Promise<void> {
        await this.act("new_file");
    }

    /**
     * **The session file at `path` in the window**, as its File > Open opens
     * one, letting go of the session it was opened over as
     * {@link MultitrackEditor.newFile} does. A session another window of the
     * host holds is not opened twice: that window comes forward and this
     * rejects.
     */
    async openFile(path: string): Promise<void> {
        await this.act("open", { session: String(path) });
    }

    /**
     * **No session in the window**, as its File > Close session leaves it,
     * letting go of the one it was opened over as
     * {@link MultitrackEditor.newFile} does.
     */
    async closeFile(): Promise<void> {
        await this.act("close_file");
    }
}

/** The marking surface, merged so `editor.selected()` types as its own. */
export interface MultitrackEditor extends Marking<Region[], Iterable<Region>> {}
marking(MultitrackEditor);

/** Whether `structure` is a {@link Multitrack}. */
export function isMultitrack(structure: unknown): structure is Multitrack {
    return structure instanceof Multitrack;
}
