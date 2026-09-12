/**
 * What makes a piece **sound**, kept in step with what the editor draws.
 *
 * The multitrack editor's other half. {@link MultitrackView} says what a piece
 * looks like and {@link MultitrackDomain} says what a gesture makes of it; this
 * says what it is heard as.
 *
 * **It decides nothing.** What a track and a clip *are* on the server is the
 * shared core's (`mt.piece`, `mt.track`, `mt.clip`, `mt.reader` and the channel
 * strip under all of them); which of them a given piece needs is the document
 * crate's; and the **difference** between that and what is already sounding is
 * `Instance`, in the shared crate. So what is left here is three things a
 * language genuinely owns: a socket, an allocator, and one table from the
 * crate's handles to the objects this page made.
 *
 * **Why a diff and not a rebuild.** A piece plays itself from the transport:
 * every reader reads the engine's own position, so a locate is no message at all
 * and moving a box is one `set`. That only holds if the nodes **stay**:
 * rebuilding the tree on every edit would restart everything that is sounding,
 * and a hand dragging a box would hear its own gesture as a stutter. So a track,
 * a clip and a reader are each added once and set thereafter, and only what a
 * set cannot express is torn down and made again — which is the reconciler's
 * rule and is written down there.
 *
 * **Handles: the crate names what it cannot make.** An operation never carries a
 * node id, a bus index or a buffer number, because the crate allocates none of
 * them. It carries a **handle** — a string it mints from the document's own
 * ids — and `Playback` keeps the one table from handle to whatever it made. A
 * port that has to name a resource names it the same way, and `value` is where
 * that is resolved.
 *
 * @module
 */

import { Instance } from "../../core/clausters_core_web.js";
import { Buffer } from "../../defs/buffer.ts";
import { Bus } from "../../defs/bus.ts";
import { AddAction, Group, Synth } from "../../defs/node.ts";
import type { Server } from "../../defs/server/index.ts";
import type { GuiHost } from "../host.ts";
import { Transport } from "../transport.ts";
import type { MultitrackEditor } from "./multitrack.ts";

/** One value of one port: a number, or a handle of something this made. */
export type PortValue = number | { bus: string; offset?: number } | { buffer: string };

/** The ports of one node, as an operation states them. */
export type Ports = Record<string, PortValue>;

/** **One thing to do to the server**, as the reconciler states it. */
export interface Op {
    op: string;
    handle?: string;
    target?: string;
    before?: string;
    slot?: string;
    graph?: string;
    def?: string;
    family?: string;
    spec?: unknown;
    port?: string;
    bus?: string;
    channels?: number;
    samples?: number[];
    ports?: Ports;
    forget?: string[];
}

export class Playback {
    readonly editor: MultitrackEditor;
    readonly server: Server;
    /** The master's own level. */
    readonly gain: number;
    /**
     * What is sounding, as the crate holds it. It answers the difference
     * between that and the piece; nothing here decides what a difference is.
     */
    private readonly instance = new Instance();
    /** Handle → the node this page made for it. */
    private readonly nodes = new Map<string, Group | Synth>();
    /** Handle → the control bus. */
    private readonly buses = new Map<string, Bus>();
    /** Handle → the buffer. */
    private readonly buffers = new Map<string, Buffer>();
    readonly transport: Transport;

    constructor(
        editor: MultitrackEditor,
        { server, host = null, gain = 0.5 }: {
            server: Server;
            host?: GuiHost | null;
            gain?: number;
        },
    ) {
        this.editor = editor;
        this.server = server;
        this.gain = Number(gain);
        const bridge = editor.bridge;
        this.transport = new Transport(
            host,
            () => (editor.pieceWidget === null ? [] : [editor.pieceWidget]),
            {
                headClock: "piece",
                governed: true,
                tempoMap: bridge.tempo,
                sampleRate: bridge.rate,
                extent: () => editor.structure.end,
            },
        );
        this.transport.server = server;
    }

    /**
     * Instantiate the piece, bind the group the transport governs, and put
     * every track, clip and reader where the piece says.
     *
     * The half of building one that talks to the server, which in a page is a
     * promise: the same calls the script makes in its constructor, made where
     * they can be waited for.
     */
    async prepare(): Promise<this> {
        await this.syncAsync();
        this.transport.locate(this.editor.cursor ?? 0.0);
        return this;
    }

