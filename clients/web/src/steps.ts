/**
 * Steps carried out against a server, through the crate's runner.
 *
 * A verb of the shared crate answers **steps** -- a message to send, a `/done`
 * the rest waits for, a barrier -- and {@link runSteps} is the one walk of them
 * in this client: a roll's playback goes through it. Which reply releases what
 * is the runner's (`StepRunner`), as it is the script's and the GUI host's;
 * what is left here is a socket and waiting on it.
 *
 * The steps that work on files (a render, a document stored, a file moved) are
 * the GUI host's application's, which holds every editor that saves or
 * renders; none reaches a client, and the walk refuses one, naming it, rather
 * than doing in this page's storage what the application does on its own.
 *
 * @module
 */

import type { StepRunner } from "./core/clausters_core_web.js";
import type { MsgArg } from "./base/osc.ts";
import type { Server } from "./defs/server/index.ts";

/** One argument of a step, tagged as the crate encoded it. */
export type StepArg =
    | { i: number }
    | { h: number }
    | { f: number }
    | { s: string }
    | { b: number[] }
    | { x: string };

/** **One step**, as the crate states it. */
export type Step =
    | { send: { addr: string; args: StepArg[] } }
    | { await: { command: string; index: number | null } }
    | { sync: number }
    | { render: RenderStep }
    | { move: { from: string; to: string } }
    | { store: { path: string; contents: string } };

/** **An offline render a step asks for**: the score, in hex, and the file it
 *  is rendered into. */
export interface RenderStep {
    score: string;
    path: string;
    rate: number;
    channels: number;
    format: string;
    /** What the render is for, as its progress is shown: the words of the
     *  verb that asked for it. Empty where none did. */
    label?: string;
}

/** How {@link runSteps} carries its steps out. */
export interface StepOptions {
    /** Which server the runner addresses the steps to. */
    to?: "sound" | "samples";
    /** How long each awaited reply may take, in seconds. */
    timeout?: number;
}

/** The walk each runner is on, so the next one starts after it. */
const walking = new WeakMap<StepRunner, Promise<void>>();

/**
 * Carry `steps` out on `server` through `runner`.
 *
 * What may go out is sent; where something is awaited, the runner puts the
 * message it waits on last, and that one is sent as a request -- so the reply
 * cannot arrive before anyone is listening for it -- and its reply is handed
 * back, which releases the rest. `to` is which server the runner addresses them
 * to, `"sound"` or `"samples"`; a client's is one server either way. Rejects
 * when the server refuses a step or answers one with something it does not
 * wait on, or when a step that works on files arrives.
 *
 * **One walk at a time per runner.** The runner holds one sequence, so a
 * second walk begun while the first waits on a reply would find the first's
 * step awaited and nothing of its own to send. A script never meets this --
 * each call there returns when its steps are done -- but a page does not
 * block, and a verb nobody awaited (a `cue`, a `stop`) is still walking when
 * the next one comes: so the walks on a runner go in the order they were
 * asked for, and a failed one does not hold up the next: whatever ends a
 * walk early retires the steps it did not send, so none of them goes out
 * ahead of the next walk's.
 */
export function runSteps(
    server: Server,
    runner: StepRunner,
    steps: unknown[],
    options: StepOptions = {},
): Promise<void> {
    const before = walking.get(runner) ?? Promise.resolve();
    const walk = before.then(() => walkSteps(server, runner, steps, options));
    walking.set(runner, walk.catch(() => undefined));
    return walk;
}

/** The steps only the application that holds an editor carries out. */
const THE_APPLICATIONS = ["render", "store", "move", "measure"] as const;

async function walkSteps(
    server: Server,
    runner: StepRunner,
    steps: unknown[],
    { to = "sound", timeout }: StepOptions,
): Promise<void> {
    const call = (request: object): Record<string, unknown> =>
        JSON.parse(runner.call(JSON.stringify(request))) as Record<string, unknown>;
    call({ verb: "push", to, steps });
    try {
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
            const held = THE_APPLICATIONS.find((kind) => kind in awaiting.step);
            if (held !== undefined) {
                throw new Error(`clausters: a ${held} step is carried out by the application that holds the editor, never by a client`);
            }
            const last = messages.pop();
            if (last === undefined) throw new Error("clausters: the runner waits on a step nothing was sent for");
            for (const message of messages) {
                server.sendMsg(message.addr, ...message.args.map(stepArg));
            }
            const reply = "sync" in awaiting.step
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
    } catch (error) {
        // **A walk given up on leaves nothing behind**: what it did not send,
        // the step it waited on included, is retired, so the next walk on
        // this runner sends nothing of this one.
        call({ verb: "retire" });
        throw error;
    }
}

/** One step argument, tagged as the crate encoded it. */
export function stepArg(arg: StepArg): MsgArg {
    if ("i" in arg) return ["i", arg.i];
    if ("h" in arg) return ["h", BigInt(arg.h)];
    if ("f" in arg) return ["f", arg.f];
    if ("b" in arg) return new Uint8Array(Float32Array.from(arg.b).buffer);
    if ("x" in arg) {
        // A blob of bytes that are not samples -- a bundle -- in hex.
        const bytes = new Uint8Array(arg.x.length / 2);
        for (let i = 0; i < bytes.length; i++) bytes[i] = parseInt(arg.x.slice(2 * i, 2 * i + 2), 16);
        return bytes;
    }
    return arg.s;
}

/**
 * One reply argument in the tagged shape the runner reads -- the other direction
 * of `stepArg`. A reply carries ints, floats and strings; a JS number is tagged
 * by whether it is integral, which is what those replies hold.
 */
export function tagged(value: number | string | Uint8Array | boolean | null): StepArg {
    if (typeof value === "boolean") return { i: value ? 1 : 0 };
    if (typeof value === "number") return Number.isInteger(value) ? { i: value } : { f: value };
    return { s: String(value) };
}
