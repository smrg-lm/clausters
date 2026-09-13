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
 * `Instance`, in the shared crate; and the messages that carry it out are the
 * crate's applier. So what is left here is what a language genuinely owns: a
 * socket, and waiting on it.
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
 * **Steps: the crate says what waits.** An operation never carries a node id,
 * a bus index or a buffer number. The crate's applier (`Applier`) keeps the
 * table from each operation's handle to what it became, allocates those from
 * the server's id spaces, and answers **steps**: a message to send, a `/done`
 * the rest waits for, a barrier. A buffer's fill waiting for its allocation is
 * one of those steps, stated once, and every endpoint — this page, the Python
 * client and the GUI host playing a piece on its own — carries out the same
 * list.
 *
 * @module
 */

import { Applier, Instance } from "../../core/clausters_core_web.js";
import type { MsgArg } from "../../base/osc.ts";
import type { Server } from "../../defs/server/index.ts";
import type { GuiHost } from "../host.ts";
import { Transport } from "../transport.ts";
import type { MultitrackEditor } from "./multitrack.ts";

/** **One thing to do to the server**, as the reconciler states it. */
export type Op = Record<string, unknown> & { op: string };

/** One argument of a step, tagged as the crate encoded it. */
export type StepArg = { i: number } | { f: number } | { s: string } | { b: number[] };

/** **One step**, as the applier states it. */
export type Step =
    | { send: { addr: string; args: StepArg[] } }
    | { await: { command: string; index: number | null } }
    | { sync: number };

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
    /**
     * The operations as steps, and the table from handle to what each became —
     * the crate's, as the instance is. Made in `prepare`, which knows how many
     * samples one fill may carry on this server.
     */
    private applier: Applier | null = null;
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
        this.applier = new Applier(0, true, await this.server.bulkChunk());
        // Node ids come back on their `/node_end`, which only a registered
        // client hears.
        await this.server.notify(true);
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
    get meters(): Map<number, [number, number]> {
        const out = new Map<number, [number, number]>();
        if (this.applier === null) return out;
        const rows = JSON.parse(this.instance.meters()) as {
            track: number;
            bus: string;
            channels: number;
        }[];
        for (const row of rows) {
            const bus = this.applier.bus(row.bus);
            if (bus !== undefined) out.set(row.track, [bus[0], row.channels]);
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
     * **The order is the answer, and so are the waits.** A def before the
     * graph that names it, a buffer's fill after its allocation answered, a
     * node freed before the one that replaces it is made — all of that is the
     * crate's, stated as steps, and this only sends them and waits where a step
     * says to.
     */
    async apply(ops: readonly Op[]): Promise<void> {
        if (this.applier === null || ops.length === 0) return;
        const answer = JSON.parse(
            this.applier.apply(JSON.stringify(ops), this.server.ids),
        ) as { steps?: Step[]; error?: string };
        if (answer.error !== undefined) throw new Error(`clausters: ${answer.error}`);
        await this.run(answer.steps ?? []);
    }

    /** Send each step, waiting where one says to. */
    private async run(steps: readonly Step[]): Promise<void> {
        for (let index = 0; index < steps.length; index++) {
            const step = steps[index]!;
            if ("send" in step) {
                const { addr } = step.send;
                const args = step.send.args.map(stepArg);
                const after = steps[index + 1];
                if (after !== undefined && "await" in after) {
                    // Sent and waited for as one command, so the `/done` cannot
                    // arrive before anyone is listening for it.
                    await this.server.command(addr, args);
                    index++;
                    continue;
                }
                this.server.sendMsg(addr, ...args);
            } else if ("sync" in step) {
                await this.server.sync();
            }
        }
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

/** One step argument, tagged as the crate encoded it. */
function stepArg(arg: StepArg): MsgArg {
    if ("i" in arg) return ["i", arg.i];
    if ("f" in arg) return ["f", arg.f];
    if ("b" in arg) return new Uint8Array(Float32Array.from(arg.b).buffer);
    return arg.s;
}
