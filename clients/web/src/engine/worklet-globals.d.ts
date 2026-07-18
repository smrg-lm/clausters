// Ambient declarations for the AudioWorkletGlobalScope, which TypeScript's
// dom lib does not model (worklet.ts is the only module evaluated in that
// scope; the declarations are global to the program, but nothing outside the
// worklet references them).

declare abstract class AudioWorkletProcessor {
    readonly port: MessagePort;
    constructor(options?: unknown);
}

declare function registerProcessor(
    name: string,
    ctor: new (options: any) => AudioWorkletProcessor,
): void;

/// The AudioWorkletGlobalScope global: the context's sample rate.
declare const sampleRate: number;
