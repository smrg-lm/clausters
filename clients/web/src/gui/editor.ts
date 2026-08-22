// `Editor`: the bridge between the arrangement and the multitrack GUI (mirrors
// `clausters/gui/editor.py`).
//
// The driver of the DAW-style view. It draws a `form` tree as a multitrack
// `GuiDef` (tracks of clips on one shared time axis), applies the clip
// edit-backs the host sends straight onto the arrangement, and re-renders it —
// the loop **data ↔ graphic ↔ sound**, which is what makes the composition
// editable at any granularity rather than merely displayable.
//
// Three things are worth knowing about how it is built.
//
// **The dependency arrow points this way.** `form` stays pure and
// transport-agnostic; the editor imports the arrangement, never the reverse.
// This module is the only one that knows both worlds.
//
// **Beats meet samples here.** The arrangement places elements in *beats*; the
// multitrack view places clips in *timeline samples*, because a clip's body is
// audio data and its sample 0 sits at the clip's offset. The editor is the only
// converter: one beat is `sampleRate / tempo` timeline units, so an audio take
// placed at its own length sits 1:1 on the axis. A musical `quant` becomes the
// lane's drag grid, so the grid a clip is dropped on is the grid the arrangement
// re-schedules on. The arithmetic itself is the core's, not a second
// implementation.
//
// **One mapping rule, not a heuristic per case.** The root `Aggregate`'s members
// are the *lanes*; a lane's members are its *clips*; a `Vector` clip draws its
// take, an element of events draws a piano-roll, and a nested `Aggregate` draws
// as a labeled rectangle — its summary — until it is `expand`ed into lanes of its
// own. That collapse/expand *is* the arrangement's base level, so it needs no
// protocol of its own.
//
// **One idiom differs from the Python client and it is the page's**: there is no
// `poll`. A script there drains the host in its own loop; a page has an event
// loop already, so `open` subscribes and every `/gui_event` reaches `apply` as it
// arrives. `detach()` unsubscribes.

import { beats_to_secs, samples_to_secs, secs_to_beats, secs_to_samples } from "../core/clausters_core_web.js";
import { Document, Log } from "../document.ts";
import type { Against, Intent, Outcome, Resolved, Selection } from "../document.ts";
import { GraphPatch, synthdefPorts } from "../defs/patch.ts";
import type { PortSpec } from "../defs/patch.ts";
import { SynthDef } from "../defs/synthdef.ts";
import { pointsToEnv } from "../defs/ugens/index.ts";
import type { Server } from "../defs/server/index.ts";
import {
    CONCRETE,
    LOGICAL,
    SIMULTANEOUS,
    Aggregate,
    Clang,
    Element,
    Segment,
    Segments,
    Vector,
    docIdOf,
    flatten,
    leafConfig,
    leafNode,
    nextNodeId,
    render as renderElement,
    setDocId,
    toDocument,
} from "../form/index.ts";
import type { Member } from "../form/index.ts";
import { FIRST_VERSION } from "../form/document.ts";
import { Automation } from "../seq/automation.ts";
import { Event as SeqEvent } from "../seq/event.ts";
import { OscEvent, Timeline } from "../seq/timeline.ts";
import type { Playhead } from "../seq/timeline.ts";
import type { TempoClock } from "../base/clock.ts";
import {
    clip,
    flatNotes,
    flatPoints,
    patch,
    pianoroll,
    scroll,
    signal,
    timeruler,
    track,
    waveform,
    window as guiWindow,
} from "./guidef.ts";
import type { GuiNode } from "./guidef.ts";
import type { GuiHost, PropValue } from "./host.ts";
import type { WindowHandle } from "./handle.ts";
import { Transport } from "./transport.ts";

/**
 * The pitch range a piano-roll lane falls back to when its notes give none
 * (C3..C6 — the span a melodic line usually lives in).
 */
const DEFAULT_PITCH: [number, number] = [48.0, 72.0];
/** Semitones of headroom above and below the notes of a piano-roll clip. */
const PITCH_PAD = 2.0;

/**
 * The measures a signal view can stack, in the order a reader thinks of them:
 * what the signal reached, and what it held inside that.
 */
export const MEASURES = ["peak", "rms"] as const;
/** One of the measures above. */
export type Measure = (typeof MEASURES)[number];

/** A note as the roll draws it: `[start, dur, pitch, velocity, channel]`. */
type Note = [number, number, number, number, number];

/**
 * What a clip widget was drawn from: the placement it shows (`owner` aggregate
 * and `member` handle, the arrangement's stable identity), the `base` in beats
 * its aggregate sits at (a clip's offset is absolute on the shared axis, a
 * placement is relative to its aggregate — this bridges the two), and the
 * `offset`/`dur` in timeline units it was drawn with (so an edit-back can tell
 * what actually moved).
 */
class Placed {
    owner: Aggregate | null;
    member: Member | null;
    base: number;
    offset: number;
    dur: number;

    constructor(
        owner: Aggregate | null,
        member: Member | null,
        base: number,
        offset: number,
        dur: number,
    ) {
        this.owner = owner;
        this.member = member;
        this.base = Number(base);
        this.offset = Number(offset);
        this.dur = Number(dur);
    }
}

/** What {@link Editor} is built with. */
export interface EditorOptions {
    /** The engine's sample rate; with `tempo` it fixes the beats↔samples axis. */
    sampleRate: number;
    /** The clock's tempo in **beats per second** (2.0 is 120 bpm). */
    tempo?: number;
    /** The musical drag grid in beats (`0.25` = a sixteenth); 0 snaps to samples. */
    quant?: number;
    /** Re-render on every edit (the live editor). */
    follow?: boolean;
    /** Extra GuiDef nodes placed under the lanes (a transport panel, say). */
    extra?: readonly GuiNode[];
    /** The window title. */
    title?: string;
    width?: number;
    height?: number;
    /** The first widget id a **host-less** draw counts from (tests, inspection). */
    baseId?: number;
}

/**
 * A composition on screen: the arrangement tree drawn as a multitrack view,
 * editable back into the tree.
 *
 * ```ts
 * const editor = new Editor(song, { sampleRate, tempo: clock.tempo, quant: 0.25 });
 * editor.open(host);                 // draw, open the window, listen
 * editor.render(server, clock);      // play the edited composition
 * ```
 */
export class Editor {
    element: Element;
    sampleRate: number;
    tempo: number;
    quant: number;
    /**
     * Re-render on every edit (the *live editor*: drag a clip and hear it where
     * you dropped it). Off by default — an edit then only changes the
     * arrangement, and `rerender` decides when it is heard.
     */
    follow: boolean;
    /** Widgets appended to the window after the lanes. They are the script's. */
    extra: GuiNode[];
    title: string;
    size: [number, number];
    /**
     * The last selection swept in this editor's windows, as the crate's
     * `Selection` — `{start, len}` in **beats**, plus `value` where the sweep
     * restricted the value axis and `nodes` where it named an element rather
     * than the shared time axis.
     *
     * It is a plain value and not part of the composition, which is the crate's
     * own line: a selection is screen state, never persisted and never logged.
     */
    selection: Selection | Record<string, never> = {};
    /**
     * The transport driving the lanes' playhead. Its lanes are read on each use,
     * so a redraw's new widgets get the line.
     */
    readonly transport: Transport;
    /**
     * Whether the arrangement changed since the last render — an edit does not
     * interrupt what is playing, so a transport (play, a resume after pause, a
     * seek) reads this to know it must re-read the composition.
     */
    dirty = false;

    private readonly baseId: number;
    private fallbackId: number;
    /** The elements shown as lanes of their own instead of a summary clip. */
    private expanded = new Set<Element>();
    /** widget id → where the clip came from, and what was drawn for it. */
    private clips = new Map<number, Placed>();
    /** widget id → element, for every lane (the playhead addresses these). */
    private lanes = new Map<number, unknown>();
    /** widget id → the element whose samples that widget draws. */
    private signals = new Map<number, Element>();
    /** widget id → the element whose notes that widget draws. */
    private rolls = new Map<number, Element>();
    /** patch widget id → the logical aggregate and its box-order handles. */
    private patches = new Map<number, [Aggregate, Member[]]>();
    /** aggregate → `{box index: [x, y]}`, presentation only. */
    private patchGeometry = new Map<Aggregate, Record<number, [number, number]>>();
    /** Which **edit layer** of each clip the hand is on. Screen state. */
    private editLayer = new Map<number, string>();
    /**
     * The **oldest version an incoming edit may name**: raised whenever the
     * composition moves by a route that is not a host event, and by nothing
     * else. See {@link Editor.stale}, which is the only thing that reads it.
     */
    private floor: number = FIRST_VERSION;
    /**
     * The version this editor was at when it last answered a host event — what
     * turns "the version moved" into "the version moved *by someone else*".
     */
    private applied: number = FIRST_VERSION;
    /**
     * The **value axis** each curve is drawn against — remembered rather than
     * recomputed, which is screen state for the same reason the edit layer is.
     *
     * A break-point's position on screen is its value *against this axis*, so an
     * axis derived from the break-points moves every point whenever any one of
     * them is dragged: the curve jumps under the hand editing it, and the point
     * being dragged is the only one that appears not to move.
     */
    private curveAxis = new WeakMap<Automation, [number, number]>();
    private corrections: [number, Record<string, PropValue>][] = [];
    private reason: string | undefined = undefined;
    private mode: "multitrack" | "pianoroll" | "signal" = "multitrack";
    private rollElement: Element | null = null;
    private signalElement: Element | null = null;
    private measures: Measure[] = ["peak", "rms"];
    /** The composition's version — the document half of the two counters. */
    private version = FIRST_VERSION;
    private log: Log | null = null;
    private doc: Document | null = null;
    private rederive = false;
    private nextNode: number | null = null;
    /** node id → the arrangement object an intent naming it writes to. */
    private byNode = new Map<number, [Aggregate | null, Member | null, Element]>();
    private host: GuiHost | null = null;
    private windowId: number | null = null;
    private unlisten: (() => void) | null = null;
    private destination: unknown = null;
    private clock: TempoClock | null = null;

    constructor(
        element: Element,
        {
            sampleRate,
            tempo = 1.0,
            quant = 0.0,
            follow = false,
            extra = [],
            title = "Composition",
            width = 1000,
            height = 520,
            baseId = 10_000,
        }: EditorOptions,
    ) {
        this.element = element;
        this.sampleRate = Number(sampleRate);
        this.tempo = Number(tempo);
        this.quant = Number(quant);
        this.follow = Boolean(follow);
        this.extra = [...extra];
        this.title = title;
        this.size = [Math.trunc(width), Math.trunc(height)];
        this.baseId = Math.trunc(baseId);
        this.fallbackId = this.baseId;
        this.transport = new Transport(null, () => [...this.lanes.keys()], {
            source: (at) => this.renderPass(at),
            tempo: this.tempo,
            sampleRate: this.sampleRate,
            extent: () => this.extent(),
        });
    }

    // ---- the unit bridge: beats (the data) ↔ timeline samples (the view) ----

    /**
     * Timeline samples per beat — the whole of the data↔view unit bridge. One
     * timeline unit is one audio sample, so a take placed at its own frame count
     * sits 1:1 on the axis.
     */
    get unitsPerBeat(): number {
        return this.beatsToUnits(1.0);
    }

    /** Beats → timeline samples, through the core's own time arithmetic. */
    beatsToUnits(beats: number): number {
        return secs_to_samples(beats_to_secs(this.tempo, 0.0, 0.0, Number(beats)), this.sampleRate);
    }

    /**
     * Timeline samples → beats: the inverse the edit-back path takes to turn a
     * dragged clip back into a placement.
     */
    unitsToBeats(units: number): number {
        return secs_to_beats(
            this.tempo,
            0.0,
            0.0,
            samples_to_secs(Math.round(units), this.sampleRate),
        );
    }

    // ---- the base level: collapse (a summary rectangle) vs expand (lanes) ----

    /**
     * Resolve a nested `Aggregate` into lanes of its own (instead of the labeled
     * rectangle that summarizes it). The arrangement's *base level*, made an edit.
     */
    expand(element: Element): this {
        this.expanded.add(element);
        return this;
    }

    /** Summarize a nested `Aggregate` back into one labeled rectangle. */
    collapse(element: Element): this {
        this.expanded.delete(element);
        return this;
    }

    isExpanded(element: Element): boolean {
        return this.expanded.has(element);
    }

    // ---- widget ids: the host's recycling pool, or a host-less fallback ----

