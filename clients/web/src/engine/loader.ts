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
    /** Every reply, with the tag of the client it is for (`peer`). */
    onReply: ((packet: Uint8Array, peer: number) => void) | null;
    onQuit: (() => void) | null;
    onError: ((message: string) => void) | null;
    resume(): Promise<void>;
    suspend(): Promise<void>;
    /**
     * Ends the auxiliary threads this engine started (the NRT worker). The
     * `AudioContext` is the caller's to close; this is what the caller cannot
     * see to close itself.
     */
    dispose(): void;
    /** One complete OSC packet; the bytes are transferred, not copied. */
    /**
     * One complete OSC packet to the engine, authored by `peer` — which of the
     * page's clients is speaking. The server keeps a subscription and a reply
     * queue per client, so two of them sharing this engine (a script and a GUI
     * host) must not send under the same tag.
     */
    send(bytes: Uint8Array, peer: number): void;
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
    /**
     * The NRT worker: the thread that reads the page's filesystem and decodes,
     * so the AudioWorklet does neither. Optional in every sense — a browser
     * with no `Worker`, a refused port transfer or a Worker that does not
     * answer all leave the engine doing the work itself, which is what it did
     * before this existed.
     */
    nrtWorkerUrl?: URL | string;
    /**
     * How many control buses one `/bus_stream` subscription may list, the
     * page's half of the server's `--max-stream-buses` (default 4096). A
     * document whose canvases hold hundreds of live widgets subscribes a bus
     * per meter, and the union is one subscription — so this is the knob that
     * decides how large a page may grow before the engine refuses it, the same
     * decision an operator makes on a server process.
     *
     * The effective ceiling is this clamped by what one reply can carry over
     * the page's ring; `/server_query.reply` reports that number, and the GUI
     * host reads it from there rather than assuming either.
     */
    maxStreamBuses?: number;
}

type WorkletReply =
    | { type: "osc"; data: ArrayBuffer; peer: number }
    | { type: "clock"; clock: number; frame: number; epoch: number }
    | { type: "buffer_load"; index: number; ok: boolean; message?: string }
    | { type: "quit" }
    | { type: "error"; message: string };

export async function bootClausters({
    context = null,
    channels = 2,
    wasmUrl = new URL("./clausters_web_bg.wasm", import.meta.url),
    workletUrl = new URL("./worklet.js", import.meta.url),
    nrtWorkerUrl = new URL("./nrt-worker.js", import.meta.url),
    maxStreamBuses,
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
            maxStreamBuses,
        },
    });
    node.connect(ctx.destination);

    // The thread that is neither audio nor interface. It reads the page's own
    // filesystem and decodes -- work a native server gives its NRT thread and
    // this engine had nowhere to put, so it landed on the thread that owes the
    // next quantum. It is optional: without it every job runs in the worklet,
    // exactly as before, and only a page that reads a soundfile ever needs it.
    const worker = await startNrtWorker(nrtWorkerUrl, node);

    const handle: ClaustersEngine = {
        context: ctx,
        node,
        onReply: null,
        onQuit: null,
        onError: null,
        resume: () => ctx.resume(),
        suspend: () => ctx.suspend(),
        dispose: () => worker?.terminate(),
        send(bytes: Uint8Array, peer: number) {
            node.port.postMessage({ type: "osc", data: bytes, peer }, [
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
        if (msg.type === "osc") handle.onReply?.(new Uint8Array(msg.data), msg.peer);
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

/**
 * Starts the NRT worker and gives the worklet a channel straight to it.
 *
 * **Why a handshake.** The channel is a `MessagePort` transferred *into* an
 * AudioWorklet, which the HTML standard allows (a `MessagePort` is transferable
 * and is exposed to the AudioWorklet scope) but which no documentation settles
 * for every engine — WebKit has a history of transferables into worklets, and
 * nothing here can drive Safari to find out. So the Worker is asked to answer
 * over the port before anything relies on it: if the answer comes, the engine
 * starts delegating; if it does not, the page keeps the engine exactly as it
 * was, doing the work itself. A slower tab is a limit; a tab whose soundfile
 * reads silently never complete is a defect.
 *
 * Returns the Worker so the engine can end it, or `null` when the browser has
 * no Worker at all.
 */
async function startNrtWorker(
    url: URL | string,
    node: AudioWorkletNode,
): Promise<Worker | null> {
    if (typeof Worker === "undefined") return null;
    let worker: Worker;
    try {
        worker = new Worker(url, { type: "module" });
    } catch {
        return null;
    }
    const channel = new MessageChannel();
    const ready = new Promise<boolean>((resolve) => {
        const done = (ok: boolean) => {
            clearTimeout(timer);
            channel.port1.onmessage = null;
            resolve(ok);
        };
        const timer = setTimeout(() => done(false), NRT_HANDSHAKE_MS);
        channel.port1.onmessage = (e: MessageEvent) => {
            done((e.data as { type?: string }).type === "ready");
        };
    });
    // port2 to the Worker, port1 stays here for the handshake; once it answers,
    // port1 goes to the worklet, which is the leg that has to work.
    worker.postMessage({ type: "port", port: channel.port2 }, [channel.port2]);
    channel.port1.postMessage({ type: "ping" });
    channel.port1.start();
    if (!(await ready)) {
        worker.terminate();
        return null;
    }
    const toWorklet = new MessageChannel();
    worker.postMessage({ type: "port", port: toWorklet.port2 }, [toWorklet.port2]);
    try {
        node.port.postMessage({ type: "nrt-port", port: toWorklet.port1 }, [
            toWorklet.port1,
        ]);
    } catch {
        // The transfer into the worklet is the one leg no documentation
        // settles. Refused: keep the engine as it was.
        worker.terminate();
        return null;
    }
    return worker;
}

/** How long the NRT worker has to answer before the page gives up on it. */
const NRT_HANDSHAKE_MS = 3000;
