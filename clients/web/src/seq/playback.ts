// A sequence played on the server: an event lane on a transport (mirrors
// `clausters/seq/playback.py`).
//
// `play` of an `EventSequence` -- and the notes editor's own playback -- write
// the sequence as the data of an **event lane** on one of the server's
// transports, and the server plays it by the transport's position: a pause, a
// locate and a loop are the transport's, and a note sounding when it stops is
// released and rings out. The steps are the shared crate's (`NotesPlayback`);
// what is here is carrying them out on a server, and the `Transport` the lane
// is on, whose verbs then speak the sequence's beats.
//
// There is **one per sequence** on a server, each on a transport of its own,
// so two sequences sound together and a roll's play cursor is its own
// sequence's. A server has a fixed number of transports (`--transports`): a
// playback takes one when it is made, which fails when none is left, and
// `NotesPlayback.free` -- `Transport.free` -- gives it back.

import { NotesPlayback as CorePlayback, StepRunner } from "../core/clausters_core_web.js";
import type { Server } from "../defs/server/index.ts";
import type { Transport, TransportDriver } from "../defs/server/transport.ts";
import { runSteps } from "../steps.ts";
import { contexts } from "../history.ts";
import type { EventSequence } from "./sequence.ts";

/** What the playback asks of a roll over the sequence it plays. */
export interface Roll {
    structure?: unknown;
    closed?: boolean;
    showSpan?(span: [number, number] | null): void;
    showLooping?(on: boolean): void;
}

/** Where a pass ends: open, where the last note ends, or at a beat. */
export type PassEnd = null | "contents" | number;

/**
 * **What plays one sequence on one server**: the crate's playback, the steps
 * it answers carried out, and the `Transport` the lane is on. Reached through
 * {@link NotesPlayback.of}, so a sequence has one per server.
 *
 * @internal
 */
export class NotesPlayback implements TransportDriver {
    static readonly #of = new WeakMap<Server, Map<EventSequence, NotesPlayback>>();
    static readonly #all = new Set<WeakRef<NotesPlayback>>();

    /**
     * The playback of `sequence` on `server`, made on first ask. Throws when it
     * has to be made and the server has no transport left for it.
     */
    static of(server: Server, sequence: EventSequence): NotesPlayback {
        let held = NotesPlayback.#of.get(server);
        if (held === undefined) {
            held = new Map();
            NotesPlayback.#of.set(server, held);
        }
        let found = held.get(sequence);
        if (found === undefined) {
            found = new NotesPlayback(server, sequence);
            held.set(sequence, found);
            NotesPlayback.#all.add(new WeakRef(found));
        }
        return found;
    }

    /** The playback of `sequence` on `server` when it has one, without making it. */
    static held(server: Server | null, sequence: EventSequence): NotesPlayback | null {
        if (server === null) return null;
        return NotesPlayback.#of.get(server)?.get(sequence) ?? null;
    }

