/** Execute acknowledged resource cleanup chosen by the shared core. @internal */
import { EditingCore, StepRunner } from "../../core/clausters_core_web.js";
import type { Server } from "../../defs/server/index.ts";
import { runSteps } from "../../steps.ts";
import type { RenderProgress } from "../../steps.ts";
import { serviceInstance } from "./application.ts";

const executors = new WeakMap<object, Promise<void>>();

/**
 * Serialize indistinguishable command waits on one endpoint: each awaited
 * request waits for the one before it, and a render waits for nothing, so
 * batches of two files render at once. @internal
 */
export async function executeSteps(
    server: Server,
    instance: { connection: number; generation: number },
    steps: unknown[],
    progress?: RenderProgress,
    signal?: AbortSignal,
): Promise<void> {
    const current = serviceInstance(server);
    if (current.connection !== instance.connection || current.generation !== instance.generation) throw new Error("the batch belongs to another server instance");
    const exclusive = <T>(request: () => Promise<T>): Promise<T> => {
        const work = (executors.get(server) ?? Promise.resolve()).then(request);
        executors.set(server, work.then(() => {}, () => {}));
        return work;
    };
    await runSteps(server, new StepRunner(), steps as never, { progress, exclusive, signal });
}

/** Return an id only after its exact cleanup ticket is confirmed. @internal */
export async function releaseBuffer(server: Server, core: EditingCore, resource: { server: { connection: number; generation: number }; buffer: number }): Promise<boolean> {
    const current = serviceInstance(server);
    if (current.connection !== resource.server.connection || current.generation !== resource.server.generation) return false;
    const call = (args: Record<string, unknown>): Record<string, any> => JSON.parse(core.call(JSON.stringify({ verb: "cleanup", ...args })));
    const release = call({ action: "prepare", resource });
    if (release.ticket === undefined) return false;
    try {
        await executeSteps(server, resource.server, release.steps ?? []);
    } catch (error) {
        call({ action: "complete", ticket: release.ticket, success: false, reason: String(error) });
        return false;
    }
    const answer = call({ action: "complete", ticket: release.ticket, success: true });
    if (answer.accepted !== true) return false;
    server.buffers.free(resource.buffer);
    return true;
}
