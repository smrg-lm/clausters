/**
 * What makes a piece **sound**, kept in step with what the editor draws.
 *
 * The multitrack editor's other half. {@link MultitrackView} says what a piece
 * looks like and {@link MultitrackDomain} says what a gesture makes of it; this
 * says what it is heard as, and it is the same object either way — a piece is a
 * statement, so putting the readers where it says is one verb whether it is the
 * first time or the hundredth.
 *
 * **A box is a reader.** Each one is a single resident node reading its buffer
 * at the transport's position — no queue, nothing scheduled, nothing re-cued.
 * Moving a box while it plays is one `/node_set` on a node that is already
 * running, so it is heard where it was dropped with nothing that is sounding
 * cut, and the seeking, the pausing and the looping are the server transport's.
 *
 * **The time is the server's**, which settles everything under it: play, pause
 * and stop are `Server.transportPlay`, `Server.transportStop` and
 * `Server.transportLocateSample`; the readers are one **governed** group, so a
 * pause freezes them with every node's state intact and playing again continues
 * rather than starting over; and the host draws the line from the engine's own
 * position, with nothing sent per frame.
 *
 * **What is not here yet is the chain.** A track's level, its mute and its solo
 * reach the readers; the curves drawn on a track and inside a box do not,
 * because a curve's `gain` and the knob's `gain` have to name one parameter of
 * one node before either can drive it, and that is the synthesis node system's
 * design rather than this module's. Until it exists a curve is drawn, edited and
 * kept by the piece, and heard by nothing.
 *
 * @module
 */

import { Group, Synth } from "../../defs/node.ts";
import type { Server } from "../../defs/server/index.ts";
import { SynthDef } from "../../defs/synthdef.ts";
import { bufRd, control, out, transportPos } from "../../defs/ugens/index.ts";
import type { Multitrack, Region, Track } from "../../multitrack.ts";
import type { GuiHost } from "../host.ts";
import { Transport } from "../transport.ts";
import type { Bridge, MultitrackEditor } from "./multitrack.ts";

/**
 * The def every box is read by. One name for the whole client, because it is one
 * def: a box is a window onto a source and they differ by their controls.
 */
export const READER = "clausters.box";

/**
 * The controls a box cannot change without being started again: what it reads
 * and whether it wraps are read at the rate the reader is built with.
 */
export const FIXED = ["buf", "loop"] as const;

/** What one box is read with — the controls {@link boxArgs} answers. */
export type BoxArgs = Record<string, number>;

/**
 * The def a box sounds through: a buffer read at the transport's position.
 *
 * No position of its own — seeking, looping and pausing are the transport's, and
 * moving a box is one `/node_set` of `at`. `transportPos` is the transport's
 * position minus where the box starts, so the reader is at frame 0 when the
 * transport reaches it, and the gate is the box's length (`bufRd` clamps past
 * the end instead of going quiet).
 *
 * **A box is a window onto a source, and it reads from where the window opens.**
 * `start` is that frame, so trimming the left edge or splitting a box makes the
 * piece play what the picture shows: without it every box would read from frame
 * zero and both halves of a split would play the beginning.
 */
export function reader(name = READER): SynthDef {
    const buf = control("buf", 0.0, { rate: "ir" });
    const at = control("at", 0.0);           // where it starts, in transport frames
    const span = control("span", 0.0);       // how long it lasts, in frames
    const start = control("start", 0.0);     // the frame of the source its zero reads
    const wrap = control("loop", 0.0, { rate: "ir" });  // whether it wraps
    const amp = control("amp", 0.5, { lag: 0.02 });
    const pos = transportPos(at);
    const live = pos.ge(0.0).mul(pos.lt(span));
    const sig = bufRd(buf, 0.0, pos.add(start), wrap).mul(live).mul(amp);
    return new SynthDef(name, out(0.0, sig), out(1.0, sig));
}

/**
 * The readers of one piece, and the transport that moves them.
 *
 * Built by {@link MultitrackEditor} when it is given a server, and reachable as
 * its `playback`. Nothing here is subscribed to anything: the editor tells it
 * the piece changed, whoever changed it — this window's gesture, a second window
 * over the same piece, or a step of the history — and {@link Playback.sync} puts
 * the readers where the piece now says they are.
 */
export class Playback {
    readonly editor: MultitrackEditor;
    readonly server: Server;
    /** The level a box at full track level is read at. */
    readonly amp: number;
    /**
     * region id → `[node, what it was last set with]`. The controls are kept
     * beside the node because two of them are **initial-rate** — what a box
     * reads and whether it wraps are fixed when the reader is built — so telling
     * a change of those from a move means knowing what was sent.
     */
    readonly nodes = new Map<number, [Synth, BoxArgs]>();
    /**
     * **The group the transport governs**, and the call this rests on: from here
     * the engine freezes that subtree on a stop and thaws it on a play, with
     * every node's state intact. A group of its own and never the root, which
     * would freeze every sound the session has.
     */
    group: Group | null;
    /**
     * The piece's transport. `headClock: "piece"` says it once: the verbs become
     * the server's and the host draws the line from the engine's own position
     * instead of an anchor kept in step here.
     */
    readonly transport: Transport;

