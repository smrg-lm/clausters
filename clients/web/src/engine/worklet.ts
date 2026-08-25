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
//                    {type:"nrt-port", port}   the NRT worker's channel; jobs the
//                        engine cannot do itself (reading a soundfile: no
//                        filesystem here) leave through it
//   worklet -> main: {type:"osc", data, peer}  one reply packet (bytes) and
//                        which client it is for, so the page routes it
//                    {type:"clock", clock, frame, epoch}
//                    {type:"buffer_load", index, ok, message?}  the install's ack
//                    {type:"quit"}        a /server_quit arrived; processor stops
//                    {type:"error", message}  fatal; processor stops
type InMessage =
    | { type: "osc"; data: ArrayBuffer; peer: number }
    | { type: "clock" }
    | { type: "nrt-port"; port: MessagePort }
    | {
          type: "buffer_load";
          index: number;
          channels: number;
          sampleRate: number;
          data: ArrayBuffer;
      };

/** What the NRT worker sends back. */
type NrtResult =
    | {
          ticket: number;
          samples: Float32Array;
          channels: number;
          frames: number;
          sampleRate: number;
      }
    | { ticket: number; error: string };

interface ProcessorOptions {
    module: WebAssembly.Module;
    channels: number;
    unixEpoch: number;
    /** The page's `/bus_stream` ceiling; the engine's default stands when unset. */
    maxStreamBuses?: number;
}

class ClaustersProcessor extends AudioWorkletProcessor {
    channels: number;
    epoch: number;
    server: WebServer;
    interleaved: Float32Array;
    // packets awaiting ring space (backpressure), each with its author's tag
    pending: { peer: number; bytes: Uint8Array }[];
    dead: boolean;
    /** The NRT worker's channel, once the page hands one over. */
    nrt: MessagePort | null;
    /** Jobs out with the Worker: ticket -> the buffer the result installs to. */
    outstanding: Map<number, number>;

    constructor(options: { processorOptions: ProcessorOptions }) {
        super();
        const { module, channels, unixEpoch, maxStreamBuses } = options.processorOptions;
        initSync({ module });
        this.channels = channels;
        this.epoch = unixEpoch;
        // sampleRate is the AudioWorkletGlobalScope global: the context rate.
        this.server = new WebServer(sampleRate, channels, unixEpoch);
        // Before anything subscribes: the ceiling is read when a `/bus_stream`
        // arrives, and the page's first canvas may open in the same turn as
        // the boot.
        if (maxStreamBuses !== undefined) this.server.set_max_stream_buses(maxStreamBuses);
        this.interleaved = new Float32Array(128 * channels);
        this.pending = [];
        this.dead = false;
        this.nrt = null;
        this.outstanding = new Map();
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
        } else if (msg.type === "nrt-port") {
            // The page found a Worker to do the work this thread must not: the
            // engine starts handing those jobs out from here on. The channel
            // reaches the Worker directly, so a result does not queue behind
            // the main thread's frames.
            this.nrt = msg.port;
            this.nrt.onmessage = (e: MessageEvent) => this.onNrtResult(e.data as NrtResult);
            this.nrt.start();
            this.server.delegateJobs();
        } else if (msg.type === "buffer_load") {
            // Runs between quanta on this thread -- the thread that owes the
            // next one. So a take is copied in **runs** rather than in one
            // call: measured natively, a whole five-minute stereo take is some
            // fourteen times a quantum's budget, and no count of jobs divides
            // one call. `begin` allocates, each `chunk` costs what it copies,
            // `end` is a pointer swap; nothing is readable under this index
            // until then, so the engine never sees a half-written take.
            try {
                const samples = new Float32Array(msg.data);
                const frames = samples.length / msg.channels;
                const ticket = this.server.bufferLoadBegin(
                    msg.index,
                    msg.channels,
                    msg.sampleRate,
                    frames,
                );
                try {
                    const run = this.server.installFrames() * msg.channels;
                    for (let at = 0; at < samples.length; at += run) {
                        this.server.bufferLoadChunk(
                            ticket,
                            at,
                            samples.subarray(at, Math.min(at + run, samples.length)),
                        );
                    }
                    this.server.bufferLoadEnd(ticket);
                } catch (e) {
                    this.server.bufferLoadCancel(ticket);
                    throw e;
                }
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

    /**
     * Hands the engine's next job to the Worker, if there is one waiting. At
     * most one is ever out — the buffer queue waits on it, which is what keeps
     * `/buffer_*` completing in submission order the way a native server's
     * single NRT thread does.
     */
    pumpDelegated(): void {
        if (this.nrt === null) return;
        const json = this.server.takeDelegated();
        if (json === undefined) return;
        const job = JSON.parse(json) as {
            ticket: number;
            index: number;
            kind: string;
            path: string;
            fileStart: number;
            numFrames: number;
            channels: number[];
        };
        this.outstanding.set(job.ticket, job.index);
        this.nrt.postMessage({
            type: "read",
            ticket: job.ticket,
            path: job.path,
            fileStart: job.fileStart,
            numFrames: job.numFrames,
            channels: job.channels,
        });
    }

    /**
     * The Worker answered. The samples are installed here — they have to be,
     * the buffer pool being this module's memory — but in runs, under the same
     * ceiling every install pays, and only then is the command answered.
     */
    onNrtResult(result: NrtResult): void {
        const index = this.outstanding.get(result.ticket);
        if (index === undefined) return;
        this.outstanding.delete(result.ticket);
        if ("error" in result) {
            this.server.finishDelegated(result.ticket, result.error);
            return;
        }
        try {
            const ticket = this.server.bufferLoadBegin(
                index,
                result.channels,
                result.sampleRate,
                result.frames,
            );
            try {
                const run = this.server.installFrames() * result.channels;
                const samples = result.samples;
                for (let at = 0; at < samples.length; at += run) {
                    this.server.bufferLoadChunk(
                        ticket,
                        at,
                        samples.subarray(at, Math.min(at + run, samples.length)),
                    );
                }
                this.server.bufferLoadEnd(ticket);
            } catch (e) {
                this.server.bufferLoadCancel(ticket);
                throw e;
            }
            this.server.finishDelegated(result.ticket, undefined);
        } catch (e) {
            this.server.finishDelegated(result.ticket, String(e));
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
            // After the block, not before: a job leaving costs a JSON print and
            // a postMessage, and it is owed to the *next* quantum rather than
            // to this one.
            this.pumpDelegated();
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