    /**
     * A widget id for the tree being drawn. Once `open`ed it comes from the
     * host's recycling pool; host-less (a test, or inspecting `draw`), it counts
     * from `baseId`.
     */
    private newId(): number {
        return this.host !== null ? this.host.allocId() : this.fallbackId++;
    }

    /**
     * Start a fresh draw's id numbering. Host-less, the fallback counter restarts
     * at `baseId`; on a host nothing resets — the ids come from its pool, and
     * re-defining the window returns the previous tree's ids there, so the churn
     * recycles instead of climbing.
     */
    private resetIds(): void {
        if (this.host === null) this.fallbackId = this.baseId;
    }

    // ---- the forward draw: the arrangement -> GuiDef ----

    /**
     * The composition as a `window`-rooted GuiDef: one `track` lane per member of
     * the root aggregate, each holding its members as clips on the shared time
     * axis. Pure — it builds the tree and the id registry, and sends nothing.
     *
     * A **logical** aggregate draws as a directed `patch` (a server patch, not a
     * timeline lane): a box per member, its typed ports derived from the
     * `SynthDef` the member wraps, cords from the members' shared internal-bus
     * controls. A member wrapping a bare def *name* draws port-less.
     */
    draw(): GuiNode {
        if (this.mode === "pianoroll") return this.drawPianoroll();
        if (this.mode === "signal") return this.drawSignal();
        this.resetIds();
        this.clips = new Map();
        this.lanes = new Map();
        this.rolls = new Map();
        this.patches = new Map();
        this.signals = new Map();

        const lanes: GuiNode[] = [];
        const root = this.element;
        if (root instanceof Aggregate && root.kind === CONCRETE) {
            for (const member of root.handles) {
                if (member.element instanceof Aggregate && member.element.kind === LOGICAL) {
                    lanes.push(this.patchLane(member.element));
                } else {
                    lanes.push(...this.lanesFor(member.element, member.offset, root, member));
                }
            }
        } else if (root instanceof Aggregate && root.kind === LOGICAL) {
            lanes.push(this.patchLane(root));
        } else {
            lanes.push(...this.lanesFor(root, Number(root.onset ?? 0.0), null, null));
        }

        // One ruler under the stack (the DAW convention), as a **free-standing**
        // strip owning its own box: a lane's own `ruler` is reserved out of that
        // lane's height, so ruling the stack would cost the bottom lane a strip
        // of itself. A lane and a patch workspace are different containers on the
        // wire, and only the first has a time axis to rule.
        const ruler =
            lanes.some((lane) => lane.type === "field")
                ? [timeruler({ ruler: "beats", sampleRate: this.sampleRate, tempo: this.tempo })]
                : [];
        return guiWindow(
            { title: this.title, w: this.size[0], h: this.size[1], layout: "col" },
            ...lanes,
            ...ruler,
            ...this.extra,
        );
    }

    /**
     * A logical aggregate drawn as a directed `patch` inside a pan/zoom `scroll`
     * workspace — a server patch among the timeline lanes. Registers the patch
     * widget id so an edit-back resolves to the aggregate it draws.
     */
    private patchLane(aggregate: Aggregate): GuiNode {
        const [p, handles] = logicalPatch(aggregate);
        const wid = this.newId();
        this.patches.set(wid, [aggregate, handles]);
        const geometry = this.patchGeometry.get(aggregate) ?? {};
        const [contentW, contentH] = [900.0, 700.0];
        const view = patch({
            id: wid,
            ...p.toWidget(geometry),
            label: nameOf(aggregate),
            x: 0.0,
            y: 0.0,
            w: contentW,
            h: contentH,
        });
        return scroll({ id: this.newId(), contentW, contentH }, view);
    }

    /**
     * `draw` the composition and open it on `host`, and listen: every
     * `/gui_event` the host reports reaches {@link Editor.apply} from here on.
     *
     * Answers the **window handle**: it equals the window id, and it also
     * resolves the tree's named widgets, so a transport button is reachable by
     * name (`win.widget("play").onEvent(…)`).
     */
    open(host: GuiHost, id?: number): WindowHandle {
        this.host = host;
        this.transport.host = host;
        this.mode = "multitrack";
        const handle = host.open(this.draw(), id === undefined ? {} : { id });
        this.windowId = handle.id;
        this.listen(host);
        this.announce();
        return handle;
    }

    /**
     * Subscribe to the host's messages, so an edit-back reaches the arrangement.
     * `open` does it; this is the door for a caller that opened the window
     * itself.
     */
    listen(host: GuiHost): () => void {
        this.detach();
        this.unlisten = host.onMessage((msg) => {
            this.apply(msg.addr, msg.args);
        });
        return () => this.detach();
    }

    /** Stop listening. The window stays open; nothing reaches the arrangement. */
    detach(): void {
        this.unlisten?.();
        this.unlisten = null;
    }

    /**
     * Tell this editor that the arrangement moved by a route it did not take, so
     * the document it holds has to be derived again.
     *
     * **The door a held document needs.** The editor keeps one `Document` for the
     * composition's life, which is what makes a gesture cost the edit rather than
     * the composition. The price is that a script mutating the arrangement while
     * a window is open is no longer absorbed by a rebuild: without this the next
     * edit would be made against a composition that has moved, and the crate
     * would refuse it as stale.
     *
     * Cheap and safe to call: `toDocument` stamps each element with the id it
     * keeps, so a re-derivation names the same nodes and the history keeps its
     * footing.
     */
    refresh(): void {
        this.rederive = true;
        // The arrangement moved by a route no gesture took — a picture a step
        // still in flight was made against is now gone, and
        // {@link Editor.stale} has to say so. It takes no version of its own,
        // so the floor is raised here rather than noticed later.
        this.floor = this.version;
    }

    /**
     * Point this editor at another composition, redrawing the window it already
     * has — what a reopened session needs.
     *
     * **The history is dropped**, deliberately. Its inverses describe states of
     * the session that just ended; keeping them would let an undo walk back into
     * a composition the file does not contain. The transport keeps its position —
     * where you were looking is not part of what was loaded.
     */
    load(element: Element): void {
        this.element = element;
        this.expanded.clear();
        this.patchGeometry.clear();
        this.log?.free();
        this.log = null;
        this.doc?.free();
        this.doc = null;
        this.rederive = false;
        this.byNode = new Map();
        this.version = FIRST_VERSION;
        this.floor = FIRST_VERSION;
        this.applied = FIRST_VERSION;
        this.dirty = true;
        if (this.host !== null && this.windowId !== null) {
            this.resetIds();
            this.host.define(this.windowId, this.draw());
            this.announce();
        }
    }

    /**
     * The dedicated piano-roll view: one `pianoroll` widget drawing a single
     * events element's MIDI notes (grid) and OSC events (lane), instead of a
     * multitrack of clips.
     */
    private drawPianoroll(): GuiNode {
        this.resetIds();
        this.clips = new Map();
        this.lanes = new Map();
        this.rolls = new Map();
        this.signals = new Map();
        const element = this.rollElement as Element;
        const wid = this.newId();
        const notes = this.notesOf(element);
        const osc = this.oscOf(element);
        const body: Record<string, number> = {};
        if (notes.length > 0) {
            const pitches = notes.map((n) => n[2]);
            body.min = Math.min(Math.min(...pitches) - PITCH_PAD, DEFAULT_PITCH[1]);
            body.max = Math.max(Math.max(...pitches) + PITCH_PAD, DEFAULT_PITCH[0]);
        }
        const snap = this.quant > 0 ? this.beatsToUnits(this.quant) : undefined;
        // The roll is a lane (the playhead addresses these) and a roll (the note
        // edit-back resolves through these).
        this.lanes.set(wid, element);
        this.rolls.set(wid, element);
        const roll = pianoroll({
            id: wid,
            notes: notes.length > 0 ? notes : undefined,
            osc: osc.length > 0 ? osc : undefined,
            ruler: "beats",
            tempo: this.tempo,
            sampleRate: this.sampleRate,
            snap,
            label: nameOf(element),
            ...body,
        });
        return guiWindow(
            { title: this.title, w: this.size[0], h: this.size[1], layout: "col" },
            roll,
            ...this.extra,
        );
    }

    /**
     * What the dedicated signal view measures — `["peak", "rms"]` for the
     * editor's picture, `["peak"]` for the bare envelope.
     *
     * **Assigning it on an open view sends one message.** The measure is a live
     * prop, so the body appears and disappears over the peaks with the picture,
     * the axis, the zoom, the selection and the playhead all exactly where they
     * were. Redrawing for this would be the wrong tool twice over.
     */
    get layers(): Measure[] {
        return [...this.measures];
    }

    set layers(measures: readonly string[]) {
        const stack = [...measures].map(measureName);
        if (stack.length === 0) {
            throw new Error(`a signal view measures something (one of ${MEASURES.join(", ")})`);
        }
        this.measures = stack;
        if (this.mode === "signal" && this.host !== null && this.windowId !== null) {
            for (const wid of this.signals.keys()) {
                this.host.set(wid, { measure: stack.join(" ") });
            }
        }
    }

    /**
     * The dedicated signal view: the **editor-grade waveform** of a single
     * rendered element's samples, instead of a multitrack of clips.
     *
     * It is one `waveform` — the same heavy view a standalone take is shown in —
     * and the stack of measures is a prop of it, not a pile of widgets. That is
     * the shape the picture forces: every view of a signal paints its own field
     * before it draws, so two of them on one rectangle are not layers — the
     * second hides the first.
     */
    private drawSignal(): GuiNode {
        this.resetIds();
        this.clips = new Map();
        this.lanes = new Map();
        this.rolls = new Map();
        this.signals = new Map();
        const element = this.signalElement as Element;
        const body = this.sourceOf(element);
        const wid = this.newId();
        this.lanes.set(wid, element);
        this.signals.set(wid, element);
        const view = waveform({
            ...body,
            id: wid,
            label: nameOf(element),
            measure: this.measures.join(" ") as "peak" | "rms" | "peak rms",
            ruler: "time",
            sampleRate: this.sampleRate,
            tempo: this.tempo,
        });
        return guiWindow(
            { title: this.title, w: this.size[0], h: this.size[1], layout: "col" },
            view,
            ...this.extra,
        );
    }

    /**
     * The source props a signal view draws `element`'s samples from, or an error
     * naming what is missing.
     *
     * **This is the generated/generator distinction, asked at the door.** A
     * rendered element has samples a view can address; a generator has none until
     * it is rendered, and a window drawn over nothing is worse than a refusal
     * that says what to do.
     */
    private sourceOf(element: Element | null): { buffer?: number; channels?: number } {
        const body = element === null ? {} : this.bodyFor(element);
        if (!("buffer" in body)) {
            throw new Error(
                `${nameOf(element)} has no samples to draw: a signal view needs a ` +
                    "rendered element (render the composition, or bounce this one to " +
                    "a buffer, and open that)",
            );
        }
        return { buffer: body.buffer as number, channels: body.channels as number };
    }

    /**
     * `draw` a single **rendered** element as a dedicated signal view and open it
     * on `host` — the editor-grade view of one element's samples, as opposed to
     * `open`, where the same samples are only a clip's body.
     */
    openSignal(
        host: GuiHost,
        element?: Element,
        { layers = ["peak", "rms"], id }: { layers?: readonly string[]; id?: number } = {},
    ): WindowHandle {
        const target = element ?? this.element;
        // Refused **before** a window exists: an unknown measure and an element
        // with no samples are both answers to the call that was made, and
        // finding out at the first repaint would leave an empty window behind.
        const stack = [...layers].map(measureName);
        this.sourceOf(target);
        this.host = host;
        this.transport.host = host;
        this.mode = "signal";
        this.signalElement = target;
        this.measures = stack;
        const handle = host.open(this.draw(), id === undefined ? {} : { id });
        this.windowId = handle.id;
        this.listen(host);
        this.announce();
        return handle;
    }

    /**
     * `draw` a single events element as a **dedicated piano-roll** window and
     * open it on `host` — the editor-grade note view of one MIDI/OSC element.
     *
     * Edits write back exactly as the multitrack does, **when the element is
     * editable** — a `Track` (a `Timeline`). A **generator** is forward-only, so
     * its bounced notes are shown *read-only*.
     */
    openPianoroll(host: GuiHost, element?: Element, id?: number): WindowHandle {
        this.host = host;
        this.transport.host = host;
        this.mode = "pianoroll";
        this.rollElement = element ?? this.element;
        const handle = host.open(this.draw(), id === undefined ? {} : { id });
        this.windowId = handle.id;
        this.listen(host);
        this.announce();
        return handle;
    }

    /**
     * The composition's length in beats, **read from the arrangement** — the end
     * of its last placed element. It is not a constant: move a clip past the end
     * and the piece gets longer, which is exactly what a transport must ask.
     */
    extent(element?: Element): number {
        return this.extentOf(element ?? this.element);
    }

