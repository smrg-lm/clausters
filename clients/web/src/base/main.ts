// Execution context: the default session (mirrors `clausters/base/main.py`).
//
// `main` is the page's **default session** — the environment holding the
// ambient state used whenever you did not name a session explicitly. The rule
// is one line:
//
// > Everything that does not run in an explicit `Session` runs in the default
// > session, `main`.
//
// `main` is an `Environment`, the same base a `Session` extends, so it *is* a
// session: the default one. It owns what would otherwise be scattered globals
// — the default `server` (adopted first-wins), an opt-in `defaultClock`, and
// the random root — and it is the resolution authority: `resolveServer` /
// `resolveClock` implement the single rule the free `play` and every
// playable's ambient `.play()` share.
//
// **The page has one thread, so the registry is not thread-local.** Python
// keeps `current_tt` and `current_session` in a `threading.local`; here the
// running routine already lives in `./context.ts` (a module slot is exactly as
// sound, because a wake runs to its next `yield` with nothing interleaving)
// and the active session is one more slot beside it. Both `null` means the
// default session, and resolution falls back to it.

import { currentRoutine } from "./context.ts";
import { Environment } from "./environment.ts";
import { TempoClock } from "./clock.ts";
import type { Rng } from "./rand.ts";
import type { Server } from "../defs/server/index.ts";
import type { Stream } from "./stream.ts";

/**
 * What resolution needs of a `Session` without importing it — the field it
 * reads off the ambient environment, and the clock a session carries.
 */
export interface SessionLike {
    server: Server | null;
    clock?: TempoClock;
    /** The environment's random root (see `./environment.ts`). */
    readonly rng?: Rng;
}

/**
 * The default session: the ambient `Environment` resolution falls back to.
 *
 * An `Environment` like any `Session` (a server plus a random context), plus
 * the two roles only the *default* one plays: it holds the execution registry
 * (`currentRoutine` / `currentSession`) and it is the resolution authority. It
 * also keeps an opt-in `defaultClock`, created on first use so a page that
 * only draws never starts one.
 */
export class Main extends Environment {
    /**
     * The opt-in convenience clock; `null` until first needed (see
     * `getDefaultClock`). An explicit `Session` brings its own.
     */
    defaultClock: TempoClock | null = null;

    private active: SessionLike | null = null;

    /**
     * The routine being resumed right now, set by the clock around each wake
     * — so resolution can reach its session through `clock.session`. `null`
     * outside a routine.
     */
    get currentRoutine(): Stream | null {
        return currentRoutine();
    }

    /**
     * The explicit `Session` active right now, set by a session while it
     * plays or for the duration of a `with`-style block, so anything created
     * outside any routine still resolves to that session. `null` means the
     * default session (`main`) itself.
     */
    get currentSession(): SessionLike | null {
        return this.active;
    }

    set currentSession(session: SessionLike | null) {
        this.active = session;
    }

    // ---- ambient resolution (the single rule) ----

    /**
     * The session an ambient play belongs to: the running routine's (through
     * the clock driving it), else the explicit `currentSession`, else `null`
     * — the default session, which is `this`.
     */
    private ambientSession(): SessionLike | null {
        const session = this.currentRoutine?.clock?.session ?? null;
        return session ?? this.active;
    }

    /**
     * The server a free-standing play should target: the explicit one if
     * given, else the ambient session's, else the default session's. Throws
     * when none has been opened.
     */
    resolveServer(server?: Server | null): Server {
        if (server) return server;
        const session = this.ambientSession();
        if (session?.server) return session.server;
        if (this.server) return this.server;
        throw new Error(
            "no server to play on: open one with Session.embed() or " +
                "Session.live(url), or pass { server }",
        );
    }

    /**
     * The clock a play should schedule on: the explicit one if given, else
     * the clock of the routine running right now, else the ambient session's,
     * else the default session's `defaultClock` — which may be `null`, the
     * caller then reaching for `getDefaultClock`.
     */
    resolveClock(clock?: TempoClock | null): TempoClock | null {
        if (clock) return clock;
        const running = this.currentRoutine?.clock;
        if (running) return running;
        const session = this.active;
        if (session?.clock) return session.clock;
        return this.defaultClock;
    }

    /**
     * The default session's clock, created (tempo 1.0) on first use and,
     * unless told otherwise, started so what is played on it fires in real
     * time. This is what an ambient `Routine`/`Pattern` play uses when no
     * clock is in context.
     *
     * Lazily, and never at import: a page that only renders or only draws
     * must not start a clock by loading a module.
     */
    getDefaultClock(start = true): TempoClock {
        if (this.defaultClock === null) {
            this.defaultClock = new TempoClock();
            this.defaultClock.session = this;
        }
        if (start) this.defaultClock.start();
        return this.defaultClock;
    }

    /** The default clock's beat, or 0 while there is no clock. */
    elapsedBeats(): number {
        return this.defaultClock ? this.defaultClock.beats() : 0.0;
    }
}

/** The page-wide default session (an `Environment`, like any `Session`). */
export const main = new Main();

/** The same object, named for what it is. */
export const defaultSession = main;
