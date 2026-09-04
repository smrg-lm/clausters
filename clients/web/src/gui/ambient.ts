// The host the ambient visual verbs draw on (mirrors the registry in
// `clausters/gui/__init__.py`).
//
// `plot` (and, later, `scope`) opens its window on *some* host without being
// told which. The ladder they resolve through is: a host registered here, else
// the current — or default — session's `gui()` host if one is already up, else
// one the verb opens on the page and owns.
//
// The registry's reason to exist is the first rung, and it is the same one the
// reference client has: a front this module can neither open nor point
// elsewhere — a canvas over a carrier of the caller's own, a test double
// collecting packets — is registered by whoever built it, and wins outright.

import type { AppClock } from "../base/appclock.ts";
import type { GuiHost } from "./host.ts";

let registered: GuiHost | null = null;

/**
 * Registers the host the ambient visual verbs draw on, or clears it with
 * `null`. A registered host wins over everything else, which is the point:
 * it is a front the verbs could not have opened themselves.
 */
export function setAmbientHost(host: GuiHost | null): void {
    registered = host;
}

/** The registered ambient host, or `null` when none was registered. */
export function ambientHost(): GuiHost | null {
    return registered;
}

/**
 * The {@link AppClock} of `host`, or of the ambient one.
 *
 * The application's clock: **seconds**, on the loop the windows are drawn on.
 * It is where anything that touches a window belongs — an animation, a periodic
 * read-out, a follow-up to a gesture — and it is what a routine on the musical
 * `TempoClock` reaches through `defer`, since that one must never block:
 *
 * ```js
 * (await appClock()).play(blink);                     // a routine yielding seconds
 * (await appClock()).sched(0.5, () => knob.set({ value: 1.0 }));
 * (await appClock()).defer(() => win.close());        // after the current task
 * ```
 *
 * Async where the reference client's `app_clock()` is not, for the reason every
 * ambient verb here is: resolving the ambient host may have to boot it.
 */
export async function appClock(host?: GuiHost): Promise<AppClock> {
    if (host !== undefined) return host.clock;
    return (await import("../plot.ts")).resolveHost().then((h) => h.clock);
}
