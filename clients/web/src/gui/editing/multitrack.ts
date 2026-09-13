/**
 * Editing a **piece**: its vocabulary, its picture and its editor.
 *
 * The multitrack, as one of the three fundamental structures gets: a
 * {@link Domain} that turns the `multitrack` widget's `clips` and `lanes`
 * payloads into the crate's own vocabulary and back, a {@link View} that is one
 * `multitrack` widget, and an editor that is {@link Editor} with those two in it
 * and nothing else.
 *
 * **Nothing here derives the picture, and nothing here reads a gesture.** Both
 * are the crate's (`multitrackProps` and `editingIntake`), which is what
 * makes this client, the Python client and the standalone host draw the same
 * piece and read the same report: what a row and a box *are*, and what a list of
 * boxes *means*, are one rule each and not one per language. What this adds is
 * the two things only a client knows — the axis its window counts in, and which
 * server buffer a source was read into.
 *
 * **A report is the piece, so a gesture is however many edits it takes.** A
 * block drag says a move, a trim and a lane's new contents in one message; they
 * go through {@link Domain.payloads} and land as **one** entry, since they are
 * one thing a hand did.
 *
 * **Beats meet frames through the piece's own tempo map**, never through a
 * ratio: a position is the second it falls on times the rate, and a *length* is
 * the difference of two of those, because four beats last longer later than
 * earlier under a ritardando.
 *
 * @module
 */

import { TempoMap } from "../../base/time.ts";
import { MULTITRACK, editingStitch } from "../../document.ts";
import type { RecordedLeg, Selection } from "../../document.ts";
import { Multitrack } from "../../multitrack.ts";
import type { Region } from "../../multitrack.ts";
import type { Answer } from "./echo.ts";
import type { GuiNode } from "../guidef.ts";
import type { GuiHost, PropValue } from "../host.ts";
import type { WindowHandle } from "../handle.ts";
import type { Server } from "../../defs/server/index.ts";
import { Buffer, type Part } from "../../defs/buffer.ts";
import { Domain } from "./domain.ts";
import { Editor } from "./editor.ts";
import {
    MultitrackEditorCore, editingDefaultBpm,
} from "../../core/clausters_core_web.js";
import type { GenericEditorOptions } from "./editor.ts";
import { Playback } from "./playback.ts";
import { View } from "./view.ts";

/**
 * The tempo a piece that never said one is read at, in beats per second.
 *
 * It is the **reader's** default and not the document's: a piece that said no
 * tempo did not say one, and writing 120 into the format would be deciding a
 * musical question on its behalf. The number is the shared crate's
 * (`editingDefaultBpm`), the one every endpoint plays and draws a piece at —
 * the GUI host with no script behind it included.
 */
export function defaultTempo(): number {
    return editingDefaultBpm() / 60.0;
}

/**
 * The piece's beat→second function, with the reader's default where the piece
 * states nothing.
 *
 * One line, and a **binding** rather than a rule: the three decisions a run of
 * authored entries needs — a ramp reaching the next one, the default before the
 * first, an empty list being the default alone — are {@link TempoMap.fromChanges}'s,
 * in the crate that models tempo.
 */
export function tempoMap(piece: Multitrack): TempoMap {
    return (
        TempoMap.fromChanges(
            piece.tempo.map((t) => ({ beats: t.at, tempo: t.bpm / 60.0, ramp: t.ramp })),
            defaultTempo(),
        ) ?? new TempoMap(defaultTempo())
    );
}

/**
 * Which **server buffer** each of the piece's sources was read into.
 *
 * The one thing about a piece that is not in the piece: a document names a
 * source and a picture is drawn from a buffer, and only whoever loaded the
 * samples knows they are the same. It is a class rather than a map so both
 * directions have a name — a box is *drawn* from a buffer and *read back* into a
 * source.
 */
export class Sources {
    /**
     * source id → the buffer number it was read into, or **the object that
     * holds it** — a `Buffer`, a `Timeline`. Both are accepted because they
     * answer two different questions and a caller usually has the object: which
     * buffer to draw from is {@link Sources.bufnum}, and what a box **opens as**
     * is {@link Sources.structure}.
     */
    readonly buffers: Map<number, number | object>;

