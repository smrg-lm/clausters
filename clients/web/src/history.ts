/**
 * The editing context of one structure -- **whose history it is** -- and the
 * {@link UndoHistory} a page reads it through.
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
 * **It needs no window, so it is not the GUI's.** A page's change to a
 * structure that has a history -- a sequence an editor is open on, or one a
 * page asked for (`seq.history`) -- is recorded in it as a turn, the same one a
 * gesture is: the windows over it are brought in step, the playback reading it
 * is synced, and an undo in either a window or the page walks one order. A
 * structure nobody asked a history of changes freely and records nothing, so a
 * page that writes ten thousand notes keeps no ten thousand inverses.
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

import { EditingCore } from "./core/clausters_core_web.js";
import type { EventSequence } from "./seq/sequence.ts";
import type { Multitrack } from "./multitrack.ts";
import type { Score } from "./gui/notation/engraver.ts";

/**
 * **What a history asks of a structure a page changes** -- a sequence, a
 * multitrack -- through its objects: its own door, the key and the domain it
 * joins a history under, and the edit a redo applies. A structure that can be
 * restored whole says how; one that cannot is recorded as the edits made.
 */
export interface ScriptStructure {
    applyIntent(
        intent: Record<string, unknown>,
        inverse?: boolean,
    ): { applied: boolean; current?: unknown; id?: number; reason?: string };
    scriptKey(): [string, string];
    forwardOf(intent: Record<string, unknown>, minted: number | undefined): Record<string, unknown>;
    restore?(): Record<string, unknown>;
}

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
    /** A points editor's: write the curve's points back, as flat quads. */
    write?(structure: never | object, points: readonly number[]): void;
    /** An audio editor's: carry out the steps that stitch its take again. */
    run?(structure: never | object, steps: unknown[]): void;
    /**
     * An audio editor's: free takes nothing reaches any more. A take on disk
     * (`spilled`) has no buffer to free, only its number to give back.
     */
    free?(structure: never | object, buffers: number[], spilled: number[]): void;
    /** An audio editor's: write a take to disk and free its buffer. */
    store?(structure: never | object, buffer: number, steps: unknown[]): void;
    /**
     * A multitrack's: free joins no box reads and no undo or redo can put
     * back, by source id -- the buffer built for each, and its place in the
     * table.
     */
    freeSources?(structure: never | object, sources: number[]): void;
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
    /** `"multitrack"`, `"samples"`, `"audio"`, `"points"` or `"external"`. */
    kind: string;
    /** The member it is for. */
    member: number;
    /** A multitrack member's: what applying the step did to the multitrack. */
    applied?: Record<string, unknown>;
    /** A take's writes, or an external member's payloads. */
    payloads?: unknown[];
    /** An audio editor's: the steps that stitch its take again. */
    steps?: unknown[];
    /** A points editor's: the curve's points, as flat quads, to write back. */
    points?: number[];
}

/**
 * **Takes a member made that nothing reaches any more**: the buffers to free,
 * on that member's server.
 */
export interface Freed {
    /** The member whose takes they were. */
    member: number;
    /** The buffer numbers, to give back: an audio editor's takes. */
    buffers?: number[];
    /** Those whose take was on disk: no buffer on the server to free. */
    spilled?: number[];
    /** A multitrack's joins, by source id, to free and drop from its table. */
    sources?: number[];
}

/**
 * **A take leaving memory**: only the history holds it, and the resident budget
 * is past.
 */
