// The random context: one seedable source for a whole script (mirrors
// `clausters/base/rand.py`).
//
// Everything random in a Clausters script -- the random patterns (`Pwhite`,
// `Prand`), these functions, anything sequenced -- draws from **one context**,
// the sclang model, so a single root seed reproduces a take from beginning to
// end:
//
// - `seed(n)` seeds the **root** stream.
// - Every `Routine` (any `Stream`) derives its **own** stream from the context
//   that creates it, at creation time (`Rng.spawn` -- the child's seed is the
//   parent's next word). Deterministic: the same root seed and the same
//   creation order give the same streams, and concurrent routines stay
//   reproducible per routine however their wakes interleave.
// - A draw always uses the stream of the routine running **right now**
//   (`currentRng`); outside a routine it falls back to the root.
//
// The generator itself is the shared core's (one `u64` of state, the same
// splitmix64/xorshift64 as the server's `WhiteNoise`), so the same seed
// replays the same values in every client language. There are no per-pattern
// seeds -- independent seeds would break whole-script consistency; override
// locally by playing inside its own routine instead.

import { Rng as CoreRng } from "../core/clausters_core_web.js";
import { requireCore } from "./core.ts";
import { currentRoutine } from "./context.ts";
// The default session, for the root a draw falls back to. `environment.ts`
// imports `Rng` back from here, so the two modules form a cycle -- a harmless
// one: neither reaches into the other while it is still evaluating (the
// constructor is called from a method, and `main` is built from classes that
// touch no random state), and the module-graph test holds it.
import { main } from "./main.ts";

/**
 * A resumable seeded value stream over the core generator, drawing the random
 * distributions sclang names (`rand`, `rrand`, `exprand`, `coin`, `choose` ...).
 * `spawn` derives a child deterministically.
 *
 * The integer draws are their own methods (`irand`, `irand2`, `irrand`): a
 * number here cannot say whether it was written `10` or `10.0`, so the float
 * draws never answer an integer for one.
 */
export class Rng {
    private readonly inner: CoreRng;

    /**
     * The stream for `seed` (splitmix64-mixed, never zero) -- the same seeding
     * as the server's `WhiteNoise`.
     */
    constructor(seed: number);
    /**
     * Wrap a core stream that already exists -- how `spawnRng` derives a child.
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

    /** Uniform in `[0, hi)`. */
    rand(hi = 1.0): number {
        return this.inner.draw("rand", hi, 0);
    }

    /** Uniform in `[-x, x)`. */
    rand2(x = 1.0): number {
        return this.inner.draw("rand2", x, 0);
    }

    /** Uniform between `lo` and `hi`. */
    rrand(lo: number, hi: number): number {
        return this.inner.draw("rrand", lo, hi);
    }

    /**
     * Exponentially distributed between `lo` and `hi` (same sign, nonzero):
     * equal chances per octave.
     */
    exprand(lo: number, hi: number): number {
        return this.inner.draw("exprand", lo, hi);
    }

    /** In `[0, x)`, more likely toward 0. */
    linrand(x = 1.0): number {
        return this.inner.draw("linrand", x, 0);
    }

    /** In `(-x, x)`, more likely toward 0. */
    bilinrand(x = 1.0): number {
        return this.inner.draw("bilinrand", x, 0);
    }

    /** In `(-x, x)`, bell-shaped: three uniforms summed. */
    sum3rand(x = 1.0): number {
        return this.inner.draw("sum3rand", x, 0);
    }

    /** `true` with probability `p`. */
    coin(p = 0.5): boolean {
        return this.inner.draw("coin", p, 0) !== 0;
    }

    /** Gaussian with `mean` and standard deviation `dev`. */
    gauss(mean = 0.0, dev = 1.0): number {
        return this.inner.draw("gauss", mean, dev);
    }

    /** A whole number in `[0, n)`. */
    irand(n: number): number {
        return this.inner.draw("irand", n, 0);
    }

    /** A whole number in `[-n, n]`, both ends included. */
    irand2(n: number): number {
        return this.inner.draw("irand2", n, 0);
    }

    /** A whole number between `lo` and `hi`, both ends included. */
    irrand(lo: number, hi: number): number {
        return this.inner.draw("irrand", lo, hi);
    }

    /** A uniformly chosen element of `items`. */
    choose<T>(items: readonly T[]): T {
        return items[this.irand(items.length)]!;
    }

    /** An element of `items` chosen with the relative `weights` (any sum). */
    wchoose<T>(items: readonly T[], weights: readonly number[]): T {
        return items[this.inner.windex(Float64Array.from(weights))]!;
    }

    /** A child stream seeded from this one's next word. */
    spawn(): Rng {
        return new Rng(this.inner.spawn());
    }
}

/**
 * The root stream, when no routine is running. Built on first use (the core
 * wasm has to be loaded first) and seeded from the wall clock, so an unseeded
 * script differs run to run -- exactly what `seed(n)` takes away.
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
 * A new stream derived from the current context -- how a `Routine` gets its
 * own at creation, seeded by its parent.
 */
export function spawnRng(): Rng {
    return currentRng().spawn();
}

// ---- the draws, from the current context ----
//
// sclang's names for its distributions. Each draws from `currentRng`, so one
// root seed reproduces every value.

/** Uniform in `[0, hi)`. */
export const rand = (hi = 1.0): number => currentRng().rand(hi);

/** Uniform in `[-x, x)`. */
export const rand2 = (x = 1.0): number => currentRng().rand2(x);

/** Uniform between `lo` and `hi`. */
export const rrand = (lo: number, hi: number): number => currentRng().rrand(lo, hi);

/**
 * Exponentially distributed between `lo` and `hi` (same sign, nonzero): equal
 * chances per octave -- what a frequency or a duration wants.
 */
export const exprand = (lo: number, hi: number): number => currentRng().exprand(lo, hi);

/** In `[0, x)`, more likely toward 0. */
export const linrand = (x = 1.0): number => currentRng().linrand(x);

/** In `(-x, x)`, more likely toward 0. */
export const bilinrand = (x = 1.0): number => currentRng().bilinrand(x);

/** In `(-x, x)`, bell-shaped: three uniforms summed, sclang's loose Gaussian. */
export const sum3rand = (x = 1.0): number => currentRng().sum3rand(x);

/** `true` with probability `p`. */
export const coin = (p = 0.5): boolean => currentRng().coin(p);

/** Gaussian with `mean` and standard deviation `dev`. */
export const gauss = (mean = 0.0, dev = 1.0): number => currentRng().gauss(mean, dev);

/** A whole number in `[0, n)`. */
export const irand = (n: number): number => currentRng().irand(n);

/** A whole number in `[-n, n]`, both ends included. */
export const irand2 = (n: number): number => currentRng().irand2(n);

/** A whole number between `lo` and `hi`, both ends included. */
export const irrand = (lo: number, hi: number): number => currentRng().irrand(lo, hi);

/** A uniformly chosen element of `items`. */
export const choose = <T>(items: readonly T[]): T => currentRng().choose(items);

/** An element of `items` chosen with the relative `weights`. */
export const wchoose = <T>(items: readonly T[], weights: readonly number[]): T =>
    currentRng().wchoose(items, weights);
