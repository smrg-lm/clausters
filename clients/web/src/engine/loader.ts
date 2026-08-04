// The main-thread loader: boots the engine inside an AudioWorklet.
//
// Compiles the wasm module here (async, streaming), registers the processor
// module, and hands the compiled WebAssembly.Module to the worklet through
// processorOptions — the worklet instantiates it synchronously in its
// constructor, so no async work ever happens on the audio thread.
//
// The returned handle speaks raw OSC bytes (send / onReply); bring a codec
// (`base/osc.ts`, the shared-core one). resume() is the autoplay-policy
// hook: call it from a user gesture. The per-page `server()` singleton
// (`server.ts`) wraps this handle and fans its single onReply slot out;
// pages and the TS client normally go through that.

/** The raw engine handle: one reply-callback slot, raw byte I/O. */
export interface ClaustersEngine {
    context: AudioContext;
    node: AudioWorkletNode;
    /** Reply callback slot — one consumer; multiplexers own it and fan out. */
    onReply: ((packet: Uint8Array) => void) | null;
    onQuit: (() => void) | null;
    onError: ((message: string) => void) | null;
    resume(): Promise<void>;
    suspend(): Promise<void>;
    /** One complete OSC packet; the bytes are transferred, not copied. */
    send(bytes: Uint8Array): void;
    /** The engine's sample clock (a round trip through the audio thread). */
    clock(): Promise<number>;
    /**
     * The engine's clock paired with the context's own frame counter, both
     * read in the same instant on the audio thread. Their difference is a
     * fixed integer, so a client can map `AudioContext.currentTime` onto the
     * engine's sample axis afterwards with no further round trip.
     */
    clockAnchor(): Promise<ClockAnchor>;
    /**
     * Installs host-decoded samples as buffer `index` (the browser's
     * /buffer_allocRead). `samples` is interleaved and transferred.
     */
    bufferLoad(
        index: number,
        channels: number,
        sampleRate: number,
        samples: Float32Array,
    ): Promise<number>;
}

/** One reading of the engine's clock against the context's frame counter. */
export interface ClockAnchor {
    /** The engine's sample counter. */
    sample: number;
    /** The context's frame counter at the same instant. */
    frame: number;
    /** Unix seconds at engine sample 0. */
    epoch: number;
}

export interface BootOptions {
    context?: AudioContext | null;
    channels?: number;
    wasmUrl?: URL | string;
    workletUrl?: URL | string;
}

type WorkletReply =
    | { type: "osc"; data: ArrayBuffer }
    | { type: "clock"; clock: number; frame: number; epoch: number }
    | { type: "buffer_load"; index: number; ok: boolean; message?: string }
    | { type: "quit" }
    | { type: "error"; message: string };

export async function bootClausters({
    context = null,
    channels = 2,
    wasmUrl = new URL("./clausters_web_bg.wasm", import.meta.url),
    workletUrl = new URL("./worklet.js", import.meta.url),
}: BootOptions = {}): Promise<ClaustersEngine> {
    const ctx = context ?? new AudioContext();
    const [module] = await Promise.all([
        WebAssembly.compileStreaming(fetch(wasmUrl)),
        ctx.audioWorklet.addModule(workletUrl),
    ]);

    const node = new AudioWorkletNode(ctx, "clausters", {
        numberOfInputs: 0,
        numberOfOutputs: 1,
        outputChannelCount: [channels],
        processorOptions: {
            module,
            channels,
            // Unix seconds at engine sample 0: the anchor that lets
            // wall-clocked bundle timetags land on the engine's sample axis.
            unixEpoch: Date.now() / 1000,
        },
    });
    node.connect(ctx.destination);

    const handle: ClaustersEngine = {
        context: ctx,
        node,
        onReply: null,
        onQuit: null,
        onError: null,
        resume: () => ctx.resume(),
        suspend: () => ctx.suspend(),
        send(bytes: Uint8Array) {
            node.port.postMessage({ type: "osc", data: bytes }, [
                bytes.buffer as ArrayBuffer,
            ]);
        },
        clock() {
            return this.clockAnchor().then((anchor) => anchor.sample);
        },
        clockAnchor() {
            return new Promise((resolve) => {
                clockWaiters.push(resolve);
                node.port.postMessage({ type: "clock" });
            });
        },
        bufferLoad(index, channels, sampleRate, samples) {
            return new Promise((resolve, reject) => {
                loadWaiters.push({ resolve, reject });
                node.port.postMessage(
                    {
                        type: "buffer_load",
                        index,
                        channels,
                        sampleRate,
                        data: samples.buffer,
                    },
                    [samples.buffer as ArrayBuffer],
                );
            });
        },
    };

    const clockWaiters: ((anchor: ClockAnchor) => void)[] = [];
    const loadWaiters: {
        resolve: (index: number) => void;
        reject: (error: Error) => void;
    }[] = [];
    node.port.onmessage = (e) => {
        const msg = e.data as WorkletReply;
        if (msg.type === "osc") handle.onReply?.(new Uint8Array(msg.data));
        else if (msg.type === "clock") {
            clockWaiters.shift()?.({
                sample: msg.clock,
                frame: msg.frame,
                epoch: msg.epoch,
            });
        }
        else if (msg.type === "buffer_load") {
            const waiter = loadWaiters.shift();
            if (msg.ok) waiter?.resolve(msg.index);
            else waiter?.reject(new Error(msg.message));
        } else if (msg.type === "quit") handle.onQuit?.();
        else if (msg.type === "error") handle.onError?.(msg.message);
    };

    return handle;
}