export interface Stored {
    /** The member whose take it is. */
    member: number;
    /** Its buffer, which stays its number while it is on disk. */
    buffer: number;
    /** The steps that write it and free the buffer. */
    steps: unknown[];
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
    /** The takes to write to disk, once the effects are carried out. */
    stored?: Stored[];
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
    /** The takes to write to disk, once the turn's own steps are carried out. */
    stored?: Stored[];
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
     * The {@link UndoHistory.entry} blocks open over a structure: one entry,
     * recorded when the outermost closes.
     */
    readonly #blocks = new Map<object, {
        depth: number;
        before: unknown;
        changed: boolean;
        legs: { structure: number; forward: unknown; backward: unknown }[] | null;
    }>();
    /**
     * How deep an editor's or a step's own write is. A change it makes through
     * a structure's objects -- a points editor over a held curve writes the
     * curve into its sequence -- is part of the entry that editor or step
     * records, not one of its own.
     */
    #applying = 0;

    /**
     * The context of this structure, made on first ask.
     *
     * Every editor over one element gets the same one -- the whole point, and
     * the reason this is a static rather than a constructor. A structure
     * another one holds -- a curve a sequence holds -- names that one as its
     * `historyOwner`, and shares its context: a window over the curve and a
     * roll over the sequence are one order.
     */
    static of<T extends Editing>(this: new () => T, structure: object): T {
        const owner = (structure as { historyOwner?: object | null }).historyOwner;
        if (owner !== undefined && owner !== null) return (this as unknown as typeof Editing).of(owner) as T;
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
     * **Open an editor in this context** -- `verb` is `"openMultitrack"`,
     * `"openAudio"` or `"openPoints"` -- as the structure `key` names, and answer its member and
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
     * **Open a multitrack editor over `multitrack`** -- a `Multitrack`, which
     * the editor then edits in place -- as the structure `key` names, and
     * answer its member and identity. Throws with the crate's reason when it
     * refuses.
     */
    openMultitrack(
        key: string,
        multitrack: Multitrack,
        request: Record<string, unknown>,
        handler: StepHandler | null,
    ): { member: number; identity: number } {
        if (this.#core === null) throw new Error("clausters: this context is closed");
        const answer = JSON.parse(
            this.#core.openMultitrack(multitrack.handle, JSON.stringify({ key, ...request })),
        ) as Record<string, unknown>;
        if (typeof answer.error === "string" || answer.member === undefined) {
            throw new Error(`clausters: ${String(answer.error ?? "the context opened nothing")}`);
        }
        const opened = { member: Number(answer.member), identity: Number(answer.structure) };
        this.handlers.set(opened.member, { structure: multitrack, handler });
        if (!this.structures.has(multitrack)) this.structures.set(multitrack, opened);
        this.claim(multitrack);
        return opened;
    }

    /**
     * **Open a notes editor over `sequence`** -- an `EventSequence`, which the
     * editor then edits in place -- as the structure `key` names, and answer its
     * member and identity. Throws with the crate's reason when it refuses.
     */
    openNotes(
        key: string,
        sequence: EventSequence,
        request: Record<string, unknown>,
        handler: StepHandler | null,
    ): { member: number; identity: number } {
        if (this.#core === null) throw new Error("clausters: this context is closed");
        const answer = JSON.parse(
            this.#core.openNotes(sequence.seq, JSON.stringify({ key, ...request })),
        ) as Record<string, unknown>;
        if (typeof answer.error === "string" || answer.member === undefined) {
            throw new Error(`clausters: ${String(answer.error ?? "the context opened nothing")}`);
        }
        const opened = { member: Number(answer.member), identity: Number(answer.structure) };
        this.handlers.set(opened.member, { structure: sequence, handler });
        if (!this.structures.has(sequence)) this.structures.set(sequence, opened);
        this.claim(sequence);
        return opened;
    }

    /**
     * **Open a score editor over `sequence`**, on the page it is read into --
     * `score` is the `Score` the page is drawn from, loaded by the crate with
     * the sequence read -- as the structure `key` names, which is the
     * sequence's: the editor's entries are the sequence's, one order with
     * every roll over it. Answers its member and identity, and the sequence
     * is claimed. Throws with the crate's reason when it refuses.
     */
    openScoreOver(
        key: string,
        score: Score,
        sequence: EventSequence,
        request: Record<string, unknown>,
        handler: StepHandler | null,
    ): { member: number; identity: number } {
        if (this.#core === null) throw new Error("clausters: this context is closed");
        const answer = JSON.parse(
            this.#core.openScoreOver(
                score.handle,
                sequence.seq,
                JSON.stringify({ key, ...request }),
            ),
        ) as Record<string, unknown>;
        if (typeof answer.error === "string" || answer.member === undefined) {
            throw new Error(`clausters: ${String(answer.error ?? "the context opened nothing")}`);
        }
        const opened = { member: Number(answer.member), identity: Number(answer.structure) };
        this.handlers.set(opened.member, { structure: sequence, handler });
        if (!this.structures.has(sequence)) this.structures.set(sequence, opened);
        this.claim(sequence);
        return opened;
    }

    /**
     * **Open a score editor over `score`** -- a symbolic `Score`, which the
     * editor then edits in place -- as the structure `key` names, and answer
     * its member and identity. The score is claimed: an edit a page makes
     * through it is a turn of this context. Throws with the crate's reason when
     * it refuses.
     */
    openScore(
        key: string,
        score: Score,
        request: Record<string, unknown>,
        handler: StepHandler | null,
    ): { member: number; identity: number } {
        if (this.#core === null) throw new Error("clausters: this context is closed");
        const answer = JSON.parse(
            this.#core.openScore(score.handle, JSON.stringify({ key, ...request })),
        ) as Record<string, unknown>;
        if (typeof answer.error === "string" || answer.member === undefined) {
            throw new Error(`clausters: ${String(answer.error ?? "the context opened nothing")}`);
        }
        const opened = { member: Number(answer.member), identity: Number(answer.structure) };
        this.handlers.set(opened.member, { structure: score, handler });
        if (!this.structures.has(score)) this.structures.set(score, opened);
        this.claim(score);
        return opened;
    }

    /**
     * **One verb a page calls on a member** -- `call` as that member's verbs
     * read it -- recorded and answered by the crate like a gesture: the turn,
     * with the corrections it owes.
     */
    act(member: number, call: Record<string, unknown>): Turned {
        const turned = this.#call("act", { member, call }) as Turned | null;
        if (turned === null) return {};
        this.version = Number(turned.version ?? this.version);
        return turned;
    }

    /**
     * **Bind a multitrack member's `source` to `sequence`** -- an
     * `EventSequence` -- so a region over it draws the sequence's notes;
     * answers the member's corrected picture.
     */
    bindSequence(member: number, source: number, sequence: EventSequence): Record<string, unknown> {
        if (this.#core === null) throw new Error("clausters: this context is closed");
        this.claim(sequence);
        return JSON.parse(
            this.#core.bindSequence(sequence.seq, JSON.stringify({ member, source })),
        ) as Record<string, unknown>;
    }

    /**
     * **Makes this the structure's context**, so {@link Editing.of} and the
     * structure's own {@link UndoHistory} answer it. An editor opened in a context
     * the caller handed it claims what it edits: the windows are here, so a
     * page's change has to be a turn here to reach them.
     */
    claim(structure: object): void {
        contexts.set(structure, this);
    }

    /**
     * The identity in the order of a structure a page changes -- an editor's
     * when one is open on it, else an external member joined now, keyed as
     * that editor's would be so the two are one structure.
     */
    #scriptIdentity(structure: ScriptStructure): number {
        const found = this.structures.get(structure);
        if (found !== undefined) return found.identity;
        const [key, domain] = structure.scriptKey();
        return this.open("external", key, { domain }, structure, {
            project: (held, payload) =>
                (held as ScriptStructure).applyIntent(payload as Record<string, unknown>, false).applied,
        }).identity;
    }

    /**
     * Runs an editor's or a step's own write: a change made through objects
     * inside it is applied and told to the views, and recorded by nobody but
     * the entry the editor or the step already stands for.
     */
    applying<T>(run: () => T): T {
        this.#applying += 1;
        try {
            return run();
        } finally {
            this.#applying -= 1;
        }
    }

    /**
     * **One change a page makes to a structure in this context** -- a
     * sequence, a multitrack -- as a turn: applied, recorded -- as its own
     * entry, or into the {@link UndoHistory.entry} block open over the
     * structure -- and every view over it told. Answers what the structure's
     * door answers.
     */
    scriptEdit(
        structure: ScriptStructure,
        intent: Record<string, unknown>,
        label: string,
    ): { applied: boolean; current?: unknown; id?: number; reason?: string } {
        const block = this.#blocks.get(structure);
        return this.turn(null, () => {
            if (this.#applying > 0) {
                const answer = structure.applyIntent(intent, false);
                if (answer.applied) this.changed();
                return answer;
            }
            if (block !== undefined && block.legs === null) {
                // A block over a structure that is restored whole: its state
                // before and after are the entry.
                const answer = structure.applyIntent(intent, false);
                if (answer.applied) {
                    block.changed = true;
                    this.changed();
                }
                return answer;
            }
            const answer = structure.applyIntent(intent, true);
            if (answer.applied) {
                const leg = {
                    structure: this.#scriptIdentity(structure),
                    forward: { edit: structure.forwardOf(intent, answer.id) },
                    backward: answer.current,
                };
                if (block !== undefined) {
                    block.changed = true;
                    block.legs!.push(leg);
                } else {
                    this.record([leg], { label });
                }
                this.changed();
            }
            return answer;
        });
    }

    /**
     * Runs `run`, and everything a page changes in the structure inside it is
     * **one** entry, called `label`, and one turn. Blocks nest: an inner one
     * is part of the outer.
     *
     * A structure that can be **restored whole** (a sequence) is recorded as
     * its state before and after; one that cannot (a multitrack, whose
     * vocabulary states its parts) as the edits made, in order, which an undo
     * walks back in reverse.
     */
    block<T>(structure: ScriptStructure, label: string, run: () => T): T {
        const open = this.#blocks.get(structure);
        if (open !== undefined) {
            open.depth += 1;
            try {
                return run();
            } finally {
                open.depth -= 1;
            }
        }
        const whole = structure.restore !== undefined;
        const block = {
            depth: 1,
            before: whole ? structure.restore!() : null,
            changed: false,
            legs: whole ? null : [] as { structure: number; forward: unknown; backward: unknown }[],
        };
        this.#blocks.set(structure, block);
        // Recorded inside the turn, so the views are told the version the
        // entry moved to rather than the one before it.
        return this.turn(null, () => {
            try {
                return run();
            } finally {
                this.#blocks.delete(structure);
                if (block.changed && block.legs === null) {
                    this.record([{
                        structure: this.#scriptIdentity(structure),
                        forward: { edit: structure.restore!() },
                        backward: block.before,
                    }], { label });
                } else if (block.changed) {
                    this.record(block.legs!, { label });
                }
            }
        });
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

    /**
     * **Keeps at most `bytes` of the takes only the history holds in memory**,
     * or lifts the limit with `null`. Past it the oldest are written to disk
     * and read back when a step needs them.
     */
    limitResident(bytes: number | null): void {
        if (this.#core !== null) this.#call("resident", { bytes });
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
        this.applying(() => this.#carry(stepped));
        this.release(stepped.freed, stepped.stored);
    }

    #carry(stepped: Stepped): void {
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
            if (effect.kind === "points") {
                held.handler.write?.(held.structure as never, effect.points ?? []);
                continue;
            }
            for (const payload of effect.payloads ?? []) {
                if (payload !== null && typeof payload === "object") {
                    held.handler.project(held.structure as never, payload);
                }
            }
        }
    }

    /**
     * **Free the takes nothing reaches any more, and write to disk the ones past
     * the resident budget** -- what a turn or a step hands back as `freed` and
     * `stored`, each under the member that made those takes, whose server they
     * are on. Called after the turn's own steps are carried out, so no join is
     * still reading them.
     */
    release(freed: readonly Freed[] | undefined, stored?: readonly Stored[]): void {
        for (const entry of freed ?? []) {
            const held = this.handlers.get(Number(entry.member));
            if (held === undefined || held.handler === null) continue;
            if ((entry.buffers ?? []).length > 0) {
                held.handler.free?.(held.structure as never, entry.buffers ?? [], entry.spilled ?? []);
            }
            if ((entry.sources ?? []).length > 0) {
                held.handler.freeSources?.(held.structure as never, entry.sources ?? []);
            }
        }
        for (const entry of stored ?? []) {
            const held = this.handlers.get(Number(entry.member));
            if (held === undefined || held.handler === null) continue;
            held.handler.store?.(held.structure as never, Number(entry.buffer), entry.steps ?? []);
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
    turn<T>(source: Adopting | null, run: () => T): T {
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

/**
 * **A structure's history, as a page reads it**: the undo order its editors
 * share, which a page's own changes join.
 *
 * `seq.history` is the door; asking for it is what gives a sequence a history,
 * so a page that never asks and opens no editor records nothing. From then on
 * each change made through the sequence's objects is an entry, and a turn:
 * every window over it redraws, the playback reading it is synced, and one
 * Ctrl+Z in a window takes back what the page did.
 */
export class UndoHistory {
    readonly #structure: ScriptStructure;
    readonly #context: Editing;

    /** @internal */
    constructor(structure: ScriptStructure) {
        this.#structure = structure;
        this.#context = Editing.of(structure);
    }

    /**
     * Runs `run`, and makes every change inside it **one** entry, labelled
     * `label` -- the text an undo names -- and one turn, so the windows redraw
     * once, at the end. Answers what `run` answers. Python's
     * `with seq.history(label):`.
     */
    entry<T>(label: string, run: () => T): T {
        return this.#context.block(this.#structure, String(label), run);
    }

    /** Steps back over the last entry -- a page's or a window's -- and answers whether anything moved. */
    undo(): boolean {
        return this.#step("undo");
    }

    /** Steps forward again after {@link UndoHistory.undo}; whether anything moved. */
    redo(): boolean {
        return this.#step("redo");
    }

    #step(direction: "undo" | "redo"): boolean {
        const context = this.#context;
        return context.turn(null, () => {
            const stepped = context.step(direction);
            if (!stepped.stepped) return false;
            context.carry(stepped);
            context.changed();
            return true;
        });
    }

    /** Whether there is an entry to step back over. */
    get canUndo(): boolean {
        return this.#context.canUndo;
    }

    /** Whether there is an undone entry to step forward into. */
    get canRedo(): boolean {
        return this.#context.canRedo;
    }

    /** What an undo would take back, as a menu names it. */
    get undoLabel(): string | undefined {
        return this.#context.undoLabel;
    }

    /** What a redo would bring back. */
    get redoLabel(): string | undefined {
        return this.#context.redoLabel;
    }
}
