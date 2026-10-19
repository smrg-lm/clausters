/** Endpoint adapters for the shared Rust clipboard exchange. @internal */
import { EditingCore } from "../../core/clausters_core_web.js";
import type { Server } from "../../defs/server/index.ts";
import { releaseBuffer } from "./cleanup.ts";
import { serviceInstance } from "./application.ts";

type Instance = { connection: number; generation: number };
const EXCHANGES = new WeakMap<object, { instance: Instance; core: EditingCore }>();

/** A routed reader handle, keeping its owning composition alive. @internal */
export interface ExchangeLease { core: EditingCore; lease: number; instance: Instance; }

function released(server: Server, core: EditingCore, answer: Record<string, unknown>, instance: Instance): void {
    for (const source of (answer.released ?? []) as { server: Instance; buffer: number }[]) {
        if (source.server.connection === instance.connection && source.server.generation === instance.generation) {
            void releaseBuffer(server, core, source);
        }
    }
}

/** Route publication/readers to the composition that owns their resources. @internal */
export function bindExchange(server: Server, core: EditingCore): void {
    const instance = serviceInstance(server);
    const held = EXCHANGES.get(server);
    if (held !== undefined && held.core !== core) {
        const action = held.instance.connection === instance.connection && held.instance.generation === instance.generation ? "clear" : "invalidate";
        const answer = JSON.parse(held.core.call(JSON.stringify({ verb: "exchange", action, server: held.instance }))) as Record<string, unknown>;
        released(server, held.core, answer, instance);
    }
    EXCHANGES.set(server, { instance: { ...instance }, core });
}

/** The table holds explicit service handles; policy and leases stay in Rust. @internal */
export function exchange(server: Server, action: string, args: Record<string, unknown> = {}): Record<string, unknown> {
    const instance = serviceInstance(server);
    if (action === "release" && typeof args.lease === "object" && args.lease !== null) {
        const lease = args.lease as ExchangeLease;
        if (lease.instance.connection !== instance.connection || lease.instance.generation !== instance.generation) {
            lease.core.call(JSON.stringify({ verb: "exchange", action: "invalidate", server: lease.instance }));
        }
        const answer = JSON.parse(lease.core.call(JSON.stringify({ verb: "exchange", action: "release", lease: lease.lease }))) as Record<string, unknown>;
        released(server, lease.core, answer, instance);
        return answer;
    }
    let held = EXCHANGES.get(server);
    if (held === undefined || held.instance.connection !== instance.connection || held.instance.generation !== instance.generation) {
        if (held !== undefined) held.core.call(JSON.stringify({ verb: "exchange", action: "invalidate", server: held.instance }));
        held = { instance: { ...instance }, core: new EditingCore() };
        EXCHANGES.set(server, held);
    }
    const answer = JSON.parse(held.core.call(JSON.stringify({ verb: "exchange", action, server: instance, ...args }))) as Record<string, unknown>;
    released(server, held.core, answer, instance);
    if (typeof answer.error === "string") throw new RangeError(answer.error);
    if (typeof answer.lease === "number") answer.lease = { core: held.core, lease: answer.lease, instance: { ...instance } } satisfies ExchangeLease;
    return answer;
}
