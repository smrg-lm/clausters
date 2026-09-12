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
import { MULTITRACK, domainEdit } from "../../document.ts";
import type { Curve, Curved } from "../../multitrack.ts";
import {
    Multitrack, multitrackProps,
} from "../../multitrack.ts";
import type { Box, Placed, Region, Row, Strip } from "../../multitrack.ts";
import { button, label, layout, node, timeruler, window as guiWindow } from "../guidef.ts";
import type { GuiNode } from "../guidef.ts";
import type { GuiHost, PropValue } from "../host.ts";
import type { WindowHandle } from "../handle.ts";
import type { Server } from "../../defs/server/index.ts";
import { Domain } from "./domain.ts";
import { Editor } from "./editor.ts";
import type { GenericEditorOptions } from "./editor.ts";
import { Playback } from "./playback.ts";
import { View } from "./view.ts";

/**
 * What the widget's `lanes` prop takes: flat `name label height mute solo gain`
 * sextuples.
 */
const SEXTUPLE = 6;

/**
 * What its `clips` prop takes: flat `name lane offset dur start label source`
 * septuples.
 */
const SEPTUPLE = 7;

/** The thickness a row is drawn at, in logical pixels. */
const ROW_H = 96.0;

/**
 * The thickness an automation row is drawn at — shorter than a track's row,
 * because what it draws is one line and not a stack of boxes.
 */
const CURVE_H = 40.0;

/**
 * The tempo a piece that never said one is read at, in beats per second — one,
 * so a beat is a second. It is the **reader's** default and not the document's:
 * a piece that said no tempo did not say one, and writing 120 into the format
 * would be deciding a musical question on its behalf.
 */
