/**
 * The editor: what orchestrates a picture, a vocabulary and a history.
 *
 * {@link Editor} edits **one structure** -- a buffer's samples, a break-point
 * curve, a timeline of events -- and it imports nothing from the arrangement.
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
 *   through the domain -- never re-derived here, and never twice per language;
 * - **what a number is measured in is the editor's**: the unit bridge (beats and
 *   seconds <-> timeline samples) is here because it is the same bridge for every
 *   structure, and a view that computed its own would be a second answer.
 *
 * A multitrack application is this class plus what only a tree has: a held document,
 * several views of one multitrack, the lanes and clips, and a transport.
 * **Transport and render are not here** -- a bare structure at most sounds; it
 * has no multitrack to move over.
 *
 * @module
 */

import {
    samples_to_secs,
    secs_to_samples,
    viewNotAnEdit,
} from "../../core/clausters_core_web.js";
import { TempoMap } from "../../base/time.ts";
import type { Intent, Selection } from "../../document.ts";
import type { GuiNode } from "../guidef.ts";
import type { WindowHandle } from "../handle.ts";
import type { GuiHost, PropValue } from "../host.ts";
import { Editing, FIRST_VERSION } from "./context.ts";
import type { RecordingLeg } from "./context.ts";
import type { Adopting, Applier } from "./context.ts";
import type { Domain } from "./domain.ts";
import { Application, BASE_ID } from "./application.ts";
import { Echo } from "./echo.ts";
import { log } from "./trace.ts";
import type { View } from "./view.ts";

let notAnEditHeld: readonly string[] = [];

/**
 * The tags that are **not** edits: what a view is looking at, and where the
 * hand is.
 *
 * They are answered generically and never reach a domain, because the crate is
 * explicit that screen state is never part of what is edited -- and the list is
 * **the crate's** (`clausters_document::view::NOT_AN_EDIT`) rather than this
 * module's, because it was written once here and once in the Python client's
 * editor, and a table that small drifts unread: a tag one client treats as
 * screen state and the other hands to a domain is a gesture that reaches a
 * vocabulary which does not know it, answers nothing, and looks like a widget
 * that does nothing.
 *
 * Read once and kept: it answers a constant, so no event pays for it.
 */
export function notAnEdit(): readonly string[] {
    if (notAnEditHeld.length === 0) {
        notAnEditHeld = JSON.parse(viewNotAnEdit() || "[]") as string[];
    }
    return notAnEditHeld;
}

/**
 * The host an `open` acts on: the one named, else the ambient one -- the same
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
/** One leg of a history step: the structure it names, and what it must apply. */
export interface Leg {
    structure?: number;
    payloads?: readonly unknown[];
}

export interface GenericEditorOptions<S> {
    sampleRate: number;
    domain?: Domain<S> | null;
    view?: View<S> | null;
    context?: Editing | null;
    title?: string;
    /**
     * Widgets appended to the window after the picture -- a transport panel, a
     * readout. They are the script's, so the editor never touches their ids;
     * keep them clear of `baseId`.
     */
    extra?: readonly GuiNode[];
    width?: number;
    height?: number;
    /**
     * The first widget id a **host-less** draw counts from (tests and tree
     * inspection). Once opened, the ids come from the host's own recycling pool
     * instead, so the two never collide. Ignored when `app` is given, since the
     * id space is then the application's.
     */
    baseId?: number;
    /**
     * The {@link Application} to draw in -- the host, the id space, the
     * acknowledgement. Given none, the editor makes one of its own and is an
     * application of one; handed one, several editors share a window set and an
     * undo order.
     */
    /**
     * The {@link Application} this editor draws in -- the window set it shares a
     * host, an id space and an undo walk with. Its **acknowledgement stays its
     * own** ({@link Editor.echo}), since a conversation's floor is one view's.
     * Absent: one for this editor alone.
     */
    app?: Application | null;
}