    constructor(
        buffers?:
            | Iterable<readonly [number, number | object]>
            | Record<number, number | object>,
    ) {
        this.buffers =
            buffers === undefined
                ? new Map()
                : buffers instanceof Map
                  ? new Map(buffers)
                  : Symbol.iterator in Object(buffers)
                    ? new Map(buffers as Iterable<readonly [number, number | object]>)
                    : new Map(
                          Object.entries(buffers as Record<number, number | object>).map(
                              ([source, held]) =>
                                  [
                                      Number(source),
                                      typeof held === "object" ? held : Number(held),
                                  ] as const,
                          ),
                      );
    }

    /**
     * The buffer a source was read into; `-1` for one nobody loaded.
     *
     * **Negative and not zero**, because buffer 0 is a buffer — the first one an
     * allocator hands out. A source given as an object answers with the buffer
     * it holds, and one that holds none is a box with no samples to draw, which
     * is honest rather than empty.
     */
    bufnum(source: number | undefined | null): number {
        if (source === undefined || source === null) return -1;
        const held = this.buffers.get(Math.trunc(source));
        if (held === undefined) return -1;
        if (typeof held === "number") return held;
        // **Buffer 0 is a buffer**, and `|| -1` says it is not: the first buffer
        // an allocator hands out came back as "nobody loaded this", so the first
        // take a page loads was the one take its boxes could not draw. Asked for
        // explicitly instead — the same sentence the doc comment above has
        // always made.
        const bufnum = (held as { bufnum?: unknown }).bufnum;
        return bufnum === undefined || bufnum === null ? -1 : Math.trunc(Number(bufnum));
    }

    /**
     * **What a box over this source opens as** — the object a caller gave, or
     * `undefined` for a source it named by number alone.
     *
     * A piece names a source and an editor edits a structure; only whoever
     * loaded the samples holds both, which is the same reason this class exists
     * at all.
     */
    structure(source: number | undefined | null): object | undefined {
        if (source === undefined || source === null) return undefined;
        const held = this.buffers.get(Math.trunc(source));
        return typeof held === "object" ? held : undefined;
    }

    /**
     * The whole table as the instance plan reads it: source id →
     * `{ buffer, channels }`.
     *
     * The one fact about a piece that is not in the piece, handed to the crate
     * so it can say which slot a box goes in — a mono take is panned into its
     * track and a stereo one is balanced, and that follows from the source's
     * width and nothing else. A source nobody loaded is left out, and a box
     * over it is simply not playing yet.
     */
    table(): Record<string, { buffer: number; channels: number }> {
        const out: Record<string, { buffer: number; channels: number }> = {};
        for (const source of this.buffers.keys()) {
            const bufnum = this.bufnum(source);
            if (bufnum < 0) continue;
            const held = this.buffers.get(source);
            const channels =
                typeof held === "object" && held !== null
                    ? Number((held as { channels?: unknown }).channels ?? 1)
                    : 1;
            out[String(source)] = {
                buffer: bufnum,
                channels: Math.max(1, Math.trunc(channels || 1)),
            };
        }
        return out;
    }

    /**
     * The table as a **join** reads it: source id → `{ buffer, channels, frames }`.
     *
     * {@link Sources.table} plus the length, which a join needs and a plan does
     * not: a part that names no range contributes the whole of its source, and
     * only whoever loaded it knows how much that is.
     */
    held(): Record<string, { buffer: number; channels: number; frames: number }> {
        const out: Record<string, { buffer: number; channels: number; frames: number }> = {};
        for (const [source, entry] of Object.entries(this.table())) {
            const held = this.buffers.get(Number(source));
            const frames =
                typeof held === "object" && held !== null
                    ? Number((held as { frames?: unknown }).frames ?? 0)
                    : 0;
            out[source] = { ...entry, frames: Math.max(0, Math.trunc(frames || 0)) };
        }
        return out;
    }

    /**
     * How many channels a source has, `1` for one that does not say.
     *
     * What a **join** needs and the buffer number alone cannot answer: how wide
     * the assembled thing is follows from the takes it is over.
     */
    width(source: number | undefined): number {
        if (source === undefined) return 1;
        const held = this.buffers.get(Math.trunc(source));
        const channels =
            typeof held === "object" && held !== null
                ? Number((held as { channels?: unknown }).channels ?? 1)
                : 1;
        return Math.max(1, Math.trunc(channels || 1));
    }

    /** The source a buffer number came from, or `undefined`. */
    source(bufnum: number): number | undefined {
        for (const [source, held] of this.buffers) {
            const number =
                typeof held === "number"
                    ? held
                    : Number((held as { bufnum?: unknown }).bufnum ?? NaN);
            if (number === Math.trunc(bufnum)) return source;
        }
        return undefined;
    }
}

