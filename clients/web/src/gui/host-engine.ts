/**
 * **The applications' engine of a page's GUI host, as the page's engine**:
 * the engine surface (`ClaustersServer`) over the engine the host's
 * applications play on, so a page that runs the applications holds one
 * engine -- what its session, its worker and its applications all play on,
 * as a native host's engine is.
 *
 * A page's main thread may not wait and that engine lives in the host's
 * memory, so its packets go through the host (`GuiBridge.engineSend`) to the
 * application's thread, which serves them, and its replies come back through
 * the host's listener (`GuiBridge.onEngine`) by client tag. A client in a
 * worker gets a port that speaks what the page's own engine's ports speak
 * (`engine/worklet.ts`): packets, a clock read, samples to install. The
 * clock anchor is read in the applications' worklet, in the same instant as
 * the context's frame counter (`apps-worklet.ts`).
 *
 * @module
 */

import { loadCore } from "../base/core.ts";
import { encodeMessage } from "../base/osc.ts";
import type { ClaustersEngine, ClockAnchor } from "../engine/loader.ts";
import { type ClaustersServer, DEFAULT_PEER, wrap } from "../engine/server.ts";
import { appsOutput } from "./apps.ts";
import type { ClaustersGui } from "./page.ts";

/** What a port of a client in another realm says ({@link ClaustersEngine.lend}). */
type PortSays =
    | { type: "osc"; data: ArrayBuffer }
    | { type: "clock" }
    | { type: "buffer_load"; index: number; channels: number; sampleRate: number; data: ArrayBuffer }
    | { type: "close" };

/**
 * **The share of the applications' engine's ids a client of it allocates
 * from**: the first half, the applications having the second -- as the
 * native host answers it (`GuiHost.engine`).
 */
export const CLIENT_SHARE = { index: 0, of: 2 } as const;

/** What the host's bridge has of the applications' engine (the applications' build). */
interface EngineBridge {
    engineSend(peer: number, packet: Uint8Array): void;
    engineSamples(token: number, index: number, channels: number, rate: number, samples: Float32Array): void;
    onEngine(listener: (kind: string, id: number, value: unknown) => void): void;
}

/**
 * **The engine `gui`'s applications play on, as an engine of the page**: it
 * opens that engine if the host has not, and answers once its output plays.
 * Rejects for a host of the ordinary build, which runs no applications (a
 * page that is not cross-origin isolated).
 */
export async function hostEngine(gui: Pick<ClaustersGui, "bridge">): Promise<ClaustersServer> {
    const bridge = gui.bridge as unknown as Partial<EngineBridge>;
    if (typeof bridge.engineSend !== "function") {
        throw new Error(
            "this host runs no applications, so it has no engine: a page runs them " +
                "when it is cross-origin isolated",
        );
    }
    const host = bridge as EngineBridge;
    await loadCore();
    // the first packet opens the engine, as an application's first ask does
    host.engineSend(DEFAULT_PEER, encodeMessage("/server_status"));
    const { context, node } = await appsOutput();
    return wrap(rawOverHost(host, context, node));
}

function rawOverHost(host: EngineBridge, context: AudioContext, node: AudioWorkletNode): ClaustersEngine {
    /** The ports of clients in other realms, by tag. */
    const lent = new Map<number, MessagePort>();
    /** The installs waiting for their answer, by token: where it goes. */
    const installs = new Map<number, (why: string | null) => void>();
    let nextToken = 1;
    const anchors: ((anchor: ClockAnchor) => void)[] = [];
    node.port.onmessage = (e: MessageEvent<{ type?: string; clock: number; frame: number; epoch: number }>) => {
        if (e.data?.type !== "clock") return;
        const anchor = { sample: e.data.clock, frame: e.data.frame, epoch: e.data.epoch };
        for (const waiter of anchors.splice(0)) waiter(anchor);
    };
    const clockAnchor = (): Promise<ClockAnchor> =>
        new Promise((resolve) => {
            anchors.push(resolve);
            node.port.postMessage({ type: "clock" });
        });
    const install = (index: number, channels: number, rate: number, samples: Float32Array): Promise<number> =>
        new Promise((resolve, reject) => {
            const token = nextToken++;
            installs.set(token, (why) => (why === null ? resolve(index) : reject(new Error(why))));
            host.engineSamples(token, index, channels, rate, samples);
        });
    const raw: ClaustersEngine = {
        context,
        node,
        onReply: null,
        onQuit: null,
        onError: null,
        resume: () => context.resume(),
        suspend: () => context.suspend(),
        dispose: () => {},
        send(bytes, peer) {
            host.engineSend(peer, bytes);
        },
        lend(peer) {
            const channel = new MessageChannel();
            const port = channel.port1;
            port.onmessage = (e: MessageEvent<PortSays>) => {
                const msg = e.data;
                if (msg.type === "osc") {
                    host.engineSend(peer, new Uint8Array(msg.data));
                } else if (msg.type === "clock") {
                    void clockAnchor().then((a) =>
                        port.postMessage({ type: "clock", clock: a.sample, frame: a.frame, epoch: a.epoch }),
                    );
                } else if (msg.type === "buffer_load") {
                    install(msg.index, msg.channels, msg.sampleRate, new Float32Array(msg.data)).then(
                        () => port.postMessage({ type: "buffer_load", index: msg.index, ok: true }),
                        (why: unknown) =>
                            port.postMessage({
                                type: "buffer_load",
                                index: msg.index,
                                ok: false,
                                message: String(why),
                            }),
                    );
                } else if (msg.type === "close") {
                    raw.release(peer);
                }
            };
            port.start();
            lent.set(peer, port);
            return channel.port2;
        },
        release(peer) {
            const port = lent.get(peer);
            if (port === undefined) return;
            lent.delete(peer);
            port.onmessage = null;
            port.close();
        },
        clock: () => clockAnchor().then((a) => a.sample),
        clockAnchor,
        bufferLoad: install,
    };
    host.onEngine((kind, id, value) => {
        if (kind === "reply") {
            const bytes = value as Uint8Array;
            const port = lent.get(id);
            if (port !== undefined) {
                port.postMessage({ type: "osc", data: bytes, peer: id }, [bytes.buffer as ArrayBuffer]);
            } else {
                raw.onReply?.(bytes, id);
            }
        } else if (kind === "samples") {
            const answer = installs.get(id);
            installs.delete(id);
            answer?.(value === null ? null : String(value));
        }
    });
    return raw;
}