/** One structure on screen, editable back into it. */
export class Editor<S = unknown> implements Adopting {
    /**
     * What is edited. A view over an arrangement calls it `element`, which is the
     * arrangement's word for the same slot.
     */
    structure: S;
    sampleRate: number;
    title: string;
    size: [number, number];
    /**
     * Widgets appended to the window after the picture. They are the script's --
     * the editor never touches their ids.
     */
    extra: GuiNode[];
    /**
     * The editor this one was **composed inside**, when it was one.
     *
     * A structure is not a multitrack: it has no transport, so a click on a ruler
     * here is a seek of whatever this is part of. An editor composed by nobody
     * answers a transport gesture with nothing, which is the honest answer for
     * a curve opened on its own.
     */
    composedIn: Editor | null = null;
    /**
     * What of the composing editor's model this one draws -- the element a
     * dedicated roll or signal view was opened over. `null` when this editor
     * stands alone.
     */
    composedOver: unknown = null;
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
    /**
     * **Where the reader is**, in this editor's own units (beats for a timeline,
     * seconds for a multitrack, a take or a curve) --
     * the position cursor a click placed, and `null` until one
     * has been. It is where a playback starts and where a paste lands, which is
     * why it is worth keeping: the playhead is where the *music* is and moves on
     * its own, and an anchor that moved on its own would not be an anchor.
     * Screen state like the selection, never logged and never part of what is
     * edited.
     */
    cursor: number | null = null;
    /**
     * Called with the beat the **position cursor** was placed at, whenever a
     * click moves it -- on the time ruler, or on the slack a click lands on when
     * it lands on nothing. `null` to be told nothing.
     *
     * Not an edit, and deliberately not a seek: the cursor says where the
     * *reader* is, and what that means for the sound is the application's --
     * normally cueing a stopped transport there, so the next play starts from
     * the mark, and leaving a rolling one alone.
     */
    onLocate: ((beat: number) => void) | null = null;
    /** Whether the data changed since the last render. */
    dirty = false;

    /**
     * The **application** this editor draws in: the host, the widget-id space
     * and the publish -- everything true of a window set rather than of this
     * structure. Handed one, several editors share a window set and an undo
     * order; given none, this editor is an application of one, which is what
     * every editor was before there was a name for it.
     */
    readonly app: Application;
    /**
     * **This view's** end of the acknowledgement protocol -- the stamp, the
     * floor, the corrections and the reason.
     *
     * One per editor and **not** one per application, which is where it used to
     * live: the crate calls a conversation's state "one view's end", and the
     * floor rises when the version moved and no event of *this* view moved it.
     * Two windows over one structure sharing a floor would each silence the
     * other's staleness check, so a gesture made against a picture the
     * neighbouring window had already changed would be accepted rather than
     * refused.
     */
    readonly echo: Echo;
    /**
     * The version this editor's last answered event left behind.
     *
     * Read by the crate on the next message: when it differs from the version
     * then, something moved that was not an event, and that is what raises the
     * floor.
     */
    protected get applied(): number {
        return this.echo.state.applied;
    }

    protected set applied(version: number) {
        this.echo.state = { ...this.echo.state, applied: version };
    }
    protected windowId: number | null = null;
    /** The host subscription this editor is fed through, while it has one. */
    protected unlisten: (() => void) | null = null;
    /** The handle `open` answered, kept so opening twice answers the same one. */
    protected windowHandle: WindowHandle | null = null;
    /**
     * The context to register in when the caller named one; otherwise the
     * structure's own, asked for on each use.
     */
    protected readonly givenContext: Editing | null;
    /**
     * The identity this structure was registered in the history under, minted on
     * the first edit -- a structure you built has no id and is not going to be
     * given a stable one for this.
     */
    protected structureId: number | null = null;