/**
 * What a client adds to the crate's picture: an axis and a buffer table.
 *
 * Held by the domain and the view alike, because both cross the same seam — one
 * drawing a box and the other reading one back — and two copies of the scale is
 * how a box comes back somewhere it was not put.
 */
export class Bridge {
    rate: number;
    sources: Sources;
    tempo: TempoMap;
    /**
     * The server the takes are on, for the one thing an edit needs one for: **a
     * source an edit makes**. A join owns no samples, so what reaches the server
     * is the list of spans and never the audio.
     */
    server?: Server;
    /**
     * The tempo, in beats per minute, a piece that states none is read at. The
     * reader's own: a piece that never said a tempo did not say one, and a
     * document that invented 120 would be deciding a musical question.
     */
    bpm: number;

    constructor(
        piece: Multitrack,
        sampleRate: number,
        sources?: Sources,
        server?: Server,
    ) {
        this.rate = Number(sampleRate);
        this.sources = sources ?? new Sources();
        this.tempo = tempoMap(piece);
        this.bpm = defaultTempo() * 60.0;
        this.server = server;
    }

    /** Re-read the tempo map, for an edit that moved one. */
    refresh(piece: Multitrack): void {
        this.tempo = tempoMap(piece);
    }

    /** Where a beat falls on the timeline, in frames. */
    frameAt(beats: number): number {
        return this.tempo.secsAt(beats) * this.rate;
    }

    /**
     * How long a stretch of beats lasts there — **the difference of two
     * positions**, because four beats are not one length.
     */
    framesOver(start: number, length: number): number {
        return this.tempo.spanSecs(start, start + length) * this.rate;
    }

    /** The beat a frame falls on: the inverse, and the way an edit comes back. */
    /**
     * Where a beat measured **from `base`** falls, in frames from `base` — what
     * a box's own axis counts in.
     *
     * A layer is drawn inside its box, so its break-points are the box's own
     * time and not the timeline's. That is a *length* from the box's start,
     * which is why it goes through {@link Bridge.framesOver} rather than
     * {@link Bridge.frameAt}: four beats are not one length under a tempo that
     * moves.
     */
    frameIn(base: number, at: number): number {
        return this.framesOver(base, at);
    }

    /**
     * The inverse: the beat, measured from `base`, that a frame from `base`
     * falls on.
     */
    beatIn(base: number, frame: number): number {
        return this.beatAt(this.frameAt(base) + Number(frame)) - Number(base);
    }

    beatAt(frame: number): number {
        return this.tempo.beatsAt(frame / (this.rate || 1.0));
    }
}

/**
 * A piece's vocabulary, as the **history** walks it.
 *
 * It reads no gesture and decides no edit: a gesture is the editor's turn, and
 * the turn is the crate's (`MultitrackEditorCore`). What is left is what the
 * history registers a structure for — putting a step back onto the piece — and
 * that goes through the same editor, so an undo and an edit apply by one rule.
 */
export class MultitrackDomain extends Domain<Multitrack> {
    override readonly name = MULTITRACK;
    readonly bridge: Bridge;
    /** The editor whose core applies a step, set by the editor this was made for. */
    editor: MultitrackEditor | null = null;

    constructor(bridge: Bridge) {
        super();
        this.bridge = bridge;
    }

    /** The piece as the crate holds it. */
    state(piece: Multitrack): unknown {
        return piece.write();
    }

    /**
     * Never asked: an edit's inverse is read by the editor's core, which answers
     * the entry to record.
     */
    current(_piece: Multitrack, _payload: unknown): unknown {
        return undefined;
    }

    project(piece: Multitrack, payload: unknown): boolean {
        // **A source the edit makes is made before the edit lands.** A join
        // over fragments mints the source its box is a window onto, and a box
        // over a source nothing answers for is left out of the plan — so
        // realizing it after the piece already names it would be one pass of
        // silence. It runs again on a redo, which is right: the source is gone
        // the moment nothing windows it.
        this.mint((payload as { source?: unknown })?.source);
        if (this.editor === null) return false;
        const applied = this.editor.applyStep(payload);
        if (applied.applied !== true) return false;
        this.writeBack(piece, applied.piece as Record<string, unknown>);
        return true;
    }

    /**
     * Write a piece the crate answered onto **the object the page holds**: a
     * piece handed back would be a second piece, and the caller's would go
     * stale.
     *
     * @internal
     */
    writeBack(piece: Multitrack, state: Record<string, unknown>): void {
        const written = Multitrack.read(state);
        piece.version = written.version;
        piece.tracks = written.tracks;
        piece.tempo = written.tempo;
        piece.meter = written.meter;
        piece.markers = written.markers;
        piece.loopSpan = written.loopSpan;
        piece.punch = written.punch;
        // A tempo that moved changes where every box is drawn.
        this.bridge.refresh(piece);
    }

