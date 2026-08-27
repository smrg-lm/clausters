// Authoring a **component bundle**: the directory a page mounts.
//
// The counterpart of `bundle.ts`, which mounts one. A bundle is the persisted
// form of an instrument — its defs, its GuiDef, its presets, its samples —
// plus the manifest that says what mounting it needs; the same directory runs
// on three legs (a browser tab as a custom element, `clausters-gui
// --standalone`, and a loopback host against a running server). Nothing here
// runs at mount time: it only *writes*.
//
// The port of `clausters.bundle` in the Python client, verb for verb. It is
// **off the slim `dist/runtime.js`**: a page that mounts a bundle does not
// author one, and the builders this needs (the defs, the GuiDef) are the ones
// the run time deliberately leaves out.
//
// Where it runs
// -------------
//
// `write` is a **node** verb — it takes a directory — and it is the milestone's
// centre: a bundle is an *input*, produced ahead of time and saved, so that a
// static page can boot it with no interpreter at all. A page can still author
// one; what it gets is `files`, the same bundle as text, which `openBundle`
// mounts without a round trip through disk.
//
// The two kinds of hole
// ---------------------
//
// Mounting the same bundle twice on one page must not collide, so the GuiDef
// record on disk is a **template** with placeholders, told apart by sigil:
// `@name` is a symbol (an id the page allocates: a node, a bus, a buffer) and
// `$name` is a parameter (a value the tag supplies, or a preset's, or the
// declared default). This class holds the symbol table, so the author names
// things instead of numbering them.
//
// Holes live **only** in the GuiDef record. The def payloads carry none, which
// is what lets two mounted instances share the one def that was sent — and it
// forces one authoring rule, which is the right rule anyway: *a bus, a node or
// a buffer reaches a def as a control, never as a baked constant.* `validate`
// checks that through the core and refuses to emit a bundle that breaks it —
// an unmountable bundle is unwritable.
//
// The bytes are canonical
// -----------------------
//
// There are two writers of this format — this one and the Python client's —
// and the same bundle authored in either language must be the *same
// directory*, not merely an equivalent one. That is only checkable if the
// bytes are, so both emit canonical JSON: keys sorted, no space between tokens
// (two spaces of indent for the two files a person reads, `bundle.json` and a
// preset), and numbers written the shortest way that reads back. Here that
// costs nothing — it is what `JSON.stringify` already does — and the sorting
// is what keeps the two builders' key order out of the comparison.
//
// The format itself is documented in docs/clients.md.

import { bundle_validate } from "./core/clausters_core_web.js";
import type { BundleManifest, ParamSpec } from "./bundle.ts";
import type { GuiNode } from "./gui/guidef.ts";

// The writer validates through the core, so whoever authors a bundle has to
// load it — and a node script cannot reach the package facade, which registers
// custom elements a document has to exist for. Re-exported here so this
// subpath is self-sufficient: `clausters/bundle-writer` plus `clausters/defs`
// and `clausters/gui` is the whole authoring surface.
export { loadCore } from "./base/core.ts";

/** Where a page serves the component run time from — the page's business. */
export const DEFAULT_RUNTIME = "/dist/runtime.js";

/**
 * A hole in the template: the `"@symbol"` or `"$param"` string a mount fills.
 *
 * It **is** that string at run time — `` `${lfo}` `` prints `@lfo` — and its
 * type is the intersection of the two things a hole stands for. A hole goes
 * where a *value* goes: a bus index, a knob's value, an argument of a boot
 * message. Typing it as `string` alone would make every one of those a type
 * error against builders that (correctly) ask for a number, and widening the
 * builders would put the template's vocabulary into a surface that has
 * nothing to do with bundles. The intersection is assignable to both, which
 * is exactly the latitude an author needs and no more; what the hole actually
 * carries is checked where the check is real — `validate`, against the
 * declared type and range, before anything is written.
 */
export type Hole = string & number;

/** A declared parameter's type, as `bundle.json` spells it. */
export type ParamType = "float" | "int" | "string" | "bool";

/** What `param` accepts beside the type. */
export interface ParamOptions {
    default?: unknown;
    min?: number;
    max?: number;
}

/**
 * A def this writer can carry: anything with a name and a `/def_send` payload
 * — a `SynthDef`, a `FaustDef`, a `GraphDef`. Structural on purpose, so the
 * writer does not import the def families to name them.
 */
export interface WritableDef {
    name: string;
    dumpDef(): string;
}

/** What `files` and `write` accept: where the run time is, and the tag. */
export interface WriteOptions {
    /** The URL the generated module imports the run time from. */
    runtime?: string;
    /** The custom element's name; defaults to the bundle's own. */
    tag?: string;
}

