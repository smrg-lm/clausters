// The environment: an isolated place to make sound (mirrors
// `clausters/base/environment.py`).
//
// An `Environment` is the unit of isolation -- a `server`, its clock(s), and
// its own random context. **Both the default session (`base/main.ts`) and an
// explicit `Session` (`../session.ts`) are Environments**: the default one is
// simply the one used when none is named, and a named session is the same kind
// of thing with its own state. That is what lets several coexist -- a page
// against its own engine beside one against a `--ws` server, each reproducible
// on its own seed -- without touching each other.
//
// This base carries only what every environment shares: the seedable random
// context and the `server` slot. The ambient resolution (which environment a
// free-standing play belongs to) and the execution registry live on the
// default session (`Main`); the driving surface (`play`/`run`, the factories)
// lives on `Session`.

import { Rng } from "./rand.ts";
import type { Server } from "../defs/server/index.ts";
import { aliveClocks } from "./clock.ts";
import type { TempoClock } from "./clock.ts";

/**
 * The parts of a panic held by modules this one cannot import without a cycle:
 * each sets its own when it loads, and one never loaded holds nothing to stop.
 * `halt` stops the timelines sounding on the emptied clocks or on the server's
 * transports; `free` frees the responders not marked permanent.
 */
export const panicParts: {
    halt?: (clocks: TempoClock[], server: Server | null) => void;
    free?: () => void;
} = {};

/**
 * One seedable RNG root (the shared core generator).
 *
 * Each environment is its own random context, so each reproduces
 * **independently**: `seed(n)` on one never touches another's stream. A
 * `Stream` created while a context is active derives its own generator from
 * that context's root (see `./rand.ts`), so two sessions stay
 * reproducible per session regardless of interleaving.
 */
export class RandomContext {
    private rngRoot: Rng | null = null;
    private seedValue: number | null = null;

    /**
     * Seeds this context's RNG; with no value it reseeds from entropy.
     * Returns the seed actually used, so the context can be reproduced.
     */
    seed(value?: number): number {
        const used = value ?? Math.floor(Math.random() * 2 ** 53);
        this.seedValue = used;
        this.rngRoot = new Rng(used);
        return used;
    }

    /**
     * This context's value stream -- the shared core generator, so the same
     * seed replays the same values in every client language. Created lazily,
     * seeded from entropy unless `seed` was called.
     */
    get rng(): Rng {
        if (this.rngRoot === null) this.seed(this.seedValue ?? undefined);
        return this.rngRoot!;
    }
}

/**
 * A place things play: a `server` plus a random context (and, on a `Session`,
 * a clock and a driving surface).
 *
 * The shared base of the default session and an explicit session, so the two
 * are the *same kind of thing*. Resolution (`Main.resolveServer`) reads the
 * ambient environment's `server` whether that is the default session or a
 * named one.
 */
export class Environment extends RandomContext {
    /**
     * The environment's server; `null` until one is set. The default session
     * adopts one first-wins (`Session.adoptDefault`); a `Session` is built
     * around one.
     */
    server: Server | null = null;

    /**
     * **Stops everything this environment plays**, and leaves it ready to
     * play again -- sclang's `CmdPeriod`. Returns `this`.
     *
     * In order: every clock of this environment is emptied
     * (`TempoClock.clear`), so no routine, pattern or curve wakes again; every
     * timeline played on one of them, or on a transport of its server, is
     * halted; the server is silenced (`Server.panic`: transports stopped,
     * queued bundles dropped, nodes freed); and every OSC and MIDI responder
     * not marked `permanent` is freed.
     *
     * Nothing is closed: the clocks keep their beat and go on running, the
     * server keeps its defs and buffers, and a `play` on the next line sounds.
     * Two things reach beyond this environment, because they are not its own:
     * the server is silenced whole, another client's sound included, and the
     * responders are the page's. The `AppClock` is left alone -- what runs on
     * it draws, and is not played.
     */
    panic(): this {
        const clocks = aliveClocks().filter((clock) => this.owns(clock));
        for (const clock of clocks) clock.clear();
        panicParts.halt?.(clocks, this.server);
        this.server?.panic();
        panicParts.free?.();
        return this;
    }

    /** Whether `clock` plays in this environment: one it adopted. */
    protected owns(clock: TempoClock): boolean {
        return clock.session === this;
    }
}