    /**
     * Install a source an edit made, and put it in the table.
     *
     * The one place a client answers a document's statement with a server
     * command. A join owns no samples — it is spans of the takes the table
     * already holds — so this costs the list of parts and not the audio, and
     * freeing a take something is stitched over does not silence it.
     *
     * A part whose source nobody loaded leaves the join unmade rather than half
     * made: a box over it draws empty and does not play, which is what a source
     * nobody answered for has always meant here.
     *
     * **Where the two clients differ, and it is the platform's line**: the same
     * call in the same place, but a page cannot block, so the table is written
     * when the promise settles rather than on the line that sends. The command
     * goes out either way at the same point in the same order — `wait: false`,
     * because a join has nothing to copy and nothing to load, so there is
     * nothing to wait for but the round trip itself.
     *
     * @internal
     */
    mint(minted: unknown): void {
        const server = this.bridge.server;
        if (server === undefined || minted === null || typeof minted !== "object") return;
        const id = (minted as { id?: number }).id;
        if (id === undefined) return;
        // **What the join is comes from the crate** (`editingStitch`): its
        // width, its spans and the channel map a narrow part fills it with, read
        // once for every endpoint that realizes one. What is left here is the
        // command and the table.
        const made = editingStitch(minted as Record<string, unknown>, this.bridge.sources.held());
        if (made === undefined) return;
        const parts: Part[] = made.parts.map((part) => ({
            source: part.buffer,
            start: part.start,
            frames: part.frames,
            fadeIn: part.fadeIn,
            fadeOut: part.fadeOut,
            channels: part.channels,
        }));
        void Buffer.stitch(parts, {
            channels: made.channels,
            sampleRate: made.rate,
            wait: false,
            server,
        }).then((buffer) => {
            this.bridge.sources.buffers.set(Math.trunc(id), buffer);
        });
    }
}
/**
 * One `multitrack` widget: the whole piece, in one of them.
 *
 * A row per track and a box per region — the crate's own mapping, crossed to
 * this window's axis. The widget draws its own headers and its own vertical
 * scroll, so there is no stack to compose and nothing per clip to register.
 *
 * **The one thing it does not draw is the ruler**, and an editor is where a
 * position is read, so the view places a {@link timeruler} above it: a strip of
 * its own, in the same navigation group as the piece, so it labels exactly what
 * the lanes show and its ticks stand over the samples they name. It rules from
 * above, so its marks hug its bottom edge (`dir: "down"`, the default there).
 */
/**
 * The names the transport row's three widgets carry. A name and not an id,
 * because these are the widgets a **hand** addresses and a handler is hung on a
 * name — and they are the piece's own, so a page's `extra` may carry anything it
 * likes beside them.
 */
export const REWIND = "piece_rewind";
export const PLAY = "piece_play";
export const STOP = "piece_stop";
export const CLOCK = "piece_clock";

/**
 * How often the read-out asks the engine where the piece is, in seconds. The
 * *line* asks nothing — the host draws it from the segment every frame — so this
 * is the price of the number beside it and nothing else.
 */
export const CLOCK_TICK = 0.05;

export class MultitrackView extends View<Multitrack> {
    readonly bridge: Bridge;
    /** The navigation group the view joins, so a ruler beside it rules it. */
    link: number | undefined;
    /**
     * The id of the strip that rules the piece, once one has been built. Kept
     * so a correction addressed to it answers with the *ruler's* props and not
     * with the piece's.
     */
    ruler: number | null = null;
    /**
     * The id of the piece's own widget, once one has been built — what a
     * playhead is drawn on, so whoever moves the line does not have to guess
     * which of the two ids is the picture.
     */
    piece: number | null = null;
    /**
     * Whether the window carries the transport row. It is the *view's* and not a
     * page's `extra`: a piece that can be heard is played from the window it is
     * drawn in, and every window over a piece has the same three controls in the
     * same place.
     */
    transport: boolean;

    constructor(bridge: Bridge, link?: number, transport = false) {
        super();
        this.bridge = bridge;
        this.link = link;
        this.transport = Boolean(transport);
    }