    /** The `Playhead` playing the composition, or `null` before the first render. */
    get playhead(): Playhead | null {
        return this.transport.playhead;
    }

    /**
     * The open window's id, or `null` once it is closed (a `/gui_closed` seen by
     * `apply`) — what a script checks to stop.
     */
    get window(): number | null {
        return this.windowId;
    }

    /**
     * Push the current arrangement back to the open window — a whole-tree
     * redefine, the honest way to show a structural edit (an element added, an
     * aggregate expanded). A mere placement change needs no redefine: the host
     * already moved the clip that was dragged.
     *
     * A redefine **moves the version**, and that is the point rather than a side
     * effect: this is the route a change the editor did not apply arrives by, and
     * it is the case an edit log cannot see. It also rebuilds the widgets, so a
     * gesture still in flight was made against a picture that no longer exists;
     * the bump is what makes that edit come back as stale.
     */
    update(): void {
        if (this.host === null || this.windowId === null) {
            throw new Error("open(host) the editor first");
        }
        this.version += 1;
        this.host.define(this.windowId, this.draw());
        this.announce();
    }

    // ---- the edit-back: a dragged clip becomes a placement ----

    /**
     * Apply one message from the host to the **arrangement**, and **answer it**.
     * Answers whether the composition changed.
     *
     * The clip edit-back (`/gui_event <id> "clip" <offset> <dur>`) is resolved
     * through the widget registry to the placement it came from. The clip's
     * offset is **absolute** on the shared axis while a placement is relative to
     * its aggregate, so the position converts back through the base the clip was
     * drawn at; and only what actually moved is written — a drag carries the
     * clip's unchanged `dur` along, and snapping *that* to the grid would
     * silently shorten the element. `/gui_closed` drops the window; anything else
     * is ignored, so a whole message stream can be fed straight in — even one
     * shared with a second editor.
     */
    apply(addr: string, rawArgs: readonly unknown[]): boolean {
        if (addr === "/gui_closed") {
            if (rawArgs.length === 0 || this.windowId === null ||
                Math.trunc(Number(rawArgs[0])) === this.windowId) {
                this.windowId = null;
                this.detach();
            }
            return false;
        }
        if (addr !== "/gui_event" || rawArgs.length < 3) return false;
        // `<id> <seq> <version> <tag> <payload…>`: the stamp and the version the
        // gesture was made against are the second and third arguments of every
        // event. The stamp is what an acknowledgement names.
        const seq = Math.trunc(Number(rawArgs[1]));
        const against = Math.trunc(Number(rawArgs[2] ?? 0));
        const args = [rawArgs[0], ...rawArgs.slice(3)];
        this.corrections = [];
        // Why an edit did not do what it asked, when there is something to say.
        this.reason = undefined;
        const id = Math.trunc(Number(args[0]));
        // The window's own shortcuts (Ctrl+Z / Ctrl+Shift+Z), which the host
        // addresses to the **window** rather than to a widget: undo is not aimed
        // at anything under the cursor. They are answered here rather than
        // routed, because a history step is not an edit to the tree.
        if ((args[1] === "undo" || args[1] === "redo") && id === this.windowId) {
            if (args[1] === "redo") this.redo();
            else this.undo();
            this.acknowledge(seq);
            return true;
        }
        // Only what this editor draws is this editor's to answer.
        if (!this.owns(id)) return false;
        // **The answers lag, and that is not a conflict.** A host stamps every
        // event with the version it was last told, and it is told only when an
        // acknowledgement reaches it — a round trip a hand outruns, so an edit
        // naming a version this editor has moved past is the ordinary case. What
        // the check is for is the composition moving by a route the host knows
        // nothing about, so only *that* raises the floor: the version moved
        // since the last event was answered, and no event is what moved it.
        if (this.version !== this.applied) this.floor = this.version;
        if (this.stale(against)) {
            // The composition moved under the gesture, by a route no gesture
            // produced. The edit is not applied and not merged: an edit-back
            // payload is absolute *and* whole, so applying one made against an
            // older picture would silently drop whatever arrived in between.
            this.resync(id);
            this.acknowledge(seq, "the composition changed since this edit");
            return false;
        }
        const changed = this.route(args);
        this.applied = this.version;
        // Answered whatever happened, and answered with a *value*: applied,
        // transformed and refused are one message.
        this.acknowledge(seq, this.reason);
        return changed;
    }

    /**
     * One `/gui_event` payload onto the arrangement, with the stamp already taken
     * off. Answers whether the composition changed; `apply` answers the host.
     */
    private route(args: readonly unknown[]): boolean {
        const id = Math.trunc(Number(args[0]));
        const tag = String(args[1]);
        const rest = args.slice(2);
        if (tag === "locate") {
            // A click on a lane's ruler (or its empty space): seek. A transport
            // action, not an edit.
            if (this.lanes.has(id)) this.locate(this.unitsToBeats(Number(rest[0])));
            return false;
        }
        if (tag === "selection") {
            // A marquee swept on a lane or a view. Nothing in the composition
            // changed — a selection is screen state — but it is the *value* an
            // operation is handed, so it is kept typed and in beats.
            this.setSelection(id, rest);
            return false;
        }
        if (tag === "cut") return this.applyCut(id, rest);
        if (tag === "paste") return this.applyPaste(id, rest);
        if (tag === "notes") {
            const element = this.rolls.get(id);
            if (element === undefined) return false;
            if (this.applyNotes(element, rest)) return true;
            // Read-only samples: a generator's notes are a *rendering* of an
            // algorithm, so the edit is refused — and the refusal is the notes
            // as they still are, sent back so the host stops drawing the one the
            // hand moved. It says **why**: a note that springs back with nothing
            // attached teaches "sometimes it does not work" rather than "not
            // here". Two different things are read-only and naming the wrong one
            // is worse than naming none.
            this.reason =
                editableTimeline(element) === null && !(element instanceof Clang)
                    ? "this clip draws what a generator produced; render it to a " +
                      "track to edit its notes"
                    : "this clip is one placed event; drag the clip to move it, " +
                      "a track is what holds editable notes";
            this.correct(id, { notes: flatNotes(this.notesOf(element)) });
            return false;
        }
        if (this.patches.has(id)) {
            // A logical aggregate's directed patch: a cord drawn or a box moved.
            return this.applyPatch(id, tag, rest);
        }
        const placed = this.clips.get(id);
        if (placed === undefined) return false;
        if (tag === "points") return this.applyPoints(placed, rest);
        if (tag === "layer") {
            // Which layer of a clip the hand is on is **screen state**, like a
            // selection: the composition did not change.
            this.editLayer.set(id, String(rest[0]));
            return false;
        }
        if (tag === "split") return this.applySplit(placed, Number(rest[0]));
        if (tag === "join") return this.applyJoin(placed, rest.map((a) => Math.trunc(Number(a))));
        if (tag !== "clip" || rest.length < 2) return false;
        if (placed.member === null) return false; // the root: nothing places it

        let offset = Number(rest[0]);
        let dur = Number(rest[1]);
        // The **window** the trim left behind, when the host stated one. A host
        // older than windows sends three arguments and means "from the
        // beginning", which is what a take with no window has always been.
        const start = rest.length > 2 ? Number(rest[2]) : null;
        const moved = Math.abs(offset - placed.offset) >= 0.5; // half a sample
        const resized = Math.abs(dur - placed.dur) >= 0.5;
        const trimmed = this.windowMoved(placed, start);
        if (!moved && !resized && !trimmed) return false;
        if (trimmed) {
            // A trim is a gesture of its own — the placement *and* the window
            // over the samples, in one edit — so it does not go through the
            // placement road below.
            if (!this.trim(placed, offset, dur, start as number)) return false;
            placed.offset = offset;
            placed.dur = dur;
            this.version = (this.doc as Document).version;
            this.dirty = true;
            this.followRender();
            return true;
        }

        const member = placed.member;
        // Absolute (the axis) → relative (the placement). The **grid is not
        // applied here**: the intent states where the hand put it and the crate
        // snaps, which is the rule the whole document exists for.
        const askedOffset = moved
            ? this.unitsToBeats(offset) - placed.base
            : member.offset;
        const askedDur = resized ? this.unitsToBeats(dur) : member.dur;
        const node = this.nodeId(member.element, member);
        if (node === null) return false;
        const intent: Intent =
            askedDur === null
                ? { intent: "place", node, offset: Number(askedOffset) }
                : { intent: "place", node, offset: Number(askedOffset), dur: Number(askedDur) };
        const outcome = this.record(intent, moved ? "move the clip" : "resize the clip");
        if (outcome === null || !outcome.applied) {
            // Refused — a node the document does not know, or an edit made
            // against a version it has left behind.
            this.resync(id);
            return false;
        }
        const effective = outcome.effective as { offset: number; dur?: number };
        this.project(outcome.effective);
        // What the crate did to the gesture. The host drew the clip where it was
        // released; if the snap moved it, saying so is the whole point of an
        // acknowledgement carrying a value.
        const snappedOffset = this.beatsToUnits(Number(effective.offset) + placed.base);
        const snappedDur =
            effective.dur === undefined ? dur : this.beatsToUnits(Number(effective.dur));
        if (Math.abs(snappedOffset - offset) >= 0.5 || Math.abs(snappedDur - dur) >= 0.5) {
            this.correct(id, { offset: snappedOffset, dur: snappedDur });
            offset = snappedOffset;
            dur = snappedDur;
        }
        // The clip was drawn where it now is: keep the registry truthful, or the
        // next edit would measure its move against a stale placement.
        placed.offset = offset;
        placed.dur = dur;
        this.version = (this.doc as Document).version;
        this.dirty = true;
        this.followRender();
        return true;
    }

    /** Whether this editor drew the widget an event names. */
    private owns(widgetId: number): boolean {
        return (
            this.clips.has(widgetId) ||
            this.rolls.has(widgetId) ||
            this.patches.has(widgetId) ||
            this.lanes.has(widgetId) ||
            this.signals.has(widgetId)
        );
    }

    /**
     * Tell the host which version it is drawing, before any edit.
     *
     * A stamp of zero retires nothing — the host's own numbering starts at one —
     * so this is purely the version, and it is what keeps the *first* gesture
     * checked like every later one.
     */
    private announce(): void {
        this.host?.ack(0, this.version);
    }

    /**
     * Whether an edit made against document version `against` has been overtaken.
     * Zero is *unstated* rather than a version, and unstated applies unchecked.
     *
     * Overtaken means *by a route the host never saw*. Every version this
     * editor made while answering the host's own events is one the host is
     * either about to be told or has been told already, so an edit naming one
     * of them is simply an answer that had not arrived yet — a drag's later
     * frames, a second gesture begun inside one round trip. What raises the
     * floor is a script's edit, a second editor's, a redefine, an undo: the
     * cases where the picture the gesture was made against is gone.
     */
    private stale(against: number): boolean {
        return against !== 0 && against < this.floor;
    }

    /**
     * Hand back what the widget should be drawing, without applying anything: the
     * answer to an edit that arrived too late.
     *
     * **Everything the widget has**, not only what the gesture touched: one
     * widget is often both (a clip with a roll body is a placement *and* a note
     * list), and the stale edit is the one case where the host's whole picture of
     * it is in doubt.
     */
    private resync(widgetId: number): void {
        const props: Record<string, PropValue> = {};
        const placed = this.clips.get(widgetId);
        if (placed !== undefined) {
            props.offset = placed.offset;
            props.dur = placed.dur;
            const auto = automationOf(
                placed.member !== null ? placed.member.element : this.element,
            );
            if (auto !== null) {
                // A curve is as much of "what this widget should be drawing" as a
                // placement is, and an undone one is the case that needs it.
                props.points = flatPoints(
                    quads(auto.toPoints()).flatMap(([t, v, shape, curve]) => [
                        this.beatsToUnits(t),
                        v,
                        shape,
                        curve,
                    ]),
                );
            }
            const held = placed.member !== null ? placed.member.element : this.element;
            if (held instanceof Vector) {
                // A take's **window** is as much of what this widget draws as
                // its placement is: an undone trim puts the frames back, and
                // nothing else would say so. Stated even when it is zero —
                // absence is a value here as everywhere, and a prop left out is
                // a prop left standing.
                props.start = Number(held.start);
                props.loop = Boolean(held.loop);
            }
        }
        const element = this.rolls.get(widgetId);
        if (element !== undefined) props.notes = flatNotes(this.notesOf(element));
        if (Object.keys(props).length > 0) this.correct(widgetId, props);
    }

