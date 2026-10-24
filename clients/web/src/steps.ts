/**
 * Steps carried out against a server, through the crate's runner.
 *
 * A verb of the shared crate answers **steps** -- a message to send, a `/done`
 * the rest waits for, a barrier -- and {@link runSteps} is the one walk of them
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
    | { move: { from: string; to: string } };

/**
 * **How an offline render goes**, told by {@link runSteps}: the frames done and
 * the frames it comes to as it runs, and `null` for both once it has ended.
 */
export type RenderProgress = (render: RenderStep, done: number | null, total: number | null) => void;

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

/**
 * **A render step carried out**: the score rendered offline into its file, in
 * a Worker of its own in a page and on this thread under node
 * (`engine/file-render.ts`), so two renders run at once. `onProgress` is told
 * the frames done and the frames the render comes to.
 */
export async function renderStep(
    render: RenderStep,
    onProgress?: (done: number, total: number) => void,
): Promise<void> {
    const { renderFile } = await import("./engine/file-render.ts");
    const { entropySeed } = await import("./engine/render.ts");
    const score = new Uint8Array(render.score.length / 2);
    for (let i = 0; i < score.length; i++) score[i] = parseInt(render.score.slice(2 * i, 2 * i + 2), 16);
    await renderFile({
        score,
        sampleRate: render.rate,
        channels: render.channels,
        seed: entropySeed(),
        path: render.path,
        sampleFormat: render.format || "float",
    }, onProgress);
}

/** How {@link runSteps} carries its steps out. */
export interface StepOptions {
    /** Which server the runner addresses the steps to. */
    to?: "sound" | "samples";
    /** How long each awaited reply may take, in seconds. */
    timeout?: number;
    /** Told how each offline render goes. */
    progress?: RenderProgress;
    /** Runs each awaited request, alone on its server. */
    exclusive?: <T>(request: () => Promise<T>) => Promise<T>;
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
 * wait on. `progress` is told how each offline render goes -- `(render, done,
 * total)` as it runs, in frames, and `(render, null, null)` once it has ended,
 * either way. `exclusive` runs each awaited request, and never a render, so
 * two walks on one server cannot take each other's `/done` while their
 * renders run at once.
 *
 * **One walk at a time per runner.** The runner holds one sequence, so a
 * second walk begun while the first waits on a reply would find the first's
 * step awaited and nothing of its own to send. A script never meets this --
 * each call there returns when its steps are done -- but a page does not
 * block, and a verb nobody awaited (a `cue`, a `stop`) is still walking when
 * the next one comes: so the walks on a runner go in the order they were
 * asked for, and a failed one does not hold up the next.
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

/**
 * **What a step does on the disk**, by the verb: render a score into a file,
 * move a file, remove one. A test replaces them to record what a run asked
 * for rather than doing it.
 *
 * @internal
 */
export const disk = {
    render: (render: RenderStep, onProgress?: (done: number, total: number) => void): Promise<void> =>
        renderStep(render, onProgress),
    move: async (from: string, to: string): Promise<void> => {
        const { moveFileAt } = await import("./base/files.ts");
        await moveFileAt(from, to);
    },
    remove: async (path: string): Promise<void> => {
        const { removeFileAt } = await import("./base/files.ts");
        await removeFileAt(path);
    },
};

async function walkSteps(
    server: Server,
    runner: StepRunner,
    steps: unknown[],
    { to = "sound", timeout, progress, exclusive = (request) => request() }: StepOptions,
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
        if (awaiting === null || "render" in awaiting.step || "move" in awaiting.step) {
            for (const message of messages) {
                server.sendMsg(message.addr, ...message.args.map(stepArg));
            }
            if (awaiting === null) return;
            if ("move" in awaiting.step) {
                // **A file moved**, where this client's paths are: what a save
                // over a file the take reads does with it.
                const move = awaiting.step.move;
                let failed: unknown = null;
                try {
                    await disk.move(move.from, move.to);
                } catch (error) {
                    failed = error;
                }
                call(failed === null
                    ? { verb: "moved" }
                    : { verb: "moved", error: String((failed as Error)?.message ?? failed) });
                if (failed !== null) {
                    throw new Error(`clausters: moving ${move.from} to ${move.to} failed: ${String((failed as Error)?.message ?? failed)}`);
                }
                continue;
            }
            // **An offline render**: nothing is sent for it; this client
            // renders the score into its file and says how it went.
            const render = (awaiting.step as { render: RenderStep }).render;
            let failed: unknown = null;
            try {
                await disk.render(render, progress === undefined
                    ? undefined : (done, total) => progress(render, done, total));
            } catch (error) {
                failed = error;
            } finally {
                progress?.(render, null, null);
            }
            const answered = call(failed === null
                ? { verb: "rendered" }
                : { verb: "rendered", error: String((failed as Error)?.message ?? failed) });
            if (failed !== null) {
                throw new Error(`clausters: the render into ${render.path} failed: ${String((failed as Error)?.message ?? failed)}`);
            }
            if (answered.reply !== "released") {
                throw new Error("clausters: the runner did not wait on the render it asked for");
            }
            continue;
        }
        const last = messages.pop();
        if (last === undefined) throw new Error("clausters: the runner waits on a step nothing was sent for");
        const reply = await exclusive(() => {
            for (const message of messages) {
                server.sendMsg(message.addr, ...message.args.map(stepArg));
            }
            return "sync" in awaiting.step
                ? server.request(last.addr, last.args.map(stepArg), {
                      expect: ["/server_sync.reply"],
                      timeout,
                  })
                : server.request(last.addr, last.args.map(stepArg), {
                      expect: ["/done", "/fail"],
                      cmd: last.addr,
                      timeout,
                  });
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
