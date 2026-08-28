// The offline renderer: the engine's wasm, running as fast as it can.
//
// The same `clausters-web` module the AudioWorklet runs, reached through its
// other door — `render(score, rate, channels, seed)`, which is the server's
// own `--nrt` path compiled to wasm. No `AudioContext`, no worklet and no
// audio device: a score in, samples out, at whatever speed the machine
// manages.
//
// It is loaded **on demand and separately from the worklet's copy**: a page
// that only renders never boots an engine, and a page that does both compiles
// the module once here and hands the worklet its own instance (the worklet
// needs a synchronous instantiation on the audio thread, so the two cannot
// share an instance). The module is memoized, so the second render pays
// nothing.

import { compileFaustDefs } from "./faust-compiler.ts";
import type { FaustJob } from "./faust-compiler.ts";
import { linkFaustModule } from "./faust-link.ts";
import type { EngineExports } from "./faust-link.ts";

/** What one offline render produced. */
export interface RenderResult {
    /** Interleaved samples. */
    samples: Float32Array;
    /** The seed this take started from — hand it back to replay it. */
    seed: bigint;
}

/**
 * The engine wasm's own surface, as `loadRenderer` resolves it: the
 * wasm-bindgen glue's init pair plus the render entry points. Exported because
 * it is what a public signature returns — a reference that names a type the
 * reader cannot reach is a broken page.
 */
export type EngineModule = {
    default: (init?: unknown) => Promise<unknown>;
    initSync: (module: unknown) => unknown;
    render: (
        score: Uint8Array,
        sampleRate: number,
        channels: number,
        seed?: bigint | null,
    ) => Float32Array;
    last_render_seed: () => bigint;
    /** The Faust defs a score sends, as JSON — see `prepareFaust`. */
    faustJobs: (score: Uint8Array) => string;
    /** Adopts a def this side has compiled and linked, under its score name. */
    linkFaust: (name: string, compute: number, init: number, json: string) => void;
};

let loaded: Promise<EngineModule> | null = null;
/** The renderer instance's exports, for linking a Faust module into it. */
let exports: EngineExports | null = null;
/** Linked Faust modules, kept alive: the table holds their functions. */
const linked: WebAssembly.Instance[] = [];

/**
 * Loads the engine wasm once. `source` overrides the URL-relative lookup with
 * raw module bytes — the node path, where there is no page to resolve a URL
 * against (the same shape `loadCore` takes for the core).
 */
export function loadRenderer(source?: BufferSource): Promise<EngineModule> {
    loaded ??= (async () => {
        const glue = (await import("./clausters_web.js")) as unknown as EngineModule;
        // What init returns is the instance's own wasm exports -- the memory
        // and the table a Faust module is linked against.
        exports = (source ? glue.initSync({ module: source }) : await glue.default()) as
            EngineExports;
        return glue;
    })();
    return loaded;
}

/**
 * Compiles and links every Faust def the score sends, before it is rendered.
 *
 * This is the one thing an offline render in a page has to do that a native one
 * never does. `server::render` loads a def **where it stands** — time does not
 * advance until it has — and a page's Faust compiler is another scope that
 * answers later, so there is no turn in which a compiled def could arrive
 * mid-render. Doing the work in the other order is what makes the two renders
 * the same render: the score is read by the engine's own reader (`faustJobs`,
 * not a second one written here), each def is compiled by the same Worker the
 * live engine compiles with, linked into **this** instance's memory and table,
 * and handed back through `linkFaust`. The renderer's `/def_send faust` is then
 * a lookup, and everything after it — `/node_set` by name, bus summing, done
 * actions — is the code a native render runs.
 *
 * A score with no Faust def in it does none of this and loads no compiler.
 */
async function prepareFaust(engine: EngineModule, score: Uint8Array): Promise<void> {
    const jobs = JSON.parse(engine.faustJobs(score)) as FaustJob[];
    if (jobs.length === 0) return;
    const compiled = await compileFaustDefs(jobs);
    for (const def of compiled) {
        const { instance, compute, init } = linkFaustModule(
            exports as EngineExports,
            def.bytes,
        );
        linked.push(instance);
        engine.linkFaust(def.name, compute, init, def.json);
    }
}

/**
 * Renders a binary score and reports the seed the take used.
 *
 * Synchronous once the module is in **and the score's Faust defs are compiled**
 * (see `prepareFaust`; a score without one waits for nothing): the render then
 * occupies this thread until it finishes, which is what "faster than real time"
 * costs. A page rendering minutes of audio should say so in its UI — nothing
 * else runs meanwhile.
 */
export async function renderScoreBytes(
    score: Uint8Array,
    sampleRate: number,
    channels: number,
    seed?: number | bigint,
): Promise<RenderResult> {
    const engine = await loadRenderer();
    await prepareFaust(engine, score);
    const samples = engine.render(
        score,
        sampleRate,
        channels,
        seed === undefined ? entropySeed() : BigInt(seed),
    );
    return { samples, seed: engine.last_render_seed() };
}

/**
 * A fresh 64-bit seed, drawn here rather than by the renderer.
 *
 * The engine's own entropy source is `SystemTime`, which wasm does not have —
 * so a render given no seed would take a **fixed** one there and every take of
 * a noisy piece would be the same take. The platform that does have entropy is
 * this one, so the shell forwards a word from it and the rule the reference
 * client states holds here too: a render with no seed is a new take, and
 * `stats.seed` is how you get it back.
 */
function entropySeed(): bigint {
    const words = new Uint32Array(2);
    if (typeof crypto !== "undefined" && crypto.getRandomValues) {
        crypto.getRandomValues(words);
    } else {
        // Node before `globalThis.crypto`, and any environment without one:
        // still varying, which is all this has to be.
        words[0] = (Math.random() * 0x100000000) >>> 0;
        words[1] = (Date.now() ^ (Math.random() * 0x100000000)) >>> 0;
    }
    return (BigInt(words[0]!) << 32n) | BigInt(words[1]!);
}