    /**
     * Forgets every playback on `server` -- its close: the bookkeeping goes,
     * and the server is told nothing, as for anything else this handle made
     * on it.
     */
    static forgetAll(server: Server): void {
        for (const playback of [...(NotesPlayback.#of.get(server)?.values() ?? [])]) {
            playback.#forget();
        }
    }

    readonly #native = new CorePlayback(-1);
    readonly #runner = new StepRunner();
    readonly #ready: Promise<void>;
    /** The engine's sample rate. */
    rate = 48_000;
    /** The sequence it plays, held for as long as the playback is. */
    readonly sequence: EventSequence;
    /** The transport it plays on, taken from the server's. */
    readonly transportId: number;
    /** That transport, as the object a page plays. */
    readonly transport: Transport;
    /** Where a pass starts, in the sequence's beats. */
    cursor = 0;
    /** Where a pass ends. */
    end: PassEnd = null;
    /**
     * The time range a pass plays and a loop repeats, `[start, end]` in the
     * sequence's beats, or `null` -- the same one a sweep leaves on a roll,
     * and drawn there.
     */
    span: [number, number] | null = null;
    /** Whether the loop switch is on: the span, or every note with none. */
    looping = false;
    /**
     * Whether a page was handed the transport: a playback only rolls opened
     * is freed with the last of them, one a page holds is the page's to free.
     */
    kept = false;
    readonly server: Server;
    #paused = false;
    #freed = false;
    /**
     * The version of the sequence's context the lane last took: everything
     * over the sequence that hears of a change asks for it, and the lane takes
     * it once.
     */
    #taken: number | null = null;
    /**
     * The calls in flight, one after another: everything over this playback
     * shares its step runner, and two calls whose steps interleave on it wait on
     * steps the other sent.
     */
    #queue: Promise<void> = Promise.resolve();

    private constructor(server: Server, sequence: EventSequence) {
        this.server = server;
        this.sequence = sequence;
        const taken = JSON.parse(
            this.#native.call(sequence.seq, JSON.stringify({ verb: "open" }), server.ids),
        ) as { transport?: number; error?: string };
        if (typeof taken.error === "string") {
            this.#native.free();
            throw new Error(`clausters: ${taken.error}`);
        }
        this.transportId = Number(taken.transport);
        this.transport = server.transportAt(this.transportId);
        this.transport.driver = this;
        this.#ready = (async () => {
            // Node ids come back on their `/node_end`, which only a registered
            // client hears.
            await server.notify(true);
            this.rate = (await server.queryInfo()).nominalSampleRate;
        })();
    }

    /** The transport as the engine has it. */
    async state(): Promise<{ playing: boolean }> {
        const state = await this.transport.view.transportState();
        return { playing: state.playing };
    }

    /** One verb over the sequence, its steps carried out, after the calls before it. */
    call(verb: string, args: Record<string, unknown> = {}): Promise<void> {
        const run = this.#queue.then(() => this.#call(verb, args));
        this.#queue = run.catch(() => {});
        return run;
    }

    async #call(verb: string, args: Record<string, unknown>): Promise<void> {
        if (this.#freed && verb !== "close") return;
        await this.#ready;
        const answer = JSON.parse(this.#native.call(
            this.sequence.seq,
            JSON.stringify({ verb, rate: this.rate, ...args }),
            this.server.ids,
        )) as Record<string, unknown>;
        if (typeof answer.error === "string") throw new RangeError(answer.error);
        const steps = answer.steps as unknown[] | undefined;
        if (steps !== undefined && steps.length > 0) await runSteps(this.server, this.#runner, steps);
    }

    /**
     * **Frees the playback**: what sounds is released, the lane and its groups
     * are freed, and the transport goes back to the server's, for another
     * sequence to take. The `Transport` it answered is then a transport with
     * nothing loaded.
     */
    async free(): Promise<void> {
        if (this.#freed) return;
        this.#freed = true;
        try {
            await this.call("close");
        } finally {
            this.#forget();
        }
    }

    /** The bookkeeping goes, and the server is told nothing. */
    #forget(): void {
        this.#freed = true;
        if (this.transport.driver === this) this.transport.driver = null;
        NotesPlayback.#of.get(this.server)?.delete(this.sequence);
        // After the calls in flight, which read the crate's playback.
        this.#queue = this.#queue.then(() => this.#native.free());
    }

    // ---- what the lane holds ----

    /**
     * **Plays the sequence from beat `at`.** `range` plays that span, going
     * back to `at`; `looping` loops it, or with no range every note; `end` is
     * where a pass ends, kept from the last load when not given.
     */
    async load(
        at = 0,
        { range = null, looping = false, end }: {
            range?: readonly [number, number] | null;
            looping?: boolean;
            end?: PassEnd;
        } = {},
    ): Promise<void> {
        if (end !== undefined) this.end = end;
        const span = range === null ? null : [range[0], range[1]] as [number, number];
        const moved = JSON.stringify(span) !== JSON.stringify(this.span);
        this.span = span;
        this.looping = looping;
        if (moved) this.#show();
        this.cursor = at;
        this.#paused = false;
        await this.call("end", { end: this.end });
        await this.call("play", { from: at, range, loop: looping });
    }

