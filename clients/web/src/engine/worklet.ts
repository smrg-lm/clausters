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
import { linkFaustModule } from "./faust-link.ts";
import type { EngineExports } from "./faust-link.ts";

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

/** A finished soundfile read (`/buffer_allocRead`). */
type ReadResult =
    | {
          type: "read";
          ticket: number;
          samples: Float32Array;
          channels: number;
          frames: number;
          sampleRate: number;
      }
    | { type: "read"; ticket: number; error: string };

/** A streamed file's shape, learned once. */
type ShapeResult =
    | { type: "shape"; ticket: number; channels: number; sampleRate: number; frames: number }
    | { type: "shape"; ticket: number; error: string };

/** One span of a file being streamed by a `DiskIn`. */
type SpanResult =
    | { type: "span"; ticket: number; samples: Float32Array; frames: number }
    | { type: "span"; ticket: number; error: string };

/** A compiled Faust def, ready to link into the engine. */
type FaustResult =
    | { type: "faust"; ticket: number; bytes: ArrayBuffer; json: string }
    | { type: "faust"; ticket: number; error: string };

/** The acknowledgement of a `DiskOut` flush. */
type RecordResult = { type: "record"; ticket: number; error?: string };

/** What the NRT worker sends back. */
type NrtResult =
    | {
          type: "read";
          ticket: number;
          samples: Float32Array;
          channels: number;
          frames: number;
          sampleRate: number;
      }
    | { type: "read"; ticket: number; error: string }
    | { type: "span"; ticket: number; samples: Float32Array; frames: number }
    | { type: "span"; ticket: number; error: string }
    | { type: "record"; ticket: number; error?: string }
    | { type: string; ticket: number; [k: string]: unknown };

/** The unit a disk stream moves in: a tenth of a second at 48 kHz. Smaller
 *  means more messages for the same audio; larger means a longer wait before a
 *  stream that just opened has anything to play. */
