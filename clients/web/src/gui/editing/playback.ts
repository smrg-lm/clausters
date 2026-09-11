/**
 * What makes a piece **sound**, kept in step with what the editor draws.
 *
 * The multitrack editor's other half. {@link MultitrackView} says what a piece
 * looks like and {@link MultitrackDomain} says what a gesture makes of it; this
 * says what it is heard as.
 *
 * **It decides nothing.** What a track and a clip *are* on the server is the
 * shared core's (`mt.piece`, `mt.track`, `mt.clip`, `mt.reader` and the channel
 * strip under all of them), and which of them a given piece needs is the
 * document crate's — `multitrackPlan` answers it, wired to which buffer, at
 * which frame, with which level. So this module is a **diff**: it compares the
 * plan against what is already sounding and sends the difference. Both clients
 * run the same two calls, which is why one piece sounds the same in both of
 * them.
 *
 * **Why a diff and not a rebuild.** A piece plays itself from the transport:
 * every reader reads the engine's own position, so a locate is no message at all
 * and moving a box is one `set`. That only holds if the nodes **stay**:
 * rebuilding the tree on every edit would restart everything that is sounding,
 * and a hand dragging a box would hear its own gesture as a stutter. So a track,
 * a clip and a reader are each added once and set thereafter, and only the one
 * thing a set cannot express — a clip whose source changed *width*, which is a
 * different wiring — is torn down and made again.
 *
 * @module
 */

import { mixerDefs, multitrackPlan } from "../../core/clausters_core_web.js";
import { Group } from "../../defs/node.ts";
import type { Server } from "../../defs/server/index.ts";
import type { GuiHost } from "../host.ts";
import { Transport } from "../transport.ts";
import type { MultitrackEditor } from "./multitrack.ts";

/** One reader of the plan: one channel of one box. */
export interface PlannedReader {
    channel: number;
    buffer: number;
    at: number;
    span: number;
    start: number;
    looping: boolean;
}

/** One clip of the plan: a box, its strip and its readers. */
export interface PlannedClip {
    region: number;
    slot: string;
    gain: number;
    mute: number;
    readers: PlannedReader[];
}

/** One track of the plan: its strip and the clips on it. */
export interface PlannedTrack {
    track: number;
    channels: number;
    gain: number;
    mute: number;
    clips: PlannedClip[];
}

/** The whole piece as instances: one graph, and everything else a slot. */
export interface Plan {
    graph: string;
    channels: number;
    tracks: PlannedTrack[];
    widths: [number, number][];
}

/** The ports a strip is driven through. */
export type Ports = Record<string, number>;

/**
 * The instance of one piece, and the transport that moves it.
 *
 * Built by {@link MultitrackEditor} when it is given a server, and reachable as
 * its `playback`. Nothing here is subscribed to anything: the editor tells it
 * the piece changed, whoever changed it — this window's gesture, a second window
 * over the same piece, or a step of the history — and {@link Playback.sync}
 * makes what sounds be what is drawn.
 */
export class Playback {
    readonly editor: MultitrackEditor;
    readonly server: Server;
    /** The master's own level. */
    readonly gain: number;
    /** The def names already sent. A take of another width asks for more. */
    private readonly sent = new Set<string>();
    /** The `mt.piece` instance: one group, and everything else a slot in it. */
    piece: Group | null = null;
    /** Track id → its slot group. */
    readonly tracks = new Map<number, Group>();
    /**
     * Region id → its group, its slot, the ports last sent, and which track it
     * is on. The slot is kept because a source of another width is another clip
     * def, which is the one change a `set` cannot express; the track is kept so
     * a track that went away takes its clips out of the table with it.
     */
    readonly clips = new Map<number, [Group, string, Ports, number]>();
    /** `region:channel` → its group and the ports last sent. */
    readonly readers = new Map<string, [Group, Ports]>();
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
     * What the piece is, as instances: the crate's answer, not this module's
     * opinion of it.
     */
    plan(): Plan | null {
        const bridge = this.editor.bridge;
        const answer = multitrackPlan(
            JSON.stringify(this.editor.structure.write()),
            bridge.rate,
            bridge.bpm,
            JSON.stringify(bridge.sources.table()),
        );
        return answer === "" ? null : (JSON.parse(answer) as Plan);
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
        const plan = this.plan();
        if (plan === null) return;
        await this.sendDefs(plan);
        if (this.piece === null) {
            this.piece = Group.graph(plan.graph, { gain: this.gain }, {
                server: this.server,
            });
            // **The piece's group is the transport's**: from here the engine
            // freezes that subtree on a stop and thaws it on a play, and every
            // reader's position is the engine's own rather than a number kept
            // in step here.
            await this.server.transportGroup(this.piece);
        }
        this.syncTracks(plan.tracks);
    }

