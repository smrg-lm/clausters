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

import "./worklet-shim.js";
import { initSync, WebServer } from "./clausters_web.js";

// Port protocol, both directions tagged by `type`:
//   main -> worklet: {type:"osc", data}   one complete OSC packet (bytes)
//                    {type:"clock"}       ask for the sample clock
//   worklet -> main: {type:"osc", data}   one reply packet (bytes)
//                    {type:"clock", clock, epoch}
//                    {type:"quit"}        a /quit arrived; processor stops
//                    {type:"error", message}  fatal; processor stops
class ClaustersProcessor extends AudioWorkletProcessor {
    constructor(options) {
        super();
        const { module, channels, unixEpoch } = options.processorOptions;
        initSync({ module });
        this.channels = channels;
        this.epoch = unixEpoch;
        // sampleRate is the AudioWorkletGlobalScope global: the context rate.
        this.server = new WebServer(sampleRate, channels, unixEpoch);
        this.interleaved = new Float32Array(128 * channels);
        this.pending = []; // packets awaiting ring space (backpressure)
        this.dead = false;
        this.port.onmessage = (e) => this.onMessage(e.data);
    }

    onMessage(msg) {
        if (msg.type === "osc") {
            this.pending.push(new Uint8Array(msg.data));
        } else if (msg.type === "clock") {
            this.port.postMessage({
                type: "clock",
                clock: this.server.clock(),
                epoch: this.epoch,
            });
        }
    }

    process(_inputs, outputs) {
        if (this.dead) return false;
        try {
            // Feed the ring in arrival order; stop at the first refusal so
            // ordering survives backpressure (retry next quantum).
            while (this.pending.length && this.server.send(this.pending[0])) {
                this.pending.shift();
            }

            const out = outputs[0];
            const frames = out[0] ? out[0].length : 128;
            const need = frames * this.channels;
            if (this.interleaved.length !== need) {
                this.interleaved = new Float32Array(need);
            }
            this.server.process(this.interleaved);
            for (let ch = 0; ch < out.length; ch++) {
                const dst = out[ch];
                if (ch >= this.channels) { dst.fill(0); continue; }
                for (let f = 0; f < frames; f++) {
                    dst[f] = this.interleaved[f * this.channels + ch];
                }
            }

            let reply;
            while ((reply = this.server.poll()) !== undefined) {
                this.port.postMessage({ type: "osc", data: reply }, [reply.buffer]);
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