    build(editor: Editor<Multitrack>): GuiNode {
        // **The window is the application's**, composed in the shared crate
        // (`MultitrackEditorCore`), so this page, the Python client and the
        // standalone host open the same one. What is left here is the two ids a
        // hand's gestures come back on.
        //
        // **The ruler is named like any other widget of this picture**, so what
        // a hand does on it comes back to this editor: the position cursor is
        // placed on the ruler and nowhere else, and an unnamed strip would put
        // that one gesture outside the only object that could hear it.
        const wid = this.widget(editor, "multitrack", editor.structure);
        const rid = this.widget(editor, "ruler", editor.structure, "ruler");
        this.ruler = rid;
        this.piece = wid;
        const ed = editor as MultitrackEditor;
        ed.syncCore();
        const tree = ed.coreCall("window", { widget: wid, ruler: rid }) as unknown as GuiNode;
        // **A page's own widgets are its objects**, and a widget built over a
        // live source keeps a binding no JSON carries — so they are appended
        // here rather than composed in the crate.
        tree.children = [...(tree.children ?? []), ...editor.extra];
        return tree;
    }

    override props(editor: Editor<Multitrack>, widgetId: number): Record<string, PropValue> {
        const ed = editor as MultitrackEditor;
        ed.syncCore();
        return ed.coreCall("props", { widget: widgetId }) as Record<string, PropValue>;
    }
}

/** What one turn of the editor's core came to. */
interface Outcome {
    turn?: string;
    answer?: Answer;
    seq?: number;
    redo?: boolean;
    record?: { label: string; legs: { forward: unknown; backward: unknown; key: string }[] };
    changed?: boolean;
    version?: number;
    piece?: Record<string, unknown>;
    minted?: unknown[];
    locate?: number;
    selection?: Record<string, unknown>;
    enter?: string;
}

/**
 * An event's arguments as JSON carries them. A blob belongs to a widget this
 * editor did not draw, and it crosses as nothing.
 */
function plain(value: unknown): unknown {
    if (value instanceof ArrayBuffer || ArrayBuffer.isView(value)) return null;
    if (Array.isArray(value)) return value.map(plain);
    return value;
}

/** What {@link MultitrackEditor} is built with, beside a generic editor's. */
export interface MultitrackEditorOptions extends GenericEditorOptions<Multitrack> {
    /** Which server buffer each source was read into. */
    sources?:
        | Sources
        | Iterable<readonly [number, number | object]>
        | Record<number, number | object>;
    /** The navigation group the view joins. */
    link?: number;
    /**
     * The server the piece **sounds** on. Given one, the editor keeps a reader
     * per box in a group the transport governs and draws the transport row; a
     * piece opened with none still edits, which is why it is an option and not a
     * requirement.
     */
    server?: Server;
}

/**
 * A piece on screen, editable back into the `Multitrack` the caller already
 * holds.
 *
 * Nothing is handed back at the end: the object the page passed in *is* the
 * edited one, and reading it after an edit is how a caller sees what a hand did.
 * Being an {@link Editor}, it has the history every other editor has — `undo`
 * and `redo` walk it, and a second window over the same piece walks the same
 * one.
 */
export class MultitrackEditor extends Editor<Multitrack> {
    /**
     * The axis and the buffer table this window crosses to — the two things
     * about a piece that are not in the piece.
     */
    readonly bridge: Bridge;

    /**
     * The editors a hand opened by entering a box, by box name — held so a
     * second double click on the same box raises the one that is already open
     * rather than a second window over one structure.
     */
    readonly entered = new Map<string, Editor<never>>();

    /**
     * What the piece **sounds** as, when it can be heard at all: a
     * {@link Playback} over the server this was given, and `null` for a piece
     * opened with none.
     */
    playback: Playback | null = null;

    /** The last read-out written, so an unchanged one is not written again. */
    private shown: string | null = null;

    constructor(piece: Multitrack, options: MultitrackEditorOptions) {
        const { sources, link, server, title = "Multitrack", ...rest } = options;
        const bridge = new Bridge(
            piece,
            Number(options.sampleRate),
            sources instanceof Sources ? sources : new Sources(sources),
            server,
        );
        const domain = new MultitrackDomain(bridge);
        super(piece, {
            ...rest,
            title,
            tempoMap: bridge.tempo,
            domain,
            view: new MultitrackView(bridge, link, server !== undefined),
        });
        this.bridge = bridge;
        domain.editor = this;
        // **The editor's turns, in the shared crate**: what a message is, what
        // a gesture does to the piece, the window, and the answer.
        this.core = new MultitrackEditorCore(JSON.stringify({
            piece: piece.write(),
            rate: this.bridge.rate,
            defaultBpm: this.bridge.bpm,
            version: this.version,
            link: link ?? null,
            transport: server !== undefined,
            title,
            w: this.size[0],
            h: this.size[1],
        }));
        if (server !== undefined) {
            this.playback = new Playback(this, { server, host: this.host });
        }
    }

