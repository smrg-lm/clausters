// The per-page audio-server singleton.
//
// A page has one audio engine, however many components sit on it: the first
// `server()` call boots the AudioWorklet engine (AudioContext + worklet +
// wasm server, via the engine bundle's loader) and every later call — another
// component, a REPL, the TS client — gets the same instance. The raw engine
// handle exposes a single `onReply` slot, so the singleton owns it and fans
// replies out to any number of listeners; everything else passes through.
//
// The AudioContext starts suspended under the autoplay policy: call `resume()`
// from a user gesture (the `<clausters-*>` elements' power affordance does).

import { bootClausters } from "./loader.ts";
import type { BootOptions } from "./loader.ts";

export type ReplyListener = (packet: Uint8Array) => void;

/// The shared engine surface: raw OSC bytes in, fanned-out replies back —
/// what the connection seam (`base/connection.ts`) and any REPL build on.
export interface ClaustersServer {
    context: AudioContext;
    node: AudioWorkletNode;
    /// One complete OSC packet to the engine (bytes are transferred).
    send(bytes: Uint8Array): void;
    /// Subscribe to every engine reply packet.
    addReply(listener: ReplyListener): void;
    removeReply(listener: ReplyListener): void;
    clock(): Promise<number>;
    /// Installs host-decoded samples as buffer `index` (the browser's
    /// /b_allocRead); `samples` is interleaved and transferred.
    bLoad(
        index: number,
        channels: number,
        sampleRate: number,
        samples: Float32Array,
    ): Promise<number>;
    resume(): Promise<void>;
    suspend(): Promise<void>;
}

let instance: Promise<ClaustersServer> | null = null;

/// The page's engine, booting it on first call. `options` (channels, an
/// existing AudioContext) only apply to that first call.
export function server(options: BootOptions = {}): Promise<ClaustersServer> {
    instance ??= boot(options);
    return instance;
}

async function boot(options: BootOptions): Promise<ClaustersServer> {
    const raw = await bootClausters(options);
    const listeners = new Set<ReplyListener>();
    raw.onReply = (bytes) => {
        for (const listener of [...listeners]) listener(bytes);
    };
    raw.onError = (message) => console.error(`clausters engine: ${message}`);
    raw.onQuit = () => console.warn("clausters engine: /quit — engine stopped");
    return {
        context: raw.context,
        node: raw.node,
        send: (bytes) => raw.send(bytes),
        addReply: (listener) => listeners.add(listener),
        removeReply: (listener) => listeners.delete(listener),
        clock: () => raw.clock(),
        bLoad: (index, channels, sampleRate, samples) =>
            raw.bLoad(index, channels, sampleRate, samples),
        resume: () => raw.context.resume(),
        suspend: () => raw.context.suspend(),
    };
}