    /**
     * **The lane takes the sequence again**, when it has not taken it at this
     * `version` of its context already: everything over a sequence that hears
     * of a change -- the editor that made it, the others adopting it, the
     * page's own write -- asks, and the lane is one. `null` is a change no
     * context counted, always taken.
     */
    update(version: number | null = null): Promise<void> {
        if (version !== null && this.#taken === version) return Promise.resolve();
        this.#taken = version;
        return this.call("update");
    }

    /** `sequence` changed: every playback of it takes it again. */
    static changed(sequence: EventSequence, version: number | null = null): void {
        for (const held of [...NotesPlayback.#all]) {
            const playback = held.deref();
            if (playback === undefined || playback.#freed) {
                NotesPlayback.#all.delete(held);
                continue;
            }
            if (playback.sequence === sequence) playback.update(version).catch(() => {});
        }
    }

    /**
     * The position cursor was placed at beat `at`: where the next pass starts,
     * and a stopped transport is located there.
     */
    cue(at: number): Promise<void> {
        this.cursor = at;
        return this.call("cue", { at });
    }

    // ---- the transport's verbs, over what is loaded ----

    async playing(): Promise<boolean> {
        const playing = (await this.state()).playing;
        await this.call("setRolling", { rolling: playing });
        return playing;
    }

    async play(at?: number): Promise<void> {
        if (at === undefined && this.#paused) {
            this.#paused = false;
            await this.call("resume");
            return;
        }
        await this.load(at ?? this.cursor, { range: this.span, looping: this.looping });
    }

    async setEnd(end: PassEnd): Promise<void> {
        this.end = end;
        await this.call("end", { end });
    }

    async pause(): Promise<void> {
        this.#paused = true;
        await this.call("pause");
    }

    async stop(): Promise<void> {
        this.#paused = false;
        await this.call("stop", { back: this.cursor });
    }

    async locate(at: number): Promise<void> {
        this.cursor = at;
        if (await this.playing()) await this.load(at, { range: this.span, looping: this.looping });
        else await this.call("cue", { at });
    }

    /**
     * The time range: kept stopped or rolling, drawn on every roll over the
     * sequence, and followed at once by a loop in progress.
     */
    async setSpan(span: readonly [number, number] | null, { show = true }: { show?: boolean } = {}): Promise<void> {
        this.span = span === null ? null : [span[0], span[1]];
        if (show) this.#show();
        if (this.looping) await this.#loop();
    }

    /** The loop switch, as `L` is: kept stopped, and followed at once by a pass in progress. */
    async setLooping(on: boolean): Promise<void> {
        this.looping = on;
        await this.#loop();
        for (const roll of this.rolls()) roll.showLooping?.(on);
    }

    #loop(): Promise<void> {
        return this.call("loop", { range: this.span, loop: this.looping });
    }

    /** Every roll over the sequence draws the span. */
    #show(): void {
        for (const roll of this.rolls()) roll.showSpan?.(this.span);
    }

    /** The rolls open over the sequence: the views of its history that draw a span. */
    rolls(): Roll[] {
        const context = contexts.get(this.sequence);
        return [...(context?.views() ?? [])]
            .map((view) => view as unknown as Roll)
            .filter((roll) => roll.structure === this.sequence && roll.showSpan !== undefined);
    }
}

/**
 * `play` of a sequence: loaded on a transport of its own from beat `at`, its
 * pass ending where its contents do, and the `Transport` it is on answered.
 * The transport is the sequence's until it is freed (`Transport.free`).
 *
 * @internal
 */
export async function playSequence(sequence: EventSequence, at: number, server: Server): Promise<Transport> {
    const playback = NotesPlayback.of(server, sequence);
    playback.kept = true;
    await playback.load(at, { end: "contents" });
    return playback.transport;
}
