// What clausters-editor calls into this package for, besides its book
// (mirrors `clausters/_editor.py`).
//
// `interrupt` is the hook the package's `package.json` names for the editor
// (`"clausters-editor": { "interrupt": "./dist/editor.js#interrupt" }`). The
// editor calls it after its interrupt key (Ctrl+.) has stopped the code the
// session was running, and only when this package was imported there, so the
// key that stops the code stops the sound too.

import { main } from "./base/main.ts";
import { liveSessions } from "./session.ts";

/**
 * Panics every session not closed yet, and the default session -- each one's
 * `Environment.panic`. A session that fails (its server gone, say) does not
 * keep the others sounding: each is tried, and the first failure is thrown
 * after all of them.
 */
export function interrupt(): void {
    let failed: unknown;
    for (const environment of [...liveSessions(), main]) {
        try {
            environment.panic();
        } catch (error) {
            failed ??= error;
        }
    }
    if (failed !== undefined) throw failed;
}