    /**
     * What the host should be drawing instead of what it drew — a snap, or a
     * refusal. The value travels with the acknowledgement in one bundle, which is
     * what lets the host adopt it without a redefine.
     */
    private correct(widgetId: number, props: Record<string, PropValue>): void {
        this.corrections.push([Math.trunc(widgetId), props]);
    }

    /**
     * Answer the host for everything up to `seq`.
     *
     * This editor snaps a placement to the musical grid and refuses an edit to a
     * generator, and the host can learn neither on its own — so the stamp is what
     * lets it retire what it drew and adopt what actually happened. Every
     * acknowledgement carries the composition's version, which the host names
     * back on its next gesture: that round trip is the whole of the staleness
     * check, and it costs one integer.
     */
    private acknowledge(seq: number, reason?: string): void {
        if (this.host === null) return;
        if (!seq && this.corrections.length === 0) return;
        // A stamp of zero retires nothing, which is what an **unasked** push
        // needs: an undo answers no gesture.
        if (this.corrections.length > 0) {
            this.host.push(seq, this.corrections, this.version, [], reason);
        } else {
            this.host.ack(seq, this.version, [], reason);
        }
    }

    /**
     * Keep the swept selection as the crate's `Selection`, in beats.
     *
     * The host reports `start len` in timeline samples, plus `min max` where the
     * sweep restricted the value axis too. Three things happen here and each is a
     * translation the crate deliberately does not do. The time numbers become
     * **beats** (the crate holds whatever unit it is handed, because the
     * beats↔samples bridge belongs to whoever renders). The value range is
     * carried **as it came**: it is in the element's own domain. And the
     * selection is made *of* something where the widget names one element, and of
     * the shared time axis where it does not.
     */
    private setSelection(wid: number, values: readonly unknown[]): void {
        if (values.length < 2) return;
        const selection: Selection = {
            start: this.unitsToBeats(Number(values[0])),
            len: this.unitsToBeats(Number(values[1])),
        };
        if (values.length >= 4) {
            selection.value = { min: Number(values[2]), max: Number(values[3]) };
        }
        let element = this.rolls.get(wid) ?? this.signals.get(wid) ?? null;
        if (element === null) {
            const placed = this.clips.get(wid);
            element =
                placed === undefined
                    ? null
                    : placed.member !== null
                      ? placed.member.element
                      : this.element;
        }
        const node = element === null ? null : this.nodeId(element);
        if (node !== null) selection.nodes = [node];
        this.selection = selection;
    }

    /**
     * A cut asked for over the selection: the host owns none of the data, so this
     * is where it becomes an edit.
     *
     * **What this editor cuts is a placement**, because that is what it owns: a
     * clip the selection covers entirely leaves the aggregate it was in —
     * undoably, through the crate. What it does *not* do is trim: a selection
     * cutting across a clip implies a new length for the samples under it, and
     * writing samples is the business of whoever owns them. That case is refused
     * **out loud**, because a cut that silently did nothing would read as a
     * broken key.
     */
    private applyCut(wid: number, values: readonly unknown[]): boolean {
        const placed = this.clips.get(wid);
        if (placed === undefined || placed.member === null || values.length < 2) return false;
        const start = this.unitsToBeats(Number(values[0]));
        const end = start + this.unitsToBeats(Number(values[1]));
        const member = placed.member;
        // The clip's span on the **shared** axis.
        const at = placed.base + member.offset;
        const span: [number, number] = [at, at + (member.length ?? 0.0)];
        if (!(start <= span[0] && end >= span[1])) {
            this.resync(wid);
            this.reason =
                "a cut across a clip is a new length for its samples, " +
                "which is the buffer owner's edit";
            return false;
        }
        const owner = placed.owner;
        if (owner === null) return false;
        const node = this.nodeId(owner);
        if (node === null) return false;
        // The members as they would stand: the document's own serialization,
        // minus the one being cut.
        const whole = toDocument(owner, { version: this.version }).root;
        const handles = owner.handles;
        const keep = ((whole.members ?? []) as unknown[]).filter(
            (_m, i) => handles[i] !== member,
        );
        const outcome = this.record(
            { intent: "setmembers", node, members: keep },
            "cut the clip",
        );
        if (outcome === null || !outcome.applied) {
            this.resync(wid);
            return false;
        }
        this.project(outcome.effective);
        return true;
    }

    /**
     * A paste asked for over a view, carrying the clipboard with it.
     *
     * **What this editor can place is elements.** A block of *samples* is
     * samples, and samples are written by whoever owns them against a working
     * copy; an arrangement editor placing a nameless block of audio would be
     * inventing both a source and a source's owner.
     */
    private applyPaste(wid: number, values: readonly unknown[]): boolean {
        if (!this.clips.has(wid) && !this.lanes.has(wid)) return false;
        const kind = values.length > 1 ? String(values[1]) : "";
        this.reason =
            `this editor places elements; a ${kind || "clipboard"} block is ` +
            "samples, and samples are written by their owner";
        return false;
    }

    /**
     * The **samples under the current selection**, through the crate.
     *
     * The other half of what a selection is for: `selection` says what was swept,
     * and this says what is underneath it — one entry per leaf, with the
     * placement's base, the element's trim and the clamp at both ends already
     * applied. Empty when nothing with samples was under the sweep.
     */
    resolveSelection(): Resolved[] {
        if (Object.keys(this.selection).length === 0) return [];
        const [, document] = this.history();
        return document.resolve(this.selection as Selection, this.unitsPerBeat, true);
    }

    /**
     * Whether the host reported a **window** that is not the one the element
     * already reads — half a frame's worth, the same threshold a move uses.
     */
    private windowMoved(placed: Placed, start: number | null): boolean {
        if (start === null || placed.member === null) return false;
        const element = placed.member.element;
        return element instanceof Vector && Math.abs(Number(start) - element.start) >= 0.5;
    }

    /**
     * A **trim**: the clip begins later or ends earlier, and the window over its
     * samples moves with the edge.
     *
     * One intent, not two. Where a clip sits is its placement's and what it reads
     * is its element's, so a trim touches both — and a gesture that recorded them
     * separately would take two undos to reverse, the first of which leaves a
     * clip showing frames it does not play.
     */
    private trim(placed: Placed, offset: number, dur: number, start: number): boolean {
        const { member, owner } = placed;
        if (member === null || owner === null) return false;
        const node = this.nodeId(owner);
        const index = owner.handles.indexOf(member);
        if (node === null || index < 0) return false;
        const whole = toDocument(owner, { version: this.version }).root;
        const members = ((whole.members ?? []) as Record<string, unknown>[]).map((m) => ({ ...m }));
        const edited = members[index];
        if (edited === undefined) return false;
        edited.offset = this.unitsToBeats(offset) - placed.base;
        edited.dur = this.unitsToBeats(dur);
        const holder = { ...(edited.node as Record<string, unknown>) };
        holder.config = { ...((holder.config as Record<string, unknown>) ?? {}), start: Number(start) };
        edited.node = holder;
        const outcome = this.record(
            { intent: "setmembers", node, members },
            "trim the clip",
        );
        if (outcome === null || !outcome.applied) {
            this.resync(this.widgetOf(member.element, member) ?? 0);
            return false;
        }
        this.project(outcome.effective);
        return true;
    }

    /**
     * A clip cut in two at `atUnits` of its own time.
     *
     * Both halves keep the **same samples** and take a window of it: the first
     * reads what it always did and stops early, the second begins where the first
     * left off. That is the whole of what a split is on a memory view, and it is
     * why the frames neither of them shows are still there.
     *
     * The second half is built here rather than left for the projection to
     * invent, because an element is an object this client holds and a document
     * node only *describes* one. What goes into the log is **one** intent over the
     * parent's members, so an undo puts the clip back whole.
     */
    private applySplit(placed: Placed, atUnits: number): boolean {
        const { member, owner } = placed;
        if (member === null || owner === null) return false;
        const element = member.element;
        if (!(element instanceof Vector) && !(element instanceof Segments)) {
            // Only a window onto samples can be cut into windows.
            this.reason = `only a clip over samples can be split: this one holds ${nameOf(element)}`;
            return false;
        }
        const length = member.length ?? element.duration;
        const at = this.unitsToBeats(atUnits);
        if (length === null || !(at > 0.0 && at < Number(length))) return false;
        const node = this.nodeId(owner);
        if (node === null) return false;
        const second = this.tailElement(element, at, Number(length));
        // The cut, on the arrangement: the first half stops early — its
        // *placement* does, the element is untouched — and the second is placed
        // where it stops. Stamped with an id of its own **before** any conversion
        // sees it, or the next one would renumber the tree around it.
        const wasDur = member.dur;
        member.dur = at;
        const handle = owner.add(second, member.offset + at, Number(length) - at);
        setDocId(handle, this.mintId());
        const whole = toDocument(owner, { version: this.version }).root;
        const outcome = this.record(
            { intent: "setmembers", node, members: (whole.members ?? []) as unknown[] },
            "split the clip",
        );
        if (outcome === null || !outcome.applied) {
            // Put the arrangement back: the log refused, so nothing happened.
            owner.remove(handle);
            member.dur = wasDur;
            this.resync(this.widgetOf(element, member) ?? 0);
            return false;
        }
        // No projection: the tree already *is* what the intent says. What the
        // index has to learn is the element that was not there a moment ago.
        this.rederive = true;
        return this.changed();
    }

    /**
     * The element the **second half** of a cut reads: the same samples, from `at`
     * beats in.
     *
     * The first half is not built at all — it is the element it always was, with
     * its placement shortened, which is the arrangement's own rule and what makes
     * an undo of a split one step.
     */
    private tailElement(element: Vector | Segments, at: number, length: number): Element {
        if (element instanceof Segments) {
            const after: Segment[] = [];
            for (const [offset, seg] of element.placed()) {
                const end = offset + seg.duration;
                if (offset >= at - 1e-9) {
                    after.push(seg);
                } else if (end > at + 1e-9) {
                    const head = at - offset;
                    after.push(
                        new Segment(
                            seg.buffer,
                            seg.start + this.beatsToUnits(head),
                            seg.duration - head,
                        ),
                    );
                }
            }
            return this.joinedElement([element], after);
        }
        return new Vector(element.buffer, null, length - at, {
            instrument: element.instrument,
            controls: element.controls,
            start: element.start + this.beatsToUnits(at),
            loop: element.loop,
            name: element.name,
        });
    }

    /**
     * The segments a placement of `length` beats actually shows of a `Segments` —
     * the placement being a window onto the samples like every other placement
     * here, so a half whose placement was shortened by a split holds the samples
     * it *plays*.
     */
    private segmentsWithin(element: Segments, length: number | null): Segment[] {
        if (length === null) return element.segments;
        const out: Segment[] = [];
        for (const [offset, seg] of element.placed()) {
            if (offset >= Number(length) - 1e-9) break;
            const room = Number(length) - offset;
            out.push(
                seg.duration <= room + 1e-9
                    ? seg
                    : new Segment(seg.buffer, seg.start, room),
            );
        }
        return out;
    }

    /**
     * Clips read as one.
     *
     * Two shapes, one verb, and which one it takes is a fact about the samples
     * rather than a mode: fragments that are **one run of one buffer** (what a
     * split makes) join back into the single window they were cut from, and
     * anything else becomes a `Segments` — the element whose contents are a list
     * of windows read back to back.
     */
    private applyJoin(placed: Placed, ids: number[]): boolean {
        const { member, owner } = placed;
        if (member === null || owner === null) return false;
        const run = ids
            .map((i) => this.clips.get(i))
            .filter(
                (p): p is Placed => p !== undefined && p.member !== null && p.owner === owner,
            );
        if (run.length < 2) return false;
        run.sort((a, b) => (a.member as Member).offset - (b.member as Member).offset);
        const elements = run.map((p) => (p.member as Member).element);
        if (!elements.every((e) => e instanceof Vector || e instanceof Segments)) {
            this.reason = "only clips over samples can be joined";
            return false;
        }
        // The segments the run holds, in reading order: a `Vector` is one, a
        // `Segments` is however many it already carries.
        const segments: Segment[] = [];
        for (let i = 0; i < run.length; i++) {
            const p = run[i] as Placed;
            const element = elements[i] as Vector | Segments;
            const length = (p.member as Member).length ?? element.duration;
            if (length === null) {
                this.reason = "a clip with no length has no samples to join";
                return false;
            }
            if (element instanceof Segments) {
                segments.push(...this.segmentsWithin(element, length));
            } else {
                segments.push(new Segment(element.buffer, element.start, Number(length)));
            }
        }
        const node = this.nodeId(owner);
        if (node === null) return false;
        const joined = this.joinedElement(elements as (Vector | Segments)[], segments);
        const keep = (run[0] as Placed).member as Member;
        const dropped = new Set(run.slice(1).map((p) => p.member as Member));
        const total = segments.reduce((sum, seg) => sum + seg.duration, 0.0);
        // The members as they would stand — built rather than mutated, which is
        // the cut's shape too: nothing on this side moves until the crate has
        // said what the edit becomes.
        const whole = toDocument(owner, { version: this.version }).root;
        const handles = owner.handles;
        const members: Record<string, unknown>[] = [];
        ((whole.members ?? []) as Record<string, unknown>[]).forEach((m, i) => {
            const handle = handles[i];
            if (handle === undefined || dropped.has(handle)) return;
            const copy = { ...m };
            if (handle === keep) {
                copy.dur = total;
                copy.node = { ...(copy.node as Record<string, unknown>), ...leafNode(joined) };
            }
            members.push(copy);
        });
        const outcome = this.record(
            { intent: "setmembers", node, members },
            "join the clips",
        );
        if (outcome === null || !outcome.applied) return false;
        // The kept placement now holds a different element, which the projection
        // cannot invent from a node: it is written here, where the object is.
        keep.element = joined;
        keep.dur = total;
        this.project(outcome.effective);
        this.rederive = true;
        return this.changed();
    }

