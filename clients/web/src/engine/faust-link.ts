// Linking a compiled Faust def into an engine instance.
//
// This is the whole of the Faust design in a dozen lines (`docs/decisions.md`,
// "The page's Faust is a second wasm module linked into the engine's own
// memory"): the module the compiler emitted is instantiated against the
// engine's **own** linear memory, with every import it declares resolved from
// the engine's own exports — the memory itself and the transcendentals — so
// nothing is copied and no JavaScript runs on the audio path. Its `compute` and
// `init` are appended to the engine's indirect function table, and the two slot
// numbers are what the engine calls through.
//
// It lives here rather than in the worklet because there are **two** engine
// instances in a page that does both things — the worklet's, instantiated
// synchronously on the audio thread, and the offline renderer's — and the def
// has to be linked into whichever one is going to run it. Two copies of this
// would be two answers to "what does the engine export", which is exactly the
// kind of divergence nothing finds.

/**
 * An engine instance's own wasm exports, past the binding's surface.
 *
 * A Faust def is a second wasm module: it is instantiated against this `memory`
 * and its `compute` is appended to this `__indirect_function_table`, so the
 * engine calls it as a plain indirect call with no JavaScript frame in the way.
 * The math functions are the rest of what such a module imports (`env._sinf`
 * and its neighbours) — bound to these, so Faust and our own UGens go through
 * one libm.
 */
export type EngineExports = {
    memory: WebAssembly.Memory;
    __indirect_function_table: WebAssembly.Table;
} & Record<string, unknown>;

/** Where a linked def's two entry points landed in the engine's table. */
export interface LinkedDef {
    compute: number;
    init: number;
    /** The instance itself: the table holds its functions, and it owns them. */
    instance: WebAssembly.Instance;
}

/**
 * Instantiates one compiled Faust module against `engine` and appends its two
 * entry points to the engine's table.
 *
 * `bytes` is what the compiler emitted, already stripped of its data section.
 * They travel as bytes rather than as a `WebAssembly.Module` because a Module
 * posted into an AudioWorklet is dropped on arrival without an error (a worklet
 * is its own agent cluster), so the compile happens on this side; a Faust
 * module is a couple of kilobytes, which is microseconds once per def.
 *
 * An import the engine does not export is **named** rather than left to fail as
 * a link error nobody can read.
 */
export function linkFaustModule(engine: EngineExports, bytes: BufferSource): LinkedDef {
    const module = new WebAssembly.Module(bytes);
    const env: WebAssembly.ModuleImports = {};
    for (const wanted of WebAssembly.Module.imports(module)) {
        if (wanted.module !== "env") {
            throw new Error(`the module imports ${wanted.module}.${wanted.name}`);
        }
        const ours = (wanted.name === "memory" ? engine.memory : engine[wanted.name]) as
            | WebAssembly.ImportValue
            | undefined;
        if (ours === undefined) throw new Error(`the engine exports no ${wanted.name}`);
        env[wanted.name] = ours;
    }
    const instance = new WebAssembly.Instance(module, { env });
    return {
        instance,
        compute: slot(engine, instance.exports.compute),
        init: slot(engine, instance.exports.init),
    };
}

/** Appends one function to the engine's table and returns its slot. */
function slot(engine: EngineExports, fn: WebAssembly.ExportValue | undefined): number {
    if (typeof fn !== "function") throw new Error("the module exports no such function");
    const table = engine.__indirect_function_table;
    const at = table.grow(1);
    table.set(at, fn);
    return at;
}