    /** The editor's turns, in the shared crate. */
    private readonly core: MultitrackEditorCore;

    /**
     * The id of the piece's own widget — what a playhead is drawn on. `null`
     * before the picture has been drawn once.
     */
    get pieceWidget(): number | null {
        return (this.view as MultitrackView | null)?.piece ?? null;
    }

    // ---- the crate's turns ----

    /**
     * One verb of the core, with its arguments; the answer, parsed.
     *
     * @internal
     */
    coreCall(verb: string, args: Record<string, unknown> = {}): Record<string, unknown> {
        return JSON.parse(this.core.call(JSON.stringify({ verb, ...args }))) as Record<
            string,
            unknown
        >;
    }

    /**
     * Hand the core what this page holds: the piece a page may have changed,
     * the buffer table, the meters, the cursor and the window.
     *
     * @internal
     */
    syncCore(): void {
        const meters: { track: number; bus: number; channels: number }[] = [];
        for (const [track, [bus, channels]] of this.playback?.meters ?? []) {
            meters.push({ track, bus, channels });
        }
        this.coreCall("sync", {
            piece: this.structure.write(),
            sources: this.bridge.sources.table(),
            meters,
            cursor: this.cursor ?? null,
            window: this.windowId,
        });
    }

    /**
     * One payload of a history step, applied by the core.
     *
     * @internal
     */
    applyStep(payload: unknown): Record<string, unknown> {
        this.syncCore();
        return this.coreCall("apply", { payload });
    }

    protected override deliver(addr: string, rawArgs: readonly unknown[]): boolean {
        this.syncCore();
        const outcome = this.coreCall("event", {
            addr,
            args: plain([...rawArgs]),
            version: this.version,
        }) as Outcome;
        if (outcome.turn === "closed") return this.closedWindow();
        if (outcome.turn === "step") {
            // **What it answers is whether anything moved**, and a step nobody
            // could apply says why: the entry named a structure nothing in this
            // context can write to, and it is still there rather than stepped
            // over.
            const stepped = outcome.redo === true ? this.redo() : this.undo();
            const reason = !stepped && this.app.unreachable !== null
                ? `${this.app.unreachable}: nothing here can put that edit back`
                : null;
            this.echo.send(this.coreCall("acknowledge", {
                seq: outcome.seq ?? 0,
                version: this.version,
                reason,
            }) as unknown as Answer);
            return stepped;
        }
        return this.take(outcome);
    }

    /**
     * One `/gui_event` payload, with the stamp already taken off: the same turn
     * as a message, unstamped.
     */
    protected override route(args: readonly unknown[]): boolean {
        this.syncCore();
        const [wid, tag, ...values] = args;
        return this.take(this.coreCall("event", {
            addr: "/gui_event",
            args: plain([wid, 0, 0, tag, ...values]),
            version: this.version,
        }) as Outcome);
    }

    /**
     * Carry out what a turn came to, and answer the host. Answers whether the
     * piece changed.
     */
    private take(outcome: Outcome): boolean {
        if (outcome.turn === undefined || outcome.turn === "nothing") return false;
        const domain = this.domain as MultitrackDomain;
        for (const minted of outcome.minted ?? []) domain.mint(minted);
        const changed = outcome.changed === true;
        if (changed) {
            const record = outcome.record;
            if (record !== undefined) {
                this.editing.history.record(
                    record.legs.map((leg) => ({ structure: this.registered(), ...leg }) as RecordedLeg),
                    { label: record.label },
                );
            }
            domain.writeBack(this.structure, outcome.piece ?? {});
            this.version = outcome.version ?? this.version;
            this.dirty = true;
            this.editing.changed();
        }
        if (outcome.locate !== undefined) {
            // **Whoever has the transport is told**: this editor, and the piece
            // it is composed inside when it is one.
            this.cursor = outcome.locate;
            this.locate(this.cursor);
            this.composedIn?.locate(this.cursor);
            this.onLocate?.(this.cursor);
        }
        if (outcome.selection !== undefined) {
            this.selection = outcome.selection as unknown as Selection;
        }
        if (outcome.enter !== undefined) void this.enter(outcome.enter);
        this.echo.send(outcome.answer);
        return changed;
    }

