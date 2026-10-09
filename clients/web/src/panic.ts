// The free-standing `panic` -- the stop-everything key (mirrors
// `clausters/panic.py`).
//
// `panic` stops whatever the **session in force** plays: the session a
// `session.activate()` or a session driving a call put in force, the running
// routine's, else the default session (`defaultSession`). It is that
// session's `Environment.panic`: its clocks emptied, its timelines halted, its
// server silenced (transports stopped, queued bundles dropped, every node
// freed) and every responder not marked `permanent` freed. Nothing is closed,
// so the next `play` sounds.
//
//     const session = await Session.live();
//     session.activate();
//     play(sine(880).mul(0.1));   // sounds until it is freed
//     panic();                    // the session in force: this one
//     session.panic();            // the same, naming the session

import { Environment } from "./base/environment.ts";
import { main } from "./base/main.ts";

/**
 * Stops everything the session in force plays -- that session's
 * `Environment.panic` -- and answers the session.
 *
 * The session in force is the one an ambient `play` would use: the session
 * put in force, the running routine's, else the default session.
 */
export function panic(): Environment {
    const session = main.ambientSession();
    return (session instanceof Environment ? session : main).panic();
}