    /**
     * What a run of clips joins **into**: the single window they were cut from
     * when they are one run of one buffer, else the `Segments` that reads their
     * windows back to back.
     *
     * The first case is not an optimization — it is what makes a join the inverse
     * of a split.
     */
    private joinedElement(elements: (Vector | Segments)[], segments: Segment[]): Element {
        const first = elements[0] as Vector | Segments;
        const instrument = first.instrument;
        const controls = first.controls;
        const head = segments[0];
        if (head === undefined) return first;
        let contiguous = true;
        let expected = head.start;
        for (const seg of segments) {
            if (seg.buffer !== head.buffer || Math.abs(seg.start - expected) >= 0.5) {
                contiguous = false;
                break;
            }
            expected += this.beatsToUnits(seg.duration);
        }
        const total = segments.reduce((sum, seg) => sum + seg.duration, 0.0);
        if (contiguous) {
            return new Vector(head.buffer, null, total, {
                instrument,
                controls,
                start: head.start,
                loop: first instanceof Vector ? first.loop : false,
                name: first.name,
            });
        }
        return new Segments(segments, null, null, { instrument, controls, name: first.name });
    }

    /**
     * A curve edited in place on an automation clip: the break-points go back
     * onto the element's `Automation`, with their times converted from timeline
     * units to beats. The `Env` is the automation's source of truth, so this *is*
     * the edit — the next render plays the curve as drawn.
     */
    private applyPoints(placed: Placed, values: readonly unknown[]): boolean {
        const clip = placed.member !== null ? placed.member.element : this.element;
        // **The leaf that carries the curve, not the clip that draws it.** A
        // simultaneous aggregate is one clip with its members' bodies layered,
        // so an envelope drawn on it belongs to a *member* — and a `configure`
        // addressed to the aggregate replaced an empty configuration with a
        // `points` the crate had nowhere to keep: the edit reported success,
        // changed nothing and left no undo behind.
        const [element, member] = curveOwner(clip, placed.member);
        const auto = automationOf(element);
        if (auto === null || values.length === 0) return false;
        const flat: number[] = [];
        for (const [t, v, shape, curve] of quads(values.map((x) => Number(x)))) {
            flat.push(this.unitsToBeats(t), Number(v), Math.trunc(shape), Number(curve));
        }
        const node = this.nodeId(element, member);
        if (node === null) return false;
        // **Through the log, like every other edit.** A curve's break-points are a
        // leaf's configuration, so the intent is a `configure` — and it replaces
        // the configuration whole, which is why it starts from what the leaf
        // already carries rather than from the points alone.
        const config = leafConfig(element);
        config.points = flat;
        const outcome = this.record({ intent: "configure", node, config }, "edit the curve");
        if (outcome === null) return false;
        // The effective value is the crate's, and `configure` is the one door
        // that writes it onto the automation *and* refills the control buffer the
        // lane synth reads — so the envelope, the sound and the picture cannot
        // disagree about which of the three happened.
        this.project(outcome.effective);
        return this.changed();
    }

    /**
     * Notes edited in a roll — a clip's body or the dedicated piano-roll alike:
     * rebuilt onto the element's editable `Timeline` as `Event`s, times converted
     * to beats, preserving any OSC items already on it. Answers `false` for a
     * forward-only generator element (read-only), so the edit is a no-op.
     */
    private applyNotes(element: Element, values: readonly unknown[]): boolean {
        const timeline = editableTimeline(element);
        if (timeline === null) return false;
        const node = this.nodeId(element);
        if (node === null) return false;
        const fresh: [number, SeqEvent][] = [];
        for (const [start, dur, pitch, vel, channel] of quintuples(values.map((x) => Number(x)))) {
            const params: Record<string, unknown> = {
                midinote: Math.trunc(pitch),
                dur: this.unitsToBeats(dur),
                amp: Math.max(0.0, Math.min(1.0, Math.trunc(vel) / 127.0)),
                velocity: Math.trunc(vel),
                legato: 1.0,
            };
            if (Math.trunc(channel)) params.channel = Math.trunc(channel);
            fresh.push([this.unitsToBeats(start), new SeqEvent(params)]);
        }
        // **Through the log**: the roll's edit is a `setmembers` — "notes added,
        // moved and removed arrive as the resulting list. Members keep their
        // ids". Keeping them is the whole difficulty, because the payload carries
        // no ids: a roll sends the resulting notes in order, so **order is the
        // only information there is**. The i-th note inherits the i-th note's id
        // and the extras are minted past everything the arrangement holds.
        const kept = [...timeline]
            .filter(([, item]) => pitchOf(item) !== null)
            .map(([, item]) => docIdOf(item));
        const members = fresh.map(([beat, event], i) => {
            const nid = kept[i] ?? this.mintId();
            return {
                offset: Number(beat),
                node: { id: Math.trunc(nid ?? this.mintId()), kind: "clang", config: { ...event.props } },
            };
        });
        const outcome = this.record({ intent: "setmembers", node, members }, "edit the notes");
        if (outcome === null) return false;
        this.project(outcome.effective);
        return this.changed();
    }

    /**
     * A node id nothing in this arrangement holds, for a note a gesture added.
     * Follows the conversion's own rule, so a minted id and a converted one
     * cannot collide.
     */
    private mintId(): number {
        this.nextNode ??= nextNodeId(this.element);
        const nid = this.nextNode;
        this.nextNode += 1;
        return nid;
    }

    /**
     * One edit on a logical aggregate's directed patch. A `"wire"` rewrites the
     * two members' controls so they share a bus — the connection *is* a bus, the
     * same fact `Aggregate.toGraphdef` reads. A `"move"` only persists the box's
     * canvas position (a signal graph has no timeline, so positions are the
     * editor's, not the arrangement's).
     */
    private applyPatch(wid: number, tag: string, values: readonly unknown[]): boolean {
        const found = this.patches.get(wid);
        if (found === undefined) return false;
        const [aggregate, handles] = found;
        if (tag === "wire" && values.length >= 4) {
            return this.applyWire(aggregate, handles, values.slice(0, 4));
        }
        if (tag === "move" && values.length >= 3) {
            const geometry = this.patchGeometry.get(aggregate) ?? {};
            geometry[Math.trunc(Number(values[0]))] = [Number(values[1]), Number(values[2])];
            this.patchGeometry.set(aggregate, geometry);
            return false;
        }
        return false;
    }

    /**
     * Draw a cord `src.outlet → dst.inlet` onto the arrangement: name the bus the
     * connection implies (reusing one either end already writes/reads, else a
     * fresh name declared on the aggregate) and point both members' controls at
     * it. The bus rate comes from the source outlet's def.
     */
    private applyWire(
        aggregate: Aggregate,
        handles: Member[],
        values: readonly unknown[],
    ): boolean {
        const srcBox = Math.trunc(Number(values[0]));
        const outlet = String(values[1]);
        const dstBox = Math.trunc(Number(values[2]));
        const inlet = String(values[3]);
        if (srcBox < 0 || srcBox >= handles.length || dstBox < 0 || dstBox >= handles.length) {
            return false;
        }
        const src = (handles[srcBox] as Member).element as Element & {
            controls?: Record<string, unknown> | null;
        };
        const dst = (handles[dstBox] as Member).element as Element & {
            controls?: Record<string, unknown> | null;
        };
        const rate = outletRate(src, outlet);
        if (rate === null) return false; // a port-less member, or an unknown outlet
        const srcCtls = { ...(src.controls ?? {}) };
        const dstCtls = { ...(dst.controls ?? {}) };
        const bus =
            namedBus(srcCtls[outlet]) ?? namedBus(dstCtls[inlet]) ?? this.freshBus(aggregate);
        srcCtls[outlet] = bus;
        dstCtls[inlet] = bus;
        src.controls = srcCtls;
        dst.controls = dstCtls;
        aggregate.declareBus(bus, rate);
        // The one gesture left that writes the arrangement *directly*: a cord is
        // a pair of controls naming a bus, which no intent describes yet. The
        // held document is behind after it, and says so.
        this.rederive = true;
        this.changed();
        return true;
    }

    /**
     * A bus name not yet declared on `aggregate` (`w0`, `w1`, …) — the private
     * wire a brand-new cord introduces.
     */
    private freshBus(aggregate: Aggregate): string {
        const taken = new Set(aggregate.busNames);
        let i = 0;
        while (taken.has(`w${i}`)) i += 1;
        return `w${i}`;
    }

    /**
     * The OSC events of an element as `[timeUnits, label]` pairs — the
     * piano-roll's event lane. Display only: a marker carries the time and a
     * label, not the full message, so it is not written back.
     */
    private oscOf(element: Element): [number, string][] {
        if (element instanceof Aggregate || element instanceof Vector) return [];
        let events: [number, unknown][];
        try {
            events = flatten(element, 0.0);
        } catch {
            return [];
        }
        const out: [number, string][] = [];
        for (const [beat, item] of events) {
            if (item instanceof OscEvent) {
                out.push([this.beatsToUnits(beat), String(item.message[0])]);
            }
        }
        return out;
    }

    /**
     * The arrangement was edited: mark it, and re-render now when `follow` is on.
     * Otherwise the edit simply waits — a render already in flight is not
     * interrupted, and the next one plays the piece as it now stands, because
     * rendering always re-flattens the tree.
     */
    private changed(): boolean {
        this.dirty = true;
        this.version += 1;
        this.followRender();
        return true;
    }

    // ---- the history: the crate's log, over the crate's document -----------

    /**
     * The log and the document — **one of each, held** for as long as this editor
     * is drawing this composition.
     *
     * Rebuilding them per gesture would hand back the whole of what holding the
     * tree in the crate won: converting the arrangement and opening a fresh
     * handle is linear in the composition, against a constant for the edit
     * itself. Held, a drag costs the edit.
     */
    private history(): [Log, Document] {
        this.log ??= new Log();
        if (this.doc === null || this.rederive) {
            const document = toDocument(this.element, { version: this.version });
            this.doc?.free();
            this.doc = new Document(document);
            this.byNode = new Map();
            this.index(this.element, null, null);
            this.nextNode = nextNodeId(this.element);
            this.rederive = false;
        }
        return [this.log, this.doc];
    }

    /**
     * Walk the arrangement collecting node id → what an intent writes to.
     *
     * A `place` needs the owning aggregate and the member handle (a placement is
     * the aggregate's, not the element's); everything else needs the element. The
     * walk mirrors `form/document.ts`'s own, which is what keeps the two agreeing
     * about what has an id.
     */
    private index(element: Element, owner: Aggregate | null, member: Member | null): void {
        // The id belongs to the **placement** when there is one: a clip is a
        // window onto samples, so what an intent names is the window.
        const node = docIdOf(member ?? element);
        if (node !== null) this.byNode.set(node, [owner, member, element]);
        if (element instanceof Aggregate) {
            for (const handle of element.handles) {
                this.index(handle.element, element, handle);
            }
        }
    }

