import type { Server } from "./server/index.ts";
import { resolveServer } from "./wire.ts";

/** One native audio port routed to consecutive server buses. */
export interface PluginPortReference { id: number; channels: number; bus?: number }
/** Persistent recipe, with opaque initial state bytes and native identities. */
export interface PluginReference {
    format: string;
    path: string;
    id: string;
    inputs: PluginPortReference[];
    outputs: PluginPortReference[];
    state?: number[];
    depth?: number;
}
/** Native plugin recipe; each Synth owns one isolated, persistent helper.
 * Controls are `param:<id>` (CLAP plain units, VST3 normalized `[0, 1]`),
 * `in:<id>` and `out:<id>`.
 * IDs come from scanPlugins; every native audio port must be represented.
 */
export class PluginDef {
    name: string;
    reference: PluginReference;
    constructor(name: string, reference: PluginReference) { this.name = name; this.reference = reference; }
    /** Serialize the reference and optional initial state. */
    dumpDef(): string { return JSON.stringify(this.reference); }
    /** Send asynchronously; wait=false requires a subsequent sync barrier. */
    async send(server?: Server, { wait = true, timeout = 10.0 }: { wait?: boolean; timeout?: number } = {}): Promise<string> {
        const target = resolveServer(server);
        if (wait) await target.command("/def_send", ["plugin", this.name, this.dumpDef()], timeout);
        else target.sendMsg("/def_send", "plugin", this.name, this.dumpDef());
        return this.name;
    }
}
