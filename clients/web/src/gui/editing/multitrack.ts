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
 * are the crate's (`multitrackPicture` and `multitrackRead`), which is what
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
    Multitrack, multitrackPicture, multitrackRead, multitrackReadPoints, multitrackReadRows,
} from "../../multitrack.ts";
import type { Box, Placed, Region, Row, Strip } from "../../multitrack.ts";
import { node, timeruler, window as guiWindow } from "../guidef.ts";
import type { GuiNode } from "../guidef.ts";
import type { PropValue } from "../host.ts";
import { Domain } from "./domain.ts";
import { Editor } from "./editor.ts";
import type { GenericEditorOptions } from "./editor.ts";
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
 * What the widget's `points` prop takes and reports: flat
 * `curve t v shape amount` quintuples, each naming the curve it is on.
 */
const POINT_QUINTUPLE = 5;

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
        return Math.trunc(Number((held as { bufnum?: unknown }).bufnum ?? -1)) || -1;
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

    constructor(piece: Multitrack, sampleRate: number, sources?: Sources) {
        this.rate = Number(sampleRate);
        this.sources = sources ?? new Sources();
        this.tempo = tempoMap(piece);
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

/** A flat payload as groups of `n`; a trailing partial group is dropped rather
 * than half-read, the rule every flat payload here follows. */
function groups(values: readonly unknown[], n: number): unknown[][] {
    const out: unknown[][] = [];
    for (let i = 0; i + n <= values.length; i += n) out.push(values.slice(i, i + n));
    return out;
}

/**
 * A piece's vocabulary: the crate's `MultitrackIntent`, both ways.
 *
 * It reads nothing itself. A report of the boxes goes to `multitrackRead`, which
 * is the same reader the standalone host uses, and an edit is applied through
 * `domainEdit`, which is where the inverse comes from.
 */
export class MultitrackDomain extends Domain<Multitrack> {
    override readonly name = MULTITRACK;
    readonly bridge: Bridge;

    /** What an undo menu calls each of the piece's verbs. */
    static readonly LABELS: Record<string, string> = {
        placeregion: "move a clip",
        trimregion: "trim a clip",
        setlane: "edit the clips",
        settracks: "mix a track",
        splitregion: "split a clip",
        joinregions: "join the clips",
        setautomation: "draw a curve",
    };

    constructor(bridge: Bridge) {
        super();
        this.bridge = bridge;
    }

    // ---- a gesture, as edits ----

    override payloads(piece: Multitrack, tag: string, values: readonly unknown[]): unknown[] {
        if (tag === "clips") return multitrackRead(this.state(piece), this.placed(values));
        if (tag === "lanes") return this.strips(piece, values);
        if (tag === "points") {
            const state = this.state(piece);
            return multitrackReadPoints(
                state,
                this.curved(values, basesOf(multitrackPicture(state))),
            );
        }
        return [];
    }

    /**
     * The singular door, for the one-edit case. {@link MultitrackDomain.payloads}
     * is what a multitrack actually goes through: a report is the piece, so one
     * message is however many edits it takes.
     */
    payload(piece: Multitrack, tag: string, values: readonly unknown[]): unknown {
        const found = this.payloads(piece, tag, values);
        return found.length === 1 ? found[0] : null;
    }

    /**
     * The flat `clips` payload as the crate's boxes: names as they came,
     * positions in beats, the window's own numbers in seconds.
     */
    private placed(values: readonly unknown[]): Placed[] {
        const out: Placed[] = [];
        for (const group of groups(values, SEPTUPLE)) {
            const [name, lane, at, dur, start, , source] = group;
            const row = Number(String(lane));
            // A row is named by its track's id and never renamed, so a name that
            // is not one names no row this piece has.
            if (!Number.isFinite(row)) continue;
            const position = this.bridge.beatAt(Number(at));
            const length = this.bridge.beatAt(Number(at) + Number(dur)) - position;
            out.push({
                name: String(name),
                row: Math.trunc(row),
                position,
                length,
                start: Number(start) / (this.bridge.rate || 1.0),
                // How much a **new** box shows: the stretch it occupies, crossed
                // to the wall clock the only way a length may be.
                content: this.bridge.tempo.spanSecs(position, position + length),
                source: this.bridge.sources.source(Number(source)),
            });
        }
        return out;
    }

    /**
     * The flat `points` payload as the crate's curves: one entry per curve
     * named, its break-points back on the musical axis.
     *
     * The widget reports **every** curve there is, in one list, so they are
     * gathered by name here — the crate reads the difference and says nothing
     * about the ones that did not move.
     */
    private curved(values: readonly unknown[], bases: Map<string, number>): Curved[] {
        const found = new Map<string, Curved["points"]>();
        for (const group of groups(values, POINT_QUINTUPLE)) {
            const [name, at, value, shape, amount] = group;
            const points = found.get(String(name)) ?? [];
            // **Against the same base the picture was drawn from**: a layer's
            // time is its box's own, so a break-point inside one comes back as
            // a beat from that box's start.
            const base = bases.get(String(name)) ?? 0;
            points.push({
                at: this.bridge.beatIn(base, Number(at)),
                value: Number(value),
                // **What a shape is stays the page's**: the crate carries a
                // point's data and never reads it, which is what keeps an undo
                // from putting a bent curve back straight.
                data: { shape: Math.trunc(Number(shape)), curve: Number(amount) },
            });
            found.set(String(name), points);
        }
        return [...found].map(([name, points]) => ({ name, points }));
    }

    /**
     * The mixer's payload: mute, solo and the fader, in the piece's one verb
     * over a track.
     *
     * A strip saying what the track already says is not an edit, which is what
     * keeps one fader drag from rewriting every track — and the whole list
     * travels because the piece has no verb for one track.
     */
    /**
     * The rows' payload as the crate reads it: what a report of every row
     * *means*, in the piece's one verb over its tracks.
     *
     * The whole list travels because the piece has no verb for one track — a
     * report is the piece here as it is for the boxes — so the difference is
     * what comes out, and it is one `settracks` whatever changed: a fader
     * moved, a track added, a track gone with its boxes.
     *
     * **The rule is the crate's**, like the boxes' and the curves': a client
     * that read this payload itself would be writing the mapping a second time
     * in its own language, which is how one client comes to add a track the
     * other cannot.
     */
    private strips(piece: Multitrack, values: readonly unknown[]): unknown[] {
        return multitrackReadRows(piece.write(), rowProps(values));
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

    override label(payload: unknown): string {
        const intent = String((payload as { intent?: unknown })?.intent ?? "");
        return MultitrackDomain.LABELS[intent] ?? "edit the piece";
    }
}

/** The crate's rows as the widget's flat sextuples. */
/**
 * The flat `lanes` payload as the crate's strips.
 *
 * The label and the height are dropped rather than sent: a row's label is the
 * track's name where it has one and a made-up one where it has not, and its
 * height is this window's. Neither is a fact about the piece, so neither is
 * reported into it.
 */
function rowProps(values: readonly unknown[]): Strip[] {
    return [...groups(values, SEXTUPLE)].map((group) => {
        const [name, , , mute, solo, gain] = group;
        return {
            name: String(name),
            mute: Number(mute) !== 0,
            solo: Number(solo) !== 0,
            gain: Number(gain),
        };
    });
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
 * **What each curve's time is measured from**, by curve name.
 *
 * A track automation runs the timeline, so it is measured from the origin; a
 * clip envelope is drawn inside its box and is measured from where that box
 * starts. It is the one thing that differs between the two on the wire, and the
 * reason it is worked out here is that the beat→frame crossing is the page's.
 */
function basesOf(picture: { boxes: readonly Box[]; curves: readonly Curve[]; layers: readonly Curve[] }): Map<string, number> {
    const where = new Map(picture.boxes.map((box) => [String(box.region), Number(box.position)]));
    const bases = new Map<string, number>();
    for (const curve of picture.curves) bases.set(String(curve.automation), 0);
    for (const curve of picture.layers) {
        bases.set(String(curve.automation), where.get(String(curve.owner)) ?? 0);
    }
    return bases;
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

    constructor(bridge: Bridge, link?: number) {
        super();
        this.bridge = bridge;
        this.link = link;
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
            ...editor.extra,
        );
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
        const picture = multitrackPicture(editor.structure.write());
        const props: Record<string, PropValue> = {
            lanes: laneProps(picture.rows) as PropValue,
            clips: clipProps(picture.boxes, this.bridge) as PropValue,
            curves: curveProps(picture.curves) as PropValue,
            layers: layerProps(picture.layers) as PropValue,
            points: pointProps(
                [...picture.curves, ...picture.layers],
                this.bridge,
                basesOf(picture),
            ) as PropValue,
            // **What is drawn is what the piece says was open.** Which curves a
            // person had showing is part of reopening the piece as they left
            // it, so it is read out of the document rather than kept here.
            hidden: [...picture.curves, ...picture.layers]
                .filter((c) => !c.visible)
                .map((c) => String(c.automation))
                .join(" "),
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

    constructor(piece: Multitrack, options: MultitrackEditorOptions) {
        const { sources, link, title = "Multitrack", ...rest } = options;
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
            view: new MultitrackView(bridge, link),
        });
        this.bridge = bridge;
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