/**
 * A bundle being written: its symbols, its parameters, its defs and its
 * GuiDef.
 *
 * `name` names the bundle and **prefixes its def names** (a def name is a
 * global namespace on the server, so two bundles defining `voice` differently
 * must not collide). It is also the custom element's tag by default — HTML
 * wants a hyphen in one, so a one-word name needs an explicit
 * `write(dir, { tag })`.
 *
 * ```ts
 * const b = new Bundle("fm-voice");
 * const freq = b.param("freq", "float", { default: 220.0, min: 60.0, max: 700.0 });
 * const lfo = b.bus("lfo");
 * const node = b.node("voice");
 * b.synthdef(voice());                        // named "fm-voice.voice"
 * b.gui(scene(lfo, node, freq));
 * b.preset("bright", { freq: 660.0 });
 * await b.write("./fm-voice");
 * ```
 *
 * and the page gets a tag from one import (`write` generates `index.js`):
 *
 * ```html
 * <script type="module" src="./fm-voice/index.js"></script>
 * <fm-voice freq="440"></fm-voice>
 * ```
 */
export class Bundle {
    readonly name: string;
    /** The GuiDef's file stem under `defs/guidefs/`. */
    readonly guiName: string;

    #params: Record<string, ParamSpec> = {};
    #nodes: string[] = [];
    #buses: { name: string; rate: "audio" | "control"; channels: number }[] = [];
    #buffers: string[] = [];
    #bufferFiles: Record<string, string> = {};
    #synthdefs: WritableDef[] = [];
    #graphdefs: WritableDef[] = [];
    #presets: Record<string, Record<string, unknown>> = {};
    #gui: GuiNode | null = null;
    #boot: unknown[][] = [];

    constructor(name: string, { guiName }: { guiName?: string } = {}) {
        this.name = String(name);
        this.guiName = guiName ?? this.name;
    }

    // ---- the contract ----

    /**
     * Declares a parameter and returns its placeholder (`"$name"`).
     *
     * A parameter with no `default` is **required**: the tag or a preset must
     * supply it, and mounting without it is an error rather than a silent
     * zero. `min`/`max` bound the numeric kinds, checked at mount.
     */
    param(name: string, kind: ParamType = "float", options: ParamOptions = {}): Hole {
        if (!["float", "int", "string", "bool"].includes(kind)) {
            throw new Error(`parameter "${name}": type must be float, int, string or bool`);
        }
        const spec: ParamSpec = { type: kind };
        if (options.default !== undefined) spec.default = options.default;
        if (options.min !== undefined) spec.min = options.min;
        if (options.max !== undefined) spec.max = options.max;
        this.#params[String(name)] = spec;
        return `$${name}` as Hole;
    }

    /**
     * Declares a node symbol and returns its placeholder (`"@name"`) — the id
     * the page allocates for a synth or graph this bundle boots.
     */
    node(name: string): Hole {
        this.#declare(name);
        this.#nodes.push(String(name));
        return `@${name}` as Hole;
    }

    /**
     * Declares a bus symbol and returns its placeholder (`"@name"`).
     *
     * The placeholder reads naturally where a bus index goes
     * (`meter(lfo)`); a def that uses the bus takes it **as a control**,
     * never baked in.
     */
    bus(
        name: string,
        { rate = "control", channels = 1 }: { rate?: "audio" | "control"; channels?: number } = {},
    ): Hole {
        if (rate !== "control" && rate !== "audio") {
            throw new Error(`bus "${name}": rate must be 'control' or 'audio'`);
        }
        this.#declare(name);
        this.#buses.push({ name: String(name), rate, channels: Math.trunc(channels) });
        return `@${name}` as Hole;
    }

    /**
     * Declares a sample and returns its placeholder (`"@name"`).
     *
     * `path` is relative to the bundle directory (the file is the author's to
     * place there — it is data, not something a writer emits). The mount
     * allocates the buffer index and loads the file into it.
     */
    buffer(name: string, path: string): Hole {
        this.#declare(name);
        this.#buffers.push(String(name));
        this.#bufferFiles[String(name)] = String(path);
        return `@${name}` as Hole;
    }