export const DEFAULT_TEMPO = 1.0;

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
            DEFAULT_TEMPO,
        ) ?? new TempoMap(DEFAULT_TEMPO)
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
     * The tempo, in beats per minute, a piece that states none is read at. The
     * reader's own: a piece that never said a tempo did not say one, and a
     * document that invented 120 would be deciding a musical question.
     */
    bpm: number;

    constructor(piece: Multitrack, sampleRate: number, sources?: Sources) {
        this.rate = Number(sampleRate);
        this.sources = sources ?? new Sources();
        this.tempo = tempoMap(piece);
        this.bpm = DEFAULT_TEMPO * 60.0;
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
 * A piece's vocabulary: the crate's `MultitrackIntent`, both ways.
 *
 * It reads nothing itself. A gesture goes to `editingIntake`, which is the same
 * reading the standalone host does, and an edit is applied through `domainEdit`,
 * which is where the inverse comes from.
 */
export class MultitrackDomain extends Domain<Multitrack> {
    override readonly name = MULTITRACK;
    override readonly ingested = true;
    readonly bridge: Bridge;

    constructor(bridge: Bridge) {
        super();
        this.bridge = bridge;
    }

    /**
     * The report, the piece it is over, and the axis a beat lands on.
     *
     * A report of the boxes, the rows or the break-points is the **whole**
     * structure rather than the gesture, so the piece has to be in hand for the
     * reading to say what the difference is. The rate and the source table are
     * the same two the picture is drawn with, which is what keeps a box from
     * going out on one axis and coming back on another.
     */
    override request(
        piece: Multitrack,
        _tag: string,
        values: readonly unknown[],
    ): Record<string, unknown> {
        return {
            values: [...values],
            state: this.state(piece),
            rate: this.bridge.rate,
            defaultBpm: this.bridge.bpm,
            sources: this.bridge.sources.table(),
        };
    }

    // ---- the state, and writing one back ----

    /**
     * The piece as the crate holds it — what {@link MultitrackDomain.current} is
     * read against and what {@link MultitrackDomain.project} writes back.
     */
    state(piece: Multitrack): unknown {
        return piece.write();
    }

    current(piece: Multitrack, payload: unknown): unknown {
        return domainEdit(this.name, this.state(piece), payload)?.current;
    }

    project(piece: Multitrack, payload: unknown): boolean {
        const edited = domainEdit(this.name, this.state(piece), payload);
        if (edited === undefined || !edited.applied) return false;
        const written = Multitrack.read(edited.state as Record<string, unknown>);
        // The object the page holds **is** the edited one: a piece handed back
        // would be a second piece, and the caller's would go stale.
        piece.version = written.version;
        piece.tracks = written.tracks;
        piece.tempo = written.tempo;
        piece.meter = written.meter;
        piece.markers = written.markers;
        piece.loopSpan = written.loopSpan;
        piece.punch = written.punch;
        // A tempo that moved changes where every box is drawn.
        this.bridge.refresh(piece);
        return true;
    }
}
/**
 * The position cursor in timeline samples: where the editor last saw it placed,
 * and the top of the piece until a hand places one.
 */
function cursorOf(editor: Editor<Multitrack>): number {
    return editor.beatsToUnits(editor.cursor ?? 0.0);
}

function laneProps(rows: readonly Row[]): unknown[] {
    const out: unknown[] = [];
    for (const row of rows) {
        out.push(String(row.track), String(row.label ?? ""), ROW_H,
                 Boolean(row.mute), Boolean(row.solo), Number(row.gain ?? 1.0));
    }
    return out;
}

/**
 * The playback's meter buses as the widget's flat quadruples: the lane, the
 * first bus of the level run, the first of the mark run, and how many channels
 * each run is.
 */
function meterBuses(editor: Editor<Multitrack>): unknown[] {
    const playback = (editor as { playback?: { meters?: Map<number, [{ index: number }, number]> } })
        .playback;
    if (playback?.meters === undefined) return [];
    const out: unknown[] = [];
    for (const [track, [bus, channels]] of playback.meters) {
        out.push(String(track), bus.index, bus.index + channels, channels);
    }
    return out;
}

/**
 * The crate's **track automations** as the widget's flat sextuples: a row of
 * its own under the track it names.
 */
function curveProps(curves: readonly Curve[]): unknown[] {
    const out: unknown[] = [];
    for (const curve of curves) {
        const [lo, hi] = domainOf(curve);
        out.push(String(curve.automation), String(curve.owner),
                 String(curve.label ?? ""), lo, hi, CURVE_H);
    }
    return out;
}

/**
 * The crate's **region automations** as the widget's flat quintuples: a layer
 * inside the box it names, and no height, because it is as tall as that box.
 */
function layerProps(layers: readonly Curve[]): unknown[] {
    const out: unknown[] = [];
    for (const curve of layers) {
        const [lo, hi] = domainOf(curve);
        out.push(String(curve.automation), String(curve.owner),
                 String(curve.label ?? ""), lo, hi);
    }
    return out;
}

/**
 * The value range a curve is drawn over.
 *
 * **The page's, and read out of the target.** The document says what a curve
 * automates and never reads it; which range that parameter has — a gain over
 * one, a pan over another — is a fact about the parameter, so it is stated
 * where the parameter is. Unity is the default, which is what an unlabelled
 * level means.
 */
function domainOf(curve: Curve): [number, number] {
    const target = curve.target as Record<string, unknown> | undefined;
    if (!target || typeof target !== "object") return [0.0, 1.0];
    return [Number(target.min ?? 0.0), Number(target.max ?? 1.0)];
}

/**
 * Every curve's break-points as the widget's flat quintuples, each naming the
 * curve it is on — one list for the rows and the layers alike.
 */
function pointProps(
    curves: readonly Curve[],
    bridge: Bridge,
    bases: Map<string, number>,
): unknown[] {
    const out: unknown[] = [];
    for (const curve of curves) {
        const name = String(curve.automation);
        const base = bases.get(name) ?? 0;
        for (const point of curve.points ?? []) {
            const data = (point.data ?? {}) as Record<string, unknown>;
            out.push(name, bridge.frameIn(base, Number(point.at ?? 0)),
                     Number(point.value ?? 0),
                     Number(data.shape ?? 1), Number(data.curve ?? 0));
        }
    }
    return out;
}

/** The crate's boxes as the widget's flat septuples, on this axis. */
function clipProps(boxes: readonly Box[], bridge: Bridge): unknown[] {
    const out: unknown[] = [];
    for (const box of boxes) {
        const position = Number(box.position);
        const length = Number(box.length);
        out.push(
            String(box.region),
            String(box.row),
            bridge.frameAt(position),
            bridge.framesOver(position, length),
            Number(box.start ?? 0.0) * bridge.rate,
            String(box.label ?? ""),
            bridge.sources.bufnum(box.source),
        );
    }
    return out;
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
     * **What the host was last told things are called** — the rows and the
     * boxes, by name. See {@link props}.
     */
    told: readonly [ReadonlySet<string>, ReadonlySet<string>] | null = null;
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
        // **The props are already what the wire takes**, so the node is made
        // from them directly rather than through `guidef.multitrack`, whose
        // `lanes`/`clips` are the *tuples* a page types and which would flatten
        // an already-flat list a second time — one row per number. The flat form
        // is the one `props` has to answer in anyway, since a correction rides
        // as a `/gui_set`.
        const wid = this.widget(editor, "multitrack", editor.structure);
        // **The ruler is named like any other widget of this picture**, so what
        // a hand does on it comes back to this editor: the position cursor is
        // placed on the ruler and nowhere else, and an unnamed strip would put
        // that one gesture outside the only object that could hear it.
        const rid = this.widget(editor, "ruler", editor.structure, "ruler");
        this.ruler = rid;
        this.piece = wid;
        return guiWindow(
            { title: editor.title, w: editor.size[0], h: editor.size[1], layout: "col" },
            timeruler({
                id: rid,
                link: this.group(wid),
                ruler: "beats",
                cursor: cursorOf(editor),
                sampleRate: this.bridge.rate,
                tempoMap: this.bridge.tempo.dump(),
            }),
            node("multitrack", { id: wid, ...this.props(editor, wid) }),
            ...(this.transport ? this.chrome() : []),
            ...editor.extra,
        );
    }

    /**
     * The transport row: play/pause, stop, and where the piece is.
     *
     * Named rather than numbered, because these are the only widgets of this
     * window a *hand* addresses and a handler is hung on a name. The names are
     * the piece's own (`piece_*`), so a page's `extra` may carry anything it
     * likes beside them.
     */
    chrome(): GuiNode[] {
        // **Rewind is not stop.** Stop goes back to the *mark* — which is what
        // tells it from pause — and the mark is wherever a hand last put it, so
        // with nothing else the way back to the top is finding beat zero on
        // screen and clicking it. Rewind puts the mark there, which is a
        // statement about the cursor and not about the transport.
        return [layout(
            { flow: "row", h: 40.0, gap: 6.0 },
            button({ label: "|<", name: REWIND, w: 44.0 }),
            button({ label: "play/pause", name: PLAY, w: 110.0 }),
            button({ label: "stop", name: STOP, w: 110.0 }),
            label("", { name: CLOCK, textSize: 2.0, weight: 1.0 }),
        )];
    }

    /**
     * **The navigation group the piece and its ruler share.**
     *
     * A ruler rules by being on the same axis as what it is beside, and an
     * unlinked widget is a group of one keyed by itself — so the two would pan
     * and zoom apart. The piece's own widget id names the group when the caller
     * did not name one, which is the id nothing else can collide with.
     */
    group(widgetId: number): number {
        return this.link ?? widgetId;
    }

    override props(editor: Editor<Multitrack>, widgetId: number): Record<string, PropValue> {
        if (widgetId === this.ruler) {
            // The strip's own state, which is the axis' and nothing else: the
            // piece's payloads are the piece widget's.
            return { cursor: cursorOf(editor) };
        }
        const props: Record<string, PropValue> = {
            // **The piece's own props are the projection's**: the rows, the
            // boxes, the automations over both, their break-points, which of
            // them are hidden and which boxes loop. All of it is a function of
            // the piece and of where a beat lands, so all of it is written once
            // and every client and the standalone host ask the same question.
            ...(JSON.parse(
                multitrackProps(
                    JSON.stringify(editor.structure.write()),
                    this.bridge.rate,
                    this.bridge.bpm,
                    JSON.stringify(this.bridge.sources.table()),
                ),
            ) as Record<string, PropValue>),
            // **Where each track's level is read from**: the control buses its
            // meters write, which the host reads every frame straight out of
            // the shared segment. A piece with no playback names none, and a
            // header with nothing to read draws no strip.
            meters: meterBuses(editor),
            weight: 1.0,
            ruler: "beats",
            sample_rate: this.bridge.rate,
            // **The window is the reader's.** In an editor a content change is
            // mostly the reader's own edit, so the axis does not re-frame itself
            // on one; the extent is still registered.
            autofit: false,
            // The head is anchored at 0 because the counter it sweeps from is
            // already the piece's position.
            playhead_at: 0.0,
            // The piece's own map rules the beats, so the labels and the boxes
            // cannot disagree.
            tempo_map: this.bridge.tempo.dump(),
            // **A piece opens with the reader at the top.** The position cursor
            // is where a playback starts, so a piece that stated none would open
            // with nowhere to play from; and it is reported from the editor's
            // own copy rather than fixed at zero, or every resync would drag the
            // mark back to the start.
            cursor: cursorOf(editor),
        };
        props.link = this.group(widgetId);
        // **What the host was last told things are called.** A row or a box the
        // *host* made carries a word it minted (`track 1`, `white 2`); the id is
        // the document's and is minted when the report is read, so until the
        // picture goes back the two are naming the same thing differently — and
        // every later report about it names something the piece does not have,
        // which mints it **again**. `MultitrackEditor.dataChanged` compares this
        // with what the piece now holds and answers with the picture when they
        // differ.
        this.told = [
            new Set((props.lanes as unknown[]).filter((_v, i) => i % SEXTUPLE === 0)
                .map(String)),
            new Set((props.clips as unknown[]).filter((_v, i) => i % SEPTUPLE === 0)
                .map(String)),
        ];
        return props;
    }
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
        );
        super(piece, {
            ...rest,
            title,
            tempoMap: bridge.tempo,
            domain: new MultitrackDomain(bridge),
            view: new MultitrackView(bridge, link, server !== undefined),
        });
        this.bridge = bridge;
        if (server !== undefined) {
            this.playback = new Playback(this, { server, host: this.host });
        }
    }

    /**
     * The id of the piece's own widget — what a playhead is drawn on. `null`
     * before the picture has been drawn once.
     */
    get pieceWidget(): number | null {
        return (this.view as MultitrackView | null)?.piece ?? null;
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
     * A gesture is normally answered with an acknowledgement and nothing else,
     * because the report described the result: the host drew what it sent and
     * the piece agreed. The cases where it does not are the ones where the host
     * **makes** something — a track from a double click, a box from a split or
     * a paste. There the host mints the word (`track 1`, `white 2`) and the
     * document mints the id, so until the picture goes back the two are naming
     * the same thing differently.
     *
     * And a name the piece does not know is not ignored: it is read as
     * something *new*. So the next report about that row or that box mints it
     * again, and again after that — a split box took a fresh id on every drag,
     * losing whatever was hung on it, and a box dropped on a new track landed
     * on a track nobody had.
     *
     * So when the names the host was last told differ from the ones the piece
     * now holds, the whole picture goes back as a correction. It carries the
     * `meters` prop with it, which is the other half of the same fact for a
     * track: one that reached the server has buses to read, and a host that
     * never heard of it draws no strip.
     */
    private answerWithThePicture(): void {
        const view = this.view as MultitrackView | null;
        const piece = this.pieceWidget;
        if (this.host === null || this.windowId === null || piece === null) return;
        const told = view?.told ?? null;
        if (told === null) return;
        const rows = new Set(this.structure.tracks.map((track) => String(track.id)));
        const boxes = new Set(
            this.structure.tracks.flatMap((track) =>
                track.lanes.flatMap((lane) => lane.regions.map((r) => String(r.id)))),
        );
        const same = (a: ReadonlySet<string>, b: ReadonlySet<string>) =>
            a.size === b.size && [...a].every((name) => b.has(name));
        if (same(told[0], rows) && same(told[1], boxes)) return;
        this.corrections = [];
        this.resync(piece);
        this.acknowledge(0);
        this.corrections = [];
    }

    /**
     * **A box was entered** — the double click the multitrack reports as
     * `"enter"`, with the box's name.
     */
    protected override interface(
        _widgetId: number,
        tag: string,
        values: readonly unknown[],
    ): boolean {
        if (tag !== "enter" || values.length === 0) return false;
        void this.enter(String(values[0]));
        return true;
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
