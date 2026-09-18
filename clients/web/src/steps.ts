/**
 * Steps carried out against a server, through the crate's runner.
 *
 * A verb of the shared crate answers **steps** — a message to send, a `/done`
 * the rest waits for, a barrier — and {@link runSteps} is the one walk of them
 * in this client: a multitrack's playback and a session's load go through it. Which
 * reply releases what is the runner's (`StepRunner`), as it is the script's and
 * the GUI host's; what is left here is a socket and waiting on it.
 *
 * @module
 */

import type { StepRunner } from "./core/clausters_core_web.js";
import type { MsgArg } from "./base/osc.ts";
import type { Server } from "./defs/server/index.ts";

/** One argument of a step, tagged as the crate encoded it. */
export type StepArg = { i: number } | { h: number } | { f: number } | { s: string } | { b: number[] };

/** **One step**, as the crate states it. */
export type Step =
    | { send: { addr: string; args: StepArg[] } }
    | { await: { command: string; index: number | null } }
    | { sync: number };

/**
 * Carry `steps` out on `server` through `runner`.
 *
 * What may go out is sent; where something is awaited, the runner puts the
 * message it waits on last, and that one is sent as a request — so the reply
 * cannot arrive before anyone is listening for it — and its reply is handed
 * back, which releases the rest. `to` is which server the runner addresses them
 * to, `"sound"` or `"samples"`; a client's is one server either way. Rejects
 * when the server refuses a step or answers one with something it does not
 * wait on.
 */
export async function runSteps(
    server: Server,
    runner: StepRunner,
    steps: unknown[],
    { to = "sound", timeout }: { to?: "sound" | "samples"; timeout?: number } = {},
): Promise<void> {
    const call = (request: object): Record<string, unknown> =>
        JSON.parse(runner.call(JSON.stringify(request))) as Record<string, unknown>;
    call({ verb: "push", to, steps });
    for (;;) {
        const ready = call({ verb: "ready" }) as {
            messages?: { addr: string; args: StepArg[] }[];
            awaiting?: { step: Step } | null;
        };
        const messages = ready.messages ?? [];
        const awaiting = ready.awaiting ?? null;
        if (awaiting === null) {
            for (const message of messages) {
                server.sendMsg(message.addr, ...message.args.map(stepArg));
            }
            return;
        }
        const last = messages.pop();
        if (last === undefined) throw new Error("clausters: the runner waits on a step nothing was sent for");
        for (const message of messages) {
            server.sendMsg(message.addr, ...message.args.map(stepArg));
        }
        const reply =
            "sync" in awaiting.step
                ? await server.request(last.addr, last.args.map(stepArg), {
                      expect: ["/server_sync.reply"],
                      timeout,
                  })
                : await server.request(last.addr, last.args.map(stepArg), {
                      expect: ["/done", "/fail"],
                      cmd: last.addr,
                      timeout,
                  });
        const answered = call({
            verb: "reply",
            from: to,
            addr: reply.addr,
            args: reply.args.map(tagged),
        });
        if (answered.reply === "refused") {
            throw new Error(`clausters: ${last.addr} failed: ${reply.args.slice(1).join(" ")}`);
        }
        if (answered.reply !== "released") {
            throw new Error(`clausters: ${last.addr} was answered by ${reply.addr}, which is not what it waits on`);
        }
    }
}

/** One step argument, tagged as the crate encoded it. */
export function stepArg(arg: StepArg): MsgArg {
    if ("i" in arg) return ["i", arg.i];
    if ("h" in arg) return ["h", BigInt(arg.h)];
    if ("f" in arg) return ["f", arg.f];
    if ("b" in arg) return new Uint8Array(Float32Array.from(arg.b).buffer);
    return arg.s;
}

/**
 * One reply argument in the tagged shape the runner reads — the other direction
 * of `stepArg`. A reply carries ints, floats and strings; a JS number is tagged
 * by whether it is integral, which is what those replies hold.
 */
export function tagged(value: number | string | Uint8Array | boolean | null): StepArg {
    if (typeof value === "boolean") return { i: value ? 1 : 0 };
    if (typeof value === "number") return Number.isInteger(value) ? { i: value } : { f: value };
    return { s: String(value) };
}
