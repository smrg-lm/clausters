// Native plugins on a native server: the catalog of what is installed, one
// plugin as an object, and the def that instantiates it.
//
// The information has three layers. The **catalog** (`PluginCatalog`, from
// `server.plugins`) is what the server's discovery found. A **plugin**
// (`Plugin`) is one entry of it: what it is called, its ports and parameters,
// what it supports and what it declares itself to be -- with its identity
// inside it, so a program names a plugin by what it is called and never types
// its native id. A **def** (`PluginDef`) is the decision about one plugin:
// which buses its ports use and the state it starts from, which is what the
// server stores and what a `Synth` instantiates.
//
// The vocabulary, the search and the port rule are the shared core's
// (`clausters_core::plugin_catalog`), so a page reads a catalog as the Python
// client does and a def that does not fit its plugin is refused before it is
// sent.
import { plugin_catalog, plugin_reference, plugin_search } from "../core/clausters_core_web.js";
import type { Server } from "./server/index.ts";
import { resolveServer } from "./wire.ts";

/** One native audio port routed to consecutive server buses; a negative bus
 * leaves it unconnected. */
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
/** Where one direction's ports go: `null` leaves them unconnected, a bus
 * puts the main port there and leaves the auxiliary ones unconnected (an
 * insert, as a track takes one), `{port: bus}` routes the ports it names (by
 * control or native-id name). A port's channels take consecutive buses from
 * its own. */
export type PluginRoute = null | number | Record<string, number>;
/** The terms a catalog search takes; all must hold. */
export interface PluginQuery {
    vendor?: string;
    format?: string;
    kind?: string;
    tag?: string;
}

/** One core door: a JSON request in, its answer out, an error thrown. */
function door(call: (request: string) => string, request: unknown): Record<string, unknown> {
    const answer = JSON.parse(call(JSON.stringify(request))) as Record<string, unknown>;
    if (typeof answer.error === "string") throw new Error(answer.error);
    return answer;
}

/**
 * **One installed native plugin**, read-only, as the server's discovery
 * reports it.
 *
 * Its identity is `format`, `path` (the binary, on the server's machine) and
 * `id` (the plugin's native id within the binary); `key` is the three in one
 * string. **What it is** is read from what it declares in either format's
 * vocabulary (`features`, as declared): `kind` is one of `"instrument"`,
 * `"effect"`, `"note_effect"`, `"analyzer"` or `"other"`, `tags` the common
 * words for what it does and `channels` its layout. Data a plugin loads from
 * outside (an impulse response, samples) has no standard declaration; it
 * lives in the plugin's state. `controls` maps the names a node's controls
 * answer to -- `in`/`out` for the main audio ports, `in_<name>` and
 * `out_<name>` for the others, a parameter's name -- to the native-id names,
 * which always work too.
 */
