// The random context: one seedable source for a whole script (mirrors
// `clausters/base/rand.py`).
//
// Everything random in a Clausters script — the random patterns (`Pwhite`,
// `Prand`), these functions, anything sequenced — draws from **one context**,
// the sclang model, so a single root seed reproduces a piece from beginning to
// end:
//
// - `seed(n)` seeds the **root** stream.
// - Every `Routine` (any `Stream`) derives its **own** stream from the context
//   that creates it, at creation time (`Rng.spawn` — the child's seed is the
//   parent's next word). Deterministic: the same root seed and the same
//   creation order give the same streams, and concurrent routines stay
//   reproducible per routine however their wakes interleave.
// - A draw always uses the stream of the routine running **right now**
//   (`currentRng`); outside a routine it falls back to the root.
//
// The generator itself is the shared core's (one `u64` of state, the same
// splitmix64/xorshift64 as the server's `WhiteNoise`), so the same seed
// replays the same values in every client language. There are no per-pattern
// seeds — independent seeds would break whole-script consistency; override
// locally by playing inside its own routine instead.

import { Rng as CoreRng } from "../core/clausters_core_web.js";
import { requireCore } from "./core.ts";
import { currentRoutine } from "./context.ts";
// The default session, for the root a draw falls back to. `environment.ts`
// imports `Rng` back from here, so the two modules form a cycle — a harmless
// one: neither reaches into the other while it is still evaluating (the
// constructor is called from a method, and `main` is built from classes that
// touch no random state), and the module-graph test holds it.
import { main } from "./main.ts";

/**
 * A resumable seeded value stream over the core generator: uniform doubles in
 * `[0, 1)` and bounded integers. `spawn` derives a child deterministically.
 */
export class Rng {
    private readonly inner: CoreRng;

    /**
     * The stream for `seed` (splitmix64-mixed, never zero) — the same seeding
     * as the server's `WhiteNoise`.
     */
    constructor(seed: number);
    /**
     * Wrap a core stream that already exists — how `spawnRng` derives a child.
     *
     * @internal
     */
    constructor(inner: CoreRng);
    constructor(source: number | CoreRng) {
        // The one door every random draw goes through, so an unloaded core says
        // so here rather than as an unreadable read of an uninitialised binding.
        if (typeof source === "number") requireCore("a random draw");
        this.inner = typeof source === "number" ? new CoreRng(source) : source;
    }

    /** Uniform in `[0, 1)` with 53-bit resolution. */
    nextF64(): number {
        return this.inner.nextF64();
    }

    /** Uniform in `[lo, hi)` (degenerate to `lo` when `hi <= lo`). */
    uniform(lo: number, hi: number): number {
        return this.inner.uniform(lo, hi);
    }

    /** Uniform integer in `[0, n)`; 0 when `n` is 0. */
    nextBelow(n: number): number {
        return this.inner.nextBelow(n);
    }

    /** A uniformly chosen element of `items`. */
    choice<T>(items: readonly T[]): T {
        return items[this.nextBelow(items.length)]!;
    }

    /** A child stream seeded from this one's next word. */
    spawn(): Rng {
        return new Rng(this.inner.spawn());
    }
}

/**
 * The root stream, when no routine is running. Built on first use (the core
 * wasm has to be loaded first) and seeded from the wall clock, so an unseeded
 * script differs run to run — exactly what `seed(n)` takes away.
 */
/**
 * Seeds the **default session's** root stream, reproducing every draw made
 * outside a routine and every routine created after this call.
 *
 * A named `Session` is its own random context: `session.seed(n)` reproduces
 * *that* session's own sound without touching another's, which is what lets
 * two sessions on one page stay reproducible independently.
 */
export function seed(value: number): void {
    main.seed(value);
}

/**
 * The stream a draw comes from: the routine running right now, else the
 * ambient session's root (the running routine's session, or the one active on
 * this page), else the default session's. This is where every random value in
 * the library comes from.
 */
export function currentRng(): Rng {
    const routine = currentRoutine();
    if (routine?.rng) return routine.rng;
    const session = routine?.clock?.session ?? main.currentSession;
    return session?.rng ?? main.rng;
}

/**
 * A new stream derived from the current context — how a `Routine` gets its
 * own at creation, seeded by its parent.
 */
export function spawnRng(): Rng {
    return currentRng().spawn();
}

/** Uniform in `[0, 1)` from the current context. */
export const nextF64 = (): number => currentRng().nextF64();

/** Uniform in `[lo, hi)` from the current context. */
export const uniform = (lo: number, hi: number): number =>
    currentRng().uniform(lo, hi);

/** Uniform integer in `[0, n)` from the current context. */
export const nextBelow = (n: number): number => currentRng().nextBelow(n);

/** A uniformly chosen element of `items`, from the current context. */
export const choice = <T>(items: readonly T[]): T => currentRng().choice(items);
