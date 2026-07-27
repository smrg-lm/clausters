// The ambient "what is running right now", set by the clock around each wake.
//
// The Python client keeps this in a thread-local (`main.current_tt`); the page
// has one thread, and a wake is synchronous — the clock resumes a routine and
// the routine runs to its next `yield` without anything else interleaving — so
// a module-level slot is exactly as sound here, and is what lets
// `Event().play()` inside a routine find its own logical beat and its own
// random stream with no parameters threaded through.
//
// It holds no imports of its own at run time (only types), so it can sit under
// both `rand.ts` and `clock.ts` without a cycle between them.

import type { Stream } from "./stream.ts";

let current: Stream | null = null;

/// The stream the clock is resuming right now, or `null` outside any wake.
export function currentRoutine(): Stream | null {
    return current;
}

/// Makes `routine` the ambient one and returns the previous, which the caller
/// must restore — the clock brackets every wake with this pair.
export function setCurrentRoutine(routine: Stream | null): Stream | null {
    const previous = current;
    current = routine;
    return previous;
}