export class Plugin {
    readonly #entry: Record<string, unknown>;
    constructor(entry: Record<string, unknown>) { this.#entry = entry; }
    #get<T>(key: string, fallback: T): T { return (this.#entry[key] as T | undefined) ?? fallback; }
    /** `"clap"` or `"vst3"`. */
    get format(): string { return this.#get("format", ""); }
    /** The binary, on the server's machine. */
    get path(): string { return this.#get("path", ""); }
    /** The plugin's native id within its binary. */
    get id(): string { return this.#get("id", ""); }
    /** Its identity in one string: `format:path#id`. */
    get key(): string { return this.#get("key", ""); }
    /** What it is called. */
    get name(): string { return this.#get("name", ""); }
    /** Who makes it. */
    get vendor(): string { return this.#get("vendor", ""); }
    /** The vendor's version string. */
    get version(): string { return this.#get("version", ""); }
    /** Its own one-line description, when it has one. */
    get description(): string { return this.#get("description", ""); }
    /** The plugin's or vendor's web address. */
    get url(): string { return this.#get("url", ""); }
    /** What it is: `"instrument"`, `"effect"`, `"note_effect"`, `"analyzer"` or `"other"`. */
    get kind(): string { return this.#get("kind", "other"); }
    /** Its channel layout: `"mono"`, `"stereo"`, `"surround"` or `"ambisonic"`. */
    get channels(): string | null { return this.#get<string | null>("channels", null); }
    /** Its declared latency, in samples. */
    get latency(): number { return this.#get("latency", 0); }
    /** What it declares itself to be, in its format's vocabulary, as declared. */
    get features(): string[] { return [...this.#get<string[]>("features", [])]; }
    /** What it does, in the common vocabulary (`"synth"`, `"reverb"`, ...). */
    get tags(): string[] { return [...this.#get<string[]>("tags", [])]; }
    /** Its audio inputs, in processing order. */
    get inputs(): Record<string, unknown>[] { return [...this.#get<Record<string, unknown>[]>("inputs", [])]; }
    /** Its audio outputs, in processing order. */
    get outputs(): Record<string, unknown>[] { return [...this.#get<Record<string, unknown>[]>("outputs", [])]; }
    /** Its note inputs. */
    get noteInputs(): Record<string, unknown>[] { return [...this.#get<Record<string, unknown>[]>("note_inputs", [])]; }
    /** Its note outputs. */
    get noteOutputs(): Record<string, unknown>[] { return [...this.#get<Record<string, unknown>[]>("note_outputs", [])]; }
    /** Its parameters, each with `id`, `name`, `module`, `flags`, `min`, `max`,
     * `step` and `default` in the format's native units. */
    get parameters(): Record<string, unknown>[] { return [...this.#get<Record<string, unknown>[]>("parameters", [])]; }
    /** The names its node's controls answer to, each with the native-id name it stands for. */
    get controls(): Record<string, string> { return { ...this.#get<Record<string, string>>("controls", {}) }; }
    /** Whether the plugin saves and loads its state. */
    get hasState(): boolean { return Boolean(this.#entry.state); }
    /** Whether the plugin has a native editor window. */
    get hasEditor(): boolean { return Boolean(this.#entry.editor); }
    /** The catalog's entry for it, as JSON writes it. */
    data(): Record<string, unknown> { return JSON.parse(JSON.stringify(this.#entry)); }
}

/**
 * **What is installed**: the plugins the server's discovery found, each a
 * {@link Plugin}, and the binaries that failed (`errors`, each `{path,
 * error}`). Made by `server.plugins`; iterate it, index it or search it.
 */
export class PluginCatalog {
    readonly plugins: Plugin[];
    readonly errors: { path: string; error: string }[];
    readonly #entries: Record<string, unknown>[];
    constructor(data: { plugins?: Record<string, unknown>[]; errors?: { path: string; error: string }[] }) {
        this.#entries = data.plugins ?? [];
        this.plugins = this.#entries.map((entry) => new Plugin(entry));
        this.errors = [...(data.errors ?? [])];
    }
    /** The catalog of `/plugin_scan` rows, read by the core. */
    static fromRows(rows: Record<string, unknown>[]): PluginCatalog {
        return new PluginCatalog(door(plugin_catalog, { rows }) as ConstructorParameters<typeof PluginCatalog>[0]);
    }
    get length(): number { return this.plugins.length; }
    [Symbol.iterator](): Iterator<Plugin> { return this.plugins[Symbol.iterator](); }
    #query(name: string | null, query: PluginQuery): Record<string, string> {
        const terms: Record<string, string | undefined> = { name: name ?? undefined, ...query };
        return Object.fromEntries(Object.entries(terms).filter(([, v]) => v !== undefined)) as Record<string, string>;
    }
    /**
     * **The one plugin these name.** `name` is part of the name, any case, and
     * a whole-name match wins over the rest; `vendor`, `format` (`"clap"` or
     * `"vst3"`), `kind` and `tag` must match too. Throws when none matches, or
     * when several still do -- the message names them, so a format or a vendor
     * can say which.
     */
    find(name: string | null = null, query: PluginQuery = {}): Plugin {
        const answer = door(plugin_search, { plugins: this.#entries, query: this.#query(name, query), one: true });
        return this.plugins[answer.index as number];
    }
    /** **Every plugin these hold for**, in catalog order ({@link find}'s terms). */
    filter(name: string | null = null, query: PluginQuery = {}): Plugin[] {
        const answer = door(plugin_search, { plugins: this.#entries, query: this.#query(name, query), one: false });
        return (answer.indices as number[]).map((i) => this.plugins[i]);
    }
    /** The instruments. */
    instruments(): Plugin[] { return this.filter(null, { kind: "instrument" }); }
    /** The effects. */
    effects(): Plugin[] { return this.filter(null, { kind: "effect" }); }
}

/**
 * **A persistent recipe for one native plugin**: which plugin, which buses its
 * audio ports use, and the state it starts from. Each `Synth` of it is an
 * isolated instance with its own helper process.
 *
 * Built from a {@link Plugin}, its ports are routed by `inputs` and `outputs`
 * ({@link PluginRoute}): outputs default to bus 0 and inputs to unconnected
 * (an input reads silence); a node changes them as controls (`in`, `out`). A
 * port the plugin does not have is refused here, before anything is sent.
 * `state` is optional initial-state bytes. A {@link PluginReference} is the
 * stored form, and a stored def is rebuilt from one.
 *
 * A node's controls are named as the plugin's `controls` say, and by their
 * native ids: parameters `param:<native-id>` (CLAP plain units, VST3
 * normalized `[0, 1]`), ports `in:<native-id>` and `out:<native-id>`.
 */
export class PluginDef {
    name: string;
    reference: PluginReference;
    /** The plugin it was built from; `null` for a stored reference. */
    plugin: Plugin | null;
    constructor(name: string, plugin: Plugin | PluginReference,
        { inputs = null, outputs = 0, state = null }: { inputs?: PluginRoute; outputs?: PluginRoute; state?: Uint8Array | number[] | null } = {}) {
        this.name = name;
        if (plugin instanceof Plugin) {
            this.plugin = plugin;
            this.reference = door(plugin_reference, {
                plugin: plugin.data(), inputs, outputs, state: state === null ? null : [...state],
            }) as unknown as PluginReference;
        } else {
            if (inputs !== null || outputs !== 0 || state !== null) {
                throw new Error("a reference routes its own ports: pass inputs, outputs and state with a Plugin");
            }
            this.plugin = null;
            this.reference = plugin;
        }
    }
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