    /**
     * The piece went on screen: draw the line from the engine's own position.
     *
     * A playback is built before the window is — a piece can be played by a page
     * that never draws it — so this is where the two meet, and it is one
     * statement: `GuiHost.headClock` and the transport's own are the same
     * decision, and letting them disagree draws a line nobody put there.
     */
    attach(host: GuiHost | null): void {
        if (host === null) return;
        this.transport.host = host;
        host.headClock("piece");
        this.transport.locate(this.transport.position);
    }

    // ---- the instance ----

    /**
     * Track id → the control buses its meters write and how wide it is: a run
     * of `2 * channels`, the level first and the mark that waits after it.
     *
     * What the host reads every frame, straight out of the shared segment,
     * which is why a level that moves every block costs no message. The crate
     * says which run belongs to which track; the buses are this page's, because
     * it is this page that allocates them.
     */
    get meters(): Map<number, [Bus, number]> {
        const out = new Map<number, [Bus, number]>();
        const rows = JSON.parse(this.instance.meters()) as {
            track: number;
            bus: string;
            channels: number;
        }[];
        for (const row of rows) {
            const bus = this.buses.get(row.bus);
            if (bus !== undefined) out.set(row.track, [bus, row.channels]);
        }
        return out;
    }

    /**
     * Make what sounds be what is drawn.
     *
     * The whole of it, and it runs on every edit whoever made it. Everything
     * that is already right is left alone, which is what lets a hand drag a box
     * without hearing the rest of the piece restart.
     */
    sync(): void {
        void this.syncAsync();
    }

    /** {@link Playback.sync}, waited for — what a page's own setup uses. */
    async syncAsync(): Promise<void> {
        const bridge = this.editor.bridge;
        const answer = this.instance.reconcile(
            JSON.stringify(this.editor.structure.write()),
            bridge.rate,
            bridge.bpm,
            JSON.stringify(bridge.sources.table()),
            this.gain,
        );
        await this.apply(JSON.parse(answer) as Op[]);
    }

    /**
     * Do what the reconciler says, in order.
     *
     * **The order is the answer.** A def before the graph that names it, a
     * buffer before the reader pointed at it, a node freed before the one that
     * replaces it is made — all of that is decided in the crate and this only
     * carries it out, which is why a second client cannot carry it out
     * differently.
     */
    async apply(ops: readonly Op[]): Promise<void> {
        for (const op of ops) {
            switch (op.op) {
                case "def":
                    this.server.sendMsg(
                        "/def_send",
                        String(op.family),
                        JSON.stringify(op.spec),
                    );
                    break;
                case "barrier":
                    // **The batch is closed before anything else is sent.** A
                    // def send is asynchronous and answers `/done`, so a
                    // `/done` left in flight is one the next command that waits
                    // for one takes as its own — and a buffer alloc that
                    // returns before it ran is written into before it exists.
                    await this.server.sync();
                    break;
                case "graph":
                    this.nodes.set(
                        String(op.handle),
                        Group.graph(String(op.graph), this.ports(op), { server: this.server }),
                    );
                    break;
                case "transport":
                    await this.server.transportGroup(this.node(op.handle) as Group);
                    break;
                case "group":
                    this.nodes.set(
                        String(op.handle),
                        new Group({
                            target: this.node(op.before) as Group,
                            action: AddAction.BEFORE,
                            server: this.server,
                        }),
                    );
                    break;
                case "slot":
                    this.nodes.set(
                        String(op.handle),
                        (this.node(op.target) as Group).addSlot(String(op.slot), this.ports(op)),
                    );
                    break;
                case "synth":
                    this.nodes.set(
                        String(op.handle),
                        new Synth(String(op.def), this.ports(op), {
                            target: this.node(op.target) as Group,
                            server: this.server,
                        }),
                    );
                    break;
                case "bus":
                    this.buses.set(
                        String(op.handle),
                        Bus.control(Number(op.channels), { server: this.server }),
                    );
                    break;
                case "buffer":
                    this.buffers.set(
                        String(op.handle),
                        await Buffer.fromSamples(
                            Float32Array.from(op.samples ?? []),
                            1,
                            0.0,
                            { server: this.server },
                        ),
                    );
                    break;
                case "set":
                    this.node(op.handle)?.set(this.ports(op));
                    break;
                case "map": {
                    const node = this.node(op.handle);
                    const bus = this.buses.get(String(op.bus));
                    if (node !== undefined && bus !== undefined) {
                        this.server.sendMsg(
                            "/graph_map",
                            node.id,
                            String(op.port),
                            Math.trunc(bus.index),
                        );
                    }
                    break;
                }
                case "unmap": {
                    // **A handle with nothing behind it is a node that is
                    // already gone**, and a message naming one would reach
                    // whatever holds that id next. The reconciler does not emit
                    // these — freeing a node takes its map with it, and there
                    // is a crate test saying so — and this is the second half
                    // of that: the two clients answer an impossible handle the
                    // same way, instead of one raising and the other
                    // addressing node 0.
                    const node = this.node(op.handle);
                    if (node !== undefined) {
                        this.server.sendMsg("/graph_map", node.id, String(op.port), -1);
                    }
                    break;
                }
                case "free": {
                    this.nodes.get(String(op.handle))?.free();
                    this.nodes.delete(String(op.handle));
                    // Freeing a group frees what is inside it, so these only
                    // leave the table: a second free would name a node that is
                    // already gone.
                    for (const handle of op.forget ?? []) this.nodes.delete(handle);
                    break;
                }
                case "freeBus":
                    this.buses.get(String(op.handle))?.free();
                    this.buses.delete(String(op.handle));
                    break;
                case "freeBuffer":
                    this.buffers.get(String(op.handle))?.free();
                    this.buffers.delete(String(op.handle));
                    break;
                default:
                    break;
            }
        }
    }

