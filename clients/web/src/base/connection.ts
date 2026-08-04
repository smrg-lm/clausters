// The carrier seam: one connection interface, two carriers.
//
// Everything the client builds (defs, sequencing, the GUI driver) sits above
// `Connection` and never names a transport — the same rule the Python client
// keeps ("only a Server object knows the connection"). The two carriers:
//
// - `WsConnection` — a browser `WebSocket` to a `--ws` clausters server
//   (default port 57120): the remote/native-server carrier, one OSC packet
//   per binary frame (the server's WS wire format). Also works under node,
//   whose global `WebSocket` speaks the same standard API.
// - `pageConnection()` — the in-page engine: the audio server compiled to
//   wasm in this page's AudioWorklet, reached through the per-page
//   `server()` singleton. No process, no socket.
//
// Beside the two carriers there is a third thing shaped like one and going
// nowhere: `ScoreConnection`, which **writes time instead of waiting for it**.
// It accumulates what a `Server` would have sent as a timestamped score for an
// offline render, which is why it declares a `timeMode` — a live carrier
// stamps a bundle with the wall clock, a score with seconds from the render's
// start. The Python client's `OscNrtInterface`, at this client's seam.

import { encodeScoreBundle, oscArg } from "./osc.ts";
import type { BundleMessage, MsgArg, OscArg } from "./osc.ts";
import { server } from "../engine/server.ts";
import type { ClaustersServer } from "../engine/server.ts";

/**
 * A synchronous view of a server's sample counter — what a sample-locked
 * clock paces against and a `/sched_at` target is computed from.
 */
export interface SampleClock {
    /** The server's current sample, read with no round trip. */
    sample(): number;
    /** The rate that counter advances at. */
    sampleRate: number;
}

/** A duplex OSC byte pipe to an audio server. */
export interface Connection {
    /**
     * What a time on this carrier *means*, and so how a `Server` stamps what
     * it emits: `"unix"` (absent, the default) is the wall clock a live
     * server reads as an NTP timetag; `"score"` is seconds from the start of
     * an offline render. The one thing above the seam that varies with the
     * carrier, and it varies because time itself does.
     */
    readonly timeMode?: "unix" | "score";
    /**
     * Where this carrier goes, when it goes anywhere addressable — a socket's
     * URL. The in-page engine and a score have none, and the receiving door
     * reads them as `"page"`: it is what a responder filtering by sender
     * (`OscFunc`'s `src`) compares, a browser having no `(host, port)` to
     * offer.
     */
    readonly url?: string;
    /**
     * Accumulates a bundle at `secs` from the render's start — a score
     * carrier's structured entry point, and the reason a score never has to
     * decode bytes to learn when they were meant to happen. Live carriers
     * leave it out.
     */
    addBundle?(secs: number, messages: readonly BundleMessage[]): void;
    /** Sends one complete OSC packet. */
    send(packet: Uint8Array): void;
    /** Subscribes to every reply packet. */
    addReply(listener: (packet: Uint8Array) => void): void;
    removeReply(listener: (packet: Uint8Array) => void): void;
    /** Releases the carrier (never stops the shared in-page engine). */
    close(): void;
    /**
     * The server's sample clock, where the carrier *shares* one with it —
     * the in-page engine runs in this page's `AudioContext`, so its counter
     * is readable synchronously and exactly. A socket has no such thing and
     * leaves this out; `Server.sampleTimebase()` then anchors over `/clock_query`
     * instead.
     */
    sampleClock?(): Promise<SampleClock>;
    /**
     * Installs decoded samples straight into a server buffer, where the
     * carrier *shares* memory with the server — the in-page engine takes a
     * whole file in one call, no `/buffer_getRange.reply` chunking and no OSC envelope per
     * sample. A socket has no such thing and leaves this out; `Buffer.load`
     * then writes the chunks instead. `samples` are interleaved.
     */
    bulkLoad?(
        bufnum: number,
        channels: number,
        sampleRate: number,
        samples: Float32Array,
    ): Promise<void>;
}

export class WsConnection implements Connection {
    private socket: WebSocket;
    private listeners = new Set<(packet: Uint8Array) => void>();

    /** The socket's URL — what a receiver reports as this carrier's `src`. */
    get url(): string {
        return this.socket.url;
    }

    private constructor(socket: WebSocket) {
        this.socket = socket;
        socket.addEventListener("message", (event: MessageEvent) => {
            const dispatch = (bytes: Uint8Array) => {
                for (const listener of [...this.listeners]) listener(bytes);
            };
            if (event.data instanceof ArrayBuffer) {
                dispatch(new Uint8Array(event.data));
            } else if (typeof Blob !== "undefined" && event.data instanceof Blob) {
                event.data.arrayBuffer().then((b) => dispatch(new Uint8Array(b)));
            }
        });
    }

    /**
     * Opens a WebSocket to `url` (e.g. `ws://127.0.0.1:57120`), resolving
     * once the handshake completes (sends never race the handshake).
     */
    static open(url: string): Promise<WsConnection> {
        return new Promise((resolve, reject) => {
            const socket = new WebSocket(url);
            socket.binaryType = "arraybuffer";
            socket.addEventListener("open", () =>
                resolve(new WsConnection(socket)));
            socket.addEventListener("error", () =>
                reject(new Error(`cannot open ${url}`)));
        });
    }