    constructor(
        structure: S,
        {
            sampleRate,
            domain = null,
            view = null,
            context = null,
            title = "Editor",
            extra = [],
            width = 1000,
            height = 520,
            baseId = BASE_ID,
            app = null,
        }: GenericEditorOptions<S>,
    ) {
        this.structure = structure;
        this.sampleRate = Number(sampleRate);
        this.title = title;
        this.size = [Math.trunc(width), Math.trunc(height)];
        this.extra = [...extra];
        this.domain = domain;
        this.view = view;
        this.givenContext = context;
        this.app =
            app ??
            new Application({ context, baseId, version: () => this.version });
        this.echo = new Echo(() => this.version, this.app.host);
        this.app.register(this);
    }

    // ---- the unit bridge: the data <-> timeline samples ----

    /**
     * The structure's beat->second map, asked for on each use, or `null` for a
     * structure that holds no tempo.
     *
     * The map is **the structure's data**: a `Timeline` holds its own
     * (`Timeline.map`), and an editor over a structure kept elsewhere says where
     * it is by overriding this. Keeping a copy here is what let a tempo edited on
     * the structure be drawn at the old one.
     */
    tempoMap(): TempoMap | null {
        return (this.structure as { map?: TempoMap } | null)?.map ?? null;
    }

    private mapOf(): TempoMap {
        const tempoMap = this.tempoMap();
        if (tempoMap === null) {
            const name = (this.structure as object | null)?.constructor?.name ?? "structure";
            throw new Error(`a ${name} holds no tempo, so it has no beats to convert`);
        }
        return tempoMap;
    }

    /**
     * Timeline samples as a position in the structure's own units: beats for a
     * structure with a tempo map, seconds for one without (a take, a curve).
     */
    private position(units: number): number {
        return this.tempoMap() === null ? this.unitsToSecs(units) : this.unitsToBeats(units);
    }

    /**
     * Timeline samples in the **first** beat -- the nominal ratio of the data<->view
     * bridge. A ratio at a position, not a constant: under a tempo that changes,
     * a later beat is a different number of samples wide.
     */
    get unitsPerBeat(): number {
        return this.beatsToUnits(1.0) - this.beatsToUnits(0.0);
    }

    /**
     * Beats -> timeline samples, through the multitrack's time map (and the core's
     * seconds->samples rounding every client shares).
     */
    beatsToUnits(beats: number): number {
        return Number(secs_to_samples(this.mapOf().secsAt(Number(beats)), this.sampleRate));
    }

    /** Timeline samples -> beats: the inverse the edit-back path takes. */
    unitsToBeats(units: number): number {
        return this.mapOf().beatsAt(samples_to_secs(Math.round(units), this.sampleRate));
    }

    /**
     * Timeline samples per second -- the axis *is* samples, so this is the
     * engine's sample rate. A length in seconds crosses on this one, and only an
     * onset crosses on {@link Editor.unitsPerBeat}.
     */
    get unitsPerSecond(): number {
        return this.sampleRate;
    }

    /** Seconds -> timeline samples. */
    secsToUnits(secs: number): number {
        return Number(secs_to_samples(Number(secs), this.sampleRate));
    }

    /** Timeline samples -> seconds. */
    unitsToSecs(units: number): number {
        return samples_to_secs(Math.round(units), this.sampleRate);
    }

    // ---- widget ids: the host's recycling pool, or a host-less fallback ----

    /**
     * A widget id for the tree being drawn. Once opened, it comes from the host's
     * recycling pool; host-less (a test, or inspecting `draw`), it counts from
     * `baseId`.
     */
    /**
     * A widget id for the tree being drawn. Public where the Python client's is
     * `_new_id`: a {@link View} is a collaborator and builds the tree with these,
     * which TypeScript's `protected` would refuse.
     */
    newId(): number {
        return this.app.newId(this);
    }

