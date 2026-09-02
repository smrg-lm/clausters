/**
 * The editor: what orchestrates a picture, a vocabulary and a history.
 *
 * {@link Editor} edits **one structure** — a buffer's samples, a break-point
 * curve, a timeline of events — and it imports nothing from the arrangement.
 * What makes that possible is that it performs almost nothing itself: it opens a
 * window through a {@link View}, turns a gesture into a payload through a
 * {@link Domain}, answers the host through an {@link Echo}, and records what
 * happened in the {@link Editing} context the **data** owns rather than one of
 * its own.
 *
 * So the boundaries are:
 *
 * - an editor owns **neither the data nor the history**. It asks the structure
 *   for its context ({@link Editing.of}) and never builds one, which is what
 *   makes two windows over one thing walk one undo order;
 * - **how an edit inverts is the crate's** (`history::Editable`), reached
 *   through the domain — never re-derived here, and never twice per language;
 * - **what a number is measured in is the editor's**: the unit bridge (beats and
 *   seconds ↔ timeline samples) is here because it is the same bridge for every
 *   structure, and a view that computed its own would be a second answer.
 *
 * {@link FormEditor} is this class plus what only a tree has: a held document,
 * several views of one composition, the lanes and clips, and a transport.
 * **Transport and render are not here** — a bare structure at most sounds; it
 * has no piece to move over.
 *
 * @module
 */

import { samples_to_secs, secs_to_samples } from "../../core/clausters_core_web.js";
import { TempoMap } from "../../base/time.ts";
import type { Intent, Selection } from "../../document.ts";
import type { GuiNode } from "../guidef.ts";
import type { WindowHandle } from "../handle.ts";
import type { GuiHost, PropValue } from "../host.ts";
import { Editing, FIRST_VERSION } from "./context.ts";
import type { Adopting } from "./context.ts";
import type { Domain } from "./domain.ts";
import { Echo } from "./echo.ts";
import type { View } from "./view.ts";

/**
 * The tags that are **not** edits: what a view is looking at, and where the hand
 * is. They are answered generically and never reach a domain, because the crate
 * is explicit that screen state is never part of what is edited.
 */
export const NOT_AN_EDIT = [
    "selection",
    "view",
    "view_x",
    "view_y",
    "layer",
    "focus",
    "locate",
    "height",
];

/**
 * The host an `open` acts on: the one named, else the ambient one — the same
 * resolution `guidef.View.open`, `plot` and `scope` share, so an editor is not
 * the one resource that has to be handed a host.
 *
 * Async where the Python client's `_resolve_host` is not, for the reason
 * `View.open` is async here: resolving the ambient host may have to boot it, and
 * a page boots asynchronously.
 */
export async function resolveEditorHost(host?: GuiHost): Promise<GuiHost> {
    if (host !== undefined) return host;
    return (await import("../../plot.ts")).resolveHost();
}

/** What {@link Editor} is built with. */
export interface GenericEditorOptions<S> {
    sampleRate: number;
    tempo?: number;
    tempoMap?: TempoMap | null;
    domain?: Domain<S> | null;
    view?: View<S> | null;
    context?: Editing | null;
    title?: string;
    width?: number;
    height?: number;
    baseId?: number;
}

/** One structure on screen, editable back into it. */
export class Editor<S = unknown> implements Adopting {
    /**
     * What is edited. {@link FormEditor} calls it `element`, which is the
     * arrangement's word for the same slot.
     */
    structure: S;
    sampleRate: number;
    /**
     * The piece's beat→second map — the whole of the beat side of the unit
     * bridge. Given one, the editor draws against the same function the clock
     * plays by; given only a `tempo`, it is that tempo as a single segment.
     */
    tempoMap: TempoMap;
    title: string;
    size: [number, number];
    /**
     * The vocabulary this structure's edits are written in, and the picture of
     * it. Both are per-structure and neither is per-window.
     */
    domain: Domain<S> | null;
    view: View<S> | null;
    /**
     * The last selection swept in this editor's windows. It is a plain value and
     * not part of what is edited, which is the crate's own line: a selection is
     * screen state, never persisted and never logged.
     */
    selection: Selection | Record<string, never> = {};
    /** Whether the data changed since the last render. */
    dirty = false;

