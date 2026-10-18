/** Endpoint adapters for the shared Rust clipboard exchange. @internal */
import { EditingCore } from "../../core/clausters_core_web.js";
import type { Server } from "../../defs/server/index.ts";
import { serviceInstance } from "./application.ts";

const EXCHANGES = new WeakMap<object, {
    instance: { connection: number; generation: number }; core: EditingCore;
}>();

/** The table holds explicit service handles; policy and leases stay in Rust. @internal */
export function exchange(server: Server, action: string, args: Record<string, unknown> = {}): Record<string, unknown> {
    const instance = serviceInstance(server);
    let held = EXCHANGES.get(server);
    if (held === undefined || held.instance.connection !== instance.connection || held.instance.generation !== instance.generation) {
        if (held !== undefined) {
            held.core.call(JSON.stringify({ verb: "exchange", action: "invalidate", server: held.instance }));
            held.core.free();
        }
        held = { instance: { ...instance }, core: new EditingCore() };
        EXCHANGES.set(server, held);
    }
    const answer = JSON.parse(held.core.call(JSON.stringify({ verb: "exchange", action, server: instance, ...args }))) as Record<string, unknown>;
    for (const source of (answer.released ?? []) as { server: typeof instance; buffer: number }[]) {
        if (source.server.connection === instance.connection && source.server.generation === instance.generation) {
            void server.sendMsg("/buffer_free", ["i", source.buffer]);
            server.buffers.free(source.buffer);
        }
    }
    if (typeof answer.error === "string") throw new RangeError(answer.error);
    return answer;
}
