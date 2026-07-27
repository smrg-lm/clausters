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
//   main -> worklet: {type:"osc", data}   one complete OSC packet (bytes)
//                    {type:"clock"}       ask for the sample clock
//                    {type:"b_load", index, channels, sampleRate, data}
//                        install host-decoded samples as buffer `index` (the
//                        browser's /b_allocRead: fetch + decodeAudioData on
//                        the main thread, interleaved floats in here)
//   worklet -> main: {type:"osc", data}   one reply packet (bytes)
//                    {type:"clock", clock, frame, epoch}
//                    {type:"b_load", index, ok, message?}  the install's ack
//                    {type:"quit"}        a /quit arrived; processor stops
//                    {type:"error", message}  fatal; processor stops
type InMessage =
    | { type: "osc"; data: ArrayBuffer }
    | { type: "clock" }
    | {
          type: "b_load";
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
    pending: Uint8Array[]; // packets awaiting ring space (backpressure)
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
            this.pending.push(new Uint8Array(msg.data));
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
        } else if (msg.type === "b_load") {
            // Runs between quanta on this thread — the same inline install
            // the native headless embed mode performs (see ClaustersHeadless).
            try {
                this.server.b_load(
                    msg.index,
                    msg.channels,
                    msg.sampleRate,
                    new Float32Array(msg.data),
                );
                this.port.postMessage({
                    type: "b_load",
                    index: msg.index,
                    ok: true,
                });
            } catch (e) {
                this.port.postMessage({
                    type: "b_load",
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
                this.server.send(this.pending[0]!)
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

            let reply: Uint8Array | undefined;
            while ((reply = this.server.poll()) !== undefined) {
                this.port.postMessage({ type: "osc", data: reply }, [
                    reply.buffer,
                ]);
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
