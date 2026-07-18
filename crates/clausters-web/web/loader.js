// The main-thread loader: boots the engine inside an AudioWorklet.
//
// Compiles the wasm module here (async, streaming), registers the processor
// module, and hands the compiled WebAssembly.Module to the worklet through
// processorOptions — the worklet instantiates it synchronously in its
// constructor, so no async work ever happens on the audio thread.
//
// The returned handle speaks raw OSC bytes (send / onReply); the page brings
// its own codec (osc.js here). resume() is the autoplay-policy hook: call it
// from a user gesture. B4 will grow this into the per-page singleton of the
// npm package; here it stays a plain function the harness pages share.

export async function bootClausters({
    context = null,
    channels = 2,
    wasmUrl = new URL("./clausters_web_bg.wasm", import.meta.url),
    workletUrl = new URL("./worklet.js", import.meta.url),
} = {}) {
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

    const handle = {
        context: ctx,
        node,
        onReply: null,          // (Uint8Array) => void
        onQuit: null,           // () => void
        onError: null,          // (message) => void
        resume: () => ctx.resume(),
        suspend: () => ctx.suspend(),
        // One complete OSC packet; the bytes are transferred, not copied.
        send(bytes) {
            node.port.postMessage({ type: "osc", data: bytes }, [bytes.buffer]);
        },
        // The engine's sample clock (a round trip through the audio thread).
        clock() {
            return new Promise((resolve) => {
                clockWaiters.push(resolve);
                node.port.postMessage({ type: "clock" });
            });
        },
    };

    const clockWaiters = [];
    node.port.onmessage = (e) => {
        const msg = e.data;
        if (msg.type === "osc") handle.onReply?.(new Uint8Array(msg.data));
        else if (msg.type === "clock") clockWaiters.shift()?.(msg.clock);
        else if (msg.type === "quit") handle.onQuit?.();
        else if (msg.type === "error") handle.onError?.(msg.message);
    };

    return handle;
}