    protected readonly baseId: number;
    protected fallbackId: number;
    /**
     * This view's end of the acknowledgement protocol — the stamp, the floor,
     * the corrections and the reason. It reads the version out of the context
     * rather than keeping one, because two windows over one structure report one
     * counter.
     */
    protected readonly echo: Echo;
    /**
     * The version this editor was at when it last answered a host event — what
     * turns "the version moved" into "it moved *by someone else*".
     */
    protected applied: number = FIRST_VERSION;
    protected windowId: number | null = null;
    /**
     * The context to register in when the caller named one; otherwise the
     * structure's own, asked for on each use.
     */
    protected readonly givenContext: Editing | null;
    /**
     * The identity this structure was registered in the history under, minted on
     * the first edit — a structure you built has no id and is not going to be
     * given a stable one for this.
     */
    protected structureId: number | null = null;

    constructor(
        structure: S,
        {
            sampleRate,
            tempo = 1.0,
            tempoMap,
            domain = null,
            view = null,
            context = null,
            title = "Editor",
            width = 1000,
            height = 520,
            baseId = 10_000,
        }: GenericEditorOptions<S>,
    ) {
        this.structure = structure;
        this.sampleRate = Number(sampleRate);
        this.tempoMap = tempoMap?.copy() ?? new TempoMap(Number(tempo));
        this.title = title;
        this.size = [Math.trunc(width), Math.trunc(height)];
        this.domain = domain;
        this.view = view;
        this.baseId = Math.trunc(baseId);
        this.fallbackId = this.baseId;
        this.givenContext = context;
        this.echo = new Echo(() => this.version);
    }

    // ---- the unit bridge: the data ↔ timeline samples ----

    /**
     * The tempo the piece **starts** at, in beats per second. A reading of
     * {@link Editor.tempoMap}, not a second copy of it.
     */
    get tempo(): number {
        return this.tempoMap.tempoAt(0.0);
    }

    set tempo(tempo: number) {
        this.tempoMap = new TempoMap(Number(tempo));
    }

    /**
     * Timeline samples in the **first** beat — the nominal ratio of the data↔view
     * bridge. A ratio at a position, not a constant: under a tempo that changes,
     * a later beat is a different number of samples wide.
     */
    get unitsPerBeat(): number {
        return this.beatsToUnits(1.0) - this.beatsToUnits(0.0);
    }

    /**
     * Beats → timeline samples, through the piece's time map (and the core's
     * seconds→samples rounding every client shares).
     */
    beatsToUnits(beats: number): number {
        return Number(secs_to_samples(this.tempoMap.secsAt(Number(beats)), this.sampleRate));
    }

    /** Timeline samples → beats: the inverse the edit-back path takes. */
    unitsToBeats(units: number): number {
        return this.tempoMap.beatsAt(samples_to_secs(Math.round(units), this.sampleRate));
    }

    /**
     * Timeline samples per second — the axis *is* samples, so this is the
     * engine's sample rate. A length in seconds crosses on this one, and only an
     * onset crosses on {@link Editor.unitsPerBeat}.
     */
    get unitsPerSecond(): number {
        return this.sampleRate;
    }

    /** Seconds → timeline samples. */
    secsToUnits(secs: number): number {
        return Number(secs_to_samples(Number(secs), this.sampleRate));
    }

    /** Timeline samples → seconds. */
    unitsToSecs(units: number): number {
        return samples_to_secs(Math.round(units), this.sampleRate);
    }

    // ---- widget ids: the host's recycling pool, or a host-less fallback ----

    /**
     * A widget id for the tree being drawn. Once opened, it comes from the host's
     * recycling pool; host-less (a test, or inspecting `draw`), it counts from
     * `baseId`.
     */
    protected newId(): number {
        return this.host === null ? this.fallbackId++ : this.host.allocId();
    }

    /**
     * Start a fresh draw's id numbering. Host-less, the fallback counter restarts
     * at `baseId`; on a host nothing resets — the ids come from its pool.
     */
    protected resetIds(): void {
        if (this.host === null) this.fallbackId = this.baseId;
    }

    // ---- the acknowledgement, delegated to the `Echo` ----