    /** Refuses one name in two namespaces — `@name` would not say which. */
    #declare(name: string): void {
        const taken = new Set([...this.#nodes, ...this.#buses.map((b) => b.name), ...this.#buffers]);
        if (taken.has(String(name))) {
            throw new Error(`symbol "${name}" is already declared in this bundle`);
        }
    }

    // ---- the contents ----

    /**
     * Adds a SynthDef (or a FaustDef), prefixing its name with the bundle's,
     * and returns the prefixed name — what a `/synth_new` in the boot list
     * spawns. The def itself is renamed, so sending it directly sends the
     * bundle's name too.
     */
    synthdef(def: WritableDef): string {
        return this.#addDef(def, this.#synthdefs);
    }

    /**
     * Adds a GraphDef, prefixing its name with the bundle's, and returns the
     * prefixed name.
     */
    graphdef(def: WritableDef): string {
        return this.#addDef(def, this.#graphdefs);
    }

    #addDef(def: WritableDef, into: WritableDef[]): string {
        const prefixed = def.name.startsWith(`${this.name}.`)
            ? def.name
            : `${this.name}.${def.name}`;
        // A def's name is `readonly` to whoever builds one, and the writer is
        // the exception: the prefix has to reach the payload, not only the
        // file it is written to.
        Object.assign(def, { name: prefixed });
        into.push(def);
        return prefixed;
    }

    /**
     * Sets the GuiDef tree — the template. Its widgets should be numbered
     * `1..N`; the mount offsets them by an allocated base, so the numbers are
     * local to the bundle and never collide between instances.
     */
    gui(tree: GuiNode): void {
        this.#gui = tree;
    }

    /**
     * Adds boot messages — `[addr, ...args]` each, with placeholders where ids
     * and values go:
     *
     * ```ts
     * b.boot(["/graph_new", "fm-voice.graph", graph, 0, 0],
     *        ["/node_set", graph, "freq", freq]);
     * ```
     *
     * They run once per instance, after its defs are in. A parameter that
     * nothing draws reaches the synthesis this way; one a widget carries
     * reaches it through that widget's `bind`.
     */
    boot(...messages: unknown[][]): void {
        for (const message of messages) this.#boot.push([...message]);
    }