    send(packet: Uint8Array): void {
        // Our packets always sit on a plain ArrayBuffer; the cast bridges
        // TS's `ArrayBufferLike` typed-array generics to WebSocket's input.
        this.socket.send(packet as Uint8Array<ArrayBuffer>);
    }

    addReply(listener: (packet: Uint8Array) => void): void {
        this.listeners.add(listener);
    }

    removeReply(listener: (packet: Uint8Array) => void): void {
        this.listeners.delete(listener);
    }

    close(): void {
        this.socket.close();
    }
}

/**
 * The in-page carrier: a `Connection` over an engine in this tab.
 *
 * Defaults to the page's shared engine, which is what a page wants — its
 * components play into one mix. Pass one built by `engine()` to carry a client
 * that must not share a node, bus and buffer space with the rest of the
 * document; several such clients in one page is the case this exists for.
 *
 * Closing detaches this connection's listeners; the engine keeps running (it
 * is the page's, or its owner's — not this connection's to stop).
 */
export async function pageConnection(
    target?: Promise<ClaustersServer> | ClaustersServer,
): Promise<Connection> {
    const engine = await (target ?? server());
    const mine = new Set<(packet: Uint8Array) => void>();
    return {
        send: (packet) => engine.send(packet),
        addReply: (listener) => {
            mine.add(listener);
            engine.addReply(listener);
        },
        removeReply: (listener) => {
            mine.delete(listener);
            engine.removeReply(listener);
        },
        close: () => {
            for (const listener of mine) engine.removeReply(listener);
            mine.clear();
        },
        bulkLoad: async (bufnum, channels, sampleRate, samples) => {
            await engine.bufferLoad(bufnum, channels, sampleRate, samples);
        },
        // One round trip pairs the engine's counter with the context's frame
        // counter; their difference is a fixed integer (the engine advances
        // one quantum per render quantum of this very context), so from here
        // the counter is `currentTime` read synchronously — exact, and drift
        // is not a thing between a clock and itself.
        sampleClock: async () => {
            const anchor = await engine.clockAnchor();
            const { sampleRate } = engine.context;
            const offset = anchor.frame - anchor.sample;
            return {
                sampleRate,
                sample: () =>
                    Math.round(engine.context.currentTime * sampleRate) - offset,
            };
        },
    };
}

// ---- the score: a carrier that writes time instead of waiting for it ----

/**
 * Accumulated NRT bundles, ordered by time, serialized to the binary score
 * (`[i32 len][packet]…`) the offline renderer consumes — the Python client's
 * `OscScore`.
 */
export class Score {
    private readonly bundles: { at: number; packet: Uint8Array }[] = [];

    /** How many bundles are in the score. */
    get length(): number {
        return this.bundles.length;
    }

    /** Adds one already-encoded bundle, stamped at `at` seconds. */
    add(at: number, packet: Uint8Array): void {
        this.bundles.push({ at, packet });
    }

    /** Drops everything accumulated so far. */
    clear(): void {
        this.bundles.length = 0;
    }

    /**
     * The binary score: every bundle in time order, each framed by its
     * big-endian `i32` byte count. Sorting is stable, so two bundles at the
     * same instant keep the order they were emitted in — which is what makes
     * a def and the synth that names it land in the right sequence.
     */
    bytes(): Uint8Array {
        const ordered = [...this.bundles].sort((a, b) => a.at - b.at);
        const total = ordered.reduce((n, b) => n + 4 + b.packet.length, 0);
        const out = new Uint8Array(total);
        const view = new DataView(out.buffer);
        let offset = 0;
        for (const { packet } of ordered) {
            view.setInt32(offset, packet.length, false);
            out.set(packet, offset + 4);
            offset += 4 + packet.length;
        }
        return out;
    }
}

/**
 * The offline carrier: instead of sending, it accumulates a timestamped
 * `Score` an offline render consumes (`Session.nrt()` builds one; `render()`
 * drains it).
 *
 * There is no server at the other end and nothing ever replies, which is the
 * whole point — an offline piece is written by the same code that plays a live
 * one, and the difference is which carrier the `Server` was opened over.
 */
export class ScoreConnection implements Connection {
    readonly timeMode = "score";
    readonly score = new Score();

    addBundle(secs: number, messages: readonly BundleMessage[]): void {
        this.score.add(secs, encodeScoreBundle(secs, messages));
    }

    /** A message alone has no time, so it lands at the top of the score. */
    sendMsg(addr: string, args: readonly MsgArg[] = []): void {
        this.addBundle(0, [{ addr, args: args.map(oscArg) as OscArg[] }]);
    }

    /**
     * The byte door, for anything that reaches past the structured one: the
     * packet is wrapped in a score bundle at time 0. Nothing in the client
     * takes this path — `Server` branches on `timeMode` first — and it exists
     * so a `ScoreConnection` is a `Connection` in full rather than one with a
     * hole in it.
     */
    send(packet: Uint8Array): void {
        // The 16-byte `#bundle` header at time 0, built by the core rather
        // than spelled out here: a timetag is a time, and this package
        // computes none of those itself.
        const header = encodeScoreBundle(0, []);
        const framed = new Uint8Array(header.length + 4 + packet.length);
        framed.set(header, 0);
        new DataView(framed.buffer).setInt32(header.length, packet.length, false);
        framed.set(packet, header.length + 4);
        this.score.add(0, framed);
    }

    /** Nothing ever replies to a score. */
    addReply(): void {}
    removeReply(): void {}
    close(): void {
        this.score.clear();
    }
}
