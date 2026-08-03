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
// elsewhere — a notebook cell's canvas over a kernel comm, a test double
// collecting packets — is registered by whoever built it, and wins outright.

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
