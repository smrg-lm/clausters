/**
 * The editing context of one structure -- **whose history it is**.
 *
 * An undo stack belongs to the data, not to the view. Two windows over one
 * structure share a history, and an undo in either updates both; a stack
 * minted per editor sees only the gestures *that* editor made, so stepping one
 * of them reverts across the other's edits and writes a state nobody was ever
 * in. The crate placed its pile beside the data for exactly that reason, and
 * this is the same argument one level up: an editor asks the *data* for its
 * editing context instead of building one of its own.
 *
 * **The context is the shared crate's** (`EditingCore`): the history, the
 * version, and its **members** -- the multitrack and samples editors opened in
 * it, whose turns and steps it takes, and the structures the crate does not
 * apply (a curve, a timeline, a score), which join as external members and get
 * their legs back. No application holds a history of its own: one opened alone
 * is a context of one, and two opened in the same one walk one order. What is
 * here is what a language owns -- the objects each member edits, what puts a
 * step back onto them, and the windows to tell.
 *
 * What stays a view's is what a view can see -- its selection, its zoom, which
 * layer the hand is on. Those never enter a history either, which is the same
 * line drawn twice.
 *
 * The context is reached through {@link Editing.of}, which keeps it in a
 * `WeakMap` keyed by the structure: what is edited is loose objects, so the
 * object itself is the only thing two editors are guaranteed to have in common.
 * It lives as long as the data does and dies with it, which is what the crate's
 * own rule asks for -- a history is session state, never serialized, and it goes
 * when the data goes.
 *
 * @module
 */

import { EditingCore } from "../../core/clausters_core_web.js";

/**
 * The version an unedited context is at. One rather than zero, because zero is
 * what an edit means by *unstated* when it names the state it was made against
 * -- the same reservation the GUI host's sequence numbers make.
 *
 * It is the same number as `form/document.ts`'s `FIRST_VERSION` and deliberately
 * not the same symbol: that one is what a **file** says its version is, this one
 * is what an editing context counts from.
 */
export const FIRST_VERSION = 1;

/** What can put one payload of a history step back onto a structure. */
export interface Applier {
    project(structure: never | object, payload: unknown): boolean;
}

/**
 * What carries a step out for a member: an {@link Applier}, and -- for a member
 * whose structure the crate applied the step to itself -- what writes that back
 * onto the object a page holds.
 */
export interface StepHandler extends Applier {
    stepped?(structure: never | object, applied: Record<string, unknown>): void;
    /** An audio editor's: carry out the steps that stitch its take again. */
    run?(structure: never | object, steps: unknown[]): void;
    /** An audio editor's: free takes nothing reaches any more. */
    free?(structure: never | object, buffers: number[]): void;
}

/**
 * What a context needs of a view: something with a window it can bring back in
 * step. {@link Editor} is the one that implements it.
 */
export interface Adopting {
    /**
     * Another view of this structure edited it: bring this window in step.
     *
     * It carries nothing: what {@link Editor.adopt} does is already props, one
     * correction per widget and never a redefine.
     */
    adopt(): void;
    /**
     * The data changed in a turn -- this view's own gesture, another's, or a
     * step of the history.
     *
     * Separate from {@link Adopting.adopt}, which is about *drawing*: a page
     * that sounds a multitrack or writes a file wants to be told whoever made the
     * edit, and the window that made it is not exempt from having changed.
     */
    dataChanged?(): void;
}

/** One leg of an entry a page records for an external member. */
export interface RecordingLeg {
    /** The structure, by the identity the context named it. */
    structure: number;
    /** `{"edit": <payload>}`. */
    forward: unknown;
    /** The payload that puts it back. */
    backward: unknown;
    /** What makes two edits the same thing done the same way. */
    key?: string | null;
}

/** One thing a step does, for a page to carry out. */
export interface Effect {
    /** `"multitrack"`, `"samples"`, `"audio"` or `"external"`. */
    kind: string;
    /** The member it is for. */
    member: number;
    /** A multitrack member's: what applying the step did to the multitrack. */
    applied?: Record<string, unknown>;
    /** A take's writes, or an external member's payloads. */
    payloads?: unknown[];
    /** An audio editor's: the steps that stitch its take again. */
    steps?: unknown[];
}