    constructor(
        editor: MultitrackEditor,
        { server, host = null, group, amp = 0.5 }: {
            server: Server;
            host?: GuiHost | null;
            group?: Group;
            amp?: number;
        },
    ) {
        this.editor = editor;
        this.server = server;
        this.amp = Number(amp);
        this.group = group ?? new Group({ server });
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
     * Send the def, bind the group the transport governs, and put the readers
     * where the piece says.
     *
     * The half of building one that talks to the server, which in a page is a
     * promise: the same three calls the script makes in its constructor, made
     * where they can be waited for.
     */
    async prepare(): Promise<this> {
        await reader().send(this.server);
        if (this.group !== null) await this.server.transportGroup(this.group);
        this.transport.locate(this.editor.cursor ?? 0.0);
        this.sync();
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

    // ---- the readers ----

    /**
     * Make what is drawn be what sounds.
     *
     * The whole of it, and it runs on every edit whoever made it. A box that went
     * away takes its node with it; one that moved is a set on the node that is
     * already sounding.
     */
    sync(): void {
        const seen = new Set<number>();
        for (const track of this.editor.structure.tracks) {
            const gain = this.amp * trackLevel(this.editor.structure, track);
            const lane = track.activeLane;
            for (const region of lane?.regions ?? []) {
                const args = boxArgs(this.editor.bridge, region, gain);
                if (args === null) continue;
                seen.add(region.id);
                const held = this.nodes.get(region.id);
                if (held === undefined) {
                    this.nodes.set(region.id, [this.start(args), args]);
                    continue;
                }
                const [node, sent] = held;
                // `buf` and `loop` are **initial-rate**: what a box reads and
                // whether it wraps are fixed when the reader is built, so a
                // change of either is a new reader and everything else is a set
                // on the one that is already sounding.
                if (FIXED.some((key) => args[key] !== sent[key])) {
                    node.free();
                    this.nodes.set(region.id, [this.start(args), args]);
                    continue;
                }
                const moved: BoxArgs = {};
                for (const [key, value] of Object.entries(args)) {
                    if (!(FIXED as readonly string[]).includes(key) && value !== sent[key]) {
                        moved[key] = value;
                    }
                }
                if (Object.keys(moved).length > 0) node.set(moved);
                this.nodes.set(region.id, [node, args]);
            }
        }
        for (const [id, [node]] of [...this.nodes]) {
            if (!seen.has(id)) {
                node.free();
                this.nodes.delete(id);
            }
        }
    }

    /** One reader, in the governed group. */
    private start(args: BoxArgs): Synth {
        return new Synth(READER, args, { target: this.group ?? undefined, server: this.server });
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

    /** Freeze the readers where they stand, with every node's state intact. */
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
     * Free the readers and the group they live in. The piece is untouched: what
     * a playback holds is nodes, and nodes are not the composition.
     */
    close(): void {
        for (const [node] of this.nodes.values()) node.free();
        this.nodes.clear();
        if (this.group !== null) {
            this.group.free();
            this.group = null;
        }
    }
}

/**
 * What one box is read with, or `null` for a box that cannot be read — one whose
 * source nobody loaded, or one that is a window onto something that is not a
 * source at all.
 *
 * The whole crossing from the piece to the readers, and it is where the two axes
 * meet: a box is placed in **beats** and read in **frames**, and the conversion
 * is the editor's bridge rather than a ratio written here.
 */
export function boxArgs(bridge: Bridge, region: Region, gain: number): BoxArgs | null {
    const window = windowOf(region);
    if (window === null) return null;
    const source = (window as { source?: { source?: number } }).source?.source;
    const bufnum = source === undefined ? -1 : bridge.sources.bufnum(source);
    if (bufnum < 0) return null;
    return {
        buf: bufnum,
        loop: region.content.looping ? 1.0 : 0.0,
        at: bridge.frameAt(region.position),
        span: bridge.framesOver(region.position, region.length),
        // **Where the window opens**, in the source's own frames: a trim of the
        // left edge and a split both move it, and a reader that ignored it would
        // play the beginning twice.
        start: Number((window as { start?: number }).start ?? 0.0) * bridge.rate,
        amp: region.muted ? 0.0 : gain,
    };
}

/**
 * What a track contributes: nothing when it is muted, nothing when another is
 * soloed, its level otherwise.
 *
 * The mixer's rules, and they are the client's because the **document** holds
 * the flags and never reads them — a level is not a fact about the piece, it is
 * what somebody set the knob to.
 */
export function trackLevel(piece: Multitrack, track: Track): number {
    const soloing = piece.tracks.some((t) => t.soloed);
    if (track.muted || (soloing && !track.soloed)) return 0.0;
    const level = (track.config as { level?: number } | null | undefined)?.level;
    return level === undefined ? 1.0 : Number(level);
}

/**
 * The window a region is, or `null` for a box that is a window onto something
 * else (a composite).
 */
function windowOf(region: Region): object | null {
    const content = region.content.write() as { window?: unknown };
    const window = content.window;
    return typeof window === "object" && window !== null ? window : null;
}
