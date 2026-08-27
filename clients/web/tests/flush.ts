/**
 * Lets the clock's deferred pump run.
 *
 * `TempoClock.sched` resumes what is due in a microtask rather than on the
 * scheduling call's own stack, so `play()` returns before a routine's first
 * pass — the ordering the Python client has, and what
 * `Routine.run(function* () { … })` needs to be able to read its own binding.
 * A test that drives the clock by hand therefore has one thing to await: this.
 *
 * It is a macrotask on purpose. Awaiting a resolved promise would drain only
 * the microtasks already queued, and a routine that schedules another one
 * queues more while the first is running; a turn of the event loop drains the
 * lot, whatever depth they reach.
 */
export const flush = (): Promise<void> =>
    new Promise((resolve) => {
        setTimeout(resolve, 0);
    });