    /** The host this editor answers, or `null` before it is opened. */
    protected get host(): GuiHost | null {
        return this.echo.host;
    }

    protected set host(host: GuiHost | null) {
        this.echo.host = host;
    }

    protected get corrections(): [number, Record<string, PropValue>][] {
        return this.echo.corrections;
    }

    protected set corrections(value: [number, Record<string, PropValue>][]) {
        this.echo.corrections = value;
    }

    protected get floor(): number {
        return this.echo.floor;
    }

    protected set floor(value: number) {
        this.echo.floor = value;
    }

    protected get reason(): string | undefined {
        return this.echo.reason;
    }

    protected set reason(value: string | undefined) {
        this.echo.reason = value;
    }

    protected announce(): void {
        this.echo.announce();
    }

    protected stale(against: number): boolean {
        return this.echo.stale(against);
    }

    protected correct(widgetId: number, props: Record<string, PropValue>): void {
        this.echo.correct(widgetId, props);
    }

    protected acknowledge(seq: number, reason?: string): void {
        this.echo.acknowledge(seq, reason);
    }

    // ---- the history: the data's, not this editor's ----

    /**
     * The structure's editing context — its history, and the views over it.
     *
     * Reached through the **data**, so a second window gets the same one. That is
     * the whole of what makes an undo in either view update both, and it is why
     * none of this is a field here: a history belongs to the data, never to a
     * view.
     */
    protected get editing(): Editing {
        return this.givenContext ?? Editing.of(this.structure as object);
    }

    /** The version — the counter the host names back on its next gesture. */
    protected get version(): number {
        return this.editing.version;
    }

    protected set version(value: number) {
        this.editing.version = Math.trunc(value);
    }

    /** This structure's identity in the history, minted on first use. */
    protected registered(): number {
        this.structureId ??= this.editing.register(this.domain?.name ?? "");
        return this.structureId;
    }

    // ---- the forward draw ----

    /**
     * The structure as a `window`-rooted GuiDef. Pure — it builds the tree and
     * the view's registry, and sends nothing.
     */
    draw(): GuiNode {
        if (this.view === null) throw new Error("this editor has no view to draw with");
        this.resetIds();
        return this.view.draw(this);
    }

    /**
     * `draw` the structure and open it on `host`, or on the **ambient** host when
     * none is named.
     */
    async open(
        host?: GuiHost,
        { id, stage }: { id?: number; stage?: unknown } = {},
    ): Promise<WindowHandle> {
        const resolved = await resolveEditorHost(host);
        this.host = resolved;
        const handle = resolved.open(this.draw(), { id, element: stage as never });
        this.windowId = handle.id;
        this.editing.attach(this);
        this.announce();
        return handle;
    }

    /** The open window's id, or `null`. */
    get window(): number | null {
        return this.windowId;
    }

    // ---- the edit-back ----

    /**
     * Apply one message from the host to the structure, and **answer it**.
     * Answers whether the data changed.
     *
     * **Every other window over this structure is told**, on the way out: an
     * acknowledgement goes to the window whose gesture it answered, so a second
     * view would go on drawing something that moved under it.
     */
    apply(addr: string, rawArgs: readonly unknown[]): boolean {
        return this.editing.turn(this, () => {
            const changed = this.deliver(addr, rawArgs);
            if (changed) this.editing.changed();
            return changed;
        });
    }