    /**
     * The document id of an arrangement element, building the document if that is
     * what it takes.
     *
     * **The id is the placement's**, so a caller holding the member handle passes
     * it and gets that window's node. Without one the element is looked up in the
     * index, which answers when it is placed **once** and declines when it is
     * placed twice — there being no way to tell from an element alone which of
     * its windows an edit meant.
     */
    private nodeId(element: Element | Aggregate, member: Member | null = null): number | null {
        if (member !== null) {
            let node = docIdOf(member);
            if (node === null) {
                this.history();
                node = docIdOf(member);
            }
            return node;
        }
        // **The index first, the stamp second.** An aggregate's id is the
        // *placement's* — the handle that holds it in its parent — so the number
        // on the aggregate object is not the document's answer: converting a
        // subtree on its own numbers that subtree as a root and stamps its top.
        this.history();
        const found = [...this.byNode.entries()].filter(([, [, , held]]) => held === element);
        if (found.length === 1) return (found[0] as [number, unknown])[0];
        let node = docIdOf(element);
        if (node === null) {
            this.history();
            node = docIdOf(element);
        }
        return node;
    }

    /**
     * Apply one edit **through the crate**, recording its inverse.
     *
     * This is what makes the editor's own gestures undoable, and it is also where
     * the deciding happens: the crate snaps a placement to `quant`, refuses an
     * edit to a node it cannot find, and reports an edit made against a version
     * the document has left behind. What comes back is the **effective** value,
     * which `project` then writes onto the arrangement — so this editor decides
     * nothing an intent could decide.
     */
    private record(intent: Intent, label: string): Outcome | null {
        const [log, document] = this.history();
        const against: Against = { version: this.version };
        return log.apply(document, intent, { against, quant: this.quant, label });
    }

    /**
     * Write an intent's value onto the arrangement, and say which widget was
     * drawing it.
     *
     * The editor is to the document what the host is to the editor: it emits an
     * intent and adopts the value that comes back. Nothing here decides anything
     * — the snap, the clamp and the refusal already happened in the crate.
     *
     * It also keeps the **drawn record** in step. An undo reaches the arrangement
     * through here and nowhere else, so without it the registry would still hold
     * the position the hand dropped the clip at — and a correction is read
     * straight out of that registry, so an undo would move the model, tell the
     * host to go on drawing the clip exactly where it was, and look like a dead
     * button.
     */
    private project(intent: Intent): Set<number> {
        const found = this.byNode.get(Math.trunc(Number(intent.node ?? -1)));
        if (found === undefined) return new Set();
        const [owner, member, element] = found;
        const moved = new Set<number>();
        if (intent.intent === "place" && owner !== null && member !== null) {
            owner.move(member, Number(intent.offset));
            // **A `place` states the whole placement, so absence is a value.**
            // An intent with no `dur` says this member takes the element's own
            // length again — which is exactly what the inverse of the *first*
            // resize of a clip has to say. `Aggregate.move` reads a null as
            // "leave the length alone", the opposite, so it is written here
            // instead of passed to it.
            member.dur = intent.dur === undefined || intent.dur === null
                ? null
                : Number(intent.dur);
        } else if (intent.intent === "configure") {
            if (!this.configure(element, (intent.config ?? {}) as Record<string, unknown>)) {
                return new Set();
            }
        } else if (intent.intent === "setmembers") {
            const members = (intent.members ?? []) as Record<string, unknown>[];
            // Two things carry members and they are not the same thing: an
            // `Aggregate`'s placements, and the notes of an editable timeline.
            // The element decides which, because the intent names a node and the
            // node is whichever of the two it is.
            if (element instanceof Aggregate) {
                if (!this.setPlacements(element, members)) return new Set();
                // ...and the drawn record of every member it states, since the
                // clip a trim or a split moved is not the aggregate the intent
                // names.
                for (const handle of element.handles) {
                    const wid = this.redrawn(handle.element, handle);
                    if (wid !== null) moved.add(wid);
                }
            } else if (!this.setNotes(element, members)) {
                return new Set();
            }
        } else {
            return new Set();
        }
        const wid = this.redrawn(element, member);
        if (wid !== null) moved.add(wid);
        return moved;
    }

    /**
     * Write a leaf's configuration onto what the arrangement holds.
     *
     * One curve, one door: the projection of an inverse, the adoption of a redone
     * document and the edit itself all land here, so the envelope the script
     * holds, the buffer it sounds through and the picture cannot disagree about
     * which of the three happened.
     */
    private configure(element: Element, config: Record<string, unknown>): boolean {
        if (element instanceof Vector) {
            // A take's configuration is the **window** it reads. The
            // configuration is written whole, so a key the intent does not carry
            // is the default — reading from the first frame, once.
            element.start = Number(config.start ?? 0.0);
            element.loop = Boolean(config.loop ?? false);
            return true;
        }
        const auto = automationOf(element);
        const flat = config.points as number[] | undefined;
        if (auto === null || flat === undefined) return false;
        auto.env = pointsToEnv([...flat]);
        auto.refill();
        return true;
    }

    /**
     * Bring the **drawn record** of one placement back in step with the
     * arrangement, and say which widget draws it.
     */
    private redrawn(element: Element, member: Member | null): number | null {
        const wid = this.widgetOf(element, member);
        const placed = wid === null ? undefined : this.clips.get(wid);
        if (placed !== undefined && member !== null) {
            placed.offset = this.beatsToUnits(member.offset + placed.base);
            // Through the same rule the draw used, not a shorter one of its
            // own: a placement whose length went back to *unstated* is drawn at
            // the element's own length, and a member with neither reaches the
            // element's extent — which `member.length` alone cannot say, so the
            // record kept the size the gesture left.
            placed.dur = this.drawnDur(element, member);
        }
        return wid;
    }

    /**
     * A `setmembers` onto an `Aggregate`: the placements as the document states
     * them, whole.
     *
     * **Only what the document still names survives**, which is what makes a cut a
     * removal and an undo of it a restoration: the members arrive by node id, so a
     * handle whose id is no longer among them leaves, and the ones that stay keep
     * their identity rather than being rebuilt (a rebuilt handle would be a
     * different object to the widget registry).
     */
    private setPlacements(aggregate: Aggregate, members: Record<string, unknown>[]): boolean {
        const keep = new Set(
            members
                .map((m) => (m.node as Record<string, unknown> | undefined)?.id)
                .filter((id): id is number => id !== undefined)
                .map((id) => Math.trunc(Number(id))),
        );
        const byId = new Map<number, Member>();
        for (const handle of aggregate.handles) {
            const node = docIdOf(handle);
            if (node === null) continue;
            byId.set(node, handle);
            if (!keep.has(node)) aggregate.remove(handle);
        }
        // ...and the offsets the document states, for the ones that stayed — plus
        // the ones that are **back**, which is what an undo of a cut is. The
        // element itself outlives its placement (the node index still names it),
        // so restoring is placing that same object again rather than rebuilding
        // one.
        for (const m of members) {
            const holder = (m.node ?? {}) as Record<string, unknown>;
            if (holder.id === undefined) continue;
            const node = Math.trunc(Number(holder.id));
            const handle = byId.get(node);
            const offset = Number(m.offset ?? 0.0);
            // A member carries its node, and a node carries what the leaf is
            // configured as: a trimmed take's window, an edited curve's points.
            // Written here so one `setmembers` states the whole of what a
            // gesture did — and so an undo of it restores both. **Absence is a
            // value**: a node with no configuration is a leaf configured as it
            // was made, which is what the state before a trim is, so the empty
            // table is written rather than skipped. Skipping it left the window
            // over the samples where the trim had put it while the placement
            // went back — a clip the right size showing the wrong frames.
            const config = (holder.config ?? {}) as Record<string, unknown>;
            const target = this.byNode.get(node);
            if (target !== undefined) this.configure(target[2], config);
            if (handle !== undefined) {

                aggregate.move(handle, offset);
                // ...and the length the document states, which a split, a join
                // and an undo of either all change.
                handle.dur = m.dur === undefined || m.dur === null ? null : Number(m.dur);
                continue;
            }
            if (target !== undefined) {
                const restored = aggregate.add(
                    target[2],
                    offset,
                    m.dur === undefined || m.dur === null ? null : Number(m.dur),
                );
                // **The placement keeps the id the document gave it.** A handle
                // that came back unstamped is a new node to the next conversion,
                // which renumbers the tree under every intent still naming the
                // old one.
                setDocId(restored, node);
            }
        }
        return true;
    }

    /**
     * A `setmembers` onto an element's editable timeline: the notes as the
     * document states them, whole. Answers whether it landed.
     */
    private setNotes(element: Element, members: Record<string, unknown>[]): boolean {
        const timeline = editableTimeline(element);
        if (timeline === null) return false;
        const fresh: [number, unknown][] = [];
        for (const placed of members) {
            const node = (placed.node ?? {}) as Record<string, unknown>;
            const config = (node.config ?? {}) as Record<string, unknown>;
            if (!("midinote" in config)) continue;
            fresh.push([Number(placed.offset ?? 0.0), new SeqEvent({ ...config })]);
        }
        rewriteTimeline(timeline, (item) => pitchOf(item) === null, fresh);
        return true;
    }

    /**
     * Which widget is drawing this element — the id→object route read backwards,
     * which is what an undo needs in order to correct the picture without
     * redefining the window.
     */
    private widgetOf(element: Element, member: Member | null = null): number | null {
        for (const [wid, placed] of this.clips) {
            if (member !== null && placed.member === member) return wid;
            if (placed.member !== null && placed.member.element === element) return wid;
        }
        for (const [wid, drawn] of this.rolls) {
            if (drawn === element) return wid;
        }
        // **A layered clip draws an aggregate, and an edit inside it names a
        // member.** A simultaneous aggregate is one clip with its members'
        // bodies over each other, so the curve an edit configures is not the
        // element any clip is registered against — and without this an undo of
        // that curve moved the model and told the host nothing, which is a dead
        // button with the drawing left on the edited shape.
        for (const [wid, placed] of this.clips) {
            const held = placed.member !== null ? placed.member.element : this.element;
            if (
                held instanceof Aggregate &&
                held.handles.some((h) => h === member || h.element === element)
            ) {
                return wid;
            }
        }
        return null;
    }

    /**
     * Step back one edit, and tell the host what to draw instead. The inverse is
     * an ordinary intent, so undoing needs no second path.
     */
    undo(): boolean {
        return this.step((log, doc) => log.undo(doc)?.undone, "undone");
    }

    /**
     * Step forward again after `undo`.
     *
     * A step the crate **cannot perform** — a deterministic operation kept as its
     * parameters rather than as a span — comes back for its owner to re-run.
     * Nothing in the multitrack editor produces one yet.
     */
    redo(): boolean {
        return this.step((log, doc) => log.redo(doc)?.redone, "redone");
    }

    private step(
        walk: (log: Log, doc: Document) => Intent[] | undefined,
        _key: string,
    ): boolean {
        if (this.log === null) return false;
        const [log, document] = this.history();
        const intents = walk(log, document);
        if (intents === undefined) return false;
        const widgets = new Set<number>();
        for (const intent of intents) {
            for (const wid of this.project(intent)) widgets.add(wid);
        }
        this.version = document.version;
        this.dirty = true;
        this.followRender();
        this.corrections = [];
        for (const wid of widgets) this.resync(wid);
        this.acknowledge(0);
        this.corrections = [];
        return true;
    }

    /** Whether there is an edit to step back over. */
    get canUndo(): boolean {
        return this.log !== null && this.log.canUndo;
    }

    /** Whether there is an undone edit to step forward into. */
    get canRedo(): boolean {
        return this.log !== null && this.log.canRedo;
    }

    /** What an undo would be called, for a menu item. */
    get undoLabel(): string | undefined {
        return this.log?.undoLabel;
    }

    /**
     * Re-schedule after an edit when `follow` is on **and there is something to
     * re-schedule**.
     *
     * The guard has two halves. `rerender` needs a destination and a clock, which
     * only a `render` or a `play` supplies. And what `follow` means is *what is
     * sounding follows the edit* — so it re-schedules a pass in flight and
     * *starts* nothing: an edit made while the transport is stopped would
     * otherwise have the drag itself press play.
     */
    private followRender(): void {
        if (this.follow && this.destination !== null && this.transport.playing) {
            void this.rerender();
        }
    }

    // ---- rendering: the edited arrangement back to sound ----

    /**
     * Render the composition onto `destination` — RT (a `Server` and a running
     * clock) or NRT (a score) — and anchor the lanes' playhead so the line sweeps
     * the clips as it plays.
     *
     * This is the arrangement's own `render` (flatten to absolute beats, play
     * through a playhead): the editor adds no rendering path of its own, it only
     * remembers the destination so `rerender` can re-schedule after an edit.
     */
    async render(
        destination: unknown,
        clock?: TempoClock,
        { at = 0.0 }: { at?: number } = {},
    ): Promise<Playhead | null> {
        this.destination = destination;
        this.clock = clock ?? null;
        const playhead = await this.transport.play(destination as Server, { at });
        this.dirty = false; // what plays now *is* the arrangement
        return playhead;
    }

