// The environment: an isolated place to make sound (mirrors
// `clausters/base/environment.py`).
//
// An `Environment` is the unit of isolation — a `server`, its clock(s), and
// its own random context. **Both the default session (`base/main.ts`) and an
// explicit `Session` (`../session.ts`) are Environments**: the default one is
// simply the one used when none is named, and a named session is the same kind
// of thing with its own state. That is what lets several coexist — a page
// against its own engine beside one against a `--ws` server, each reproducible
// on its own seed — without touching each other.
//
// This base carries only what every environment shares: the seedable random
// context and the `server` slot. The ambient resolution (which environment a
// free-standing play belongs to) and the execution registry live on the
// default session (`Main`); the driving surface (`play`/`run`, the factories)
// lives on `Session`.

import { Rng } from "./rand.ts";
import type { Server } from "../defs/server/index.ts";

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
     * This context's value stream — the shared core generator, so the same
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
}