/**
 * **Takes a member made that nothing reaches any more**: the buffers to free,
 * on that member's server.
 */
export interface Freed {
    /** The member whose takes they were. */
    member: number;
    /** The buffers. */
    buffers: number[];
}

/** **What a step of the order came to.** */
export interface Stepped {
    /** Whether anything moved. */
    stepped?: boolean;
    /** Why nothing did, where the crate says. */
    reason?: string;
    /** What each member carries out. */
    effects?: Effect[];
    /** The takes to free, once the effects are carried out. */
    freed?: Freed[];
    /** The version after the step. */
    version?: number;
}

/** **What one message to a member came to.** */
export interface Turned {
    /** The member's own outcome: its entry is recorded, its answer is to send. */
    outcome?: Record<string, unknown>;
    /** The step an undo or a redo asked for, already taken. */
    stepped?: Stepped;
    /** The takes to free, once the turn's own steps are carried out. */
    freed?: Freed[];
    /** The version after the turn. */
    version?: number;
}

/** Where a structure's context lives, keyed by the object it belongs to. */
export const contexts = new WeakMap<object, Editing>();

/** Each object's key in a context, since a page has no object address. */
const keys = new WeakMap<object, number>();
let nextKey = 1;

/**
 * A key naming `object` for as long as it lives -- what a member declares when
 * the structure is a page's object rather than something the crate can name.
 */
export function keyOf(prefix: string, object: object): string {
    let key = keys.get(object);
    if (key === undefined) {
        key = nextKey++;
        keys.set(object, key);
    }
    return `${prefix}:${key}`;
}

/**
 * One editing context: its history, its members, and the views drawing them.
 *
 * Not built through `new` for an editor: {@link Editing.of} is the door, so two
 * editors over one thing cannot end up with two. A context built by hand is one
 * a page hands several editors (`context`) so they share an order.
 */
export class Editing {
    /** The context itself, in the shared crate. */
    #core: EditingCore | null = new EditingCore();
    /**
     * The version -- the counter a view reports to its host and the host names
     * back on its next gesture. The crate's: read back from every turn, step and
     * record, and never moved here.
     */
    version = FIRST_VERSION;
    /** The member each object first joined as, and the identity it was named. */
    protected readonly structures = new Map<object, { member: number; identity: number }>();
    /**
     * What carries a step out for each member. Kept **here** rather than on a
     * window, because the order is the context's: a step must reach a structure
     * whether or not a window over it is open.
     */
    protected readonly handlers = new Map<
        number,
        { structure: object; handler: StepHandler | null }
    >();
    protected readonly attached = new Set<WeakRef<Adopting>>();
    /**
     * How deep the current turn is, and whether anything moved in it. One
     * gesture can reach here twice, and the other windows want *one* redraw.
     */
    protected depth = 0;
    protected changedInTurn = false;

    /**
     * The context of this structure, made on first ask.
     *
     * Every editor over one element gets the same one -- the whole point, and
     * the reason this is a static rather than a constructor.
     */
    static of<T extends Editing>(this: new () => T, structure: object): T {
        let context = contexts.get(structure);
        if (context === undefined) {
            context = new this();
            contexts.set(structure, context);
        }
        return context as T;
    }

