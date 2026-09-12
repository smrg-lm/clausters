/**
 * The application: what a window set owns, as against what one structure owns.
 *
 * An {@link Editor} edits **one structure** — a buffer's samples, a break-point
 * curve, a timeline of events. Almost nothing an editor does is about that
 * structure, though: resolving a host, handing out widget ids, answering the
 * acknowledgement, publishing a picture, walking the undo order. All of it is
 * true of the **session on screen** rather than of the data, and an editor that
 * owns it is an editor that has to be copied whole the moment a second one
 * wants to share a window set.
 *
 * So this is the other half of the split the subpackage already makes between
 * an editor and the {@link Editing} context. The context is what the **data**
 * owns — its history, its version, the views to tell. An application is what
 * the **screen** owns — the host and the id space. What is left in
 * between is what an editor genuinely is: a structure bound to a {@link Domain}
 * and a {@link View}.
 *
 * Two consequences worth stating, because they are why this exists rather than
 * being a tidier arrangement of the same code:
 *
 * - **Several editors can share one.** One host, one id space and one undo
 *   order across a bundle of subviews over structures that have nothing
 *   composed behind them. That is an application in the ordinary sense.
 * - **An editor with no window is not a special case.** An application with no
 *   host resolves nothing, hands out ids from its own counter and answers the
 *   acknowledgement by doing nothing, which is exactly what inspecting `draw()`
 *   in a test needs and what an editor used to carry as a branch per method.
 *
 * Every editor holds one, and makes its own when it is not handed one, so
 * nothing a page writes changes.
 *
 * @module
 */

import { CAPACITY, GuiIdAllocator } from "../ids.ts";
import type { GuiNode } from "../guidef.ts";
import type { GuiHost, PropValue } from "../host.ts";
import { Editing, FIRST_VERSION } from "./context.ts";
import type { Adopting } from "./context.ts";
import type { Echo } from "./echo.ts";
import { log } from "./trace.ts";

/**
 * The base a host-less draw counts widget ids from. Above the hand-picked range
 * and above `gui/ids`'s own base, so a tree drawn with no host does not collide
 * with one drawn on a host that is allocating.
 */
export const BASE_ID = 10_000;

/**
 * The drawer of a call that named none — `newId()` asked of the application
 * itself rather than by an editor.
 *
 * A real object rather than `null` so that it can be a weak key like every
 * other drawer. One is enough for the whole module: the tables it keys live on
 * an application, so two applications sharing this key still get their own.
 */
const ANYONE: object = { anyone: true };

/** What an application asks of the editors registered in it. */
export interface Drawing extends Adopting {
    /** Bring this editor's own widgets back in step after a walk. */
    reflectStep(): void;
}

/** One window set: its host and its widget ids. */
export class Application {
    /**
     * The host this window set answers, or `null` before it is opened.
     *
     * The **acknowledgement is not here**: an {@link Echo} is one *view's* end
     * of the conversation, and it is the editor's. See the module comment.
     */
    #host: GuiHost | null = null;

    /** The context when one was named; otherwise the editors' own. */
    #context: Editing | null;
    #baseId: number;
    /** How this application answers a version, when it was given a way. */
    readonly #versionOf: (() => number) | null;
    /**
     * Where a host-less draw takes its ids from — one table per drawer, built
     * on the first ask and never used again once there is a host.
     *
     * **Per drawer** and not per application, because an unopened draw's ids
     * reach nothing: two of them cannot collide with each other, and keeping
     * them apart is what lets a draw with no window start its numbering over,
     * so that drawing one picture twice gives one tree.
     */
    readonly #offline = new WeakMap<object, GuiIdAllocator>();
    /** Each drawer's owner in each table it has drawn on. */
    readonly #owners = new WeakMap<object, WeakMap<GuiIdAllocator, number>>();
    /**
     * The editors drawing in this window set, in the order they registered.
     * Held strongly, the way the host holds an open editor: an application is
     * what a page keeps, and its editors go when it does.
     */
    #editors: Drawing[] = [];

