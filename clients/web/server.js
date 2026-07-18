// The per-page audio-server singleton.
//
// A page has one audio engine, however many components sit on it: the first
// `server()` call boots the AudioWorklet engine (AudioContext + worklet +
// wasm server, via the engine bundle's loader) and every later call — another
// component, a REPL, a future TS client — gets the same instance. The raw
// engine handle exposes a single `onReply` slot, so the singleton owns it and
// fans replies out to any number of listeners; everything else passes through.
//
// The AudioContext starts suspended under the autoplay policy: call `resume()`
// from a user gesture (the `<clausters-*>` elements' power affordance does).

import { bootClausters } from "./engine/loader.js";

let instance = null;

/// The page's engine, booting it on first call. `options` (channels, an
/// existing AudioContext) only apply to that first call.
export function server(options = {}) {
    instance ??= boot(options);
    return instance;
}

async function boot(options) {
    const raw = await bootClausters(options);
    const listeners = new Set();
    raw.onReply = (bytes) => {
        for (const listener of [...listeners]) listener(bytes);
    };
    raw.onError = (message) => console.error(`clausters engine: ${message}`);
    raw.onQuit = () => console.warn("clausters engine: /quit — engine stopped");
    return {
        context: raw.context,
        node: raw.node,
        /// One complete OSC packet to the engine (bytes are transferred).
        send: (bytes) => raw.send(bytes),
        /// Subscribe to every engine reply packet: `listener(Uint8Array)`.
        addReply: (listener) => listeners.add(listener),
        removeReply: (listener) => listeners.delete(listener),
        clock: () => raw.clock(),
        bLoad: (index, channels, sampleRate, samples) =>
            raw.bLoad(index, channels, sampleRate, samples),
        resume: () => raw.context.resume(),
        suspend: () => raw.context.suspend(),
    };
}