    /**
     * One pass for the `transport`: the arrangement, flattened and played from
     * beat `at`. Called afresh on every play, which is what makes a play — or a
     * resume, or a seek — read the composition as it now stands.
     */
    private renderPass(at: number): Playhead | null {
        const out = renderElement(this.element, this.destination, this.clock, { at });
        return out instanceof Promise ? null : (out as Playhead);
    }

    /**
     * Re-schedule the (edited) composition from the playhead's current position:
     * stop, re-flatten, play again.
     *
     * The honest semantics are **re-schedule from here**, not a sample-exact
     * splice — a synth already sounding keeps sounding, and what changes is what
     * has not been scheduled yet.
     */
    rerender({ at }: { at?: number } = {}): Promise<Playhead | null> {
        if (this.destination === null) {
            throw new Error("render(destination, clock) the editor first");
        }
        return this.render(this.destination, this.clock ?? undefined, {
            at: at ?? this.position,
        });
    }

    // ---- the transport: play, pause, stop, locate ----

    /** The transport's position in beats. */
    get position(): number {
        return this.transport.position;
    }

    /**
     * Play (or resume) from the transport's position — a fresh render, so it
     * plays the composition **as it now stands**. Reuses the destination and
     * clock of the last `render` when they are not given.
     */
    play(
        destination?: unknown,
        clock?: TempoClock,
        { at }: { at?: number } = {},
    ): Promise<Playhead | null> {
        const target = destination ?? this.destination;
        if (target === null || target === undefined) {
            throw new Error("nothing to play onto: render(destination, clock) first");
        }
        return this.render(target, clock ?? this.clock ?? undefined, {
            at: at ?? this.transport.at,
        });
    }

    /**
     * Halt where we are: the playhead stops scheduling and the position stays, so
     * a `play` resumes from here. What is already sounding keeps sounding —
     * stopping a playhead is not a panic button.
     */
    pause(): number {
        return this.transport.pause();
    }

    /** Halt and return to the top. */
    stop(): this {
        this.transport.stop();
        return this;
    }

    /**
     * Seek: put the transport at `beat`. Playing, it re-renders from there (so a
     * seek also picks up any edit); stopped, it just moves the cursor the lanes
     * draw.
     *
     * A composition holding a **resident generator** has no position to seek to —
     * its samples are produced on the server, so its position is that def's
     * internal state and no number moves it.
     */
    locate(beat: number): this {
        if (!this.locatable) {
            throw new Error(
                "this composition contains a resident generator, which has no " +
                    "position to locate to; render it first to give it one",
            );
        }
        this.transport.locate(beat);
        return this;
    }

    /**
     * Whether the composition can be seeked at all — false when any element is a
     * resident generator.
     */
    get locatable(): boolean {
        return this.element.locatable;
    }

    /** Continue where `pause` left off, **without re-rendering**. */
    resume(): Promise<Playhead | null> {
        return this.transport.resume();
    }

    /**
     * Anchor every lane's playhead to the engine clock, so the line starts at
     * beat `at` of the timeline and sweeps on with the audio.
     */
    anchor(server: Server, { at = 0.0 }: { at?: number } = {}): Promise<boolean> {
        return this.transport.anchor(server, { at });
    }

    /** Take the sweeping playhead line off the lanes (the cursor, if any, stays). */
    unanchor(): void {
        this.transport.unanchor();
    }

    // ---- the tree walk ----

    /**
     * The lanes an element contributes: a concrete `Aggregate` becomes one lane
     * holding its members as clips (plus a lane of its own for every *expanded*
     * nested aggregate); anything else becomes a lane with one clip. `base` is its
     * start in beats, `owner`/`member` the placement an edit-back writes through.
     */
    private lanesFor(
        element: Element,
        base: number,
        owner: Aggregate | null,
        member: Member | null,
    ): GuiNode[] {
        if (
            element instanceof Aggregate &&
            element.kind === CONCRETE &&
            element.length > 1 &&
            element.temporalRelation() === SIMULTANEOUS &&
            !this.isExpanded(element)
        ) {
            // Its members start and end together: they are *one* thing on the
            // timeline, so they are one clip with layered bodies — not a lane of
            // clips that must be dragged one by one.
            return [this.lane([this.clipFor(element, base, owner, member)], nameOf(element))];
        }
        if (element instanceof Aggregate && element.kind === CONCRETE) {
            const clips: GuiNode[] = [];
            const extra: GuiNode[] = [];
            for (const child of element.handles) {
                const childBase = base + child.offset;
                if (child.element instanceof Aggregate && this.isExpanded(child.element)) {
                    extra.push(...this.lanesFor(child.element, childBase, element, child));
                } else {
                    clips.push(this.clipFor(child.element, childBase, element, child));
                }
            }
            const lane = clips.length > 0 ? [this.lane(clips, nameOf(element))] : [];
            return [...lane, ...extra];
        }
        return [this.lane([this.clipFor(element, base, owner, member)], nameOf(element))];
    }

    /** One `track` lane holding `clips`, with the shared time chrome. */
    private lane(clips: GuiNode[], label: string): GuiNode {
        const wid = this.newId();
        const lane = track(
            {
                id: wid,
                label,
                sampleRate: this.sampleRate,
                tempo: this.tempo,
                snap: this.quant > 0 ? this.beatsToUnits(this.quant) : undefined,
            },
            ...clips,
        );
        this.lanes.set(wid, label);
        return lane;
    }

    /**
     * One `clip`: the element placed at `base` beats (absolute on the shared
     * axis), with the body (or **bodies**) its kind calls for. Registers what it
     * drew, which is what the edit-back path resolves against.
     */
    /**
     * The length one clip is drawn at, **in beats** — the placement's when it
     * overrides, else the element's own, else what the element extends to.
     *
     * One rule, in one place, because two of them is how a picture and a model
     * come to disagree: the draw asks this, and so does every path that has to
     * put a placement back ({@link Editor.redrawn}, after an inverse or a redo).
     */
    private drawnBeats(element: Element, member: Member | null): number {
        let beats = member !== null && member.dur !== null ? member.dur : null;
        if (beats === null && element instanceof Element) beats = element.duration;
        if (beats === null) beats = this.extentOf(element);
        return beats;
    }

    /**
     * The same length in **timeline units**, which needs the body: a take with no
     * duration given is as long as it is (1 unit = 1 sample).
     */
    private drawnDur(
        element: Element,
        member: Member | null,
        body?: Record<string, unknown>,
    ): number {
        const beats = this.drawnBeats(element, member);
        const drawn = body ?? this.bodyFor(element, beats);
        return "buffer" in drawn && beats <= 0.0
            ? Number((element as Vector).buffer.frames ?? 0)
            : this.beatsToUnits(beats);
    }

    private clipFor(
        element: Element,
        base: number,
        owner: Aggregate | null,
        member: Member | null,
    ): GuiNode {
        const wid = this.newId();
        const offset = this.beatsToUnits(base);
        const durBeats = this.drawnBeats(element, member);
        const body = this.bodyFor(element, durBeats);
        const dur = this.drawnDur(element, member, body);

        // The placement's own base: a clip's offset is absolute on the shared
        // axis, a member's offset is relative to its aggregate.
        const parentBase = base - (member !== null ? member.offset : 0.0);
        this.clips.set(wid, new Placed(owner, member, parentBase, offset, dur));
        // A roll body is the `notes` element itself, and it edits: a body carries
        // no id of its own, so a note dragged inside one arrives tagged with
        // *this clip's* id.
        const roll = rollOwner(element);
        if ("notes" in body && roll !== null) this.rolls.set(wid, roll);
        return clip({ id: wid, offset, dur, label: nameOf(element), ...body });
    }

    /**
     * The clip-body props an element draws with — and a **simultaneous** aggregate
     * draws with *all of its members'*, layered in one clip.
     *
     * `limit` is the **placement's** length in beats when it has one: a placement
     * is a window onto an element, so a clip shortened over samples assembled from
     * segments draws the segments it plays and not the ones it no longer reaches.
     *
     * That is the arrangement's own answer to "attach an envelope to the event it
     * shapes": an aggregate whose members start and end together *is* one thing
     * on the timeline, so it is one clip — dragging it moves the whole aggregate,
     * and the bodies overlay instead of hiding each other.
     */
    /**
     * The value axis this curve is drawn against — the one it was **first** drawn
     * against, kept.
     *
     * `curveRange` answers what the break-points alone would ask for, and that is
     * the right answer exactly once: recomputed on every redraw it makes an edit
     * rescale the picture, so dragging one point visibly moves every other one.
     * The axis is therefore remembered per `Automation` and only ever
     * **widened**, when a curve no longer fits inside it (a script replaced the
     * envelope, an undo restored a taller one) — never narrowed, so a point
     * dragged down and back up leaves the drawing where it was.
     */
    private axisFor(
        auto: Automation,
        points: readonly [number, number, number, number][],
    ): [number, number] {
        let [lo, hi] = curveRange(points);
        const kept = this.curveAxis.get(auto);
        if (kept !== undefined) {
            const [klo, khi] = kept;
            const values = points.length > 0 ? points.map(([, v]) => v) : [0.0];
            // **One side at a time.** Only the end that stopped holding the data
            // moves; taking the union of the two padded ranges would drop the
            // floor as well whenever the ceiling grew, which is the same jump one
            // step removed.
            if (Math.min(...values) >= klo) lo = klo;
            if (Math.max(...values) <= khi) hi = khi;
            if (lo === klo && hi === khi) return kept;
        }
        this.curveAxis.set(auto, [lo, hi]);
        return [lo, hi];
    }

    private bodyFor(element: Element, limit: number | null = null): Record<string, unknown> {
        // A simultaneous aggregate first: it is one thing on the timeline, and
        // its members' bodies layer (each keeps its own value axis).
        if (
            element instanceof Aggregate &&
            element.length > 1 &&
            element.temporalRelation() === SIMULTANEOUS
        ) {
            const body: Record<string, unknown> = {};
            for (const m of element.handles) Object.assign(body, this.bodyFor(m.element, limit));
            return body;
        }

        const auto = automationOf(element);
        if (auto !== null) {
            const points = quads(auto.toPoints()).map(
                ([t, v, shape, curve]) =>
                    [this.beatsToUnits(t), v, shape, curve] as [number, number, number, number],
            );
            const [lo, hi] = this.axisFor(auto, points);
            // The curve keeps its own value axis: an envelope's units are not
            // the pitches under it. `points_min`/`points_max` are written in
            // **wire form** because the builder has no option for them — the
            // pass-through door for a prop this client does not name, and the
            // Python editor reaches it the same way.
            //
            // The points go flat, because these quads are already resolved: a
            // `points` argument of tuples is read as `(t, v, curve_spec)`, so a
            // resolved shape number would be re-read as a curvature.
            return { points: points.flat(), points_min: lo, points_max: hi };
        }

        if (element instanceof Segments) {
            const segments = this.segmentsWithin(element, limit);
            // **One clip, one take per segment.** The samples are several windows
            // read as one thing, so the clip holds one body per segment, each
            // over its own stretch of the clip and each reading its own buffer
            // from its own frame.
            const children: GuiNode[] = [];
            let cursor = 0.0;
            for (const seg of segments) {
                const offset = cursor;
                cursor += seg.duration;
                const buf = seg.buffer;
                if (buf.channels === undefined) continue;
                const take: Record<string, unknown> = {
                    view: "trace",
                    buffer: buf.bufnum,
                    channels: Math.max(1, buf.channels),
                    at: this.beatsToUnits(offset),
                    dur: this.beatsToUnits(seg.duration),
                };
                if (seg.start) take.start = Number(seg.start);
                children.push(signal(take));
            }
            return children.length > 0 ? { children } : {};
        }

        if (element instanceof Vector) {
            const buf = element.buffer;
            // The take rides the bulk path: the host fetches the server buffer
            // and decimates it through its peak pyramid.
            //
            // An element this process does not hold draws as a **clip with no
            // waveform** rather than not at all: a session reopened without its
            // sources resolved knows the buffer number the document recorded and
            // nothing about its shape.
            if (buf.channels === undefined) return {};
            const body: Record<string, unknown> = {
                buffer: buf.bufnum,
                channels: Math.max(1, buf.channels),
            };
            // The **window** onto those samples, sent only when there is one to
            // state, which keeps a whole-take clip's props exactly what they were.
            if (element.start) body.start = Number(element.start);
            if (element.loop) body.loop = true;
            return body;
        }

        const notes = this.notesOf(element);
        if (notes.length > 0) {
            const pitches = notes.map((n) => n[2]);
            const body: Record<string, unknown> = {
                notes,
                min: Math.min(Math.min(...pitches) - PITCH_PAD, DEFAULT_PITCH[1]),
                max: Math.max(Math.max(...pitches) + PITCH_PAD, DEFAULT_PITCH[0]),
            };
            // **Say it before the hand tries.** These notes are a *rendering* of a
            // forward-only generator when there is no editable timeline behind
            // them, so the roll refuses the press instead of offering a drag it
            // will unwind.
            //
            // **The roll's own key, not the clip's.** `editable` is a statement
            // about the whole clip and reaches every body it carries, so saying
            // it here locked the *envelope* drawn over these notes as well — a
            // sweep whose curve drew and could not be touched. A body says its
            // own with `notes_editable`, as it already keeps its own value axis
            // with `points_min`/`points_max`.
            // The builder's own name, so the flag is coerced the way every
            // other clip flag is (`0`/`1` on the wire) and the two clients
            // emit one GuiDef rather than two spellings of one.
            if (editableTimeline(element) === null) body.notesEditable = false;
            return body;
        }
        // No body: a collapsed aggregate (or an element with nothing to draw) is
        // the labeled rectangle — the summary of the level above it.
        return {};
    }