    /** Draw what a history walk left behind: every widget corrected, the host told once. */
    override reflectStep(): void {
        this.dirty = true;
        this.syncCore();
        this.echo.send(this.coreCall("resync", { version: this.version }) as unknown as Answer);
    }

    /** Another view of this piece edited it: bring this window in step. */
    override adopt(): void {
        if (this.host === null || this.windowId === null) return;
        this.syncCore();
        this.echo.send(this.coreCall("resync", { version: this.version }) as unknown as Answer);
    }

    // ---- the piece, heard ----

    /**
     * Open the window, and hang the transport row on the playback.
     *
     * The wiring is here rather than in the constructor because that is where
     * the window comes into being: {@link edit} builds the editor and opens it
     * in two steps, so a piece has its readers before it has a screen — which is
     * the right order anyway, since a piece can be played by a page that never
     * draws it.
     */
    override async open(
        host?: GuiHost,
        options: { id?: number; stage?: unknown } = {},
    ): Promise<WindowHandle> {
        const handle = await super.open(host, options);
        const playback = this.playback;
        if (playback !== null) {
            await playback.prepare();
            playback.attach(this.host);
            handle.widget(REWIND).onClick(() => this.rewind());
            handle.widget(PLAY).onClick(() => void this.toggle());
            handle.widget(STOP).onClick(() => this.stop());
            void this.tick();
        }
        return handle;
    }

    /**
     * The read-out, and the one round trip: the position is the engine's. It
     * schedules itself until the window is gone, which is how it stops without
     * anybody stopping it.
     */
    private async tick(): Promise<void> {
        const playback = this.playback;
        if (this.closed || playback === null || this.windowHandle === null) return;
        await playback.refresh();
        const text = `${playback.position.toFixed(3).padStart(8)} s   of ` +
            `${this.structure.end.toFixed(3)} s`;
        if (text !== this.shown) {
            this.windowHandle.widget(CLOCK).set({ text });
            this.shown = text;
        }
        setTimeout(() => void this.tick(), CLOCK_TICK * 1000);
    }

    /**
     * Play, or pause where it stands. A pause freezes the governed group, so
     * playing again continues rather than starting over.
     */
    async toggle(): Promise<void> {
        const playback = this.playback;
        if (playback === null) return;
        await playback.refresh();
        if (playback.playing) playback.pause();
        else await playback.play();
    }

    /**
     * Put the **position cursor** back at the top, and cue a stopped transport
     * there.
     *
     * The cursor's own verb, not the transport's: it is where the next play
     * starts, and stop goes back to it rather than to the top. A hand that has
     * been working at bar forty otherwise has to find beat zero on screen to
     * get back to it.
     */
    rewind(): void {
        this.cursor = 0.0;
        this.locate(0.0);
        // The host owns where the cursor *is*, so it is told rather than left
        // to find out on the next redraw — the same way the transport tells it
        // where the playhead stands.
        const piece = this.pieceWidget;
        if (this.host !== null && piece !== null) {
            this.host.set(piece, { cursor: 0.0 });
        }
    }

    /** Play the piece from where the position cursor is. */
    async play(): Promise<void> {
        await this.playback?.play();
    }

    /** Freeze the piece where it stands. */
    pause(): void {
        this.playback?.pause();
    }

    /** Halt and go back to the mark the position cursor is on. */
    stop(): void {
        this.playback?.stop();
    }

    /**
     * The position cursor was placed, here or in a window entered from here: cue
     * a stopped transport there and leave a rolling one alone.
     *
     * This is what a box's own ruler reaches, because a structure inside a piece
     * has no transport of its own — the piece is the one that has one.
     */
    override locate(beat: number): void {
        this.playback?.cue(beat);
    }

    /**
     * The piece changed, whoever changed it: put the readers where it now says
     * they are, and then tell the page.
     *
     * The readers go first because the page's own handler may look at what is
     * sounding, and because a piece is a statement: making it true again is not
     * a reaction to an edit, it is the same call the first one was.
     */
    override dataChanged(): void {
        this.playback?.sync();
        this.answerWithThePicture();
        super.dataChanged();
    }

    /**
     * **A name the host minted is answered with the one the piece kept.**
     *
     * The host mints the word for a track it made or a box it split, and the
     * document mints the id; the crate compares what the host was last told
     * with what the piece now holds and answers with the picture when they
     * differ (`settle`). It runs after the readers are synced, so a minted
     * source's box draws.
     */
    private answerWithThePicture(): void {
        if (this.host === null || this.windowId === null) return;
        this.syncCore();
        this.echo.send(this.coreCall("settle", { version: this.version }) as unknown as Answer);
    }

