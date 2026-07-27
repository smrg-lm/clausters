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

import { server } from "../engine/server.ts";

/// A synchronous view of a server's sample counter — what a sample-locked
/// clock paces against and a `/sched` target is computed from.
export interface SampleClock {
    /// The server's current sample, read with no round trip.
    sample(): number;
    /// The rate that counter advances at.
    sampleRate: number;
}

/// A duplex OSC byte pipe to an audio server.
export interface Connection {
    /// Sends one complete OSC packet.
    send(packet: Uint8Array): void;
    /// Subscribes to every reply packet.
    addReply(listener: (packet: Uint8Array) => void): void;
    removeReply(listener: (packet: Uint8Array) => void): void;
    /// Releases the carrier (never stops the shared in-page engine).
    close(): void;
    /// The server's sample clock, where the carrier *shares* one with it —
    /// the in-page engine runs in this page's `AudioContext`, so its counter
    /// is readable synchronously and exactly. A socket has no such thing and
    /// leaves this out; `Server.sampleTimebase()` then anchors over `/clock`
    /// instead.
    sampleClock?(): Promise<SampleClock>;
}

export class WsConnection implements Connection {
    private socket: WebSocket;
    private listeners = new Set<(packet: Uint8Array) => void>();

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

    /// Opens a WebSocket to `url` (e.g. `ws://127.0.0.1:57120`), resolving
    /// once the handshake completes (sends never race the handshake).
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

/// The in-page carrier: a `Connection` over the per-page engine singleton.
/// Closing detaches this connection's listeners; the engine keeps running
/// (it is shared page state, not this connection's to stop).
export async function pageConnection(): Promise<Connection> {
    const engine = await server();
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