const DISK_SPAN_FRAMES = 4800;

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
    /** `DiskIn` streams: where each one has read to, and what it could not
     *  hand over yet. */
    spans: Map<
        number,
        { frame: number; held: Float32Array | null; waiting: boolean; ended: boolean }
    >;
    /** `DiskOut` streams: where each one is being written. */
    recording: Map<number, { path: string; format: string; waiting: boolean }>;
    /** The engine instance's own wasm exports: its memory and its table. */
    engine: EngineExports;
    /** Faust compilations out with the Worker. The value is the def's name,
     *  kept for the message a failure reports. */
    compiling: Map<number, string>;
    /** Linked Faust modules, kept alive for the page's life: a def's `compute`
     *  is a slot of the engine's table and the instance owns the function. */
    linked: WebAssembly.Instance[];

    constructor(options: { processorOptions: ProcessorOptions }) {
        super();
        const { module, channels, unixEpoch, maxStreamBuses } = options.processorOptions;
        // `initSync` hands back the instance's own exports. Two of them are
        // not the binding's business and are kept anyway: the linear memory
        // and the indirect function table, which is where a Faust module's
        // `compute` has to land for the engine to call it.
        this.engine = initSync({ module }) as unknown as EngineExports;
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
        this.spans = new Map();
        this.recording = new Map();
        this.compiling = new Map();
        this.linked = [];
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
            this.nrt.onmessage = (e: MessageEvent) => {
                const result = e.data as NrtResult;
                if (result.type === "shape") this.onShape(result as ShapeResult);
                else if (result.type === "span") this.onSpan(result as SpanResult);
                else if (result.type === "record") this.onRecorded(result as RecordResult);
                else if (result.type === "faust") this.onFaustResult(result as FaustResult);
                else this.onNrtResult(result as ReadResult);
            };
            // A message that cannot be deserialized here arrives as
            // `messageerror`, not as an error anywhere: without this a dropped
            // result is an engine that simply never answers.
            this.nrt.onmessageerror = () =>
                this.port.postMessage({
                    type: "error",
                    message: "a message from the NRT worker could not be deserialized",
                });
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
    onNrtResult(result: ReadResult): void {
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

    /**
     * Hands the Worker every Faust def waiting to be compiled.
     *
     * Natively `/def_send faust` goes to a compiler thread and answers late.
     * The page's compiler thread is the Worker, so this is the same hand-off:
     * unlike a soundfile read there is no ordering to keep, so all of them go
     * at once and each answers on its own.
     */
    pumpFaust(): void {
        if (this.nrt === null) return;
        const jobs = JSON.parse(this.server.takeFaustJobs()) as {
            ticket: number;
            name: string;
            kind: string;
            def: string;
        }[];
        for (const job of jobs) {
            this.compiling.set(job.ticket, job.name);
            this.nrt.postMessage({
                type: "faust",
                ticket: job.ticket,
                name: job.name,
                kind: job.kind,
                def: job.def,
            });
        }
    }

    /**
     * Links a compiled def into the engine and answers the command that asked
     * for it. The linking itself is `faust-link.ts`, shared with the offline
     * renderer, which links into an engine instance of its own.
     */
    onFaustResult(result: FaustResult): void {
        const name = this.compiling.get(result.ticket);
        if (name === undefined) return;
        this.compiling.delete(result.ticket);
        if ("error" in result) {
            this.server.finishFaust(result.ticket, 0, 0, undefined, result.error);
            return;
        }
        try {
            const { instance, compute, init } = linkFaustModule(this.engine, result.bytes);
            // Kept for the page's life: the table holds the functions, and the
            // instance is what owns them.
            this.linked.push(instance);
            this.server.finishFaust(result.ticket, compute, init, result.json, undefined);
        } catch (e) {
            this.server.finishFaust(result.ticket, 0, 0, undefined, `${name}: ${String(e)}`);
        }
    }

    /**
     * Keeps every open disk stream fed or drained.
     *
     * Natively each `DiskIn` has a thread of its own racing the audio thread
     * through a ring. Here the ring is the same and the reader is the Worker,
     * so this is the leg between them: ask what is hungry, ask the Worker for
     * the next span, and hand back what the last answer brought. **How far
     * ahead it reads is the design** -- without shared memory a span has to
     * travel, and an underrun is silence exactly as a slow disk would give.
     *
     * At most one span per stream is in flight, so a slow answer costs latency
     * and never order.
     */
    pumpDisk(): void {
        if (this.nrt === null) return;
        const streams = JSON.parse(this.server.diskPoll()) as {
            id: number;
            direction: "in" | "out";
            path: string;
            channels: number;
            looping: boolean;
            format: string;
            samples: number;
        }[];
        const live = new Set<number>();
        for (const s of streams) {
            live.add(s.id);
            if (s.direction === "in") this.feed(s);
            else this.drain(s);
        }
        // A synth that was freed took its stream with it; close its recording.
        for (const [id, take] of this.recording) {
            if (live.has(id)) continue;
            this.recording.delete(id);
            this.nrt.postMessage({
                type: "record",
                ticket: id,
                path: take.path,
                samples: new Float32Array(0),
                channels: 1,
                sampleRate,
                format: take.format,
                final: true,
            });
        }
        for (const id of [...this.spans.keys()]) if (!live.has(id)) this.spans.delete(id);
    }

    /** One `DiskIn`: hand it what arrived, then ask for more if there is room. */
    private feed(s: {
        id: number;
        path: string;
        channels: number;
        looping: boolean;
        samples: number;
    }): void {
        const state = this.spans.get(s.id) ?? { frame: 0, held: null, waiting: false, ended: false };
        this.spans.set(s.id, state);
        // A stream is born not knowing its file's shape, and plays silence
        // until it does. Asking is the first thing owed it.
        if (s.channels === 0) {
            if (!state.waiting) {
                state.waiting = true;
                this.nrt!.postMessage({ type: "shape", ticket: s.id, path: s.path });
            }
            return;
        }
        if (state.held !== null) {
            const took = this.server.diskPush(s.id, state.held);
            state.held = took >= state.held.length ? null : state.held.subarray(took);
        }
        if (state.held !== null || state.waiting) return;
        if (state.ended && !s.looping) return;
        // Read ahead by whatever the ring can hold: the ring's own size is the
        // lookahead, and it is sized for about a second.
        const frames = Math.floor(s.samples / s.channels);
        if (frames < DISK_SPAN_FRAMES) return;
        state.waiting = true;
        this.nrt!.postMessage({
            type: "span",
            ticket: s.id,
            path: s.path,
            frame: state.frame,
            frames: Math.min(frames, DISK_SPAN_FRAMES * 4),
        });
    }

    /** One `DiskOut`: take what it has recorded and hand it to the Worker. */
    private drain(s: {
        id: number;
        path: string;
        format: string;
        samples: number;
    }): void {
        if (!this.recording.has(s.id)) {
            this.recording.set(s.id, { path: s.path, format: s.format, waiting: false });
        }
        const take = this.recording.get(s.id)!;
        if (take.waiting || s.samples < DISK_SPAN_FRAMES) return;
        const samples = this.server.diskPull(s.id, s.samples);
        if (samples.length === 0) return;
        take.waiting = true;
        this.nrt!.postMessage(
            {
                type: "record",
                ticket: s.id,
                path: s.path,
                samples,
                channels: 1,
                sampleRate,
                format: s.format,
                final: false,
            },
            [samples.buffer],
        );
    }

    onShape(result: { ticket: number; channels?: number; error?: string }): void {
        const state = this.spans.get(result.ticket);
        if (state !== undefined) state.waiting = false;
        if (result.error !== undefined || !result.channels) {
            // Unreadable: the stream stays shapeless and silent, which is what
            // a file a native server could not open already does.
            return;
        }
        this.server.diskShape(result.ticket, result.channels);
    }

    onSpan(result: { ticket: number; samples?: Float32Array; frames?: number; error?: string }): void {
        const state = this.spans.get(result.ticket);
        if (state === undefined) return;
        state.waiting = false;
        if (result.error !== undefined || result.samples === undefined) return;
        if (result.samples.length === 0) {
            // End of file: loop back to the top, or stop asking.
            state.ended = true;
            state.frame = 0;
            return;
        }
        state.frame += result.frames ?? 0;
        const took = this.server.diskPush(result.ticket, result.samples);
        state.held = took >= result.samples.length ? null : result.samples.subarray(took);
    }

    onRecorded(result: { ticket: number; error?: string }): void {
        const take = this.recording.get(result.ticket);
        if (take !== undefined) take.waiting = false;
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
            this.pumpDisk();
            this.pumpFaust();
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
