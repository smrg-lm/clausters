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
import { Multitrack, multitrackPicture, multitrackRead, multitrackReadPoints }
    from "../../multitrack.ts";
import type { Box, Placed, Row } from "../../multitrack.ts";
import { node, window as guiWindow } from "../guidef.ts";
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
    /** source id → buffer number. */
    readonly buffers: Map<number, number>;

    constructor(buffers?: Iterable<readonly [number, number]> | Record<number, number>) {
        this.buffers =
            buffers === undefined
                ? new Map()
                : buffers instanceof Map
                  ? new Map(buffers)
                  : Symbol.iterator in Object(buffers)
                    ? new Map(buffers as Iterable<readonly [number, number]>)
                    : new Map(
                          Object.entries(buffers as Record<number, number>).map(
                              ([source, buf]) => [Number(source), Number(buf)] as const,
                          ),
                      );
    }

    /**
     * The buffer a source was read into; `-1` for one nobody loaded.
     *
     * **Negative and not zero**, because buffer 0 is a buffer — the first one an
     * allocator hands out.
     */
    bufnum(source: number | undefined | null): number {
        if (source === undefined || source === null) return -1;
        return this.buffers.get(Math.trunc(source)) ?? -1;
    }

    /** The source a buffer number came from, or `undefined`. */
    source(bufnum: number): number | undefined {
        for (const [source, buf] of this.buffers) {
            if (buf === Math.trunc(bufnum)) return source;
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
            return multitrackReadPoints(this.state(piece), this.curved(values));
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
    private curved(values: readonly unknown[]): Curved[] {
        const found = new Map<string, Curved["points"]>();
        for (const group of groups(values, POINT_QUINTUPLE)) {
            const [name, at, value, shape, amount] = group;
            const points = found.get(String(name)) ?? [];
            points.push({
                at: this.bridge.beatAt(Number(at)),
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
    private strips(piece: Multitrack, values: readonly unknown[]): unknown[] {
        // **The piece is not touched here.** A payload states what the piece
        // *would* be; the inverse is read against what it is, and mutating first
        // would leave nothing to read — a fader that moved and an undo that put
        // it back where it already was.
        const tracks = piece.tracks.map((t) => t.write());
        const byId = new Map(tracks.map((t) => [Number(t.id), t] as const));
        let changed = false;
        for (const group of groups(values, SEXTUPLE)) {
            const [name, , , mute, solo, gain] = group;
            const found = byId.get(Number(String(name)));
            if (found === undefined) continue;
            const muted = Number(mute) !== 0;
            const soloed = Number(solo) !== 0;
            const level = Number(gain);
            const config = (found.config ?? {}) as Record<string, unknown>;
            const held = Number(config.level ?? 1.0);
            if (found.muted === muted && found.soloed === soloed && held === level) continue;
            found.muted = muted;
            found.soloed = soloed;
            // The fader is this client's key in an opaque table, so it is written
            // over what is there: a track's config is its instrument and its
            // routing too.
            found.config = { ...config, level };
            changed = true;
        }
        return changed ? [{ intent: "settracks", tracks }] : [];
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
 * Every curve's break-points as the widget's flat quintuples, each naming the
 * curve it is on — one list for the rows and the layers alike.
 */
function pointProps(curves: readonly Curve[], bridge: Bridge): unknown[] {
    const out: unknown[] = [];
    for (const curve of curves) {
        const name = String(curve.automation);
        for (const point of curve.points ?? []) {
            const data = (point.data ?? {}) as Record<string, unknown>;
            out.push(name, bridge.frameAt(Number(point.at ?? 0)),
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
 * this window's axis. The widget draws its own ruler, its own headers and its
 * own vertical scroll, so there is no stack to compose and nothing per clip to
 * register.
 */
export class MultitrackView extends View<Multitrack> {
    readonly bridge: Bridge;
    /** The navigation group the view joins, so a ruler beside it rules it. */
    link: number | undefined;

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
        return guiWindow(
            { title: editor.title, w: editor.size[0], h: editor.size[1], layout: "col" },
            node("multitrack", { id: wid, ...this.props(editor, wid) }),
            ...editor.extra,
        );
    }

    override props(editor: Editor<Multitrack>, _widgetId: number): Record<string, PropValue> {
        const picture = multitrackPicture(editor.structure.write());
        const props: Record<string, PropValue> = {
            lanes: laneProps(picture.rows) as PropValue,
            clips: clipProps(picture.boxes, this.bridge) as PropValue,
            curves: curveProps(picture.curves) as PropValue,
            layers: layerProps(picture.layers) as PropValue,
            points: pointProps([...picture.curves, ...picture.layers], this.bridge) as PropValue,
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
        };
        if (this.link !== undefined) props.link = this.link;
        return props;
    }
}

/** What {@link MultitrackEditor} is built with, beside a generic editor's. */
export interface MultitrackEditorOptions extends GenericEditorOptions<Multitrack> {
    /** Which server buffer each source was read into. */
    sources?: Sources | Iterable<readonly [number, number]> | Record<number, number>;
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
}

/** Whether {@link edit} should open this as a multitrack. */
export function isPiece(structure: unknown): structure is Multitrack {
    return structure instanceof Multitrack;
}