    /**
     * Send the defs this piece's widths need, and only the ones not sent.
     *
     * In the order the core gives them: a graph never names one that has not
     * been sent, and getting that wrong fails in another process at
     * instantiation with nothing to point at.
     */
    private async sendDefs(plan: Plan): Promise<void> {
        const answer = mixerDefs(JSON.stringify(plan.widths), plan.channels);
        if (answer === "") return;
        const defs = JSON.parse(answer) as {
            synth: { name: string }[];
            graph: { name: string }[];
        };
        for (const [family, specs] of [
            ["synth", defs.synth],
            ["graph", defs.graph],
        ] as const) {
            for (const spec of specs) {
                if (this.sent.has(spec.name)) continue;
                await this.server.command("/def_send", [family, JSON.stringify(spec)]);
                this.sent.add(spec.name);
            }
        }
    }

    private syncTracks(planned: PlannedTrack[]): void {
        const seen = new Set<number>();
        for (const track of planned) {
            seen.add(track.track);
            let group = this.tracks.get(track.track);
            if (group === undefined) {
                group = this.piece!.addSlot("tracks");
                this.tracks.set(track.track, group);
            }
            group.set({ gain: track.gain, mute: track.mute });
            this.syncClips(track.track, group, track.clips);
        }
        for (const id of [...this.tracks.keys()].filter((id) => !seen.has(id))) {
            this.freeTrack(id);
        }
    }

    private syncClips(id: number, track: Group, planned: PlannedClip[]): void {
        const seen = new Set<number>();
        for (const clip of planned) {
            seen.add(clip.region);
            const ports: Ports = { gain: clip.gain, mute: clip.mute };
            let held = this.clips.get(clip.region);
            // A source of another width is another clip def — a mono take is
            // panned into the track and a stereo one is balanced — so it is the
            // one change that cannot be a set.
            if (held !== undefined && held[1] !== clip.slot) {
                this.freeClip(clip.region);
                held = undefined;
            }
            let group: Group;
            if (held === undefined) {
                group = track.addSlot(clip.slot, ports);
            } else {
                group = held[0];
                const moved = changed(ports, held[2]);
                if (moved !== null) group.set(moved);
            }
            this.clips.set(clip.region, [group, clip.slot, ports, id]);
            this.syncReaders(clip.region, group, clip.readers);
        }
        const mine = [...this.clips.entries()].filter(([, held]) => held[3] === id);
        for (const [region] of mine.filter(([region]) => !seen.has(region))) {
            this.freeClip(region);
        }
    }

    private syncReaders(region: number, clip: Group, planned: PlannedReader[]): void {
        const seen = new Set<string>();
        for (const reader of planned) {
            const key = `${region}:${reader.channel}`;
            seen.add(key);
            const ports: Ports = {
                buf: reader.buffer,
                chan: reader.channel,
                at: reader.at,
                span: reader.span,
                start: reader.start,
                loop: reader.looping ? 1.0 : 0.0,
            };
            const held = this.readers.get(key);
            if (held === undefined) {
                this.readers.set(key, [clip.addSlot("source", ports), ports]);
                continue;
            }
            // Every one of these is an ordinary control, `buf` included, so a
            // box that was re-cut over a different buffer keeps sounding.
            const moved = changed(ports, held[1]);
            if (moved !== null) held[0].set(moved);
            this.readers.set(key, [held[0], ports]);
        }
        for (const key of [...this.readers.keys()]) {
            if (key.startsWith(`${region}:`) && !seen.has(key)) {
                this.readers.get(key)![0].free();
                this.readers.delete(key);
            }
        }
    }

    private freeClip(region: number, freeing = true): void {
        for (const key of [...this.readers.keys()]) {
            if (key.startsWith(`${region}:`)) this.readers.delete(key);
        }
        const held = this.clips.get(region);
        this.clips.delete(region);
        if (freeing && held !== undefined) held[0].free();
    }

    private freeTrack(id: number): void {
        // Freeing the group frees everything inside it, so the clips only have
        // to leave the table — which is what the track id in it is for.
        for (const [region, held] of [...this.clips.entries()]) {
            if (held[3] === id) this.freeClip(region, false);
        }
        this.tracks.get(id)?.free();
        this.tracks.delete(id);
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
        this.readers.clear();
        this.clips.clear();
        this.tracks.clear();
        if (this.piece !== null) {
            this.piece.free();
            this.piece = null;
        }
    }
}

/** The ports of `now` that differ from `sent`, or `null` when none do. */
function changed(now: Ports, sent: Ports): Ports | null {
    const moved: Ports = {};
    let any = false;
    for (const [key, value] of Object.entries(now)) {
        if (value !== sent[key]) {
            moved[key] = value;
            any = true;
        }
    }
    return any ? moved : null;
}
