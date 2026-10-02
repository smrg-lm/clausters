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
// There is **one per server**, on one transport, so the sequences played on a
// server share it: the one played last is the one that sounds, as on any
// transport with one thing loaded.

import { NotesPlayback as CorePlayback, StepRunner } from "../core/clausters_core_web.js";
import type { Server } from "../defs/server/index.ts";
import type { Transport, TransportDriver } from "../defs/server/transport.ts";
import { runSteps } from "../steps.ts";
import { EventSequence } from "./sequence.ts";

/** Where a pass ends: open, where the last note ends, or at a beat. */
export type PassEnd = null | "contents" | number;

/**
 * **What plays sequences on one server**: the crate's playback, the steps it
 * answers carried out, and the `Transport` the lane is on. Reached through
 * {@link NotesPlayback.of}, so a server has one.
 *
 * @internal
 */
export class NotesPlayback implements TransportDriver {
    static readonly #of = new WeakMap<Server, NotesPlayback>();
    static readonly #all = new Set<WeakRef<NotesPlayback>>();

    /** The playback of `server`, made on first ask. */
    static of(server: Server): NotesPlayback {
        let found = NotesPlayback.#of.get(server);
        if (found === undefined) {
            found = new NotesPlayback(server);
            NotesPlayback.#of.set(server, found);
            NotesPlayback.#all.add(new WeakRef(found));
        }
        return found;
    }

    readonly #native = new CorePlayback(-1);
    readonly #runner = new StepRunner();
    readonly #ready: Promise<void>;
    /** The engine's sample rate. */
    rate = 48_000;
    /** The transport it plays on -- the crate's word for it. */
    readonly transportId: number;
    /** That transport, as the object a page plays. */
    readonly transport: Transport;
    /** The sequence the lane holds, if any: the one played last. */
    planned: EventSequence | null = null;
    /** Where a pass starts, in the planned sequence's beats. */
    cursor = 0;
    /** Where a pass ends. */
    end: PassEnd = null;
    readonly server: Server;
    #paused = false;
    /**
     * The sequence and the version the lane last took: everything over the
     * sequence that hears of a change asks for it, and the lane takes it once.
     */
    #taken: [EventSequence, number] | null = null;
    /**
     * The calls in flight, one after another: everything over this playback
     * shares its step runner, and two calls whose steps interleave on it wait on
     * steps the other sent.
     */
    #queue: Promise<void> = Promise.resolve();

    private constructor(server: Server) {
        this.server = server;
        this.transportId = Number(
            JSON.parse(this.#native.call(new EventSequence().seq, JSON.stringify({ verb: "state" }), server.ids))
                .transport,
        );
        this.transport = server.transportAt(this.transportId);
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

    /** One verb over `sequence`, its steps carried out, after the calls before it. */
    call(verb: string, sequence: EventSequence, args: Record<string, unknown> = {}): Promise<void> {
        const run = this.#queue.then(() => this.#call(verb, sequence, args));
        this.#queue = run.catch(() => {});
        return run;
    }

    async #call(verb: string, sequence: EventSequence, args: Record<string, unknown>): Promise<void> {
        await this.#ready;
        const answer = JSON.parse(this.#native.call(
            sequence.seq,
            JSON.stringify({ verb, rate: this.rate, ...args }),
            this.server.ids,
        )) as Record<string, unknown>;
        if (typeof answer.error === "string") throw new RangeError(answer.error);
        const steps = answer.steps as unknown[] | undefined;
        if (steps !== undefined && steps.length > 0) await runSteps(this.server, this.#runner, steps);
    }

    // ---- what the lane holds ----

    /**
     * **Plays `sequence` from beat `at`**: it becomes what the lane holds, and
     * the transport's verbs are its own from now on. `range` plays that span,
     * going back to `at`; `looping` loops it, or with no range every note;
     * `end` is where a pass ends, kept from the last load when not given.
     */
    async load(
        sequence: EventSequence,
        at = 0,
        { range = null, looping = false, end }: {
            range?: readonly [number, number] | null;
            looping?: boolean;
            end?: PassEnd;
        } = {},
    ): Promise<void> {
        if (end !== undefined) this.end = end;
        this.planned = sequence;
        this.cursor = at;
        this.#paused = false;
        this.transport.driver = this;
        await this.call("end", sequence, { end: this.end });
        await this.call("play", sequence, { from: at, range, loop: looping });
    }

    /**
     * **The lane takes `sequence` again**, when it is the one the lane holds and
     * it has not taken it at this `version` of its context already: everything
     * over a sequence that hears of a change -- the editor that made it, the
     * others adopting it, the page's own write -- asks, and the lane is one.
     * `null` is a change no context counted, always taken.
     */
    update(sequence: EventSequence, version: number | null = null): Promise<void> {
        if (this.planned !== sequence) return Promise.resolve();
        const taken = this.#taken;
        if (version !== null && taken !== null && taken[0] === sequence && taken[1] === version) {
            return Promise.resolve();
        }
        this.#taken = version === null ? null : [sequence, version];
        return this.call("update", sequence);
    }

    /** `sequence` changed: every playback holding it takes it again. */
    static changed(sequence: EventSequence, version: number | null = null): void {
        for (const held of [...NotesPlayback.#all]) {
            const playback = held.deref();
            if (playback === undefined) {
                NotesPlayback.#all.delete(held);
                continue;
            }
            playback.update(sequence, version).catch(() => {});
        }
    }

    // ---- the transport's verbs, over what is loaded ----

    async playing(): Promise<boolean> {
        const playing = (await this.state()).playing;
        if (this.planned !== null) await this.call("setRolling", this.planned, { rolling: playing });
        return playing;
    }

    async play(at?: number): Promise<void> {
        if (this.planned === null) return;
        if (at === undefined && this.#paused) {
            this.#paused = false;
            await this.call("resume", this.planned);
            return;
        }
        await this.load(this.planned, at ?? this.cursor);
    }

    async setEnd(end: PassEnd): Promise<void> {
        this.end = end;
        if (this.planned !== null) await this.call("end", this.planned, { end });
    }

    async pause(): Promise<void> {
        if (this.planned === null) return;
        this.#paused = true;
        await this.call("pause", this.planned);
    }

    async stop(): Promise<void> {
        if (this.planned === null) return;
        this.#paused = false;
        await this.call("stop", this.planned, { back: this.cursor });
    }

    async locate(at: number): Promise<void> {
        if (this.planned === null) return;
        this.cursor = at;
        if (await this.playing()) await this.load(this.planned, at);
        else await this.call("cue", this.planned, { at });
    }

    async loop(span: [number, number] | null): Promise<void> {
        if (this.planned === null) return;
        await this.call("loop", this.planned, { range: span, loop: span !== null });
    }
}

/**
 * `play` of a sequence: loaded on the server's notes transport from beat `at`,
 * its pass ending where its contents do, and the `Transport` it is on answered.
 *
 * @internal
 */
export async function playSequence(sequence: EventSequence, at: number, server: Server): Promise<Transport> {
    const playback = NotesPlayback.of(server);
    await playback.load(sequence, at, { end: "contents" });
    return playback.transport;
}