    /** `apply`, without the turn around it: what the message actually does. */
    protected deliver(addr: string, rawArgs: readonly unknown[]): boolean {
        if (addr === "/gui_closed") {
            if (
                rawArgs.length === 0 || this.windowId === null ||
                Math.trunc(Number(rawArgs[0])) === this.windowId
            ) {
                this.windowId = null;
                // Closing a *view* is not an event of the history, so the
                // context stays exactly as it is -- what goes is this window's
                // place in the list of who to tell.
                this.editing.detach(this);
                this.detach();
            }
            return false;
        }
        if (addr !== "/gui_event" || rawArgs.length < 3) return false;
        // `<id> <seq> <version> <tag> <payload…>`: the stamp and the version the
        // gesture was made against are the second and third arguments of every
        // event. The stamp is what an acknowledgement names.
        const seq = Math.trunc(Number(rawArgs[1]));
        const against = Math.trunc(Number(rawArgs[2] ?? 0));
        const args = [rawArgs[0], ...rawArgs.slice(3)];
        this.corrections = [];
        // Why an edit did not do what it asked, when there is something to say.
        this.reason = undefined;
        const id = Math.trunc(Number(args[0]));
        // The window's own shortcuts (Ctrl+Z / Ctrl+Shift+Z), which the host
        // addresses to the **window** rather than to a widget: undo is not aimed
        // at anything under the cursor. They are answered here rather than
        // routed, because a history step is not an edit to the data.
        if ((args[1] === "undo" || args[1] === "redo") && id === this.windowId) {
            // **What it answers is whether anything moved**, not whether the
            // keystroke was understood. A history at its end is the ordinary
            // case, and reporting a change there told every other view to bring
            // itself in step with an edit that never happened.
            const stepped = args[1] === "redo" ? this.redo() : this.undo();
            this.acknowledge(seq);
            return stepped;
        }
        // Only what this editor draws is this editor's to answer.
        if (!this.owns(id)) return false;
        // **The answers lag, and that is not a conflict.** A host stamps every
        // event with the version it was last told, and it is told only when an
        // acknowledgement reaches it — a round trip a hand outruns. What the
        // check is for is the data moving by a route the host knows nothing
        // about, so only *that* raises the floor.
        if (this.version !== this.applied) this.floor = this.version;
        if (this.stale(against)) {
            // The data moved under the gesture, by a route no gesture produced.
            // The edit is not applied and not merged: an edit-back payload is
            // absolute *and* whole, so applying one made against an older
            // picture would silently drop whatever arrived in between.
            this.resync(id);
            this.acknowledge(seq, "the composition changed since this edit");
            return false;
        }
        const changed = this.route(args);
        this.applied = this.version;
        // Answered whatever happened, and answered with a *value*: applied,
        // transformed and refused are one message.
        this.acknowledge(seq, this.reason);
        // ...and *then* the redefine, when the gesture added or removed a widget.
        this.restructure();
        return changed;
    }

    /** Whether this editor drew the widget an event names. */
    protected owns(widgetId: number): boolean {
        return this.view !== null && this.view.owns(widgetId);
    }

    /**
     * One `/gui_event` payload onto the structure, with the stamp already taken
     * off. Answers whether the data changed; `apply` answers the host.
     *
     * The tags that are not edits are answered here and never reach the domain;
     * everything else is the domain's to read, and a tag it does not recognize is
     * nothing rather than an error.
     */
    protected route(args: readonly unknown[]): boolean {
        const id = Math.trunc(Number(args[0]));
        const tag = String(args[1]);
        const rest = args.slice(2);
        if (NOT_AN_EDIT.includes(tag)) return this.observe(id, tag, rest);
        if (this.domain === null) return false;
        const payload = this.domain.payload(this.structure, tag, rest);
        if (payload === null || payload === undefined) return false;
        return this.edit(payload, this.domain.label(payload));
    }

    /**
     * A tag that says what the view is looking at rather than what changed.
     *
     * Nothing here reaches a history: the crate is explicit that a selection, a
     * zoom and which layer the hand is on are never part of what is edited. The
     * selection is still kept **typed**, because it is the value an operation is
     * handed.
     */
    protected observe(wid: number, tag: string, values: readonly unknown[]): boolean {
        if (tag === "selection") {
            this.selection = {
                start: values.length > 0 ? this.unitsToBeats(Number(values[0])) : 0.0,
                len: values.length > 1 ? this.unitsToBeats(Number(values[1])) : 0.0,
            } as Selection;
        }
        return false;
    }

    /**
     * Apply one payload to the structure and record how to put it back.
     *
     * The inverse is read **before** the edit lands ({@link Domain.current}),
     * which is the whole reason this is one call: a surface that let you apply
     * first and record second would let you record the wrong thing. A payload the
     * structure was already at is applied by nobody and recorded by nobody — a
     * resend is not an edit.
     */
    protected edit(payload: unknown, label: string, coalesce = false): boolean {
        if (this.domain === null) return false;
        const before = this.domain.current(this.structure, payload);
        if (!this.domain.project(this.structure, payload)) return false;
        if (before !== null && before !== undefined) {
            this.editing.history.record(
                [{
                    structure: this.registered(),
                    forward: { edit: payload as Intent },
                    backward: before,
                    key: this.domain.coalesceKey(payload),
                }],
                { label, coalesce },
            );
        }
        this.version += 1;
        this.dirty = true;
        const moved = { structure: this.registered(), payload };
        this.editing.moved(moved as unknown as Intent);
        return true;
    }