    /**
     * Open the contents of the box called `name` in an editor of its own, and
     * hand it back (`null` for a box with nothing to open).
     *
     * **The multitrack places; a box is entered to edit.** What a box holds is
     * a structure like any other — a take's samples, a timeline of notes — so
     * entering one is {@link edit} over that structure, with no second
     * implementation of any editor.
     *
     * **One undo order, and it is the piece's.** The editor is opened on this
     * piece's editing context, so a note written inside a box and a box dragged
     * on the stack walk one history: an undo that needed a window reopened to
     * reach it is a hole in the order that does not announce itself. What that
     * costs is that the entered structure stays in the context while the piece
     * is open even if its window is closed — which the context already does,
     * since it holds what it registered.
     *
     * **And one window set**, which is the piece's {@link Application}: a box
     * entered out of a piece is part of looking at the piece, so it draws on the
     * same host, names widgets in the same id space and walks the same order
     * without resolving anything of its own. Its **acknowledgement stays its
     * own** — an {@link Echo} is one view's end of the conversation, and a box
     * sharing the piece's floor would silence the piece's staleness check every
     * time a hand edited inside the box.
     *
     * The object comes from {@link Sources}, which is where the one fact about
     * a piece that is not in the piece already lives: the document names a
     * source and only whoever loaded it holds the structure.
     */
    async enter(name: string): Promise<Editor<never> | null> {
        const { edit } = await import("./edit.ts");
        const found = this.entered.get(name);
        if (found !== undefined) return found;
        const region = regionNamed(this.structure, name);
        if (region === null) return null;
        const held = this.bridge.sources.structure(sourceOf(region));
        if (held === undefined) return null;
        const opened = await edit(held, {
            sampleRate: this.bridge.rate,
            context: this.editing,
            app: this.app,
            host: this.host ?? undefined,
            title: String(region.name ?? name),
            // **On the host the piece is on, or on no screen at all.** A piece
            // that was never opened has no window to enter one *from*, and
            // resolving an ambient host there would put a box on screen while
            // the piece it belongs to is not.
            open: this.host !== null,
        });
        // **The piece is what this window is composed inside**, which is what a
        // ruler clicked in there needs: a take has no transport of its own, so
        // the position it places is the piece's to act on.
        opened.composedIn = this as Editor;
        this.entered.set(name, opened);
        // **A window the reader closed is enterable again**, and it is the only
        // way one leaves this table: an editor that is merely not on screen is
        // still the one that box is open in, so a second double click raises it
        // rather than making a second editor over one structure.
        if (this.host !== null) opened.onClosed(() => this.entered.delete(name));
        return opened;
    }

    /**
     * Close this piece's window, and the boxes opened out of it with it.
     *
     * A window entered *from* the piece is part of looking at the piece: what
     * outlives both is the history, which is the data's and was never a
     * window's.
     */
    override close(): this {
        this.playback?.close();
        for (const opened of [...this.entered.values()]) {
            if (!opened.closed) opened.close();
        }
        // The handlers cleared their own entries; this is for the ones that
        // were never on screen to clear.
        this.entered.clear();
        return super.close();
    }
}

/**
 * The region of this name, wherever it is, and `null` for a box the piece has
 * none of.
 *
 * A box is named by its region's id, so the name is the address — the same
 * thing that makes a report readable with no map on the side.
 */
function regionNamed(piece: Multitrack, name: string): Region | null {
    const wanted = Number(name);
    if (!Number.isFinite(wanted)) return null;
    for (const track of piece.tracks) {
        for (const lane of track.lanes) {
            for (const region of lane.regions) {
                if (Number(region.id) === Math.trunc(wanted)) return region;
            }
        }
    }
    return null;
}

/**
 * The source a region is a window onto, or `undefined` for a box that is a
 * window onto something else.
 */
function sourceOf(region: Region): number | undefined {
    const content = region.content as unknown as { write?(): unknown };
    const written = (typeof content?.write === "function" ? content.write() : content) as
        | Record<string, unknown>
        | undefined;
    const onto = (written?.window ?? {}) as Record<string, unknown>;
    const source = ((onto.source ?? {}) as Record<string, unknown>).source;
    return source === undefined || source === null ? undefined : Number(source);
}

/** Whether {@link edit} should open this as a multitrack. */
export function isPiece(structure: unknown): structure is Multitrack {
    return structure instanceof Multitrack;
}
