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
import { Buffer } from "../../defs/buffer.ts";
import { Bus } from "../../defs/bus.ts";
import { AddAction, Group, Synth } from "../../defs/node.ts";
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

/** One curve of the plan: the port it drives and the table a reader follows. */
export interface PlannedCurve {
    id: number;
    port: string;
    at: number;
    step: number;
    table: number[];
}

/** One clip of the plan: a box, its strip and its readers. */
export interface PlannedClip {
    region: number;
    slot: string;
    gain: number;
    mute: number;
    readers: PlannedReader[];
    curves: PlannedCurve[];
}

/** One track of the plan: its strip and the clips on it. */
export interface PlannedTrack {
    track: number;
    channels: number;
    gain: number;
    mute: number;
    clips: PlannedClip[];
    curves: PlannedCurve[];
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
/**
 * The ports the **hand** writes: everything a curve is not driving.
 *
 * A mapped control is taken back by a plain `/node_set` — that is the
 * protocol's own rule, and the right one, since it is what gives the fader
 * back when a curve is deleted. It also means that anything sending a value
 * for a port a curve drives **silences that curve**, and a piece re-syncs on
 * every edit, so adding a box to a track was enough to stop its automation
 * from being heard.
 */
function handPorts(ports: Ports, curves: PlannedCurve[]): Ports {
    const driven = new Set(curves.map((curve) => curve.port));
    return Object.fromEntries(
        Object.entries(ports).filter(([port]) => !driven.has(port)),
    );
}

/**
 * Whether a clip that is already sounding can be **set** into its new shape,
 * or has to be made again.
 *
 * Two things it cannot be set into. A source of another **width** is another
 * clip def — a mono take is panned into the track and a stereo one is
 * balanced — and that is the wiring, not a control. And a clip that changed
 * **track**: a clip is a slot *inside* a track's group, so the node carries no
 * track id to update. Setting it would leave it sounding through the track it
 * came from — that track's fader, that track's mute, that track's automation —
 * while the picture drew it on the new one.
 */
function staysPut(
    held: [Group, string, Ports, number],
    slot: string,
    track: number,
): boolean {
    return held[1] === slot && held[3] === track;
}

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
     * Track id → the control buses its meters write and how wide it is: a run
     * of `2 * channels`, the level first and the mark that waits after it.
     *
     * What the host reads every frame, straight out of the shared segment,
     * which is why a level that moves every block costs no message.
     */
    readonly meters = new Map<number, [Bus, number]>();
    /**
     * Region id → its group, its slot, the ports last sent, and which track it
     * is on. The slot is kept because a source of another width is another clip
     * def, which is the one change a `set` cannot express; the track is kept so
     * a track that went away takes its clips out of the table with it.
     */
    readonly clips = new Map<number, [Group, string, Ports, number]>();
    /** `region:channel` → its group and the ports last sent. */
    readonly readers = new Map<string, [Group, Ports]>();
    /**
     * Automation id → its node, its table, its bus, the group whose port it
     * drives, that port, and the table last written.
     *
     * A curve is a node of this client's own rather than a member of the
     * piece's graph: it writes a control bus and the port is **mapped** to it,
     * which is what lets one curve drive a control three levels down without
     * anybody learning the node behind it.
     */
    readonly curves = new Map<
        number,
        [Synth, Buffer, Bus, Group, string, number[]]
    >();
    /**
     * The group the curve nodes live in, before the piece so a value is written
     * in the block it is read.
     */
    curveGroup: Group | null = null;
    /** The name of the curve def, as the core gives it. */
    private curveDef = "";
    /** How long a meter's mark waits, in seconds, as the core says. */
    private meterHold = 0;
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
        if (this.curveGroup === null) {
            // Before the piece: a control bus written after it is read is a
            // block late, every block, which on a fade is an audible lag.
            this.curveGroup = new Group({
                target: this.piece,
                action: AddAction.BEFORE,
                server: this.server,
            });
        }
        await this.syncTracks(plan.tracks);
        this.reapCurves(plan);
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
            curve: string;
            meterHold: number;
        };
        this.curveDef = defs.curve;
        this.meterHold = Number(defs.meterHold ?? 0);
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

    private async syncTracks(planned: PlannedTrack[]): Promise<void> {
        const seen = new Set<number>();
        for (const track of planned) {
            seen.add(track.track);
            let group = this.tracks.get(track.track);
            if (group === undefined) {
                group = this.piece!.addSlot("tracks");
                this.tracks.set(track.track, group);
                this.meter(track.track, group, track.channels);
            }
            group.set(handPorts({ gain: track.gain, mute: track.mute }, track.curves));
            await this.syncCurves(group, track.curves);
            await this.syncClips(track.track, group, track.clips);
        }
        for (const id of [...this.tracks.keys()].filter((id) => !seen.has(id))) {
            this.freeTrack(id);
        }
    }

    /**
     * Put this track's meters on it, and remember where they write.
     *
     * **Two of them**, which is one def twice: with no hold it is the level,
     * with the core's hold it is the mark that stays up long enough to be read.
     * Both are slot instances, so a piece nobody meters holds none — and the
     * buses are allocated here because it is this client that allocates buses,
     * and told to the meter as a port because it is the host that reads them.
     */
    private meter(id: number, track: Group, channels: number): void {
        const width = Math.max(1, Math.trunc(channels));
        const bus = Bus.control(2 * width, { server: this.server });
        for (const [run, hold] of [[0, 0], [width, this.meterHold]] as const) {
            const ports: Ports = {
                "meter/out0": bus.index + run,
                "meter/hold": hold,
            };
            if (width > 1) ports["meter/out1"] = bus.index + run + 1;
            track.addSlot("meters", ports);
        }
        this.meters.set(id, [bus, width]);
    }

    private async syncClips(
        id: number,
        track: Group,
        planned: PlannedClip[],
    ): Promise<void> {
        const seen = new Set<number>();
        for (const clip of planned) {
            seen.add(clip.region);
            const ports = handPorts({ gain: clip.gain, mute: clip.mute }, clip.curves);
            let held = this.clips.get(clip.region);
            if (held !== undefined && !staysPut(held, clip.slot, id)) {
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
            await this.syncCurves(group, clip.curves);
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

    /**
     * Put each curve's table on the server and map the port to it.
     *
     * The table is read at the transport's own position, so a locate costs no
     * message at all — which is the whole reason a curve is a table and not a
     * stream of sets.
     */
    private async syncCurves(owner: Group, planned: PlannedCurve[]): Promise<void> {
        for (const curve of planned) {
            const held = this.curves.get(curve.id);
            if (held === undefined) {
                const buffer = await Buffer.fromSamples(Float32Array.from(curve.table), 1, 0, {
                    server: this.server,
                });
                const bus = Bus.control(1, { server: this.server });
                const node = new Synth(
                    this.curveDef,
                    { out: bus.index, buf: buffer.bufnum, at: curve.at, step: curve.step },
                    { target: this.curveGroup ?? undefined, server: this.server },
                );
                this.server.sendMsg(
                    "/graph_map",
                    ["i", owner.id],
                    ["s", curve.port],
                    ["i", bus.index],
                );
                this.curves.set(curve.id, [
                    node, buffer, bus, owner, curve.port, curve.table,
                ]);
                continue;
            }
            const [node, buffer, bus, heldOwner, port, sent] = held;
            if (heldOwner.id !== owner.id) {
                // **The map belongs to the node, not to the curve.** A clip
                // that changed track is a new node — a clip is a slot inside
                // its track's group — and the port that was mapped went away
                // with the old one, while the curve went on writing a bus
                // nobody reads. The hand does not send that port either
                // (`handPorts` leaves it to the curve), so the box came back
                // at the def's own default.
                this.server.sendMsg(
                    "/graph_map",
                    ["i", owner.id],
                    ["s", curve.port],
                    ["i", bus.index],
                );
            }
            let table = buffer;
            if (!same(curve.table, sent)) {
                // A curve whose points moved is a new table, and a table is
                // replaced rather than written into — its length changes with
                // its first and last point. `buf` is an ordinary control, so
                // the reader follows without stopping.
                table = await Buffer.fromSamples(Float32Array.from(curve.table), 1, 0, {
                    server: this.server,
                });
                node.set({ buf: table.bufnum, at: curve.at, step: curve.step });
                buffer.free();
            } else {
                node.set({ at: curve.at, step: curve.step });
            }
            this.curves.set(curve.id, [node, table, bus, owner, port, curve.table]);
        }
    }

    /**
     * Free the curves the piece no longer has, and give their ports back.
     *
     * **Unmapping is not optional**: a port left mapped to a bus nobody writes
     * holds whatever was in it, so a curve that was deleted would go on driving
     * the control it drove, at the last value it happened to say.
     */
    private reapCurves(plan: Plan): void {
        const alive = new Set<number>();
        for (const track of plan.tracks) {
            for (const curve of track.curves) alive.add(curve.id);
            for (const clip of track.clips) {
                for (const curve of clip.curves) alive.add(curve.id);
            }
        }
        for (const [id, held] of [...this.curves.entries()]) {
            if (alive.has(id)) continue;
            const [node, buffer, bus, owner, port] = held;
            this.server.sendMsg("/graph_map", ["i", owner.id], ["s", port], ["i", -1]);
            node.free();
            buffer.free();
            bus.free();
            this.curves.delete(id);
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
        // Freeing the group frees everything inside it, so the clips and the
        // meters only have to leave the table — which is what the track id in
        // them is for. The buses are not the group's, so they are given back.
        for (const [region, held] of [...this.clips.entries()]) {
            if (held[3] === id) this.freeClip(region, false);
        }
        const metered = this.meters.get(id);
        if (metered !== undefined) {
            metered[0].free();
            this.meters.delete(id);
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
        for (const [, buffer, bus] of this.curves.values()) {
            buffer.free();
            bus.free();
        }
        this.curves.clear();
        if (this.curveGroup !== null) {
            this.curveGroup.free();
            this.curveGroup = null;
        }
        for (const [bus] of this.meters.values()) bus.free();
        this.meters.clear();
        this.readers.clear();
        this.clips.clear();
        this.tracks.clear();
        if (this.piece !== null) {
            this.piece.free();
            this.piece = null;
        }
    }
}

/** Whether two tables hold the same values. */
function same(a: readonly number[], b: readonly number[]): boolean {
    return a.length === b.length && a.every((v, i) => v === b[i]);
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
