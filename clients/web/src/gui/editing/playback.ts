/**
 * What makes a piece **sound**, kept in step with what the editor draws.
 *
 * The multitrack editor's other half. {@link MultitrackView} says what a piece
 * looks like and {@link MultitrackDomain} says what a gesture makes of it; this
 * says what it is heard as.
 *
 * **It decides nothing.** What a track and a clip *are* on the server is the
 * shared core's; which of them a given piece needs, the difference between that
 * and what is already sounding, the messages that carry it out and **how the
 * piece is played** — the tempo a piece that states none is read at, the sample
 * a beat is when the transport is located, what play, pause, stop and cue send,
 * and that a paused meter is zeroed — are `MultitrackPlayback`, in the shared crate.
 * The GUI host playing a session with no page behind it holds the same object,
 * so the two are one program. What is left here is what a language genuinely
 * owns: a socket, and waiting on it.
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

import { MultitrackPlayback, StepRunner } from "../../core/clausters_core_web.js";
import type { Server } from "../../defs/server/index.ts";
import { runSteps } from "../../steps.ts";
import type { Step } from "../../steps.ts";
import type { GuiHost } from "../host.ts";
import { PlayheadSync } from "../playhead-sync.ts";
import type { MultitrackEditor } from "./multitrack.ts";

export type { Step, StepArg } from "../../steps.ts";

export class Playback {
    readonly editor: MultitrackEditor;
    readonly server: Server;
    /** The master's own level. */
    readonly gain: number;
    /**
     * The piece as it is playing — the crate's, as the host's is. Made in
     * `prepare`, which knows how many samples one fill may carry on this server.
     */
    private piece: MultitrackPlayback | null = null;
    /**
     * The steps not carried out yet — the crate's walk, as the script's and the
     * GUI host's are. Made in `prepare`, beside the playback.
     */
    private runner: StepRunner | null = null;
    readonly transport: PlayheadSync;

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
        this.transport = new PlayheadSync(
            host,
            () => (editor.pieceWidget === null ? [] : [editor.pieceWidget]),
            {
                headClock: "piece",
                governed: true,
                structure: () => bridge,
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
        this.piece = new MultitrackPlayback(await this.server.bulkChunk());
        this.runner = new StepRunner();
        // Node ids come back on their `/node_end`, which only a registered
        // client hears.
        await this.server.notify(true);
        await this.syncAsync();
        await this.locateAsync(this.editor.cursor ?? 0.0);
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
        this.locate(this.transport.position);
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
        if (this.piece === null) return out;
        const rows = JSON.parse(this.piece.meters()) as {
            track: number;
            bus: number;
            channels: number;
        }[];
        for (const row of rows) out.set(row.track, [row.bus, row.channels]);
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
        if (this.piece === null) return;
        const bridge = this.editor.bridge;
        await this.run(
            this.piece.sync(
                JSON.stringify(this.editor.structure.write()),
                bridge.rate,
                JSON.stringify(bridge.sources.table()),
                this.gain,
                this.server.ids,
            ),
        );
    }

    /**
     * Carry the steps of an answer out through the crate's runner.
     *
     * What may go out is sent; where something is awaited, the runner puts the
     * message it waits on last, and that one is sent as a request — so the
     * reply cannot arrive before anyone is listening for it — and its reply is
     * handed back, which releases the rest. Which reply releases what is the
     * runner's, as it is the script's and the GUI host's.
     */
    private async run(answer: string): Promise<void> {
        const { steps = [], error } = JSON.parse(answer) as { steps?: Step[]; error?: string };
        if (error !== undefined) throw new Error(`clausters: ${error}`);
        const runner = this.runner;
        if (runner === null) throw new Error("clausters: the playback is not prepared");
        await runSteps(this.server, runner, steps);
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
        if (this.piece !== null) await this.run(this.piece.play());
        this.transport.reported({ playing: true });
        return this;
    }

    /**
     * Freeze the piece where it stands, with every node's state intact, and its
     * meters at zero.
     */
    pause(): this {
        if (this.piece !== null) void this.run(this.piece.pause());
        this.transport.reported({ playing: false });
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
        const mark = this.editor.cursor ?? 0.0;
        if (this.piece === null) return this;
        void this.run(this.piece.stop(mark));
        this.transport.reported({
            playing: false,
            positionSample: this.piece.beatsToSamples(mark),
        });
        return this;
    }

    /**
     * Seek to `beat`. The readers seek in the engine, so nothing is re-cued and
     * what is sounding carries on from there.
     */
    locate(beat: number): this {
        void this.locateAsync(beat);
        return this;
    }

    /** {@link Playback.locate}, waited for. */
    private async locateAsync(beat: number): Promise<void> {
        if (this.piece === null) return;
        const positionSample = this.piece.beatsToSamples(beat);
        this.transport.reported({ positionSample });
        await this.run(this.piece.locate(beat));
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
        if (this.piece === null) return this;
        this.piece.setRolling(this.playing);
        const answer = this.piece.cue(beat);
        if (answer !== '{"steps":[]}') {
            this.transport.reported({ positionSample: this.piece.beatsToSamples(beat) });
            void this.run(answer);
        }
        return this;
    }

    /**
     * Free the piece's instance. The piece itself is untouched: what a playback
     * holds is nodes, and nodes are not the composition.
     */
    close(): void {
        if (this.piece !== null) void this.run(this.piece.close(this.server.ids));
    }
}