    #call(verb: string, args: Record<string, unknown> = {}): Record<string, unknown> {
        if (this.#core === null) return {};
        return JSON.parse(this.#core.call(JSON.stringify({ verb, ...args }))) as Record<
            string,
            unknown
        >;
    }

    // ---- members ----

    /**
     * **Open an editor in this context** -- `verb` is `"openMultitrack"` or
     * `"openSamples"` -- as the structure `key` names, and answer its member and
     * identity. `handler` is what carries a step out for it: the editor's
     * domain. Throws with the crate's reason when it refuses the request.
     */
    open(
        verb: string,
        key: string,
        request: Record<string, unknown>,
        structure: object,
        handler: StepHandler | null,
    ): { member: number; identity: number } {
        const answer = this.#call(verb, { key, ...request });
        if (typeof answer.error === "string" || answer.member === undefined) {
            throw new Error(`clausters: ${String(answer.error ?? "the context opened nothing")}`);
        }
        const opened = { member: Number(answer.member), identity: Number(answer.structure) };
        this.handlers.set(opened.member, { structure, handler });
        if (!this.structures.has(structure)) this.structures.set(structure, opened);
        return opened;
    }

    /**
     * This structure's identity in the order, joining it as an **external
     * member** on first ask, with **what can put an edit back onto it**.
     *
     * **Once per structure, not once per view.** Two windows over one thing are
     * one structure in the undo order, so a second identity for the second
     * window would leave its undo walking legs that name somebody else. A
     * structure an editor opened in the crate already has its identity, and
     * this answers it.
     */
    identity(structure: object, domain: string, applier: Applier | null = null): number {
        const found = this.structures.get(structure);
        if (found === undefined) {
            return this.open("external", keyOf("object", structure), { domain }, structure, applier)
                .identity;
        }
        // Joined by something that could not apply (a caller that only wanted
        // the number); the first that can, wins the slot.
        const held = this.handlers.get(found.member);
        if (applier !== null && (held === undefined || held.handler === null)) {
            this.handlers.set(found.member, { structure, handler: applier });
        }
        return found.identity;
    }

    /** One verb of a member's own door, with the version filled in by the context. */
    member(member: number, verb: string, args: Record<string, unknown> = {}): Record<string, unknown> {
        return this.#call("member", { member, call: { verb, ...args } });
    }

    #memberOf(identity: number): number | undefined {
        for (const held of this.structures.values()) {
            if (held.identity === identity) return held.member;
        }
        return undefined;
    }

    // ---- the order ----

