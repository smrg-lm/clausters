// Spawning a server or a host for a test, and being sure it dies.
//
// Every WS suite boots a process, drives it and kills it in a `finally`. That
// covers the ordinary path and not the one that leaves the mess: node runs no
// `finally` when it is *signalled*, so a killed test run (a Ctrl-C, a harness
// timeout, a `kill` of the runner) leaves an audio server holding its thread
// and its device, or a GUI host holding its window — invisible until the next
// run finds the port taken.
//
// So the handlers live here rather than being copied into each suite: five
// files repeating twelve lines is five chances to forget them, and the last
// two suites written did.
//
// Not a `.test.ts`, so `node --test 'tests/*.test.ts'` does not try to run it
// as a suite.

import { spawn } from "node:child_process";
import type { ChildProcess } from "node:child_process";

/** A spawned process and the one call that takes it down for good. */
export interface Spawned {
    /** The child, for the rare test that wants to inspect it. */
    readonly child: ChildProcess;
    /**
     * Kills it and unregisters the signal handlers. Idempotent, and safe to
     * call from a `finally` beside the ordinary teardown.
     */
    stop(): void;
}

/**
 * Spawns `bin` with `args`, killing it on the ordinary teardown (`stop`) and
 * on a signal that would otherwise skip it.
 *
 * The signal path is deliberately blunt — SIGKILL, then exit — because there
 * is nothing to wind down: the process is a test fixture, and what matters is
 * that nothing survives this one.
 */
export function spawnChild(bin: string, args: readonly string[]): Spawned {
    const child = spawn(bin, [...args], { stdio: "ignore" });
    let stopped = false;
    const onSignal = () => {
        child.kill("SIGKILL");
        process.exit(143);
    };
    process.once("SIGTERM", onSignal);
    process.once("SIGINT", onSignal);
    return {
        child,
        stop() {
            if (stopped) return;
            stopped = true;
            child.kill();
            process.off("SIGTERM", onSignal);
            process.off("SIGINT", onSignal);
        },
    };
}