    /**
     * The id that draws `role`/`key` **of this structure** -- the same number for
     * as long as it keeps being drawn ({@link Application.idFor}).
     *
     * The name is the structure's identity in the history, so two views of one
     * thing agree about which widget draws which part of it, and a redraw leaves
     * every id where it was. An editor with no structure has no identity to name
     * and takes a lease instead: nothing can be in flight against a picture with
     * no data behind it.
     *
     * Public where the Python client's is `_named_id`, for the reason
     * {@link Editor.newId} is: a {@link View} is a collaborator and names its
     * widgets with these.
     */
    namedId(role: string, key = ""): number {
        if (this.structure === null || this.structure === undefined) return this.newId();
        return this.app.idFor(this.registered(), role, key, this);
    }

    /** Start this draw ({@link Application.resetIds}). */
    protected resetIds(): void {
        this.app.resetIds(this);
    }

    // ---- the acknowledgement, delegated to the `Echo` ----

    /** The host this editor answers, or `null` before it is opened. */
    protected get host(): GuiHost | null {
        return this.app.host;
    }

    protected set host(host: GuiHost | null) {
        this.app.host = host;
    }

    protected get corrections(): [number, Record<string, PropValue>][] {
        return this.echo.corrections;
    }

    protected set corrections(value: [number, Record<string, PropValue>][]) {
        this.echo.corrections = [...value];
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

    protected correct(widgetId: number, props: Record<string, PropValue>): void {
        this.echo.correct(widgetId, props);
    }

    protected acknowledge(seq: number, reason?: string): void {
        this.echo.acknowledge(seq, reason);
    }

    // ---- the history: the data's, not this editor's ----

    /**
     * The structure's editing context -- its history, and the views over it.
     *
     * Reached through the **data**, so a second window gets the same one. That is
     * the whole of what makes an undo in either view update both, and it is why
     * none of this is a field here: a history belongs to the data, never to a
     * view.
     */
    protected get editing(): Editing {
        return this.givenContext ?? Editing.of(this.structure as object);
    }

    /**
     * The version -- the counter the host names back on its next gesture. The
     * context's, moved by its turns, steps and records and never here.
     */
    protected get version(): number {
        return this.editing.version;
    }

    /**
     * This structure's identity in the history, minted on first use.
     *
     * Asked of the **context** rather than kept here, so two windows over one
     * structure name one identity: the pile is one order over the data, not one
     * per view.
     */
    protected registered(): number {
        // **The domain goes with it**, because it is what puts an edit back and
        // the pile's scope is the context's rather than this window's: a step
        // must reach this structure whether or not the window that made the
        // edit is still open.
        this.structureId ??= this.editing.identity(
            this.structure as object,
            this.domain?.name ?? "",
            (this.domain ?? null) as Applier | null,
        );
        return this.structureId;
    }

    // ---- the forward draw ----

    /**
     * The structure as a `window`-rooted GuiDef. Pure -- it builds the tree and
     * the view's registry, and sends nothing.
     */
    draw(): GuiNode {
        if (this.view === null) throw new Error("this editor has no view to draw with");
        // The draw is **bracketed**: every named widget asked for inside it
        // counts as still drawn, and what the view stopped drawing gives its id
        // back on the way out. That bracket is what lets an id be an identity
        // rather than a lease -- a widget still in the picture keeps its number,
        // and only one that is genuinely gone releases it.
        this.resetIds();
        const tree = this.view.draw(this);
        this.app.retireIds(this);
        return tree;
    }

    /**
     * `draw` the structure and open it on `host`, or on the **ambient** host when
     * none is named -- the same rule `guidef.View.open`, `plot` and `scope`
     * follow.
     *
     * **It also listens.** From here the gestures reach the structure as the
     * host reports them, with nothing routed by the caller; that is what makes
     * {@link edit} one call.
     *
     * **One editor, one window.** Opening an open editor answers the window it
     * already has rather than orphaning it: a second view of one structure is
     * `edit(x)` a second time, which gives a second editor on one history.
     */
    async open(
        host?: GuiHost,
        { id, stage }: { id?: number; stage?: unknown } = {},
    ): Promise<WindowHandle> {
        if (this.windowId !== null && this.windowHandle !== null) return this.windowHandle;
        const resolved = await this.app.resolve(host);
        const handle = resolved.open(this.draw(), { id, element: stage as never });
        this.windowId = handle.id;
        this.windowHandle = handle;
        this.editing.attach(this);
        this.listen(resolved);
        this.announce();
        return handle;
    }

    /**
     * Subscribe to the host's messages, so an edit-back reaches the structure.
     * `open` does it; this is the door for a caller that opened the window
     * itself. Answers the unsubscribe.
     */
    listen(host: GuiHost): () => void {
        this.detach();
        this.unlisten = host.onMessage((msg) => {
            this.apply(msg.addr, msg.args);
        });
        return () => this.detach();
    }

    /**
     * The open window's **handle**, or `null`.
     *
     * The same object {@link Editor.open} hands back: it carries the window's
     * id and resolves the tree's **named** widgets, which is how a script
     * reaches the widgets it passed as `extra` --
     * `editor.window.widget("play").onClick(...)`. The reference client's
     * `Editor.window` answers the same way (there a `WindowHandle` subclasses
     * `int`, so the id reads straight off it); this getter returned the bare
     * number while the handle sat beside it, which made an `extra` widget
     * unreachable from a page and reachable from a script.
     */
    get window(): WindowHandle | null {
        return this.windowHandle;
    }

    /** The open window's id, or `null` -- {@link Editor.window}'s own `id`. */
    get id(): number | null {
        return this.windowId;
    }

    /** Whether this editor's window is gone. */
    get closed(): boolean {
        return this.windowId === null;
    }

    /**
     * Close this editor's window (`/gui_free`) and stop listening.
     *
     * **The history is not closed with it.** An undo order belongs to the data
     * ({@link Editing}), so editing the same structure again resumes the same
     * order -- closing a window is not an edit, and never was.
     */
    close(): this {
        const window = this.windowId;
        this.windowId = null;
        this.windowHandle = null;
        this.detach();
        if (this.host !== null && window !== null) this.host.close(window);
        this.editing.detach(this);
        this.app.forget(this);
        return this;
    }

    /**
     * Call `handler()` when this editor's window is closed; `null` clears it.
     *
     * The same verb `PlotWindow.onClosed` and `WindowHandle.onClosed` carry,
     * over the same registry -- so a window opened by {@link edit} and one
     * opened by `plot` are told about in one way.
     */
    onClosed(handler: (() => void) | null): this {
        if (this.host === null || this.windowId === null) {
            throw new Error("open() the editor before asking to be told it closed");
        }
        this.host.setClosedHandler(this.windowId, handler);
        return this;
    }

    /**
     * Resolves when this editor's window is closed, or on `timeout` seconds --
     * `true` for the first, `false` for the second.
     *
     * The page's shape of the reference client's `Editor.wait`: there a script
     * must not exit while the window is on screen and the call blocks a thread;
     * here nothing exits and nothing may block, so it is a promise. What both
     * say is the same -- read the structure back *after* the hand is done with
     * it.
     */
    wait(timeout?: number): Promise<boolean> {
        return this.app.wait(() => !this.closed, timeout);
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
        // `<id> <seq> <version> <tag> <payload...>`: the stamp and the version the
        // gesture was made against are the second and third arguments of every
        // event, and the envelope is all the decision reads. The payload never
        // crosses for it -- what a report *means* is the domain's, and it crosses
        // once, there.
        const id = Math.trunc(Number(rawArgs[0] ?? 0));
        const turn = this.echo.read({
            addr,
            argc: rawArgs.length,
            widget: id,
            seq: Math.trunc(Number(rawArgs[1] ?? 0)),
            against: Math.trunc(Number(rawArgs[2] ?? 0)),
            tag: rawArgs.length > 3 ? String(rawArgs[3]) : "",
            version: this.version,
            // The two the page answers for, because it is the page that holds
            // the window and the view's widget table.
            isWindow: this.windowId !== null &&
                (rawArgs.length === 0 || id === this.windowId),
            owns: this.owns(id),
        });
        if (turn.turn === "closed") return this.closedWindow();
        if (turn.turn === "nothing") return false;
        const seq = turn.seq ?? 0;
        this.corrections = [];
        // Why an edit did not do what it asked, when there is something to say.
        this.reason = undefined;
        if (turn.turn === "step") {
            // **What it answers is whether anything moved**, not whether the
            // keystroke was understood. A history at its end is the ordinary
            // case, and reporting a change there told every other view to bring
            // itself in step with an edit that never happened.
            const stepped = turn.redo === true ? this.redo() : this.undo();
            // **A refusal says why.** A step nobody could apply is the one case
            // where nothing happening is not "the pile is at its end": the entry
            // belongs to a structure whose window is closed, and it is still
            // there waiting for it. Saying so is the difference between a dead
            // button and one that is telling you where to press it.
            if (!stepped && this.app.refusal !== null) this.reason = this.app.refusal;
            this.acknowledge(seq, this.reason ?? undefined);
            return stepped;
        }
        if (turn.turn === "stale") {
            // The data moved under the gesture, by a route no gesture produced.
            // The edit is not applied and not merged: an edit-back payload is
            // absolute *and* whole, so applying one made against an older
            // picture would silently drop whatever arrived in between.
            this.resync(turn.widget ?? id);
            this.acknowledge(seq, turn.reason);
            return false;
        }
        const changed = this.route([rawArgs[0], ...rawArgs.slice(3)]);
        this.applied = this.version;
        // Answered whatever happened, and answered with a *value*: applied,
        // transformed and refused are one message.
        this.acknowledge(seq, this.reason);
        // ...and *then* the redefine, when the gesture added or removed a widget.
        this.restructure();
        return changed;
    }

    /**
     * This editor's window closed. Answers `false`: nothing changed.
     *
     * Closing a *view* is not an event of the history, so the context stays
     * exactly as it is -- what goes is this window's place in the list of who to
     * tell.
     */
    protected closedWindow(): boolean {
        this.windowId = null;
        this.windowHandle = null;
        this.editing.detach(this);
        this.onWindowGone();
        return false;
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
        if (notAnEdit().includes(tag)) {
            log.debug("event  %s %s -> screen state", id, tag);
            return this.observe(id, tag, rest);
        }
        if (this.interface(id, tag, rest)) {
            log.debug("event  %s %s -> this editor's own", id, tag);
            return false;
        }
        if (this.domain === null) return false;
        // **One reading of one gesture.** The payloads, what an undo menu calls
        // them and why the gesture was refused all come off the same answer,
        // because they are three things about *one* report and reading it three
        // times is how they come to be three answers.
        const taken = this.domain.read(this.structure, tag, rest);
        const payloads = taken.payloads;
        log.debug(
            "event  %s %s -> %s",
            id,
            tag,
            payloads.length === 0
                ? "no payload"
                : payloads
                    .map((p) => String((p as { intent?: unknown }).intent))
                    .join(", "),
        );
        if (payloads.length === 0) {
            // Nothing, or a refusal. A refusal says why and hands the widget
            // back what it should be drawing, so the picture stops agreeing with
            // the hand instead of with the structure.
            if (taken.refusal !== undefined) {
                this.reason = taken.refusal;
                this.resync(id);
            }
            return false;
        }
        const label = taken.label;
        if (payloads.length === 1) return this.edit(payloads[0], label);
        return this.editAll(payloads, label);
    }

    /**
     * **An interface event**: a tag that asks this editor for something rather
     * than stating an edit or saying what a view is looking at.
     *
     * The third kind, and it is the editor's rather than the domain's because
     * what it asks for is a *window* -- a multitrack's `"enter"` opens the box
     * that was double clicked, and opening a window is not something a
     * vocabulary of edits can say. Nothing here reaches a history: what the
     * editor it opened does afterwards is what lands in one.
     *
     * Answers whether it was handled, so an editor that says no leaves the tag
     * to the domain exactly as before.
     */
    protected interface(_widgetId: number, _tag: string, _values: readonly unknown[]): boolean {
        return false;
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
        if (tag === "locate" && values.length > 0) {
            // A click on the time ruler: the reader put the position cursor
            // there. It is kept here whatever else happens to it -- a play starts
            // from it, a paste lands on it -- and it is not a seek: the playhead
            // is never placed.
            this.cursor = this.position(Number(values[0]));
            // **Whoever has the transport is told**, and that is this editor
            // when it has one and the multitrack it is composed inside when it does
            // not: a structure has no transport of its own, and a window inside
            // a multitrack is not a second place to keep a position.
            this.locate(this.cursor);
            this.composedIn?.locate(this.cursor);
            this.onLocate?.(this.cursor);
            return false;
        }
        if (tag === "selection") {
            const selection: Record<string, unknown> = {
                start: values.length > 0 ? this.position(Number(values[0])) : 0.0,
                len: values.length > 1 ? this.position(Number(values[1])) : 0.0,
            };
            if (values.length >= 4) {
                // The sweep restricted the value axis too. Carried **as it
                // came**: it is in the structure's own domain, and no unit of
                // this editor's applies to it.
                selection.value = { min: Number(values[2]), max: Number(values[3]) };
            }
            this.selection = selection as unknown as Selection;
            this.selected();
        }
        return false;
    }

    /**
     * This editor's selection moved.
     *
     * Nothing on its own -- a structure's selection is that structure's. A view
     * **composed** inside a bigger editor hands it up instead, because the range
     * an operation is given must be the same value whichever of the multitrack's
     * windows it was swept in.
     */
    protected selected(): void {
        this.composedIn?.adoptSelection(this as Editor);
    }

    /**
     * A view composed inside this one swept a marquee. Nothing by default;
     * a view over an arrangement names what it is a selection *of*.
     */
    adoptSelection(_editor: Editor): void {}

    /**
     * The position cursor was placed at `at`, in the structure's own units, here
     * or in a window composed inside this one.
     *
     * Nothing by default, and that is the honest answer for a structure opened
     * on its own: the mark is kept ({@link Editor.cursor}) and what it means for
     * the sound needs a transport, which only something that can be *played*
     * has. An editor that has one cues it here.
     */
    locate(_at: number): void {}

    /**
     * Apply one payload to the structure and record how to put it back.
     *
     * The inverse is read **before** the edit lands ({@link Domain.current}),
     * which is the whole reason this is one call: a surface that let you apply
     * first and record second would let you record the wrong thing. A payload the
     * structure was already at is applied by nobody and recorded by nobody -- a
     * resend is not an edit.
     */
    protected edit(payload: unknown, label: string, coalesce = false): boolean {
        if (this.domain === null) return false;
        const before = this.domain.current(this.structure, payload);
        if (!this.domain.project(this.structure, payload)) return false;
        log.debug("record [%s] %s", label, (payload as { intent?: unknown }).intent);
        if (before !== null && before !== undefined) {
            this.editing.record(
                [{
                    structure: this.registered(),
                    forward: { edit: payload },
                    backward: before,
                    key: this.domain.coalesceKey(payload),
                }],
                { label, coalesce },
            );
        } else {
            this.editing.moved();
        }
        this.dirty = true;
        const moved = { structure: this.registered(), payload };
        this.editing.changed();
        return true;
    }

    /**
     * Apply a run of payloads as **one** entry, so a block edit undoes the way
     * it was made.
     *
     * The same rule as {@link Editor.edit}, and it is spelled out only because
     * there is no one-call form for a transaction: each inverse is read
     * immediately before *that* payload lands, never all of them up front -- an
     * inverse read against a state two edits ago puts back a state that never
     * held.
     */
    protected editAll(payloads: readonly unknown[], label: string): boolean {
        if (this.domain === null) return false;
        const legs: RecordingLeg[] = [];
        let moved = false;
        for (const payload of payloads) {
            const before = this.domain.current(this.structure, payload);
            if (!this.domain.project(this.structure, payload)) continue;
            moved = true;
            if (before !== null && before !== undefined) {
                legs.push({
                    structure: this.registered(),
                    forward: { edit: payload as Intent },
                    backward: before,
                    key: this.domain.coalesceKey(payload),
                });
            }
            this.editing.changed();
        }
        if (!moved) return false;
        log.debug("record [%s] %d leg(s)", label, legs.length);
        if (legs.length > 0) this.editing.record(legs, { label });
        else this.editing.moved();
        this.dirty = true;
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
     * this is nothing here and something in a tree's view.
     */
    protected restructure(): boolean {
        return false;
    }

    /** Stop listening. The window stays open; nothing reaches the structure. */
    detach(): void {
        this.unlisten?.();
        this.unlisten = null;
    }

    /**
     * This editor's window went away: stop whatever it was driving.
     *
     * Unsubscribing, here -- an editor with no window has nothing to answer for,
     * and the host holds an open editor so a script need not, which is where
     * that stops. An editor that composed other views overrides it, because a
     * view *it* composed may still be on screen and fed from the same
     * subscription; that is why
     * this is a hook of its own rather than a call to {@link Editor.detach},
     * whose public meaning is "stop listening" and must go on meaning it.
     */
    protected onWindowGone(): void {
        this.detach();
    }

    /**
     * Called with no arguments after any gesture that **changed the data** --
     * this window's, another window's over the same structure, or a step of the
     * history. `null` to be told nothing.
     *
     * The page's door onto an edit. One call per gesture however many edits it
     * took, because that is what a hand did.
     */
    onChange: (() => void) | null = null;

    /**
     * The structure changed in a turn -- this editor's own gesture, another
     * window's, or a step of the history.
     *
     * Separate from {@link Editor.adopt}, which is about *drawing*: a page that
     * sounds a multitrack or writes a file wants to be told whoever made the edit,
     * and the window that made it is not exempt from having changed.
     */
    dataChanged(): void {
        this.onChange?.();
    }

    /**
     * Another view of this structure edited it: bring this window in step, by
     * correcting **every widget this editor holds**.
     *
     * It takes nothing, and it used to take the turn's intents so a view could
     * adopt a placement or a length as a prop instead of redrawing. What this
     * does is already props -- one resync per widget and one acknowledgement,
     * never a redefine -- so the intents would only have narrowed which widgets,
     * and no view ever read one.
     *
     * A window that is not open has nothing to bring in step.
     */
    adopt(): void {
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
     * not hold is left alone -- another view of the same context owns it, and one
     * pile over several structures is the point.
     */
    protected step(direction: "undo" | "redo"): boolean {
        return this.app.step(direction, this);
    }


    /**
     * Draw what a history walk left behind: every widget resynced, and the host
     * told once.
     *
     * Called on the editor that walked, after the whole step has landed, so a
     * window whose structure the step did not touch still comes back in step --
     * one entry can move several structures, and a picture of one of them is a
     * picture of the walk.
     */
    reflectStep(): void {
        this.dirty = true;
        this.corrections = [];
        for (const wid of [...(this.view?.widgets.keys() ?? [])]) this.resync(wid);
        this.acknowledge(0);
        this.corrections = [];
    }

    /** Whether there is an edit to step back over. */
    get canUndo(): boolean {
        return this.editing.canUndo;
    }

    /** Whether there is an undone edit to step forward into. */
    get canRedo(): boolean {
        return this.editing.canRedo;
    }

    /** What an undo would be called, for a menu item. */
    get undoLabel(): string | undefined {
        return this.editing.undoLabel;
    }

    /**
     * What a redo would be called. The pair of {@link Editor.undoLabel}, and it
     * stops being decoration the moment a second window is open: with one pile
     * over all of them, a label is how a person knows which edit a keystroke is
     * about to move.
     */
    get redoLabel(): string | undefined {
        return this.editing.redoLabel;
    }
}