    /**
     * Declares a named preset — a bundle of parameter values a tag selects
     * with `preset="<name>"`. An attribute overrides it; it overrides the
     * declared defaults.
     */
    preset(name: string, values: Record<string, unknown>): void {
        const unknown = Object.keys(values).filter((k) => !(k in this.#params));
        if (unknown.length > 0) {
            throw new Error(
                `preset "${name}" sets undeclared parameter(s): ${unknown.sort().join(", ")}`,
            );
        }
        this.#presets[String(name)] = { ...values };
    }

    // ---- writing ----

    /** The `bundle.json` this bundle would write. */
    manifest(): BundleManifest {
        const out: BundleManifest = { name: this.name, gui: this.guiName };
        if (this.#synthdefs.length > 0) out.synthdefs = this.#synthdefs.map((d) => d.name);
        if (this.#graphdefs.length > 0) out.graphdefs = this.#graphdefs.map((d) => d.name);
        if (this.#gui !== null) out.widgets = widgetSpan(this.#gui);
        const symbols: NonNullable<BundleManifest["symbols"]> = {};
        if (this.#nodes.length > 0) symbols.nodes = [...this.#nodes];
        if (this.#buses.length > 0) symbols.buses = this.#buses.map((b) => ({ ...b }));
        if (this.#buffers.length > 0) symbols.buffers = [...this.#buffers];
        if (Object.keys(symbols).length > 0) out.symbols = symbols;
        if (Object.keys(this.#params).length > 0) out.params = { ...this.#params };
        const presets = Object.keys(this.#presets).sort();
        if (presets.length > 0) out.presets = presets;
        if (Object.keys(this.#bufferFiles).length > 0) out.buffers = { ...this.#bufferFiles };
        return out;
    }

    /**
     * The GuiDef record this bundle would write: `{ id: 1, gui: <tree> }`, the
     * boot list carried at the tree's root.
     */
    record(): { id: number; gui: unknown } {
        if (this.#gui === null) {
            throw new Error(`bundle "${this.name}" has no GuiDef (call .gui(...))`);
        }
        const tree: Record<string, unknown> = { ...this.#gui };
        if (this.#boot.length > 0) tree["boot"] = this.#boot;
        return { id: 1, gui: tree };
    }

    /**
     * Runs the core's pre-flight: the mount dry-run over the declared
     * defaults, plus the no-holes check on every def payload. Throws with the
     * reason — an unknown symbol, a parameter whose default does not
     * type-check, a hole baked into a def.
     *
     * `files` calls this first, so a bundle that would fail to mount fails to
     * be written. Requires a prior `loadCore()`.
     */
    validate(): void {
        const defs = [...this.#synthdefs, ...this.#graphdefs].map((d) => JSON.parse(d.dumpDef()));
        bundle_validate(
            JSON.stringify({ manifest: this.manifest(), template: this.record(), defs }),
        );
    }

    /**
     * The whole bundle as text, by path relative to its directory.
     *
     * Validates first, then builds every file `write` would write: the def
     * payloads (each its own `/def_send` spec), the GuiDef record, the
     * presets, the manifest, and the five-line ES module that registers the
     * tag. Samples are not here — the audio files are the author's to place in
     * the directory, and the manifest only names them.
     *
     * This is the writer without the disk, which is what a caller mounting a
     * bundle it has just authored wants (a page, a test, a build step that
     * serves it from memory). `write` is this plus the directory. Requires a
     * prior `loadCore()`.
     */
    files({ runtime = DEFAULT_RUNTIME, tag }: WriteOptions = {}): Record<string, string> {
        // The substance first: what the bundle *is* matters more than what it
        // will be called, and its error is the more useful one to see.
        this.validate();
        const name = tag ?? this.name;
        if (!name.includes("-") || name !== name.toLowerCase() || /^[0-9]/.test(name)) {
            throw new Error(
                `"${name}" is not a valid custom element name (lowercase, with a hyphen, ` +
                    "not starting with a digit) — pass write(dir, { tag })",
            );
        }
        const out: Record<string, string> = {
            "bundle.json": `${canonicalJson(this.manifest(), 2)}\n`,
            [`defs/guidefs/${this.guiName}.json`]: canonicalJson(this.record()),
            "index.js": generatedModule(name, runtime),
        };
        for (const def of this.#synthdefs) {
            out[`defs/synthdefs/${def.name}.json`] = canonicalJson(JSON.parse(def.dumpDef()));
        }
        for (const def of this.#graphdefs) {
            out[`defs/graphdefs/${def.name}.json`] = canonicalJson(JSON.parse(def.dumpDef()));
        }
        for (const [preset, values] of Object.entries(this.#presets)) {
            out[`presets/${preset}.json`] = `${canonicalJson(values, 2)}\n`;
        }
        return out;
    }

    /**
     * Writes the bundle to `directory` and returns the path. **Node only** —
     * a page authors with `files` and mounts what it holds.
     *
     * `files` is what it writes, and carries what each file is; this adds the
     * directories and the disk. Requires a prior `loadCore()`.
     */
    async write(directory: string, options: WriteOptions = {}): Promise<string> {
        const written = this.files(options);
        const { mkdir, writeFile } = await import("node:fs/promises");
        const { dirname, join } = await import("node:path");
        for (const [path, text] of Object.entries(written)) {
            const full = join(directory, ...path.split("/"));
            await mkdir(dirname(full), { recursive: true });
            await writeFile(full, text);
        }
        return directory;
    }
}

/** The generated ES module: one import, one call, a named tag. */
function generatedModule(tag: string, runtime: string): string {
    return (
        `// ${tag}/index.js -- generated by clausters.bundle; do not edit.\n` +
        `import { defineComponent } from "${runtime}";\n` +
        `defineComponent("${tag}", new URL(".", import.meta.url));\n`
    );
}

/**
 * The width of the id block one instance needs: the highest widget id the tree
 * uses, the root's included (the root is id 1).
 */
function widgetSpan(tree: GuiNode): number {
    let high = 1;
    const stack: unknown[] = [tree];
    while (stack.length > 0) {
        const node = stack.pop();
        if (typeof node !== "object" || node === null) continue;
        const { id, children } = node as { id?: unknown; children?: unknown };
        if (typeof id === "number" && Number.isInteger(id)) high = Math.max(high, id);
        if (Array.isArray(children)) stack.push(...children);
    }
    return high;
}

/**
 * `value` as canonical JSON — what both writers of this format emit (see the
 * module header). The round trip through `JSON.parse` is what makes the input
 * comparable: a `View` is an object with private fields and a `toJSON` may
 * stand between the two, and what has to be sorted is the document that comes
 * out of them.
 */
function canonicalJson(value: unknown, indent?: number): string {
    return JSON.stringify(sorted(JSON.parse(JSON.stringify(value))), null, indent);
}

/** `value` deep-copied with every object's keys in sorted order. */
function sorted(value: unknown): unknown {
    if (Array.isArray(value)) return value.map(sorted);
    if (typeof value !== "object" || value === null) return value;
    const out: Record<string, unknown> = {};
    for (const key of Object.keys(value).sort()) {
        out[key] = sorted((value as Record<string, unknown>)[key]);
    }
    return out;
}