    /**
     * Hand back what the widget should be drawing, without applying anything: the
     * answer to an edit that arrived too late.
     */
    protected resync(widgetId: number): void {
        if (this.view === null) return;
        const props = this.view.props(this, widgetId);
        if (Object.keys(props).length > 0) this.correct(widgetId, props);
    }

    /**
     * Redefine the window when the last edit changed **which widgets exist**, and
     * say whether it did. A structure edited in place changes none, which is why
     * this is nothing here and something in {@link FormEditor}.
     */
    protected restructure(): boolean {
        return false;
    }

    /** Unsubscribe from the host. Nothing to do until a subclass subscribes. */
    detach(): void {}

    /**
     * Another view of this structure edited it: bring this window in step. A
     * window that is not open has nothing to bring in step.
     */
    adopt(_intents: readonly Intent[], _whole: boolean): void {
        if (this.host === null || this.windowId === null) return;
        this.corrections = [];
        for (const wid of [...(this.view?.widgets.keys() ?? [])]) this.resync(wid);
        this.acknowledge(0);
        this.corrections = [];
    }

    // ---- the history walk ----

    /**
     * Step back one edit, and tell the host what to draw instead. The inverse is
     * an ordinary payload, so undoing needs no second path. Answers whether
     * anything was undone.
     */
    undo(): boolean {
        return this.editing.turn(this, () => {
            const stepped = this.step("undo");
            if (stepped) this.editing.changed();
            return stepped;
        });
    }

    /** Step forward again after `undo`. Answers whether anything was redone. */
    redo(): boolean {
        return this.editing.turn(this, () => {
            const stepped = this.step("redo");
            if (stepped) this.editing.changed();
            return stepped;
        });
    }

    /**
     * One step of the pile, projected leg by leg.
     *
     * The history holds structures the crate cannot reach, so it applies nothing:
     * what comes back is an ordered list of legs, and it is the editor that hands
     * each to the domain that owns it. A leg naming a structure this editor does
     * not hold is left alone — another view of the same context owns it, and one
     * pile over several structures is the point.
     */
    protected step(direction: "undo" | "redo"): boolean {
        const history = this.editing.history;
        const walked = direction === "undo" ? history.undo() : history.redo();
        if (walked === undefined) return false;
        const legs = (direction === "undo"
            ? (walked as { inverses?: unknown[] }).inverses
            : (walked as { edits?: unknown[] }).edits) ?? [];
        const mine = this.registered();
        let applied = false;
        for (const leg of legs as { structure?: number; payload?: unknown }[]) {
            if (Math.trunc(Number(leg.structure ?? -1)) !== mine) continue;
            if (leg.payload !== undefined && this.domain !== null) {
                applied = this.domain.project(this.structure, leg.payload) || applied;
            }
        }
        if (!applied) return false;
        this.version += 1;
        this.dirty = true;
        this.corrections = [];
        for (const wid of [...(this.view?.widgets.keys() ?? [])]) this.resync(wid);
        this.acknowledge(0);
        this.corrections = [];
        return true;
    }

    /** Whether there is an edit to step back over. */
    get canUndo(): boolean {
        return this.editing.history.canUndo;
    }

    /** Whether there is an undone edit to step forward into. */
    get canRedo(): boolean {
        return this.editing.history.canRedo;
    }

    /** What an undo would be called, for a menu item. */
    get undoLabel(): string | undefined {
        return this.editing.history.undoLabel;
    }

    /**
     * What a redo would be called. The pair of {@link Editor.undoLabel}, and it
     * stops being decoration the moment a second window is open: with one pile
     * over all of them, a label is how a person knows which edit a keystroke is
     * about to move.
     */
    get redoLabel(): string | undefined {
        return this.editing.history.redoLabel;
    }
}