    /** The node a handle names. */
    private node(handle: string | undefined): Group | Synth | undefined {
        return this.nodes.get(String(handle));
    }

    /**
     * One port's value: a number as itself, and a **handle** resolved out of
     * the table this filled when it made the thing.
     */
    value(port: PortValue): number {
        if (typeof port === "number") return port;
        if ("bus" in port) {
            return Number(this.buses.get(port.bus)?.index ?? 0) + Number(port.offset ?? 0);
        }
        return Number(this.buffers.get(port.buffer)?.bufnum ?? 0);
    }

    private ports(op: Op): Record<string, number> {
        const out: Record<string, number> = {};
        for (const [name, value] of Object.entries(op.ports ?? {})) {
            out[name] = this.value(value);
        }
        return out;
    }

    // ---- the transport ----

    /**
     * Whether the piece is rolling, as the engine last answered — the answer
     * that is already known. Asking afresh is {@link Playback.refresh}, for the
     * reason every request in a page is a promise: a page waits for an answer
     * instead of blocking on it.
     */
    get playing(): boolean {
        return this.transport.playing;
    }

    /** Where the piece is, in beats, as the engine last answered. */
    get position(): number {
        return this.transport.position;
    }

    /** Ask the engine where the piece is, and remember it. */
    async refresh(): Promise<this> {
        await this.transport.refresh();
        return this;
    }

    /**
     * Play, or continue a paused pass: the engine keeps where it stopped, so
     * resuming is the same verb as starting and nothing is re-cued.
     */
    async play(): Promise<this> {
        await this.transport.play(this.server);
        return this;
    }

    /** Freeze the piece where it stands, with every node's state intact. */
    pause(): this {
        this.transport.pause();
        return this;
    }

    /**
     * Halt and go back to **the mark**, not to the top.
     *
     * The playhead is never placed: stopped, it stands where the position cursor
     * is, so the next play starts from the mark the reader put down rather than
     * from wherever the last pass happened to end.
     */
    stop(): this {
        this.transport.pause();
        return this.locate(this.editor.cursor ?? 0.0);
    }

    /**
     * Seek to `beat`. The readers seek in the engine, so nothing is re-cued and
     * what is sounding carries on from there.
     */
    locate(beat: number): this {
        this.transport.locate(beat);
        return this;
    }

    /**
     * The **position cursor** moved: cue a stopped transport there, and leave a
     * rolling one alone.
     *
     * Two cursors, and only one of them is placed. A click that landed on
     * nothing puts the position cursor down, which is where the next play
     * starts; moving the mark mid-pass must not move the music.
     */
    cue(beat: number): this {
        if (!this.playing) this.locate(beat);
        return this;
    }

    /**
     * Free the piece's instance. The piece itself is untouched: what a playback
     * holds is nodes, and nodes are not the composition.
     */
    close(): void {
        void this.apply(JSON.parse(this.instance.teardown()) as Op[]);
    }
}
