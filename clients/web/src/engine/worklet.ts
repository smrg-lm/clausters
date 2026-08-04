// The AudioWorklet processor: the engine lives here, in pulled mode.
//
// Topology (docs/decisions.md): one wasm instance — OSC translate + engine —
// inside the AudioWorkletGlobalScope. The main thread compiles the
// WebAssembly.Module (async, off this thread) and passes it through
// processorOptions; the constructor instantiates it *synchronously* with
// initSync, so the processor is live from its first quantum. OSC bytes travel
// over the MessagePort both ways; commands cross into the engine through the
// in-memory ring (inside WebServer). No SharedArrayBuffer, no COOP/COEP.
//
// Each 128-frame render quantum is one WebServer.process call — a serving
// turn (ring, streams, garbage, async results) before each 64-frame engine
// block, so command pacing stays fine and deterministic (see
// tests/headless.rs, which drives the same path natively).

import "./worklet-shim.ts";
import { initSync, WebServer } from "./clausters_web.js";

// Port protocol, both directions tagged by `type`:
//   main -> worklet: {type:"osc", data, peer}  one complete OSC packet (bytes),
//                        tagged with which of the page's clients authored it
//                    {type:"clock"}       ask for the sample clock
//                    {type:"buffer_load", index, channels, sampleRate, data}
//                        install host-decoded samples as buffer `index` (the
//                        browser's /buffer_allocRead: fetch + decodeAudioData on
//                        the main thread, interleaved floats in here)
//   worklet -> main: {type:"osc", data, peer}  one reply packet (bytes) and
//                        which client it is for, so the page routes it
//                    {type:"clock", clock, frame, epoch}
//                    {type:"buffer_load", index, ok, message?}  the install's ack
//                    {type:"quit"}        a /server_quit arrived; processor stops
//                    {type:"error", message}  fatal; processor stops
type InMessage =
    | { type: "osc"; data: ArrayBuffer; peer: number }
    | { type: "clock" }
    | {
          type: "buffer_load";
          index: number;
          channels: number;
          sampleRate: number;
          data: ArrayBuffer;
      };

interface ProcessorOptions {
    module: WebAssembly.Module;
    channels: number;
    unixEpoch: number;
}

class ClaustersProcessor extends AudioWorkletProcessor {
    channels: number;
    epoch: number;
    server: WebServer;
    interleaved: Float32Array;
    // packets awaiting ring space (backpressure), each with its author's tag
    pending: { peer: number; bytes: Uint8Array }[];
    dead: boolean;

    constructor(options: { processorOptions: ProcessorOptions }) {
        super();
        const { module, channels, unixEpoch } = options.processorOptions;
        initSync({ module });
        this.channels = channels;
        this.epoch = unixEpoch;
        // sampleRate is the AudioWorkletGlobalScope global: the context rate.
        this.server = new WebServer(sampleRate, channels, unixEpoch);
        this.interleaved = new Float32Array(128 * channels);
        this.pending = [];
        this.dead = false;
        this.port.onmessage = (e) => this.onMessage(e.data as InMessage);
    }

    onMessage(msg: InMessage): void {
        if (msg.type === "osc") {
            this.pending.push({ peer: msg.peer, bytes: new Uint8Array(msg.data) });
        } else if (msg.type === "clock") {
            // `frame` is the context's own frame counter read in the same
            // instant as the engine's: their difference is a fixed integer, so
            // a client can map `AudioContext.currentTime` to the engine's
            // sample axis afterwards with no round trip and no drift.
            this.port.postMessage({
                type: "clock",
                clock: this.server.clock(),
                frame: currentFrame,
                epoch: this.epoch,
            });
        } else if (msg.type === "buffer_load") {
            // Runs between quanta on this thread — the same inline install
            // the native headless embed mode performs (see ClaustersHeadless).
            try {
                this.server.buffer_load(
                    msg.index,
                    msg.channels,
                    msg.sampleRate,
                    new Float32Array(msg.data),
                );
                this.port.postMessage({
                    type: "buffer_load",
                    index: msg.index,
                    ok: true,
                });
            } catch (e) {
                this.port.postMessage({
                    type: "buffer_load",
                    index: msg.index,
                    ok: false,
                    message: String(e),
                });
            }
        }
    }

    process(_inputs: Float32Array[][], outputs: Float32Array[][]): boolean {
        if (this.dead) return false;
        try {
            // Feed the ring in arrival order; stop at the first refusal so
            // ordering survives backpressure (retry next quantum).
            while (
                this.pending.length &&
                this.server.send(this.pending[0]!.peer, this.pending[0]!.bytes)
            ) {
                this.pending.shift();
            }

            const out = outputs[0] ?? [];
            const frames = out[0] ? out[0].length : 128;
            const need = frames * this.channels;
            if (this.interleaved.length !== need) {
                this.interleaved = new Float32Array(need);
            }
            this.server.process(this.interleaved);
            for (let ch = 0; ch < out.length; ch++) {
                const dst = out[ch]!;
                if (ch >= this.channels) {
                    dst.fill(0);
                    continue;
                }
                for (let f = 0; f < frames; f++) {
                    dst[f] = this.interleaved[f * this.channels + ch]!;
                }
            }

            // poll() returns [peer u32 LE, ...packet] in one allocation; the
            // packet travels on as its own view so the main thread transfers a
            // buffer rather than copying it.
            let reply: Uint8Array | undefined;
            while ((reply = this.server.poll()) !== undefined) {
                const peer = new DataView(
                    reply.buffer,
                    reply.byteOffset,
                    4,
                ).getUint32(0, true);
                const data = reply.slice(4);
                this.port.postMessage({ type: "osc", data, peer }, [data.buffer]);
            }

            if (this.server.quit_requested()) {
                this.dead = true;
                this.port.postMessage({ type: "quit" });
                return false;
            }
        } catch (e) {
            this.dead = true;
            this.port.postMessage({ type: "error", message: String(e) });
            return false;
        }
        return true;
    }
}

registerProcessor("clausters", ClaustersProcessor);