    /**
     * **Record an entry** an external member applied itself: its legs over one
     * structure. Answers whether the history took it; the version moves when it
     * did. Throws when the legs name more than one structure -- an entry over
     * several is a turn of an application, which records its own.
     */
    record(
        legs: readonly RecordingLeg[],
        { label = "edit", coalesce = false }: { label?: string; coalesce?: boolean } = {},
    ): boolean {
        if (this.#core === null || legs.length === 0) return false;
        const structures = new Set(legs.map((leg) => Number(leg.structure)));
        if (structures.size !== 1) {
            throw new Error("clausters: an entry recorded here is over one structure");
        }
        const member = this.#memberOf([...structures][0]!);
        if (member === undefined) return false;
        const answer = this.#call("record", {
            member,
            label,
            coalesce,
            legs: legs.map((leg) => ({
                forward: leg.forward,
                backward: leg.backward,
                key: leg.key ?? "",
            })),
        });
        this.version = Number(answer.version ?? this.version);
        return answer.recorded === true;
    }

    /**
     * An edit that leaves no entry -- one with no inverse to record -- still moves
     * the version. Answers it.
     */
    moved(): number {
        if (this.#core !== null) this.version = Number(this.#call("moved").version ?? this.version);
        return this.version;
    }

    /**
     * **Limits the takes only the history holds** to `bytes`, or lifts the
     * limit with `null`. Past it the oldest entries go first, and the takes
     * they held come back to be freed.
     */
    limitBytes(bytes: number | null): void {
        if (this.#core !== null) this.#call("bytes", { bytes });
    }

    /** **One message to a member**, read, recorded and answered by the crate. */
    event(member: number, addr: string, args: unknown[]): Turned {
        const turned = this.#call("event", { member, addr, args }) as Turned | null;
        if (turned === null) return {};
        this.version = Number(turned.version ?? this.version);
        return turned;
    }

    /**
     * **Take one step of the order.** The step is already taken in the crate;
     * {@link Editing.carry} is what puts it back onto the objects here.
     */
    step(direction: "undo" | "redo"): Stepped {
        if (this.#core === null) return { stepped: false };
        const stepped = this.#call("step", { direction }) as Stepped;
        this.version = Number(stepped.version ?? this.version);
        return stepped;
    }

    /**
     * **Carry a step's effects out** on the structures they name: a multitrack
     * written back, a take's writes projected, an audio editor's join stitched
     * again, an external member's payloads applied -- and then the takes the
     * step let go of freed.
     */
    carry(stepped: Stepped): void {
        for (const effect of stepped.effects ?? []) {
            const held = this.handlers.get(Number(effect.member));
            if (held === undefined || held.handler === null) continue;
            if (effect.kind === "multitrack") {
                held.handler.stepped?.(held.structure as never, effect.applied ?? {});
                continue;
            }
            if (effect.kind === "audio") {
                held.handler.run?.(held.structure as never, effect.steps ?? []);
                continue;
            }
            for (const payload of effect.payloads ?? []) {
                if (payload !== null && typeof payload === "object") {
                    held.handler.project(held.structure as never, payload);
                }
            }
        }
        this.release(stepped.freed);
    }

    /**
     * **Free the takes nothing reaches any more** -- what a turn or a step hands
     * back as `freed`, each list under the member that made those takes, whose
     * server they are on. Called after the turn's own steps are carried out, so
     * no join is still reading them.
     */
    release(freed: readonly Freed[] | undefined): void {
        for (const entry of freed ?? []) {
            const held = this.handlers.get(Number(entry.member));
            if (held === undefined || held.handler === null) continue;
            held.handler.free?.(held.structure as never, entry.buffers ?? []);
        }
    }

    #state(): { canUndo?: boolean; canRedo?: boolean; undoLabel?: string | null; redoLabel?: string | null } {
        return this.#call("state");
    }

    /** Whether there is an edit to step back over. */
    get canUndo(): boolean {
        return this.#state().canUndo === true;
    }

    /** Whether there is an undone edit to step forward into. */
    get canRedo(): boolean {
        return this.#state().canRedo === true;
    }

    /** What an undo would be called, for a menu item. */
    get undoLabel(): string | undefined {
        return this.#state().undoLabel ?? undefined;
    }

    /** What a redo would be called, for a menu item. */
    get redoLabel(): string | undefined {
        return this.#state().redoLabel ?? undefined;
    }

    // ---- the views ----

    /**
     * Take a view into this data's list, so an edit made in one window can reach
     * the others.
     */
    attach(view: Adopting): void {
        for (const held of this.attached) if (held.deref() === view) return;
        this.attached.add(new WeakRef(view));
    }

    /** The views still alive, dropping the ones that are not. */
    views(): Adopting[] {
        const alive: Adopting[] = [];
        for (const held of [...this.attached]) {
            const view = held.deref();
            if (view === undefined) this.attached.delete(held);
            else alive.push(view);
        }
        return alive;
    }

    /** Drop a view whose window is gone. */
    detach(view: Adopting): void {
        for (const held of this.attached) {
            const alive = held.deref();
            if (alive === undefined || alive === view) this.attached.delete(held);
        }
    }

    /**
     * Say that the data changed in the turn being run.
     *
     * Not the notification: a turn can reach here more than once, and what the
     * other windows want is one answer at the end of the gesture rather than
     * one per leg of it.
     */
    changed(): void {
        this.changedInTurn = true;
    }

    /**
     * One gesture, from whichever view made it.
     *
     * On the way out, every **other** view of this data is told what it is
     * drawing has moved -- which nothing else would do: an acknowledgement
     * goes to the window whose gesture it answered, so a second window would go
     * on drawing a multitrack that had changed under it. Nested turns collapse into
     * one, because a gesture that reaches here twice is still one gesture.
     */
    turn<T>(source: Adopting, run: () => T): T {
        this.depth += 1;
        try {
            return run();
        } finally {
            this.depth -= 1;
            if (this.depth === 0) {
                const changed = this.changedInTurn;
                this.changedInTurn = false;
                if (changed) {
                    for (const held of [...this.attached]) {
                        const view = held.deref();
                        if (view === undefined) this.attached.delete(held);
                        else if (view !== source) view.adopt();
                    }
                    // ...and **every** view is told the data changed, the one
                    // that made the gesture included, whether or not it is
                    // attached. See {@link Adopting.dataChanged}.
                    const tell: Adopting[] = [];
                    for (const held of [...this.attached]) {
                        const view = held.deref();
                        if (view !== undefined) tell.push(view);
                    }
                    if (source !== null && !tell.includes(source)) tell.push(source);
                    for (const view of tell) view.dataChanged?.();
                }
            }
        }
    }

    /**
     * Release the crate's context, with every editor opened in it. What the
     * data going away leaves behind; a view closing is not an event of a
     * history.
     */
    free(): void {
        this.#core?.free();
        this.#core = null;
    }
}