    /**
     * The `[start, dur, pitch, velocity, channel]` note events of an element, in
     * timeline units relative to the element — the piano-roll body. An `Aggregate`
     * is a summary, not a roll, and a note is any flattened event that resolves a
     * pitch: the *change of state* of a contained generator happens right here (a
     * pattern is bounced by `flatten`), so a generator lane shows the notes it
     * will play.
     */
    private notesOf(element: Element): Note[] {
        if (element instanceof Aggregate || element instanceof Vector) return [];
        let events: [number, unknown][];
        try {
            events = flatten(element, 0.0);
        } catch {
            return [];
        }
        const notes: Note[] = [];
        for (const [beat, item] of events) {
            const pitch = pitchOf(item);
            if (pitch === null) continue;
            notes.push([
                this.beatsToUnits(beat),
                this.beatsToUnits(eventDur(item)),
                pitch,
                velocityOf(item),
                0,
            ]);
        }
        return notes;
    }

    /**
     * An element's length in beats: its own `duration` when it has one, else what
     * it spans — an aggregate over its placed members, an envelope over its curve,
     * anything else over its flattened events (a bounced pattern included).
     */
    private extentOf(element: Element): number {
        if (element instanceof Element && element.duration !== null) return Number(element.duration);
        const auto = automationOf(element);
        if (auto !== null) return auto.duration();
        if (element instanceof Aggregate) {
            return element.handles.reduce(
                (max, m) => Math.max(max, m.offset + (m.dur ?? this.extentOf(m.element))),
                0.0,
            );
        }
        if (element instanceof Segments) {
            // Its contents are a list, and its extent is the whole of it.
            return element.segments.reduce((sum, seg) => sum + seg.duration, 0.0);
        }
        if (element instanceof Vector) {
            const buf = element.buffer;
            const rate = buf.sampleRate || this.sampleRate;
            return this.unitsToBeats(Number(buf.frames ?? 0) * (this.sampleRate / rate));
        }
        let events: [number, unknown][];
        try {
            events = flatten(element, 0.0);
        } catch {
            return 0.0;
        }
        return events.reduce((max, [beat, item]) => Math.max(max, beat + eventDur(item)), 0.0);
    }
}

// ---- module helpers -------------------------------------------------------

/**
 * A logical `Aggregate` as a `GraphPatch`, through the headless decode
 * `GraphPatch.fromGraphdef`: the aggregate renders to a `GraphDef` (its members
 * and their shared-bus controls — the arrangement's 1:1 logical mapping), and the
 * decode reads that back into a directed patch, typing each box's ports from the
 * `SynthDef` the member wraps. The `Aggregate → patch` mapping itself lives in
 * `defs`, not here — the editor is only a consumer of it.
 *
 * Answers the patch and the member handles in box order (box index == member
 * order), so an edit-back maps a box index back to the member whose controls it
 * rewrites.
 */
function logicalPatch(aggregate: Aggregate): [GraphPatch, Member[]] {
    const handles = aggregate.handles;
    const gdef = aggregate.toGraphdef(aggregate.name || "_patch");
    const defs: Record<string, unknown> = {};
    for (const h of handles) {
        const wraps = (h.element as { wraps?: unknown }).wraps;
        if (wraps instanceof SynthDef) {
            defs[(h.element as unknown as { defName: string }).defName] = wraps;
        }
    }
    return [GraphPatch.fromGraphdef(gdef, defs), handles];
}

/** A port spec's name, whether audio (a bare string) or control (`[name, rate]`). */
const portName = (port: PortSpec): string => (typeof port === "string" ? port : port[0]);

/**
 * A control value that is an internal-bus **name** — a non-empty string that is
 * not the hardware sentinel `"OUT"` — or `null`.
 */
const namedBus = (value: unknown): string | null =>
    typeof value === "string" && value && value !== "OUT" ? value : null;

/**
 * The rate of `member`'s outlet `name`, derived from the `SynthDef` it wraps — or
 * `null` when the member wraps a bare def name or has no such outlet.
 */
function outletRate(member: Element, name: string): "audio" | "control" | null {
    const wraps = (member as { wraps?: unknown }).wraps;
    if (!(wraps instanceof SynthDef)) return null;
    const [, outlets] = synthdefPorts(wraps);
    for (const port of outlets) {
        if (portName(port) === name) return typeof port === "string" ? "audio" : port[1];
    }
    return null;
}

/**
 * One layer's measure, or an error naming it — a stack is written by hand, and a
 * silent typo is a layer that quietly does not appear.
 */
function measureName(name: string): Measure {
    if (!(MEASURES as readonly string[]).includes(name)) {
        throw new Error(`unknown measure ${name} (one of ${MEASURES.join(", ")})`);
    }
    return name as Measure;
}

/**
 * An element's display name: its own `name` when it has one (an aggregate names
 * itself, an automation names the control it drives), else what it *is* — an
 * automation is an "envelope", not the `Element` that happens to wrap it.
 */
function nameOf(element: Element | null): string {
    const name = element?.name;
    if (typeof name === "string" && name) return name;
    const auto = element === null ? null : automationOf(element);
    if (auto !== null) return auto.name || "envelope";
    return (element?.constructor.name ?? "element").toLowerCase();
}

/**
 * The `Automation` an element carries, or `null`. An automation is a *curve* — the
 * List/Vector duality of the arrangement — so it needs no primitive of its own:
 * any element wrapping one draws (and edits) as an envelope.
 *
 * A **simultaneous** aggregate is searched too: an envelope attached to the event
 * it shapes is one clip, and a curve edited on it must find the automation inside.
 */
function automationOf(element: Element): Automation | null {
    const wraps = (element as { wraps?: unknown }).wraps;
    if (wraps instanceof Automation) return wraps;
    if (
        element instanceof Aggregate &&
        element.length > 1 &&
        element.temporalRelation() === SIMULTANEOUS
    ) {
        for (const [, , child] of element.members) {
            const auto = automationOf(child);
            if (auto !== null) return auto;
        }
    }
    return null;
}

/**
 * A flat `[t, v, shape, curve, …]` break-point list as quadruples (a trailing
 * partial quad is dropped).
 */
function quads(flat: readonly number[]): [number, number, number, number][] {
    const out: [number, number, number, number][] = [];
    for (let i = 0; i + 3 < flat.length; i += 4) {
        out.push([
            Number(flat[i]),
            Number(flat[i + 1]),
            Number(flat[i + 2]),
            Number(flat[i + 3]),
        ]);
    }
    return out;
}

/**
 * A flat `[start, dur, pitch, velocity, channel, …]` note list as quintuples — the
 * inverse of the piano-roll's `notes` wire form.
 */
function quintuples(flat: readonly number[]): [number, number, number, number, number][] {
    const out: [number, number, number, number, number][] = [];
    for (let i = 0; i + 4 < flat.length; i += 5) {
        out.push([
            Number(flat[i]),
            Number(flat[i + 1]),
            Number(flat[i + 2]),
            Number(flat[i + 3]),
            Number(flat[i + 4]),
        ]);
    }
    return out;
}

/**
 * The element whose `Automation` a clip's curve body draws — what a `"points"`
 * edit-back is written onto — and the member handle that places it.
 *
 * The mirror of `rollOwner`, and needed for the same reason: a **simultaneous**
 * aggregate draws as one clip with its members' bodies layered, so the curve
 * under the cursor is a member's and the intent has to name that member's node.
 * Anything else answers with itself and the handle it was placed by.
 */
function curveOwner(
    element: Element,
    member: Member | null = null,
): [Element, Member | null] {
    if (
        element instanceof Aggregate &&
        element.length > 1 &&
        element.temporalRelation() === SIMULTANEOUS
    ) {
        for (const h of element.handles) {
            if (automationOf(h.element) !== null) return [h.element, h];
        }
    }
    return [element, member];
}

/**
 * The element whose notes a clip's roll body draws — what a `"notes"` edit-back
 * is written onto.
 *
 * Usually the element itself. A **simultaneous** aggregate is the one that needs
 * asking: it draws as one clip with its members' bodies layered, so the notes
 * under the cursor belong to the member that carries them. `null` when no member
 * has an editable timeline.
 */
function rollOwner(element: Element): Element | null {
    if (
        element instanceof Aggregate &&
        element.length > 1 &&
        element.temporalRelation() === SIMULTANEOUS
    ) {
        for (const m of element.handles) {
            if (editableTimeline(m.element) !== null) return m.element;
        }
        return null;
    }
    return element;
}

/**
 * The `Timeline` an element's notes can be edited onto, or `null`. A `Track`
 * wraps one — the random-access, editable events container; a generator does not,
 * so it is forward-only and the piano-roll shows it read-only.
 */
function editableTimeline(element: Element): Timeline | null {
    const wraps = (element as { wraps?: unknown }).wraps;
    return wraps instanceof Timeline ? wraps : null;
}

/**
 * Rewrite a timeline in place: keep the items `keep(item)` is true for, drop the
 * rest, and add `fresh` — so one kind of item (the notes) is replaced while the
 * others (OSC events) are preserved. Uses only the public timeline API.
 */
function rewriteTimeline(
    timeline: Timeline,
    keep: (item: unknown) => boolean,
    fresh: readonly [number, unknown][],
): void {
    const kept = timeline.range(0.0, Infinity).filter(([, item]) => keep(item));
    timeline.clear();
    for (const [beat, item] of [...kept, ...fresh]) timeline.add(beat, item);
}

/**
 * The value axis of a curve clip: the break-points' own range with a tenth of
 * headroom (a flat curve still gets a band to be dragged in).
 */
function curveRange(points: readonly [number, number, number, number][]): [number, number] {
    const values = points.length > 0 ? points.map((p) => Number(p[1])) : [0.0];
    const lo = Math.min(...values);
    const hi = Math.max(...values);
    const pad = (hi - lo) * 0.1 || Math.abs(hi) * 0.1 || 1.0;
    return [lo - pad, hi + pad];
}

/**
 * The MIDI pitch of a flattened item, or `null` when it carries none — an
 * `OscEvent`, a rest, an automation lane.
 */
function pitchOf(item: unknown): number | null {
    if (!(item instanceof SeqEvent) || item.get("type") === "rest") return null;
    try {
        const pitch = item.midinote();
        return Number.isFinite(pitch) ? Number(pitch) : null;
    } catch {
        return null;
    }
}

/**
 * The MIDI velocity (`0..127`) of a flattened note event: an explicit `velocity`
 * key if given, else the event's linear `amp` mapped to the velocity range, else
 * the default 100.
 */
function velocityOf(item: unknown): number {
    const event = item as SeqEvent;
    const vel = event.get("velocity");
    if (vel !== undefined && vel !== null) {
        return Math.max(0, Math.min(127, Math.trunc(Number(vel))));
    }
    const amp = event.get("amp");
    if (amp !== undefined && amp !== null) {
        return Math.max(1, Math.min(127, Math.round(Number(amp) * 127)));
    }
    return 100;
}

/**
 * A flattened item's length in beats: an event's **sounding** time (`sustain`,
 * which is what a note bar should show), 0 when it is punctual.
 */
function eventDur(item: unknown): number {
    if (item instanceof SeqEvent) {
        try {
            const sustain = item.sustain();
            return Number.isFinite(sustain) ? Number(sustain) : 0.0;
        } catch {
            return 0.0;
        }
    }
    return 0.0;
}
