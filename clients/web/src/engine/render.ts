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
};

let loaded: Promise<EngineModule> | null = null;

/**
 * Loads the engine wasm once. `source` overrides the URL-relative lookup with
 * raw module bytes — the node path, where there is no page to resolve a URL
 * against (the same shape `loadOsc` takes for the core).
 */
export function loadRenderer(source?: BufferSource): Promise<EngineModule> {
    loaded ??= (async () => {
        const glue = (await import("./clausters_web.js")) as unknown as EngineModule;
        if (source) glue.initSync({ module: source });
        else await glue.default();
        return glue;
    })();
    return loaded;
}

/**
 * Renders a binary score and reports the seed the take used.
 *
 * Synchronous once the module is in: the render occupies this thread until it
 * finishes, which is what "faster than real time" costs. A page rendering
 * minutes of audio should say so in its UI — nothing else runs meanwhile.
 */
export async function renderScoreBytes(
    score: Uint8Array,
    sampleRate: number,
    channels: number,
    seed?: number | bigint,
): Promise<RenderResult> {
    const engine = await loadRenderer();
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