    /**
     * `context` is the {@link Editing} this application's undo order runs in;
     * `undefined` — the ordinary case — reads it off the editors registered
     * here. `baseId` is where the host-less id counter starts. `version`
     * answers the version to stamp an acknowledgement with, and is a callable
     * rather than a number for the reason {@link Echo} states: the version
     * belongs to the editing context and moves under this object.
     */
    constructor({
        context = null,
        baseId = BASE_ID,
        version = null,
    }: {
        context?: Editing | null;
        baseId?: number;
        version?: (() => number) | null;
    } = {}) {
        this.#context = context;
        this.#baseId = Math.trunc(baseId);
        this.#versionOf = version;
    }

    // ---- who is in it ----

    /**
     * Take an editor into this application. Idempotent, so an editor that
     * re-registers is not held twice.
     */
    register(editor: Drawing): this {
        if (!this.#editors.includes(editor)) this.#editors.push(editor);
        return this;
    }

    /** Drop an editor — what `close` does, and what a composed view does when
     * its window goes. */
    forget(editor: Drawing): this {
        this.#editors = this.#editors.filter((held) => held !== editor);
        return this;
    }

    /** The editors registered here, in registration order. */
    get editors(): Drawing[] {
        return [...this.#editors];
    }

    // ---- the editing context, and the version ----

    /**
     * The editing context this application's undo order runs in: the one named
     * at construction, or the first registered editor's — which is the ordinary
     * case, and is what keeps an application over one structure from having to
     * be told what it is editing.
     */
    get context(): Editing | null {
        if (this.#context !== null) return this.#context;
        for (const editor of this.#editors) {
            const found = (editor as unknown as { editing?: Editing }).editing;
            if (found !== undefined) return found;
        }
        return null;
    }

    set context(context: Editing | null) {
        this.#context = context;
    }

    /** The version an acknowledgement carries. */
    get version(): number {
        if (this.#versionOf !== null) return Math.trunc(this.#versionOf());
        return this.context?.version ?? FIRST_VERSION;
    }

    // ---- the host ----

    /** The host this application answers, or `null` before it is opened. */
    get host(): GuiHost | null {
        return this.#host;
    }

    set host(host: GuiHost | null) {
        this.#host = host;
        for (const editor of this.#editors) {
            const echo = (editor as { echo?: { host: GuiHost | null } }).echo;
            if (echo !== undefined) echo.host = host;
        }
    }

    /**
     * Adopt a host: the one named, else the ambient one. Answers the host
     * adopted.
     *
     * **Only when it has none**, which is the rule the multitrack learned the
     * hard way: an application already open answers *its* host, and overwriting
     * that with the one a second window opened on sends every acknowledgement
     * to the wrong place — silently, since in the ordinary case the two are the
     * same object.
     *
     * Async where the Python client's is not, for the reason `View.open` is
     * async here: resolving the ambient host may have to boot it.
     */
    async resolve(host?: GuiHost): Promise<GuiHost> {
        if (this.host === null) {
            const { resolveEditorHost } = await import("./editor.ts");
            this.host = await resolveEditorHost(host);
        }
        return this.host;
    }

    // ---- the widget-id space, and its two doors ----

    /**
     * The table `drawer` names widgets in.
     *
     * The **host's** once there is one, so that two applications on one host —
     * two editors opened on the ambient host, say — cannot hand out the same
     * number. Before that, one private table per drawer: an unopened draw's ids
     * reach nothing, so they need not be unique across drawers, and the privacy
     * is what lets such a draw restart its numbering.
     *
     * Both doors come from whichever table it is — a leased id and a named one
     * are the same resource taken two ways, and splitting them across two
     * tables is how two widgets end up with one number.
     */
    #ids(drawer?: object): GuiIdAllocator {
        const held = this.host?.ids;
        if (held !== undefined) return held;
        const who = drawer ?? ANYONE;
        let table = this.#offline.get(who);
        if (table === undefined) {
            table = new GuiIdAllocator(this.#baseId, CAPACITY);
            this.#offline.set(who, table);
        }
        return table;
    }

    /**
     * `drawer`'s owner in `table`, minted once per pair.
     *
     * Per pair rather than once, because a drawer that drew before it was
     * opened has already named widgets in a table the host knows nothing about:
     * each table hands out its own drawers.
     */
    #owner(drawer: object | undefined, table: GuiIdAllocator): number {
        const who = drawer ?? ANYONE;
        let mine = this.#owners.get(who);
        if (mine === undefined) {
            mine = new WeakMap();
            this.#owners.set(who, mine);
        }
        let owner = mine.get(table);
        if (owner === undefined) {
            owner = table.owner();
            mine.set(table, owner);
        }
        return owner;
    }

    /**
     * A **leased** widget id: one for a widget nothing names — a hand-built
     * tree, a decoration. It changes across redraws, which is why a view that
     * draws a structure asks {@link Application.idFor} instead.
     */
    newId(drawer?: object): number {
        return this.#ids(drawer).alloc();
    }

    /**
     * The id that draws `(structure, role, key)` — the **same** number for as
     * long as `drawer` keeps drawing that name.
     *
     * The door a view takes, and the whole of what makes a redraw safe: an
     * edit-back in flight, a correction on its way out and a widget's screen
     * state all name an id, and an id that changed under them lands on somebody
     * else.
     */
    idFor(structure: number, role: string, key = "", drawer?: object): number {
        const table = this.#ids(drawer);
        return table.idFor(this.#owner(drawer, table), Math.trunc(structure), role, String(key));
    }

    /**
     * Start `drawer`'s draw: from here, every name it asks for counts as drawn,
     * and {@link Application.retireIds} takes back the rest.
     *
     * Every other drawer is untouched — the cycle names whose it is, so two
     * editors on one host redraw independently.
     */
    resetIds(drawer?: object): void {
        const table = this.#ids(drawer);
        if (table !== this.host?.ids) {
            // **Nothing outside this draw holds one of these ids.** With no
            // host there is no window, no pending gesture and no second drawer
            // in this table, so it starts over and two draws of one picture
            // come out identical — the property a test that inspects a tree
            // twice rests on. On a host it would be wrong: the leases there
            // belong to every window the page has open, not to whoever is
            // drawing.
            table.clear();
            this.#owners.get(drawer ?? ANYONE)?.delete(table);
        }
        table.begin(this.#owner(drawer, table));
    }

    /**
     * End `drawer`'s draw and take back every name it stopped drawing,
     * answering the ids released. A draw that named nothing releases nothing.
     */
    retireIds(drawer?: object): number[] {
        const table = this.#ids(drawer);
        return table.retire(this.#owner(drawer, table));
    }

    // ---- the history walk ----

    /**
     * One step of the pile, **handed round the context**.
     *
     * The history applies nothing: what comes back is the legs each structure
     * has to apply, so the step is offered to **every editor in the context**
     * and each takes the ones naming the structure it holds. An editor that
     * walked only its own legs would step the cursor over somebody else's edit
     * and undo nothing, which looks exactly like a dead button.
     *
     * `walker` is whoever asked, and it is the one that draws afterwards: every
     * other window is told on the way out of the turn, the way it is told about
     * any edit, so a step is one answer per window rather than two.
     */
    /**
     * What the **last** step could not reach, when a walk was refused because no
     * participant held the structure the entry names — the label of the edit
     * that is waiting, for whoever wants to say why nothing happened. `null`
     * after a step that landed, and after one there was nothing to take.
     */
    unreachable: string | null = null;

    step(direction: "undo" | "redo", walker: Drawing): boolean {
        const context = this.context;
        if (context === null) return false;
        this.unreachable = null;
        const history = context.history;
        const waiting = direction === "undo" ? history?.undoLabel : history?.redoLabel;
        const before: [string | undefined, string | undefined] | null =
            history === null ? null : [history.undoLabel, history.redoLabel];
        const legs = context.step(direction);
        if (legs === undefined) {
            log.debug("%s   nothing stepped (at %s)", direction, before);
            return false;
        }
        if (!context.distribute(legs, walker)) {
            // **A step nobody could apply is not a step.** The walk moves the
            // pile's cursor before anything is projected, so an entry naming a
            // structure no participant holds — a box whose window was closed —
            // was stepped *over*: the edit stayed and the order lost it, which
            // is the one thing a history may not do. So the cursor goes back and
            // the answer is "nothing happened", which is true and recoverable:
            // open that window and the entry is still on top, waiting.
            context.step(direction === "undo" ? "redo" : "undo");
            this.unreachable = waiting ?? null;
            log.debug("%s   nothing could apply it (at %s)", direction, before);
            return false;
        }
        log.debug(
            "%s   %s -> %s",
            direction,
            before,
            history === null ? null : [history.undoLabel, history.redoLabel],
        );
        // **Once for the walk, not once per window.** The version is the
        // context's, and every view reports the same one.
        context.version += 1;
        walker.reflectStep();
        return true;
    }

    // ---- publishing a picture ----

    /**
     * Make the host draw `tree` for `widgetId` — **the whole tree, every
     * time**.
     *
     * A `/gui_def` over a tree the host is already drawing says *what to look
     * like* rather than what to destroy, and the host **reconciles**, matching
     * widget to widget by the id that names what it draws and keeping what is
     * its own. So this client holds no picture of the host's: a difference is
     * only correct against a copy of what the host holds, and it cannot have
     * one — the host mutates on its own (a drag writes an offset per frame, a
     * wheel writes a window) and screen state is reported by nothing, correctly,
     * because screen state is the host's.
     *
     * **What that leaves the caller is the granularity, and it is the caller's
     * for a reason.** `/gui_def` names any widget, so publish the one your edit
     * touched. Measured over a drag, the subtree of the clip that moved is flat
     * in the size of the piece; the window is not, and on a large one it is
     * megabytes a second of JSON for a gesture that touched one rectangle. Name
     * a `window` to publish a part of it: the names under the old subtree go and
     * the rest of the window keeps the ones it had.
     */
    publish(
        widgetId: number,
        tree: GuiNode,
        blobs: readonly Uint8Array[] = [],
        window?: number,
    ): void {
        const host = this.host;
        if (host === null) return;
        const id = Math.trunc(widgetId);
        log.debug(
            "publish %s: %d widget(s)%s",
            id,
            widgets(tree),
            window === undefined ? "" : ` inside window ${window}`,
        );
        if (window === undefined) host.define(id, tree, blobs);
        else host.redefine(id, tree, blobs, Math.trunc(window));
    }

    /**
     * Hold until `until()` is false — the drain the host's own `waitWhile` is,
     * so a page ends on one call. `true` when the condition cleared, `false`
     * when `timeout` ran out first. With no host there is nothing to wait for,
     * and the answer is whether the condition is already clear.
     */
    async wait(until: () => boolean, timeout?: number): Promise<boolean> {
        const host = this.host;
        if (host === null) return !until();
        return host.waitWhile(until, timeout);
    }
}

/**
 * How many widgets a published tree holds — the trace's measure of what a redraw
 * cost, now that how much of it the host rebuilds is the host's.
 */
function widgets(tree: GuiNode): number {
    const children = (tree.children ?? []) as unknown[];
    let n = 1;
    for (const child of children) {
        if (child !== null && typeof child === "object") n += widgets(child as GuiNode);
    }
    return n;
}
