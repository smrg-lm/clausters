// `FormEditor`: the bridge between the arrangement and the multitrack GUI
// (mirrors `clausters/gui/editing/formeditor.py`).
//
// The driver of the DAW-style view. It draws a `form` tree as a multitrack
// `GuiDef` (tracks of clips on one shared time axis), applies the clip
// edit-backs the host sends straight onto the arrangement, and re-renders it —
// the loop **data ↔ graphic ↔ sound**, which is what makes the composition
// editable at any granularity rather than merely displayable.
//
// It is {@link Editor} — the generic one, over any single structure — plus what
// only a tree has, and the name says which: `Editor` is what a person calls to
// edit a buffer, a curve or a timeline, and it imports nothing from here.
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

import { TempoMap } from "../../base/time.ts";
import { Document, Log } from "../../document.ts";
import type { Against, Intent, Outcome, Resolved, Selection, Step } from "../../document.ts";
import { GraphPatch, synthdefPorts } from "../../defs/patch.ts";
import type { PortSpec } from "../../defs/patch.ts";
import { SynthDef } from "../../defs/synthdef.ts";
import { pointsToEnv } from "../../defs/ugens/index.ts";
import type { Server } from "../../defs/server/index.ts";
import {
    BEATS,
    CONCRETE,
    LOGICAL,
    SECONDS,
    SIMULTANEOUS,
    Aggregate,
    Clang,
    Element,
    Generator,
    Segment,
    Segments,
    Track,
    Vector,
    docIdOf,
    flatten,
    leafConfig,
    leafNode,
    nextNodeId,
    render as renderElement,
    setDocId,
    toBeats,
    toDocument,
} from "../../form/index.ts";
import type { Member } from "../../form/index.ts";
import { FIRST_VERSION, MIXING, setMixing } from "../../form/document.ts";
import { Editing, contexts } from "./context.ts";
import type { Adopting } from "./context.ts";
import { Editor } from "./editor.ts";
import type { Leg } from "./editor.ts";
import { NotesEditor } from "./events.ts";
import { MEASURES, SamplesEditor, isSamples, measures } from "./samples.ts";
import type { Measure } from "./samples.ts";
import type { Buffer } from "../../defs/buffer.ts";
import { Automation } from "../../seq/automation.ts";
import { Event as SeqEvent } from "../../seq/event.ts";
import { MidiItem, OscItem, Timeline } from "../../seq/timeline.ts";
import type { Playhead } from "../../seq/timeline.ts";
import type { TempoClock } from "../../base/clock.ts";
import {
    clip,
    flatNotes,
    flatPoints,
    patch,
    scroll,
    signal,
    timeruler,
    track,
    window as guiWindow,
} from "../guidef.ts";
import type { GuiNode } from "../guidef.ts";
import type { GuiHost, PropValue, Stage } from "../host.ts";
import type { WindowHandle } from "../handle.ts";
import { Transport } from "../transport.ts";

/**
 * The pitch range a piano-roll lane falls back to when its notes give none
 * (C3..C6 — the span a melodic line usually lives in).
 */
const DEFAULT_PITCH: [number, number] = [48.0, 72.0];
/** Semitones of headroom above and below the notes of a piano-roll clip. */
const PITCH_PAD = 2.0;

/** A note as the roll draws it: `[start, dur, pitch, velocity, channel]`. */
type Note = [number, number, number, number, number];

/**
 * What {@link FormEditor.holdKey} files a held value under: the document node
 * (`"node:7"`) when there is one, and the arrangement object itself until the
 * first conversion numbers it.
 */
type HoldKey = string | Element | Aggregate | Member;

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
    /** The piece's starting tempo in **beats per second** (2.0 is 120 bpm). */
    tempo?: number;
    /**
     * The piece's {@link TempoMap}, when its tempo changes along the way — pass
     * the clock's (`TempoClock.map`) so the view and the sound read one
     * function. Ignored fields: given this, `tempo` is not read.
     */
    tempoMap?: TempoMap;
    /** The musical drag grid in beats (`0.25` = a sixteenth); 0 snaps to samples. */
    quant?: number;
    /** Re-render on every edit (the live editor). */
    follow?: boolean;
    /**
     * Whether the views **follow their content**. One switch, and it has three
     * faces, because a picture fits itself to what is in it in three places: the
     * **time window** refits when the composition's length changes, a roll's
     * **pitch domain** re-centres on the notes it holds, and a **clip with no
     * stated length** is drawn as long as its content, so moving the last note
     * of a phrase resized the clip it was in.
     *
     * Off by default here, which is the opposite of the host's own default and
     * deliberate — an editor's content changes are mostly the reader's *own*
     * edits, and an edit that re-frames the view is the window starting over
     * under the hand that made it.
     *
     * The time face is the widgets' `autofit` prop, which the host reads at
     * every door the content reaches a window by. The other two are derived
     * here, so they are held here, by one rule (`FormEditor.fit`).
     */
    autofit?: boolean;
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
/**
 * The host an `open` acts on: the one named, else the ambient one — the same
 * resolution {@link guidef.View.open}, `plot` and `scope` share, so an editor is
 * not the one resource that has to be handed a host.
 *
 * Async where the Python client's `_resolve_host` is not, for the reason
 * `View.open` is async here: resolving the ambient host may have to boot it,
 * and a page boots asynchronously.
 */
async function resolveEditorHost(host?: GuiHost): Promise<GuiHost> {
    if (host !== undefined) return host;
    return (await import("../../plot.ts")).resolveHost();
}

/**
 * The **arrangement's** editing context: a generic one plus the held document,
 * the index between it and the tree, and the id to mint next.
 *
 * They are here rather than in `editing/context.ts` because they are the tree's:
 * a curve or a buffer has a history and no document, and a generic context that
 * carried one would be carrying the arrangement's vocabulary into every other
 * structure's editing.
 */
export class FormEditing extends Editing {
    /** The arrangement's face of the pile. */
    log: Log | null = null;
    /**
     * The crate's held document — opened once and kept, so a gesture costs the
     * edit rather than the composition.
     */
    doc: Document | null = null;
    /**
     * Whether the held document has to be derived from the arrangement again
     * before the next edit. Set wherever the tree moves by a route that is not
     * an intent.
     */
    rederive = false;
    /** node id → the arrangement object an intent naming that node writes to. */
    byNode = new Map<number, Indexed>();
    /** The next node id to mint for a node a gesture creates. */
    nextNode: number | null = null;

    /**
     * The log and the document, deriving the document if that is what it takes.
     *
     * The document is opened once and kept: rebuilding it per gesture handed
     * back the whole of what holding the tree in the crate had won. What a
     * rebuild was quietly doing is explicit here — `toDocument` stamps each
     * element with the id it keeps, so a re-derivation names the same nodes and
     * the history keeps its footing.
     */
    held(element: Element): [Log, Document] {
        this.log ??= new Log(0, 0, this.history);
        if (this.doc === null || this.rederive) {
            const document = toDocument(element, { version: this.version });
            this.doc?.free();
            this.doc = new Document(document);
            // **The index is added to, not replaced.** An element that has left
            // the tree — a clip a cut removed, the half a join swallowed — is
            // still named by the inverses in the pile, and putting it back is
            // placing *that object* again rather than rebuilding one from a
            // node (a rebuilt element is a different identity to every widget
            // and every pending edit). Clearing here made an undo of a cut, and
            // a redo of a split, quietly do nothing.
            this.index(element, null, null);
            this.nextNode = nextNodeId(element);
            this.rederive = false;
        }
        return [this.log, this.doc];
    }

    /**
     * Walk the arrangement collecting node id → what an intent writes to.
     *
     * A `place` needs the owning aggregate and the member handle (a placement is
     * the aggregate's, not the element's); everything else needs the element.
     * The walk mirrors `form/document.ts`'s own, which is what keeps the two
     * agreeing about what has an id.
     */
    index(element: Element, owner: Aggregate | null, member: Member | null): void {
        // The id belongs to the **placement** when there is one: a clip is a
        // window onto samples, so what an intent names is the window.
        const node = docIdOf(member ?? element);
        if (node !== null) this.byNode.set(node, [owner, member, element]);
        // A view opened over a *part* of this composition — a dedicated roll of
        // one track — must reach this context and not mint a second one, so the
        // walk claims what it passes. Only where there is none: a part that
        // already had a context of its own was being edited on its own terms,
        // and taking its history away without being asked is not this walk's to
        // do.
        if (!contexts.has(element)) contexts.set(element, this);
        if (element instanceof Aggregate) {
            for (const handle of element.handles) {
                this.index(handle.element, element, handle);
            }
        }
    }

    /** The next id for a node a gesture creates — a note added in a roll. */
    mint(element: Element): number {
        const next = this.nextNode ?? nextNodeId(element);
        this.nextNode = next + 1;
        return next;
    }

    /**
     * Release the crate's handles. What the composition going away leaves
     * behind; a view closing is not an event of a history.
     */
    override free(): void {
        this.log?.free();
        this.log = null;
        this.doc?.free();
        this.doc = null;
        super.free();
    }
}

/** What an index entry says an intent naming that node writes to. */
export type Indexed = [Aggregate | null, Member | null, Element];

export class FormEditor extends Editor<Element> implements Adopting {
    quant: number;
    /**
     * Re-render on every edit (the *live editor*: drag a clip and hear it where
     * you dropped it). Off by default — an edit then only changes the
     * arrangement, and `rerender` decides when it is heard.
     */
    follow: boolean;
    /** Whether the views follow their content (see the option). */
    autofit: boolean;
    /**
     * The pitch window each roll was first drawn with, and the length each clip
     * was first drawn at — the two things this editor *derives from the
     * content*, held by {@link FormEditor.fit} while `autofit` is off, under the
     * document node {@link FormEditor.holdKey} reads rather than the object
     * itself, so a hold outlives the reparent or the restore that replaces it.
     *
     * **Not reset by a draw**, unlike the registries beside them: those describe
     * the widgets a draw made, and these describe what the reader is looking at,
     * which a redraw is precisely not entitled to move.
     */
    private pitch = new Map<HoldKey, [number, number]>();
    private length = new Map<HoldKey, [number, number]>();
    /** Widgets appended to the window after the lanes. They are the script's. */
    extra: GuiNode[];
    /**
     * The transport driving the lanes' playhead. Its lanes are read on each use,
     * so a redraw's new widgets get the line.
     */
    readonly transport: Transport;

    /** The elements shown as lanes of their own instead of a summary clip. */
    private expanded = new Set<Element>();
    /** widget id → where the clip came from, and what was drawn for it. */
    private clips = new Map<number, Placed>();
    /** widget id → element, for every lane (the playhead addresses these). */
    private lanes = new Map<number, unknown>();
    /**
     * The placement each lane draws, so an edit on its header names the window's
     * node rather than the element's — the rule every other edit-back follows.
     */
    private laneMembers = new Map<number, Member | null>();
    /**
     * Widget id → the lane's own base in beats, so a clip crossing the stack can
     * be placed relative to the lane it lands on.
     */
    private laneBases = new Map<number, number>();
    /** widget id → the element whose notes that widget draws. */
    private rolls = new Map<number, Element>();
    /** patch widget id → the logical aggregate and its box-order handles. */
    private patches = new Map<number, [Aggregate, Member[]]>();
    /** aggregate → `{box index: [x, y]}`, presentation only. */
    private patchGeometry = new Map<Aggregate, Record<number, [number, number]>>();
    /**
     * Which **edit layer** of each clip the hand is on — the placement, a roll,
     * a curve. Screen state like a selection: the composition does not change
     * when it moves.
     *
     * Keyed by the **placement's node**, not by the widget drawing it. A widget
     * id is the *drawing's* name for something and is minted afresh every time
     * the window is redefined, so anything keyed by one is silently emptied by a
     * structural edit — and a missing key and "the default layer" are the same
     * answer, which is why nothing noticed. A node id is the arrangement's own,
     * stamped by `toDocument` and kept across a re-derivation, so it outlives
     * the picture the way this state is supposed to. {@link FormEditor.editLayerOf}
     * reads it.
     */
    private editLayer = new Map<number, string>();
    /**
     * Whether the last edit changed **which members exist** — a split, a join, a
     * cord. A placement is a prop the host can be told about; a widget that was
     * not there is not, so this is what says a redefine is owed. Read and
     * cleared by {@link FormEditor.restructure}.
     */
    private restructured = false;
    private curveAxis = new WeakMap<Automation, [number, number]>();
    /**
     * The editors this one opened over **parts** of the composition — a
     * dedicated roll of one track, the editor-grade waveform of one take.
     *
     * They are {@link Editor}s, not modes: each is the generic editor with one
     * domain and one view, joined to *this* composition's editing context, so
     * what it does lands in the same order as a clip dragged here and an undo in
     * either window walks that one order.
     */
    private composedEditors: Editor[] = [];
    /**
     * The composed signal editor, if one was opened — what {@link
     * FormEditor.layers} reads and writes.
     */
    private signal: SamplesEditor | null = null;
    /** What a signal view opened from here measures, until one is open. */
    private stack: Measure[] = [...MEASURES];
    /**
     * The widgets a history walk left drawing something else, collected by
     * {@link FormEditor.projectLegs} and drained by {@link
     * FormEditor.reflectStep} — the two halves of one step, which the context
     * runs separately so the version moves once.
     */
    private stepped = new Set<number>();
    // The composition's version, the held document, the history over it and the
    // index between them are **not fields of this editor**: they belong to the
    // arrangement, and a second window over one composition reaches the same
    // {@link Editing} context through {@link FormEditor.editing}. A log kept here
    // would see only the gestures this editor made, so a script editing the
    // arrangement or a second view would leave it describing a composition that
    // has moved on, and undo would then write a state nobody was ever in. The
    // accessors below read that context.
    private unlisten: (() => void) | null = null;
    private destination: unknown = null;
    private clock: TempoClock | null = null;

    constructor(
        element: Element,
        {
            sampleRate,
            tempo = 1.0,
            tempoMap,
            quant = 0.0,
            follow = false,
            autofit = false,
            extra = [],
            title = "Composition",
            width = 1000,
            height = 520,
            baseId = 10_000,
        }: EditorOptions,
    ) {
        super(element, { sampleRate, tempo, tempoMap, title, width, height, baseId });
        this.quant = Number(quant);
        this.follow = Boolean(follow);
        this.autofit = Boolean(autofit);
        this.extra = [...extra];
        this.transport = new Transport(null, () => this.playline(), {
            source: (at) => this.renderPass(at),
            tempoMap: this.tempoMap,
            sampleRate: this.sampleRate,
            extent: () => this.extent(),
        });
    }

    // ---- the arrangement's word for what is edited ----

    /**
     * The composition. {@link Editor} calls the same slot `structure`, which is
     * the general word; here it is an `Element` and the arrangement's own word is
     * what a reader of this class expects.
     */
    get element(): Element {
        return this.structure;
    }

    set element(element: Element) {
        this.structure = element;
    }

    // ---- the unit bridge: beats (the data) ↔ timeline samples (the view) ----

    /**
     * A length of `element`, in that element's own unit, as timeline samples —
     * the one place the editor decides which of the two ratios a number crosses
     * on.
     *
     * `at` is where the length **starts**, in beats. A length in seconds does
     * not need it (its seconds are already fixed), but a length in beats does:
     * beats are a logical coordinate, so the same count of them is a different
     * stretch of time depending on where it sits, and only two positions can say
     * how long it is.
     */
    lengthToUnits(length: number, element: Element, at = 0.0): number {
        return element.durationUnit === SECONDS
            ? this.secsToUnits(length)
            : this.beatsToUnits(at + length) - this.beatsToUnits(at);
    }

    /**
     * Timeline samples as a length of `element`, in that element's own unit —
     * the inverse of {@link FormEditor.lengthToUnits}, and what an edit-back writes
     * back onto the arrangement. `at` is the length's start in beats, for the
     * same reason.
     */
    unitsToLength(units: number, element: Element, at = 0.0): number {
        return element.durationUnit === SECONDS
            ? this.unitsToSecs(units)
            : this.unitsToBeats(this.beatsToUnits(at) + units) - at;
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
        this.resetIds();
        this.clips = new Map();
        this.lanes = new Map();
        this.laneMembers = new Map();
        this.laneBases = new Map();
        this.rolls = new Map();
        this.patches = new Map();

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
    async open(
        host?: GuiHost,
        { id, stage }: { id?: number; stage?: Stage | null } = {},
    ): Promise<WindowHandle> {
        host = await resolveEditorHost(host);
        this.host = host;
        this.transport.host = host;
        const handle = host.open(this.draw(), { id, element: stage });
        this.windowId = handle.id;
        this.editing.attach(this);
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
            // **One subscription, because one host.** The composed editors draw
            // other windows on the same host, and a second subscription would
            // hand each of them the same message twice. Each answers only for
            // the widgets it drew, which is what `apply` is written to do.
            for (const editor of [...this.composedEditors]) editor.apply(msg.addr, msg.args);
        });
        return () => this.detach();
    }

    /** Stop listening. The window stays open; nothing reaches the arrangement. */
    detach(): void {
        this.unlisten?.();
        this.unlisten = null;
    }

    /**
     * This window closed. The subscription goes with it — **unless a view this
     * editor composed is still on screen**: there is one `onMessage` for all of
     * them, and dropping it would leave a window open and dead.
     */
    protected override closed(): void {
        if (this.windows.length === 0) this.detach();
    }

    /**
     * Which layer of a clip the hand last worked on — the placement, a roll, a
     * curve — or `undefined` where nothing has been touched.
     *
     * Screen state, so it is read rather than persisted or logged, and it is the
     * answer to "what is this window currently editing". Takes what
     * {@link Aggregate.add} handed back (the placement) the way every other
     * route here does, since a clip is a window onto an element and the layer
     * belongs to that window.
     */
    editLayerOf(element: Element, member: Member | null = null): string | undefined {
        const node = this.nodeId(element, member);
        return node === null ? undefined : this.editLayer.get(node);
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
        // The history is **not** dropped here, and that is the point of where
        // it lives: it belongs to the composition, so pointing this window at
        // another one simply reaches that composition's context. An undo can no
        // more walk back into a piece this window is not showing than it could
        // walk into one another window is.
        this.floor = FIRST_VERSION;
        this.applied = FIRST_VERSION;
        this.dirty = true;
        if (this.host !== null && this.windowId !== null) {
            this.resetIds();
            this.host.define(this.windowId, this.draw());
            this.announce();
        }
    }

    // ---- the views this editor composes ----

    /**
     * The editors this one opened over **parts** of the composition — a
     * dedicated roll ({@link FormEditor.openPianoroll}), the editor-grade
     * waveform of a take ({@link FormEditor.openSignal}).
     *
     * Each is an {@link Editor} of its own, joined to this composition's editing
     * context, so what it does lands in the same undo order as a clip dragged
     * here. Read it to reach the window's own editor: its selection, its undo,
     * the structure it holds.
     */
    get composed(): Editor[] {
        return [...this.composedEditors];
    }

    /**
     * Take a composed editor into this one: it draws `element`, part of this
     * composition, and a transport gesture in its window is this piece's.
     *
     * Held so the pair survives the page letting go of the window handle, and so
     * a history step reaching a leg this editor cannot read has somebody to hand
     * it to.
     */
    private compose<E extends Editor>(editor: E, element: Element): E {
        editor.composedIn = this as unknown as Editor;
        editor.composedOver = element;
        this.composedEditors.push(editor as unknown as Editor);
        return editor;
    }

    /**
     * The host a composed view is opening on, taken as this editor's when it has
     * none of its own.
     *
     * **Only when it has none.** A multitrack already open answers *its* host,
     * and overwriting that with the one a second window opened on left every
     * acknowledgement going to the wrong place — silently, because in the
     * ordinary case (one host, several windows) the two are the same object.
     */
    private adoptHost(host: GuiHost): void {
        if (this.host === null) {
            this.host = host;
            this.transport.host = host;
            this.listen(host);
        }
    }

    /**
     * Every widget the playhead line is drawn on: this window's lanes, and the
     * composed views'. Read on each use, so a redraw's new widgets get the line
     * — and so does a window opened after the transport was already running.
     */
    /**
     * Every window this editor is on screen through — its own, and the composed
     * views'.
     *
     * A `FormEditor` may be on screen through a composed view alone (a take
     * opened with {@link FormEditor.openSignal} and no multitrack), and
     * `window` is deliberately this editor's own rather than "one of the
     * windows".
     */
    get windows(): number[] {
        const held = this.window === null ? [] : [this.window];
        for (const editor of this.composedEditors) {
            if (editor.window !== null) held.push(editor.window);
        }
        return held;
    }

    private playline(): number[] {
        const ids = [...this.lanes.keys()];
        for (const editor of this.composedEditors) ids.push(...(editor.view?.widgets.keys() ?? []));
        return ids;
    }

    /**
     * What a signal view opened from here measures — `["peak", "rms"]` for the
     * editor's picture, `["peak"]` for the bare envelope.
     *
     * A reading of the composed {@link SamplesEditor} once one is open, and the
     * stack the next one will be opened with before that. **Assigning it on an
     * open view sends one message**, which is that editor's own rule: the
     * measure is a live prop, so the body appears and disappears over the peaks
     * with the picture, the axis, the zoom, the selection and the playhead all
     * exactly where they were.
     */
    get layers(): Measure[] {
        return this.signal === null ? [...this.stack] : (this.signal.layers as Measure[]);
    }

    set layers(stack: readonly string[]) {
        this.stack = measures(stack);
        if (this.signal !== null) this.signal.layers = this.stack;
    }

    /**
     * Open one **rendered** element's samples in an editor of their own, joined
     * to this composition — the editor-grade view of a take, as opposed to
     * `open`, where the same samples are only a clip's body.
     *
     * It is {@link edit} over the take: a {@link SamplesEditor}, one `waveform`,
     * and a stroke that writes the server's buffer through the samples domain.
     * What makes it *this piece's* rather than a window beside it is the editing
     * context — the composition's, so a stroke here and a clip dragged there are
     * one undo order, and an undo asked for in either window walks it.
     *
     * `layers` is what the picture measures, and {@link FormEditor.layers}
     * changes it live on the open view. The element must have **samples**: a
     * rendered take, not a generator (see the error a generator raises).
     */
    async openSignal(
        host?: GuiHost,
        element?: Element,
        { layers = MEASURES, id, stage }: {
            layers?: readonly string[];
            id?: number;
            stage?: Stage | null;
        } = {},
    ): Promise<WindowHandle> {
        const target = element ?? this.element;
        // Refused **before** a window exists: an unknown measure and an element
        // with no samples are both answers to the call that was made, and
        // finding out at the first repaint would leave an empty window behind.
        const stack = measures(layers);
        const take = takeOf(target);
        if (take === null) {
            // **The generated/generator distinction, asked at the door.** A
            // rendered element has samples a view can address; a generator has
            // none until it is rendered, and a window drawn over nothing is
            // worse than a refusal that says what to do. It is the same question
            // `openPianoroll` answers by showing a bounced generator read-only,
            // and it has a sharper answer here: notes can be bounced for a
            // picture, samples cannot be invented.
            throw new Error(
                `${nameOf(target)} has no samples to draw: a signal view needs a ` +
                    "rendered element (render the composition, or bounce this one to " +
                    "a buffer, and open that)",
            );
        }
        const resolved = await resolveEditorHost(host);
        this.adoptHost(resolved);
        this.stack = stack;
        const editor = new SamplesEditor(take, {
            sampleRate: this.sampleRate,
            tempoMap: this.tempoMap,
            layers: stack,
            title: this.title,
            extra: this.extra,
            width: this.size[0],
            height: this.size[1],
            context: this.editing,
        });
        this.signal = this.compose(editor, target);
        return await editor.open(resolved, { id, stage });
    }

    /**
     * Open one events element's notes in a roll of their own, joined to this
     * composition — the editor-grade note view (a keyboard, an editable note
     * grid, a velocity lane, an OSC lane) of one element, as opposed to `open`,
     * where the same notes are only a clip body.
     *
     * It is {@link edit} over the element's timeline: a {@link NotesEditor} on
     * this composition's editing context, so the roll and the multitrack over the
     * same piece step **one** history — and a note moved here reaches the clip
     * drawing it there without either window being redefined.
     *
     * A **generator** (a pattern) is forward-only, so what it produced is bounced
     * onto a timeline of its own and the roll is opened **read-only**: the notes
     * are a rendering of an algorithm, and the widget is told so rather than
     * refusing each drag after the hand has made it (bounce it to a `Track` to
     * edit). OSC markers are shown but not edited back yet.
     */
    async openPianoroll(
        host?: GuiHost,
        element?: Element,
        { id, stage }: { id?: number; stage?: Stage | null } = {},
    ): Promise<WindowHandle> {
        const target = element ?? this.element;
        const held = editableTimeline(target);
        const timeline = held ?? bounced(target);
        const resolved = await resolveEditorHost(host);
        this.adoptHost(resolved);
        const editor = new NotesEditor(timeline, {
            sampleRate: this.sampleRate,
            tempoMap: this.tempoMap,
            editable: held !== null,
            title: this.title,
            extra: this.extra,
            width: this.size[0],
            height: this.size[1],
            context: this.editing,
        });
        this.compose(editor, target);
        return await editor.open(resolved, { id, stage });
    }

    /**
     * A marquee swept in a **composed** view is a selection of the element that
     * view draws.
     *
     * The same value a sweep on that element's clip gives, so an operation over
     * the range is handed one thing whichever of the piece's windows it was swept
     * in — and it is the composed editor that knows the numbers were its own
     * axis's, which is why this takes the selection rather than the event.
     */
    override adoptSelection(editor: Editor): void {
        const selection = { ...editor.selection } as Record<string, unknown>;
        const over = editor.composedOver as Element | null;
        const node = over === null || over === undefined ? null : this.nodeId(over);
        if (node !== null) selection.nodes = [node];
        this.selection = selection as unknown as Selection;
    }


    /**
     * The composition's length in beats, **read from the arrangement** — the end
     * of its last placed element. It is not a constant: move a clip past the end
     * and the piece gets longer, which is exactly what a transport must ask.
     *
     * Beats whatever the element is measured in: a transport is on a clock, so
     * an element that is its own length in seconds (a lone take opened as a
     * composition) crosses here.
     */
    extent(element?: Element): number {
        const target = element ?? this.element;
        return toBeats(this.extentOf(target), target.durationUnit, this.tempo);
    }

    /** The `Playhead` playing the composition, or `null` before the first render. */
    get playhead(): Playhead | null {
        return this.transport.playhead;
    }

    /**
     * Another view of this composition edited it: bring this window in step.
     *
     * **Props where props can carry it.** A placement, a length, a curve and a
     * note list are values, so a foreign edit reaches this window the way its
     * own edits do — the drawn record is brought back in step and the widgets
     * are resynced. A redefine here would rebuild every widget and drop what the
     * host had in flight, which makes a window flicker under a hand that is not
     * even in it.
     *
     * `whole` is the case no prop can carry: a widget that was not there a
     * moment ago — a cut, a split, a join, an undo of one — or a turn that
     * changed something and projected no intent at all. Then it is
     * {@link FormEditor.update}, which is exactly what that method was written
     * for.
     *
     * A window that is not open has nothing to bring in step, and says so by
     * doing nothing. So does one that draws none of what moved.
     */
    override adopt(intents: readonly Intent[], whole: boolean): void {
        if (this.host === null || this.windowId === null) return;
        if (whole) {
            this.update();
            return;
        }
        const widgets = new Set<number>();
        if (intents.some((intent) => !("node" in (intent as object)))) {
            // An intent in **another structure's vocabulary** — a stroke on a
            // take, a note moved in a composed roll. It names no node of this
            // tree, and what it wrote are the objects this window draws, so the
            // held document has to be derived again: it is the case `refresh`
            // exists for, arriving from a view instead of from a page. What is
            // redrawn is the clip over that part, not the window.
            this.refresh();
            for (const editor of this.composedEditors) {
                const over = editor.composedOver as Element | null;
                if (over === null || over === undefined) continue;
                const wid = this.widgetOf(over);
                if (wid !== null) widgets.add(wid);
            }
        }
        for (const intent of intents) for (const wid of this.reflect(intent)) widgets.add(wid);
        if (widgets.size === 0) return;
        this.corrections = [];
        for (const wid of widgets) this.resync(wid);
        // A stamp of zero retires nothing, which is what an unasked push needs:
        // this window answered no gesture.
        this.acknowledge(0);
        this.corrections = [];
    }

    /**
     * The drawn half of {@link FormEditor.project}, for an edit **another view
     * applied**.
     *
     * The arrangement is already written — the two views hold the same objects —
     * so what is left is this window's own record of what it drew, and which of
     * its widgets is now drawing something else. A view that does not draw the
     * node an intent names answers with nothing, which is the ordinary case for
     * a dedicated roll beside a multitrack.
     */
    private reflect(intent: Intent): number[] {
        const found = this.byNode.get(Math.trunc(Number((intent as { node?: unknown }).node ?? -1)));
        if (found === undefined) return [];
        const [, member, element] = found;
        const wid = (intent as { intent?: string }).intent === "place" && member !== null
            ? this.redrawn(element, member)
            : this.widgetOf(element, member);
        return wid === null ? [] : [wid];
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
        // **And the document moves with it.** The crate refuses an edit whose
        // `against` version is not the document's — ahead of it as loudly as
        // behind, since the two would not be talking about the same piece — so
        // a version bumped here and nowhere else left the editor answering
        // "this edit was made against a different document" to every gesture
        // after the redefine, silently and forever. Re-deriving stamps the
        // document with the version this window is now drawing.
        this.rederive = true;
        this.host.define(this.windowId, this.draw());
        this.announce();
    }

    // ---- the edit-back: a dragged clip becomes a placement ----

    /**
     * One `/gui_event` payload onto the arrangement, with the stamp already taken
     * off. Answers whether the composition changed; `apply` answers the host.
     */
    protected override route(args: readonly unknown[]): boolean {
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
        if (MIXING_TAGS.includes(tag)) {
            // A lane header's toggle or fader. **The composition's**, so it goes
            // through the log like a clip's move and survives a save: what is
            // muted is a fact about the piece, not about the window.
            return this.applyMixing(id, tag, rest);
        }
        if (tag === "height") {
            // A lane's thickness, from Ctrl+wheel. **The view's** — it says
            // nothing about what the piece is, no document carries it, and the
            // host has already resized the lane it was made on. Answered here so
            // it is deliberately nothing rather than accidentally nothing.
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
        if (tag === "clips") {
            // A block of clips moved by one hand, reported on the **lane** that
            // holds them — the one message that says "these several placements
            // are one edit".
            return this.applyClips(rest);
        }
        const placed = this.clips.get(id);
        if (placed === undefined) return false;
        if (tag === "points") return this.applyPoints(placed, rest);
        if (tag === "layer") {
            // Which layer of a clip the hand is on is **screen state**, like a
            // selection: the composition did not change. Kept under the
            // *placement's* node rather than the widget's id, so it survives the
            // window being redrawn (see the field).
            const node = placed.member === null
                ? null
                : this.nodeId(placed.member.element, placed.member);
            if (node !== null) this.editLayer.set(node, String(rest[0]));
            return false;
        }
        if (tag === "lane") {
            // The clip left one lane and joined another: where it now *is*,
            // which is two setmembers rather than one placement.
            return this.applyClipLane(id, placed, Math.trunc(Number(rest[0])), Number(rest[1]));
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
        // **The length goes back in the unit of what it measures.** A placement
        // is musical and crosses on the beat; a clip of samples is as long as
        // its seconds, and dividing those by the beat would write a length the
        // next tempo change moves.
        const askedDur = resized
            ? this.unitsToLength(dur, member.element, this.unitsToBeats(offset))
            : member.dur;
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
            effective.dur === undefined
                ? dur
                : this.lengthToUnits(
                      Number(effective.dur),
                      member.element,
                      Number(effective.offset) + placed.base,
                  );
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
    protected override owns(widgetId: number): boolean {
        return (
            this.clips.has(widgetId) ||
            this.rolls.has(widgetId) ||
            this.patches.has(widgetId) ||
            this.lanes.has(widgetId) ||
            false
        );
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
    protected override resync(widgetId: number): void {
        const props: Record<string, PropValue> = {};
        const placed = this.clips.get(widgetId);
        if (placed !== undefined) {
            props.offset = placed.offset;
            props.dur = placed.dur;
            const auto = automationOf(
                placed.member !== null ? placed.member.element : this.element,
                this.tempo,
            );
            if (auto !== null) {
                // A curve is as much of "what this widget should be drawing" as a
                // placement is, and an undone one is the case that needs it.
                props.points = flatPoints(
                    quads(auto.toPoints()).flatMap(([t, v, shape, curve]) => [
                        this.secsToUnits(t),
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
        let element = this.rolls.get(wid) ?? null;
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
     * The clipboard travels *with* the request — the host's, so that a block
     * copied in one window pastes into another — and what arrives is the
     * crate's typed document: its kind, its JSON, and its bulk beside it.
     *
     * **A paste is the same edit its own copy was.** A block of notes is
     * written onto the addressed roll by `applyNotes`, the very call a drag on
     * a note goes through: one `setmembers`, one entry on the pile, one undo.
     * The three verbs are one mechanism, so a block that a roll's `Ctrl+C` put
     * on the clipboard lands the same way whether the roll pasted it itself or
     * the window asked this editor to.
     *
     * **What it cannot place is samples.** They are written by whoever owns
     * them against a working copy, and an arrangement editor placing a nameless
     * block of audio would be inventing both a source and a source's owner.
     */
    private applyPaste(wid: number, values: readonly unknown[]): boolean {
        const element = this.rolls.get(wid);
        if (element === undefined && !this.clips.has(wid) && !this.lanes.has(wid)) return false;
        const position = values.length > 0 ? Number(values[0]) : 0.0;
        const kind = values.length > 1 ? String(values[1]) : "";
        const block = clipboardNotes(kind, values.length > 2 ? String(values[2]) : "");
        if (block !== null) {
            if (element === undefined) {
                this.reason = "a block of notes is written onto a roll, and this view holds none";
                return false;
            }
            return this.pasteNotes(wid, element, position, block);
        }
        this.reason =
            `this editor places elements and notes; a ${kind || "clipboard"} ` +
            "block is samples, and samples are written by their owner";
        return false;
    }

    /**
     * Place a copied block of notes on `element`'s timeline, its first onset at
     * `position`.
     *
     * The block keeps the spread it was copied with — a paste places what was
     * taken, not a re-quantized version of it — and the notes already there keep
     * their identity, because what goes to the log is the whole resulting list
     * and the ones that were held are held by index (`applyNotes`).
     *
     * `position` is the axis the host swept, so it is in the **timeline's**
     * units while a roll's notes are in the clip's own: a clip placed late holds
     * note 0 at its offset, which is the one conversion between the axis and the
     * notes on it.
     */
    private pasteNotes(
        wid: number,
        element: Element,
        position: number,
        block: readonly Note[],
    ): boolean {
        if (editableTimeline(element) === null) {
            this.reason =
                "this clip draws what a generator produced; render it to a track " +
                "to paste into it";
            return false;
        }
        const placed = this.clips.get(wid);
        const at = Math.max(0.0, position - (placed !== undefined ? placed.offset : 0.0));
        const first = Math.min(...block.map((note) => note[0]));
        const pasted: Note[] = block.map(
            (note) => [note[0] - first + at, note[1], note[2], note[3], note[4]],
        );
        return this.applyNotes(element, flatNotes([...this.notesOf(element), ...pasted]),
            "paste the notes");
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
        return document.resolve(
            this.selection as Selection,
            this.unitsPerBeat,
            this.unitsPerSecond,
            true,
        );
    }

    /**
     * Whether the host reported a **window** that is not the one the element
     * already reads — half a frame's worth, the same threshold a move uses.
     *
     * The element is asked whether it *has* a window rather than tested for
     * which class it is: a take reads from a frame and a track reads from a
     * beat, and both are trimmed by the same drag on the same edge.
     */
    private windowMoved(placed: Placed, start: number | null): boolean {
        if (start === null || placed.member === null) return false;
        const element = placed.member.element;
        const held = element.windowStart();
        if (held === null) return false;
        // Half a frame, in whatever the element addresses itself in — the axis
        // is samples either way, so the threshold crosses with the value.
        const floor = element.durationUnit === SECONDS ? 0.5 : this.unitsToBeats(0.5);
        return Math.abs(this.windowUnits(element, Number(start)) - held) >= floor;
    }

    /**
     * The window the host reported, in the unit that element is **addressed**
     * in: the axis is samples, and a track reads its timeline by the beat.
     */
    private windowUnits(element: Element, start: number): number {
        return element.durationUnit === SECONDS ? Number(start) : this.unitsToBeats(Number(start));
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
        edited.dur = this.unitsToLength(dur, member.element, this.unitsToBeats(offset));
        const holder = { ...(edited.node as Record<string, unknown>) };
        // In the element's own addressing unit, which is the one its config
        // states a window in — frames for samples, beats for a track.
        holder.config = {
            ...((holder.config as Record<string, unknown>) ?? {}),
            start: this.windowUnits(member.element, Number(start)),
        };
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
    /**
     * A clip **moved to another lane**, at `offset` on the shared axis.
     *
     * Moving a note between rows changes a number in the same list; moving a
     * clip between lanes **reparents** — the clip is a member of the lane's
     * aggregate, and a lane is a thing that exists with a node behind it. That
     * is the one place the two boxes stop being the same object, and it is why
     * this is not `place`: `Intent.Place` moves a node **within its parent** and
     * carries no new one.
     *
     * So it is **two `setmembers` in one transaction** — the lane it left and
     * the lane it joined — atomic in both directions and undone in one step.
     * Half of it would be a clip in two lanes or in none.
     *
     * The element keeps its id across the move: it is the same element, placed
     * somewhere else, and an identity that changed would leave the history
     * describing a node the tree no longer has.
     */
    private applyClipLane(
        widgetId: number,
        placed: Placed,
        laneWidget: number,
        offset: number,
    ): boolean {
        const { member, owner } = placed;
        const target = this.lanes.get(laneWidget);
        if (member === null || owner === null || target === undefined || target === owner) {
            return false;
        }
        if (!(target instanceof Aggregate) || target.kind !== CONCRETE) {
            // A lane that is one element rather than an aggregate of clips has
            // nowhere to put a second one: it *is* the clip it draws.
            this.reason =
                "that lane holds one element, not a list of clips: " +
                "there is nowhere on it to place this one";
            return false;
        }
        const fromNode = this.nodeId(owner);
        const toNode = this.nodeId(target);
        if (fromNode === null || toNode === null) return false;
        const base = this.laneBases.get(laneWidget) ?? 0.0;
        const onset = this.unitsToBeats(offset) - base;
        const element = member.element;
        const dur = member.dur;
        const held = docIdOf(member);
        const wasOffset = member.offset;
        owner.remove(member);
        const handle = target.add(element, onset, dur);
        if (held !== null) setDocId(handle, held);
        const [, document] = this.history();
        const legs: [Intent, Intent][] = [];
        for (const [node, aggregate] of [
            [fromNode, owner],
            [toNode, target],
        ] as [number, Aggregate][]) {
            const whole = toDocument(aggregate, { version: this.version }).root;
            const intent = {
                intent: "setmembers",
                node,
                members: (whole.members ?? []) as unknown[],
            } as unknown as Intent;
            const inverse = document.inverse(intent);
            if (inverse === undefined) {
                legs.length = 0;
                break;
            }
            legs.push([intent, inverse]);
        }
        if (legs.length === 0 || !this.applyLegs("move the clip to another lane", legs)) {
            // Put the arrangement back: the log refused, so nothing happened.
            target.remove(handle);
            const back = owner.add(element, wasOffset, dur);
            if (held !== null) setDocId(back, held);
            this.resync(widgetId);
            return false;
        }
        // No projection: the tree already *is* what the intents say. What the
        // window has to learn is that a clip it drew on one lane is on another,
        // which is a redraw of the whole stack rather than a corrected value.
        this.rederive = true;
        this.restructured = true;
        return this.changed();
    }

    private applySplit(placed: Placed, atUnits: number): boolean {
        const { member, owner } = placed;
        if (member === null || owner === null) return false;
        const element = member.element;
        // **The length the clip is drawn at**, which is what the hand cut in
        // two: the placement's when it states one, else the element's own, else
        // what it extends to — the same rule the drawing asks, so a cut lands
        // where the eye put it even on a placement that states no length (which
        // every note clip written by a script is).
        const length = this.drawnLength(element, member);
        const at = this.unitsToLength(atUnits, element, placed.base + (member.offset ?? 0.0));
        if (length === null || !(at > 0.0 && at < Number(length))) return false;
        const node = this.nodeId(owner);
        if (node === null) return false;
        // **The element says whether it can be cut**, because cutting is defined
        // by the material and not by the class this client wrapped it in: a
        // window onto samples, a run of windows and a window onto a timeline of
        // notes all answer, each in its own unit. What answers `null` is a
        // generator — and not "it cannot be split" but *not until it is
        // rendered*, which is the change of state the model already names.
        const second = element.windowed(at, Number(length), this.sampleRate);
        if (second === null) {
            this.reason =
                element instanceof Generator
                    ? `a generator has no time axis to cut: render it to a track or a take first (this clip holds ${nameOf(element)})`
                    : `this clip holds ${nameOf(element)}, which has no window to cut into two`;
            return false;
        }
        // The cut, on the arrangement: the first half stops early — its
        // *placement* does, the element is untouched — and the second is placed
        // where it stops. Stamped with an id of its own **before** any conversion
        // sees it, or the next one would renumber the tree around it.
        const wasDur = member.dur;
        member.dur = at;
        // The onset is the aggregate's, so it is in beats: the cut's own seconds
        // cross here and nowhere else.
        const onset = member.offset + toBeats(at, element.durationUnit, this.tempo);
        const handle = owner.add(second, onset, Number(length) - at);
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
        // index has to learn is the element that was not there a moment ago —
        // and so does the window, which has one clip where there are now two.
        this.rederive = true;
        this.restructured = true;
        return this.changed();
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
        const lengths: number[] = [];
        for (let i = 0; i < run.length; i++) {
            const length = (run[i] as Placed).member?.length ?? elements[i].duration;
            if (length === null || length === undefined) {
                this.reason = "a clip with no length has nothing to join";
                return false;
            }
            lengths.push(Number(length));
        }
        // **The unit is the material's, and a join does not cross it.** Windows
        // over samples and windows over a timeline both join; a run mixing the
        // two would have to say what the result measures in, which is a
        // different question and not this one.
        const units = new Set(elements.map((e) => e.durationUnit));
        if (units.size > 1) {
            this.reason =
                "these clips are not measured in the same unit, so there is no one thing they read as";
            return false;
        }
        const segments: Segment[] = [];
        let joined: Element;
        let total: number;
        if (units.has(BEATS)) {
            const track = this.joinedTrack(elements, lengths);
            if (track === null) return false;
            joined = track;
            total = lengths.reduce((sum, l) => sum + l, 0.0);
        } else {
            // The segments the run holds, in reading order: a `Vector` is one, a
            // `Segments` is however many it already carries.
            for (let i = 0; i < elements.length; i++) {
                const element = elements[i];
                const length = lengths[i] as number;
                if (element instanceof Segments) {
                    segments.push(...this.segmentsWithin(element, length));
                } else if (element instanceof Vector) {
                    segments.push(new Segment(element.buffer, element.start, length));
                } else {
                    this.reason = `this clip holds ${nameOf(element)}, which has no window to read as one`;
                    return false;
                }
            }
            joined = this.joinedElement(elements as (Vector | Segments)[], segments);
            total = segments.reduce((sum, seg) => sum + seg.duration, 0.0);
        }
        const node = this.nodeId(owner);
        if (node === null) return false;
        const keep = (run[0] as Placed).member as Member;
        const dropped = new Set(run.slice(1).map((p) => p.member as Member));
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
        this.restructured = true;
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
    /**
     * What a run of **windows onto one timeline** joins into: the single window
     * they were cut from, or `null` with a reason.
     *
     * The mirror of a split over notes, and it has to be, or a cut could not be
     * undone by the verb that exists for it. Adjacent windows over one timeline
     * are one window — from the first's start, as long as the two together — and
     * nothing is copied, so cutting it again gives the same two back.
     *
     * What it cannot do yet is the other shape, the one a run of *different*
     * timelines would need: that is a body the document stores for samples only
     * (`clausters_document::SegmentRef` names a source of samples), so it is
     * refused out loud rather than half-built.
     */
    private joinedTrack(elements: Element[], lengths: number[]): Track | null {
        const first = elements[0];
        const timeline = first instanceof Track ? first.timeline : null;
        let expected = Number(first.windowStart() ?? 0.0);
        for (let i = 0; i < elements.length; i++) {
            const element = elements[i];
            if (
                !(element instanceof Track) ||
                element.timeline !== timeline ||
                Math.abs(Number(element.windowStart() ?? 0.0) - expected) > 1e-9
            ) {
                this.reason =
                    "only adjacent windows of one timeline join back into one; a run of " +
                    "several timelines needs a segments body the document does not carry " +
                    "for notes yet";
                return null;
            }
            expected += lengths[i] as number;
        }
        return new Track(timeline, null, lengths.reduce((sum, l) => sum + l, 0.0), {
            start: Number(first.windowStart() ?? 0.0),
            name: first.name,
        });
    }

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
            expected += this.secsToUnits(seg.duration);
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
        const [element, member] = curveOwner(clip, placed.member, this.tempo);
        const auto = automationOf(element, this.tempo);
        if (auto === null || values.length === 0) return false;
        const flat: number[] = [];
        for (const [t, v, shape, curve] of quads(values.map((x) => Number(x)))) {
            flat.push(this.unitsToSecs(t), Number(v), Math.trunc(shape), Number(curve));
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
        return this.changed(outcome.applied);
    }

    /**
     * Notes edited in a roll — a clip's body or the dedicated piano-roll alike:
     * written onto the element's editable `Timeline` as `Event`s, times converted
     * to beats, preserving any OSC items already on it. Answers `false` for a
     * forward-only generator element (read-only), so the edit is a no-op.
     *
     * **An edit updates the note it names; it does not rebuild it.** The i-th
     * note of the payload is the i-th note the roll drew (order is the only
     * identity the payload carries), so its event is *copied* and the edited
     * fields written onto the copy — which keeps everything the roll cannot say:
     * the instrument, and whatever else the author put on that event.
     *
     * **And the length it carries is the note's `sustain`.** A roll draws what a
     * note *sounds* (`Event.sustain`, which is `dur * legato` when nothing says
     * otherwise), so that is what a drag on its edge sets — the key that says how
     * long it sounds, leaving `dur` (its length on the grid) and `legato` (the
     * articulation) as the author wrote them. Writing the drawn length into `dur`
     * with a `legato` of 1 was a round trip that lost both: every note in the
     * lane, edited or not, came back fully legato with its grid length quietly
     * shortened to what it had been sounding.
     */
    private applyNotes(
        element: Element,
        values: readonly unknown[],
        label = "edit the notes",
    ): boolean {
        const timeline = editableTimeline(element);
        if (timeline === null) return false;
        const node = this.nodeId(element);
        if (node === null) return false;
        // **The window the roll drew.** A track is a window onto its timeline (a
        // trim reads from further in, a split gives two windows over one
        // timeline), so the payload is an edit of the notes *inside* it, placed
        // from the element's own zero. The ones outside are not in the payload
        // and must not be taken as deleted: they are carried through untouched,
        // with their own ids and their own beats.
        const [lo, hi] = this.noteWindow(element);
        const inside = [...timeline].filter(
            ([beat, item]) => pitchOf(item) !== null && Number(beat) >= lo && Number(beat) < hi,
        );
        const outside = [...timeline].filter(
            ([beat, item]) =>
                pitchOf(item) !== null && !(Number(beat) >= lo && Number(beat) < hi),
        );
        const held = inside.map(([, item]) => item as SeqEvent);
        const fresh: [number, SeqEvent][] = [];
        let index = 0;
        for (const [start, dur, pitch, vel, channel] of quintuples(values.map((x) => Number(x)))) {
            const length = this.unitsToBeats(dur);
            const was = held[index++];
            let params: Record<string, unknown>;
            if (was !== undefined) {
                params = { ...was.props, midinote: Math.trunc(pitch), sustain: length };
                // The velocity round-trips through the drawing, so writing it
                // back unconditionally would re-quantize an `amp` nobody
                // touched. It is written only when the hand actually moved it.
                if (Math.trunc(vel) !== velocityOf(was)) {
                    params.velocity = Math.trunc(vel);
                    params.amp = Math.max(0.0, Math.min(1.0, Math.trunc(vel) / 127.0));
                }
            } else {
                // A note the lane did not hold: there is nothing to keep, so it
                // is built from what the payload says, sounding its full length.
                params = {
                    midinote: Math.trunc(pitch),
                    dur: length,
                    legato: 1.0,
                    amp: Math.max(0.0, Math.min(1.0, Math.trunc(vel) / 127.0)),
                    velocity: Math.trunc(vel),
                };
            }
            if (Math.trunc(channel)) params.channel = Math.trunc(channel);
            // Back onto the timeline's own axis: the roll drew from the
            // window's zero, and the timeline is where the window opens.
            fresh.push([this.unitsToBeats(start) + lo, new SeqEvent(params)]);
        }
        // **Through the log**: the roll's edit is a `setmembers` — "notes added,
        // moved and removed arrive as the resulting list. Members keep their
        // ids". Keeping them is the whole difficulty, because the payload carries
        // no ids: a roll sends the resulting notes in order, so **order is the
        // only information there is**. The i-th note inherits the i-th note's id
        // and the extras are minted past everything the arrangement holds.
        const kept = inside.map(([, item]) => docIdOf(item));
        const carried = outside.map(([beat, item]) => ({
            offset: Number(beat),
            node: {
                id: Math.trunc(docIdOf(item) ?? this.mintId()),
                kind: "clang",
                config: leafConfig(new Clang(item as SeqEvent)) as Record<string, unknown>,
            },
        }));
        const edited = fresh.map(([beat, event], i) => {
            const nid = kept[i] ?? this.mintId();
            return {
                offset: Number(beat),
                // **Through the conversion's own door.** A note's event is not
                // plain data — a played one carries its `server`, and the intent
                // travels as JSON — so the config is written the way
                // `toDocument` writes a clang's, which turns what is not
                // JSON-able into the reference the document keeps for it.
                node: {
                    id: Math.trunc(nid ?? this.mintId()),
                    kind: "clang",
                    config: leafConfig(new Clang(event)) as Record<string, unknown>,
                },
            };
        });
        const members = [...carried, ...edited];
        const outcome = this.record({ intent: "setmembers", node, members }, label);
        if (outcome === null) return false;
        this.project(outcome.effective);
        return this.changed(outcome.applied);
    }

    /**
     * The beats of the timeline a roll over `element` **draws**: its window, or
     * everything when it has none.
     *
     * One question asked in one place, because the drawing and the edit-back
     * have to agree about it — a roll that drew a window and wrote back the
     * whole timeline would delete every note the window did not show.
     */
    private noteWindow(element: Element): [number, number] {
        const lo = Number(element.windowStart() ?? 0.0);
        const length = element.duration;
        return [lo, length === null ? Infinity : lo + Number(length)];
    }

    /**
     * A node id nothing in this arrangement holds, for a note a gesture added.
     * Follows the conversion's own rule, so a minted id and a converted one
     * cannot collide.
     */
    private mintId(): number {
        return this.editing.mint(this.element);
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
        const buses = aggregate.busSpecList;
        if (!buses.some((spec) => spec.name === bus)) {
            buses.push({ name: bus, rate, channels: 1 });
        }
        // **A cord is three configurations, in one transaction.** It was the
        // last gesture that wrote the arrangement directly, on the grounds that
        // no intent described it — and the vocabulary had said what to do all
        // along: an edit states the resulting value, and what a cord results in
        // is the two members' controls and the aggregate's buses. Three
        // `configure`s, recorded as one entry, so it undoes in one step and
        // leaves the three consistent at every point.
        this.restructured = true; // a cord is not a prop: the patch redraws
        return this.transact("draw a cord", [
            [src, handles[srcBox] ?? null, { controls: srcCtls }],
            [dst, handles[dstBox] ?? null, { controls: dstCtls }],
            [aggregate, null, { buses }],
        ]);
    }

    /**
     * Apply several configurations as **one** entry.
     *
     * A gesture that changes more than one node still has to undo in one step,
     * so the legs go in as one transaction rather than as an edit each: the
     * inverses are read *before* anything lands, the whole run is applied, and a
     * leg that refuses puts back the ones already applied and records nothing.
     * Half a transaction would undo one node and leave the other where the
     * gesture put it.
     *
     * Each leg is `[element, member, overrides]` — the overrides merged onto
     * that node's **current** configuration, since a `configure` states the
     * whole of it and a key nobody mentioned is not a key to drop. A leg the
     * document was already at is applied by nobody and recorded by nobody: a
     * resend is not an edit.
     */
    private transact(
        label: string,
        legs: readonly [Element, Member | null, Record<string, unknown>][],
    ): boolean {
        const [log, document] = this.history();
        const prepared: [Intent, Intent][] = [];
        for (const [element, member, overrides] of legs) {
            const node = this.nodeId(element, member);
            if (node === null) return false;
            // The inverse of *any* configure on a node is its current
            // configuration, which is also the base the overrides go onto.
            const probe = { intent: "configure", node, config: {} } as unknown as Intent;
            const inverse = document.inverse(probe);
            if (inverse === undefined) return false;
            const held = (inverse as unknown as { config?: Record<string, unknown> }).config ?? {};
            prepared.push([
                { intent: "configure", node, config: { ...held, ...overrides } } as unknown as Intent,
                inverse,
            ]);
        }

        return this.applyLegs(label, prepared);
    }

    /**
     * Apply prepared `[intent, inverse]` legs as **one** entry.
     *
     * The body `transact` and `applyClips` share: what differs between a cord's
     * three configurations and a block of clips' several placements is which
     * intents are prepared, never what "one gesture is one edit" means.
     */
    private applyLegs(label: string, prepared: readonly [Intent, Intent][]): boolean {
        const [log, document] = this.history();
        const applied: [Intent, Intent][] = [];
        for (const [index, [intent, inverse]] of prepared.entries()) {
            // **The staleness check is the gesture's, not each leg's.** One
            // gesture was made against one picture, and the first leg applied is
            // what moves the version — so checking the rest against the version
            // the hand saw would refuse the transaction's own consequences as
            // somebody else's edit.
            const options = index === 0 ? { against: { version: this.version } } : {};
            const outcome = document.apply(intent, options);
            if (outcome.applied) applied.push([outcome.effective, inverse]);
            else if (outcome.reason) {
                // Refused for a rule rather than for being a resend: put back
                // what landed and record nothing. **Half a transaction would
                // undo one node and leave the other where the gesture put it**,
                // which is what carrying on through the rest of the legs would
                // leave behind.
                for (const [, undo] of [...applied].reverse()) document.apply(undo);
                return false;
            }
        }
        if (applied.length === 0) return false;

        log.history.record(
            applied.map(([effective, inverse]) => ({
                structure: log.structure,
                forward: { edit: effective } as Step,
                backward: inverse,
                key: Document.coalesceKey(effective),
            })),
            { label },
        );
        for (const [effective] of applied) this.project(effective);
        return this.changed();
    }

    /**
     * A **block of clips moved by one hand** (`"clips" id offset dur start …`),
     * applied as one transaction.
     *
     * The plural of the clip route, meaning the same thing about each clip it
     * names — the same conversion from the axis to the placement, through the
     * same `place` intent. What it adds is that the several placements are **one
     * entry**: a marquee's block undoes in one step, because that is what the
     * hand did. A run of separate `"clip"` messages could not say so, which is
     * why the host sends one message and not several.
     *
     * A block move never resizes and never trims: every clip keeps the length
     * and the window it had, and only the offsets travel.
     */
    private applyClips(rest: readonly unknown[]): boolean {
        const [, document] = this.history();
        const legs: [Intent, Intent][] = [];
        const moved: [number, Placed, number][] = [];
        for (let i = 0; i + 3 < rest.length; i += 4) {
            const widgetId = Math.trunc(Number(rest[i]));
            const offset = Number(rest[i + 1]);
            const placed = this.clips.get(widgetId);
            if (placed === undefined || placed.member === null) continue;
            if (Math.abs(offset - placed.offset) < 0.5) continue; // half a sample
            const node = this.nodeId(placed.member.element, placed.member);
            if (node === null) continue;
            const intent = {
                intent: "place",
                node,
                offset: this.unitsToBeats(offset) - placed.base,
            } as unknown as Intent;
            const inverse = document.inverse(intent);
            if (inverse === undefined) continue;
            legs.push([intent, inverse]);
            moved.push([widgetId, placed, offset]);
        }
        if (legs.length === 0 || !this.applyLegs("move the clips", legs)) {
            // Refused as one, so every clip goes back as one: what the host is
            // drawing is not what the document holds.
            for (const [widgetId] of moved) this.resync(widgetId);
            return false;
        }
        // The clips were drawn where they now are: keep the registry truthful,
        // or the next edit would measure a move against a stale placement.
        for (const [, placed, offset] of moved) placed.offset = offset;
        this.version = (this.doc as Document).version;
        this.dirty = true;
        this.followRender();
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
     * The OSC (and raw MIDI) items of an element as `[timeUnits, label]` pairs
     * — the piano-roll's OSC lane. An `OscItem` labels with its address, a
     * `MidiItem` with a short tag. Display only: a marker carries the time and a
     * label, not the full message, so it is not written back.
     */
    private oscOf(element: Element): [number, string][] {
        if (element instanceof Aggregate || element instanceof Vector) return [];
        let events: [number, unknown][];
        try {
            events = flatten(element, 0.0, 1.0, null, false);
        } catch {
            return [];
        }
        const out: [number, string][] = [];
        for (const [beat, item] of events) {
            if (item instanceof OscItem) {
                out.push([this.beatsToUnits(beat), String(item.addr)]);
            } else if (item instanceof MidiItem) {
                out.push([this.beatsToUnits(beat), "midi"]);
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
    /**
     * The arrangement was edited: mark it, and re-render now when `follow` is on.
     *
     * `applied` is the crate's own answer, and passing it is not optional
     * bookkeeping: **a resend is not an edit**. The document says so — it
     * refuses one and leaves its version where it was — and an editor that moved
     * its own version anyway, and answered "the composition changed", told every
     * other view of it to come into step with nothing.
     */
    private changed(applied = true): boolean {
        if (!applied) return false;
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
        return this.editing.held(this.element);
    }

    /**
     * The composition's editing context — its held document, its history and
     * the index between them.
     *
     * Reached through the **element**, so a second window over one composition
     * gets the same one. That is the whole of what makes an undo in either view
     * update both, and it is why none of this is a field here: a history
     * belongs to the data, never to a view.
     */
    protected override get editing(): FormEditing {
        return FormEditing.of(this.element);
    }

    /** The arrangement's face of the composition's history. */
    private get log(): Log | null {
        return this.editing.log;
    }

    /** The held document, or `null` before the first edit derived it. */
    private get doc(): Document | null {
        return this.editing.doc;
    }

    /**
     * Whether the held document has to be derived from the arrangement again
     * before the next edit.
     */
    private get rederive(): boolean {
        return this.editing.rederive;
    }

    private set rederive(value: boolean) {
        this.editing.rederive = value;
    }

    /** node id → the arrangement object an intent naming it writes to. */
    private get byNode(): Map<number, Indexed> {
        return this.editing.byNode;
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
        // Every other window over this composition draws the same data, and this
        // is the one place that knows *what* moved -- so the intent is reported
        // here rather than reduced to "something changed", which would cost them
        // a redefine per edit.
        this.editing.moved(intent);
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
        // **Mixing is every node's**, so it is written before asking what kind of
        // node this is — and written whole, like the rest of a configuration,
        // which is what makes an undo of a mute a mute again rather than a lane
        // that stays silent.
        setMixing(element, config);
        const mixed = MIXING_TAGS.some((key) => key in config);
        if (element instanceof Aggregate) {
            // An aggregate's configuration is the writer's own restrictions on
            // it, and for a logical one that is its **declared buses** — the
            // private wiring a patcher cord names. Written whole, like every
            // other configuration: what the intent does not carry is not there.
            element.setBuses((config.buses as unknown[]) ?? []);
            return true;
        }
        if (element instanceof Generator) {
            // A member's configuration is what it passes to the instrument, and
            // a cord is two of those naming one bus.
            (element as Element & { controls?: Record<string, unknown> | null }).controls = {
                ...((config.controls as Record<string, unknown>) ?? {}),
            };
            return true;
        }
        if (element instanceof Vector) {
            // A take's configuration is the **window** it reads. The
            // configuration is written whole, so a key the intent does not carry
            // is the default — reading from the first frame, once.
            element.start = Number(config.start ?? 0.0);
            element.loop = Boolean(config.loop ?? false);
            return true;
        }
        if (element instanceof Track) {
            // The same configuration over the other material: which **beat** of
            // its timeline this element begins at. A trim of a roll clip is the
            // same gesture as a trim of a take, and it lands here for the same
            // reason — the window is the element's, and the placement is the
            // aggregate's.
            element.start = Number(config.start ?? 0.0);
            return true;
        }
        const auto = automationOf(element, this.tempo);
        const flat = config.points as number[] | undefined;
        // Nothing else in the configuration was for this element — but the
        // mixing was, and an edit that only mutes a track is still an edit.
        if (auto === null || flat === undefined) return mixed;
        auto.env = pointsToEnv([...flat]);
        auto.refill();
        return true;
    }

    /**
     * Bring the **drawn record** of one placement back in step with the
     * arrangement, and say which widget draws it.
     */
    /**
     * Redefine the window when the last edit changed **which members exist**,
     * and say whether it did.
     *
     * A placement, a length, a curve and a note list are **props**: the host is
     * told and it draws them. A widget that was not there a moment ago — the
     * second half of a split, the clip an undone cut brings back — is not a
     * prop, and no acknowledgement can carry one. The only channel for it is a
     * redefine, so the editor that drew the window is what owes it: without this
     * the document and the objects the script holds had two clips while the
     * picture had one, until something happened to redraw.
     *
     * Deliberately **not** a redraw after every edit. A redefine rebuilds every
     * widget and drops what the host had in flight, which is exactly wrong for a
     * drag and exactly right for a structural edit.
     */
    protected override restructure(): boolean {
        if (!this.restructured) return false;
        this.restructured = false;
        // The case no prop can carry, for the other windows as much as for this
        // one: a widget that was not there a moment ago is not a value.
        this.editing.restructured();
        if (this.host === null || this.windowId === null) return false;
        this.update();
        return true;
    }

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
            placed.dur = this.drawnDur(element, member, undefined, member.offset + placed.base);
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
            if (!keep.has(node)) {
                // A placement that is gone takes a widget with it, and no prop
                // says "this clip is not there any more".
                aggregate.remove(handle);
                this.restructured = true;
            }
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
                // ...and one that is **back** — the undo of a cut, of a split,
                // of a join — needs a widget nobody drew.
                this.restructured = true;
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
            const event = new SeqEvent({ ...config });
            // **The note keeps the id the document gave it.** The payload a roll
            // sends carries no ids, so the i-th note inherits the i-th note's —
            // read off *these* objects. A note that came back unstamped is a new
            // node to the next edit, which mints one: the same notes resent then
            // arrive as different members, the document changes for nothing, and
            // every other view of it redraws to come into step with an edit that
            // was not one. It is the placement's rule, one level down.
            if (node.id !== undefined && node.id !== null) {
                setDocId(event as object, Math.trunc(Number(node.id)));
            }
            fresh.push([Number(placed.offset ?? 0.0), event]);
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
     * One step of the pile, **through the arrangement's log**.
     *
     * The generic walk projects a payload per structure; the tree's is a
     * document, so its legs come back as intents the crate has already turned
     * into what the document now says — which is why this overrides rather than
     * extends.
     */
    /**
     * The **tree's** legs of a history step, onto the held document and the
     * arrangement.
     *
     * The generic editor projects a payload through its domain; the tree's
     * domain is a document, so its legs are intents the crate turns into what the
     * document now says, and `project` writes that onto the page's objects. What the walk hands round is the whole entry, and one entry
     * can name several structures — a stroke over a take and a bend of the curve
     * over it are one order — so the legs this document cannot read are left for
     * whoever holds them.
     *
     * An undo is authoritative: it states what the document was, so it is not
     * checked against a version it predates.
     */
    override projectLegs(legs: readonly Leg[]): boolean {
        const [log, document] = this.history();
        // Drained here rather than at the end, because the editor that walked is
        // the only one that draws from `reflectStep`; every other window is told
        // on the way out of the turn.
        this.stepped = new Set();
        let applied = false;
        for (const leg of legs) {
            if (Math.trunc(Number(leg.structure ?? -1)) !== log.structure) continue;
            const payload = leg.payload as Intent | undefined;
            if (payload === undefined) continue;
            document.apply(payload);
            for (const wid of this.project(payload)) this.stepped.add(wid);
            applied = true;
        }
        return applied;
    }

    /**
     * Draw what a history walk left behind, and re-render if `follow` is on.
     *
     * The version comes back from the **document** when the walk moved it: a step
     * can apply several intents where the context counts one edit, and the two
     * have to be talking about the same picture on the next gesture. It never
     * goes backwards — a redefine moves the context's counter and no document's —
     * so what holds is the later of the two.
     */
    override reflectStep(): void {
        const widgets = this.stepped;
        this.stepped = new Set();
        const document = this.editing.doc;
        if (document !== null) this.version = Math.max(this.version, document.version);
        this.dirty = true;
        this.followRender();
        this.corrections = [];
        // A step that changed which members exist is answered with a new tree
        // rather than with props: the widgets the corrections name are about to
        // be rebuilt, and the clip an undone split takes away has no prop that
        // says so.
        if (this.restructure()) return;
        for (const wid of widgets) this.resync(wid);
        this.acknowledge(0);
        this.corrections = [];
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
     *
     * **The clock's tempo map wins.** A view of a piece and the clock playing it
     * cannot hold two answers for when a beat falls, so handing a clock here
     * adopts its map and redraws whatever moved. Without a clock the editor
     * keeps its own, which is what lets a composition be laid out before
     * anything plays.
     */
    async render(
        destination: unknown,
        clock?: TempoClock,
        { at = 0.0 }: { at?: number } = {},
    ): Promise<Playhead | null> {
        this.destination = destination;
        this.clock = clock ?? null;
        this.adoptMap(clock?.map);
        const playhead = await this.transport.play(destination as Server, { at });
        this.dirty = false; // what plays now *is* the arrangement
        return playhead;
    }

    /**
     * Take `tempoMap` as the editor's, redrawing if it says anything different
     * from the one held. Answers whether it moved.
     *
     * The one place the view's time and the clock's are reconciled: a line drawn
     * by one function and a sound played by another disagree by whatever a tempo
     * change moved, and no amount of redrawing the *lanes* fixes that — it is
     * the axis underneath them.
     */
    private adoptMap(tempoMap?: TempoMap | null): boolean {
        if (!tempoMap) return false;
        const held = this.tempoMap;
        const same =
            held.len === tempoMap.len &&
            Array.from({ length: held.len }, (_, i) => i).every((i) => {
                const a = held.segment(i);
                const b = tempoMap.segment(i);
                return a !== undefined && b !== undefined && a.every((v, j) => v === b[j]);
            });
        if (same) return false;
        this.tempoMap = tempoMap.copy();
        this.transport.tempoMap = this.tempoMap;
        if (this.host !== null && this.windowId !== null) {
            this.host.define(this.windowId, this.draw());
        }
        return true;
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
            element.temporalRelation(this.tempo) === SIMULTANEOUS &&
            !this.isExpanded(element)
        ) {
            // Its members start and end together: they are *one* thing on the
            // timeline, so they are one clip with layered bodies — not a lane of
            // clips that must be dragged one by one.
            return [this.lane([this.clipFor(element, base, owner, member)], element, base, member)];
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
            // **A lane with no clips is still a lane.** What decides whether
            // this aggregate draws a band of its own is whether any of its
            // members were *expanded* into lanes of their own — then the band
            // would be an empty duplicate of what is drawn below it. An
            // aggregate that simply has nothing in it keeps its lane, or
            // dragging the last clip off a lane would delete the lane, and a
            // composition would lose a track by moving a clip.
            const lane =
                clips.length > 0 || extra.length === 0
                    ? [this.lane(clips, element, base, member)]
                    : [];
            return [...lane, ...extra];
        }
        return [this.lane([this.clipFor(element, base, owner, member)], element, base, member)];
    }

    /**
     * One quantity **derived from the content**, held while `autofit` is off:
     * the `[lo, hi]` just derived, widened by whatever this key has needed
     * before.
     *
     * There are two of these — a roll's pitch domain and a clip's drawn length —
     * and they are one rule, so they are one method. With the switch on, each
     * draw simply says what the content says. With it off, the value the reader
     * is looking at is the widest this key has ever needed, so a content change
     * under the hand cannot make the picture smaller: a note moved earlier does
     * not shorten the clip it is in, and a note removed does not collapse the
     * roll around what is left.
     *
     * **Grows rather than freezes.** A frozen value could not take content that
     * legitimately arrived — a roll can never be written above its own top line,
     * a clip could never hold a phrase a script lengthened — and growing is what
     * makes it stable: an edge only ever moves outward, so nothing slides under
     * the hand.
     *
     * Keyed by {@link FormEditor.holdKey}, never by the widget id: a widget is
     * allocated afresh on every redefine, and a structural edit is exactly when
     * the re-framing showed.
     */
    private fit<K>(
        held: Map<K, [number, number]>,
        key: K,
        lo: number,
        hi: number,
    ): [number, number] {
        if (this.autofit) return [lo, hi];
        const was = held.get(key);
        if (was !== undefined) [lo, hi] = [Math.min(was[0], lo), Math.max(was[1], hi)];
        held.set(key, [lo, hi]);
        return [lo, hi];
    }

    /**
     * What a held value is filed under: **the node the document names**, and the
     * object itself only until there is one.
     *
     * A hold has to outlive the object it was derived from, because the
     * arrangement replaces objects for edits that change nothing a reader can
     * see. A clip that changes lane *reparents* — the handle leaves one
     * aggregate and a fresh one joins another — and so does a placement that
     * comes back from an undo ({@link FormEditor.setPlacements} adds it again).
     * Filed under the object, the hold was lost at exactly those doors: the clip
     * was re-derived from its content and the picture shrank under the hand,
     * which is the re-framing `autofit` is off to prevent.
     *
     * The document already keeps the identity that survives all of them — the
     * node id, carried across the reparent on purpose and stamped back onto a
     * restored placement — so that is the key. Asking for one before the first
     * conversion has to trigger it, exactly as {@link FormEditor.nodeId} does, or
     * the hold a session's first draw filed would be orphaned by its first edit.
     *
     * The object remains the fallback for a holder the document has not numbered,
     * and it is only a fallback: it does not survive a rebuild.
     */
    private holdKey(holder: Element | Aggregate | Member): HoldKey {
        let node = docIdOf(holder);
        if (node === null) {
            this.history(); // `toDocument` is what stamps the id
            node = docIdOf(holder);
        }
        return node === null ? holder : `node:${node}`;
    }

    /**
     * The `min`/`max` a roll is drawn over: the notes it holds, with headroom,
     * and never narrower than the default octave range.
     *
     * The **vertical** face of the auto-fit, so it is held by
     * {@link FormEditor.fit} like the other two: a note dragged to a new pitch
     * moved the extremes, and the domain re-derived from them slid the whole
     * roll under the hand that moved one note — as did removing the highest
     * note, which collapsed the window around what was left.
     */
    private pitchWindow(
        element: Element,
        notes: readonly (readonly number[])[],
    ): { min: number; max: number } {
        const pitches = notes.map((n) => n[2]);
        const [min, max] = this.fit(
            this.pitch,
            this.holdKey(element),
            Math.min(Math.min(...pitches) - PITCH_PAD, DEFAULT_PITCH[1]),
            Math.max(Math.max(...pitches) + PITCH_PAD, DEFAULT_PITCH[0]),
        );
        return { min, max };
    }

    /**
     * One `track` lane holding `clips`, with the shared time chrome and the
     * **mixing** the element carries.
     *
     * The header's toggles and its fader are drawn from the composition, not
     * from the view: what the lane shows is what a reopened document says, and
     * pressing one writes back through the log like every other edit.
     */
    private lane(
        clips: GuiNode[],
        element: Element,
        base = 0.0,
        member: Member | null = null,
    ): GuiNode {
        const wid = this.newId();
        const lane = track(
            {
                id: wid,
                label: nameOf(element),
                sampleRate: this.sampleRate,
                tempo: this.tempo,
                snap: this.quant > 0 ? this.beatsToUnits(this.quant) : undefined,
                autofit: this.autofit,
                mute: Boolean(element.mute),
                solo: Boolean(element.solo),
                level: Number(element.level ?? 1.0),
            },
            ...clips,
        );
        this.lanes.set(wid, element);
        this.laneMembers.set(wid, member);
        this.laneBases.set(wid, base);
        return lane;
    }

    /**
     * One lane header control onto the element that lane draws.
     *
     * Mixing is a leaf's configuration like any other, so it travels the same
     * road: `Configure` replaces the configuration **whole**, which is why it
     * starts from what the element already carries and writes one key over it.
     * That is also what makes it undoable — the inverse is the configuration as
     * it stood, read out of the document rather than remembered here.
     */
    private applyMixing(wid: number, tag: string, values: readonly unknown[]): boolean {
        const element = this.lanes.get(wid) as Element | undefined;
        if (element === undefined || values.length === 0) return false;
        const node = this.nodeId(element, this.laneMembers.get(wid) ?? null);
        if (node === null) return false;
        const config = leafConfig(element);
        config[tag] = tag === "level" ? Number(values[0]) : truthy(values[0]);
        const outcome = this.record(
            { intent: "configure", node, config },
            `${tag} the lane`,
        );
        if (outcome === null) return false;
        this.project(outcome.effective as Intent);
        return this.changed(Boolean(outcome.applied));
    }

    /**
     * One `clip`: the element placed at `base` beats (absolute on the shared
     * axis), with the body (or **bodies**) its kind calls for. Registers what it
     * drew, which is what the edit-back path resolves against.
     */
    /**
     * The length one clip is drawn at, **in the element's own unit** — the
     * placement's when it overrides, else the element's own, else what the
     * element extends to.
     *
     * One rule, in one place, because two of them is how a picture and a model
     * come to disagree: the draw asks this, and so does every path that has to
     * put a placement back ({@link Editor.redrawn}, after an inverse or a redo).
     *
     * A placement that states no length is drawn **as long as its content**,
     * which is the third face of the auto-fit and the one a reader meets
     * oftenest: moving the last note of a phrase earlier shortened the clip it
     * was in, and every clip after it on the lane went with the total. So the
     * derived length is held by {@link FormEditor.fit} while `autofit` is off,
     * exactly as the pitch domain is — a stated length is the reader's own and
     * is never held.
     */
    private drawnLength(element: Element, member: Member | null): number {
        if (member !== null && member.dur !== null) return member.dur;
        let length = element instanceof Element ? element.duration : null;
        if (length === null) length = this.extentOf(element);
        return this.fit(this.length, this.holdKey(member ?? element), 0.0, length)[1];
    }

    /**
     * The same length in **timeline units**, which needs the body: a take with no
     * duration given is as long as it is (1 unit = 1 sample).
     *
     * `at` is the clip's own onset in beats, which a length **in beats** needs to
     * have a length at all — the same two-position rule
     * {@link FormEditor.lengthToUnits} states.
     */
    private drawnDur(
        element: Element,
        member: Member | null,
        body?: Record<string, unknown>,
        at = 0.0,
    ): number {
        const length = this.drawnLength(element, member);
        const drawn = body ?? this.bodyFor(element, length);
        return "buffer" in drawn && length <= 0.0
            ? Number((element as Vector).buffer.frames ?? 0)
            : this.lengthToUnits(length, element, at);
    }

    private clipFor(
        element: Element,
        base: number,
        owner: Aggregate | null,
        member: Member | null,
    ): GuiNode {
        const wid = this.newId();
        const offset = this.beatsToUnits(base);
        const durLength = this.drawnLength(element, member);
        const body = this.bodyFor(element, durLength);
        const dur = this.drawnDur(element, member, body, base);

        // The placement's own base: a clip's offset is absolute on the shared
        // axis, a member's offset is relative to its aggregate.
        const parentBase = base - (member !== null ? member.offset : 0.0);
        this.clips.set(wid, new Placed(owner, member, parentBase, offset, dur));
        // A roll body is the `notes` element itself, and it edits: a body carries
        // no id of its own, so a note dragged inside one arrives tagged with
        // *this clip's* id.
        const roll = rollOwner(element, this.tempo);
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
            element.temporalRelation(this.tempo) === SIMULTANEOUS
        ) {
            const body: Record<string, unknown> = {};
            for (const m of element.handles) Object.assign(body, this.bodyFor(m.element, limit));
            return body;
        }

        const auto = automationOf(element, this.tempo);
        if (auto !== null) {
            const points = quads(auto.toPoints()).map(
                ([t, v, shape, curve]) =>
                    // A curve's times are an `Env`'s, so they are seconds and
                    // cross on the rate: the shape is drawn where it sounds,
                    // whatever the tempo.
                    [this.secsToUnits(t), v, shape, curve] as [number, number, number, number],
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
                    at: this.secsToUnits(offset),
                    dur: this.secsToUnits(seg.duration),
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
            const body: Record<string, unknown> = {
                notes,
                ...this.pitchWindow(element, notes),
            };
            // The **window** onto the timeline, in the axis' units like every
            // other clip prop: a track reads its notes from a beat the way a
            // take reads its frames from a frame, and the host has to know it or
            // a drag on the left edge would report a trim back to zero on a clip
            // that never was there. Sent only when there is one to state.
            const window = element.windowStart();
            if (window) body.start = this.beatsToUnits(Number(window));
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
            events = flatten(element, 0.0, 1.0, null, false);
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
        // In the element's own unit: an aggregate spans beats (its members'
        // offsets are), a take and a curve their own seconds.
        if (element instanceof Element && element.duration !== null) return Number(element.duration);
        // **The aggregate rule comes first**, and the curve's own length second:
        // a simultaneous aggregate holding a curve spans *beats*, like every
        // aggregate, and answering with the envelope's seconds would hand a
        // caller a number in one unit under a name that says the other.
        if (element instanceof Aggregate) {
            return element.handles.reduce(
                (max, m) =>
                    Math.max(
                        max,
                        m.offset +
                            toBeats(
                                m.dur ?? this.extentOf(m.element),
                                m.element.durationUnit,
                                this.tempo,
                            ),
                    ),
                0.0,
            );
        }
        const auto = automationOf(element, this.tempo);
        if (auto !== null) return auto.duration();
        if (element instanceof Segments) {
            // Its contents are a list, and its extent is the whole of it.
            return element.segments.reduce((sum, seg) => sum + seg.duration, 0.0);
        }
        if (element instanceof Vector) {
            const buf = element.buffer;
            const rate = buf.sampleRate || this.sampleRate;
            // Its own seconds: the frames it holds over the rate they were
            // recorded at, which no tempo enters.
            return Number(buf.frames ?? 0) / Number(rate);
        }
        let events: [number, unknown][];
        try {
            events = flatten(element, 0.0, 1.0, null, false);
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
 * An element's display name: its own `name` when it has one (an aggregate names
 * itself, an automation names the control it drives), else what it *is* — an
 * automation is an "envelope", not the `Element` that happens to wrap it.
 */
/**
 * The lane-header edit-backs that are the **composition's**: what a document
 * carries and a reopened piece gets back. A lane's `height` is deliberately not
 * one of them.
 */
const MIXING_TAGS = Object.keys(MIXING);

/**
 * An OSC flag as a boolean. The wire carries `0|1` as a number, and an older
 * host (or a hand-written test) may spell it as a string.
 */
function truthy(value: unknown): boolean {
    if (typeof value === "string") {
        return !["", "0", "false", "off"].includes(value.trim().toLowerCase());
    }
    return Boolean(value);
}

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
function automationOf(element: Element, tempo = 1.0): Automation | null {
    const wraps = (element as { wraps?: unknown }).wraps;
    if (wraps instanceof Automation) return wraps;
    if (
        element instanceof Aggregate &&
        element.length > 1 &&
        element.temporalRelation(tempo) === SIMULTANEOUS
    ) {
        for (const [, , child] of element.members) {
            const auto = automationOf(child, tempo);
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
 * The block of notes on the clipboard a paste carried, or `null` when what is on
 * it is not one.
 *
 * The clipboard is one typed document and a note block is its `text` kind — the
 * flat `start dur pitch velocity channel` array a `/gui_set notes` takes, which
 * is the host's own vocabulary for a roll. Reading it here rather than inventing
 * a shape is what keeps a block copied in a roll and a block pasted over a clip
 * the same block.
 *
 * A three-number group is the older `start dur pitch` form, read the way the
 * `notes` prop reads it: velocity 100, channel 0.
 *
 * **The dispatch is on the kind, so another one is another branch.** The text
 * kind is deliberately the one a note block travels in: it is a string, which is
 * the only thing that crosses a *system* clipboard on every platform, so a block
 * copied here stays pasteable the day the host's own clipboard is bridged to the
 * desktop's or to the browser's. A structured `elements` block — the tree's own
 * placed members — is the kind that will arrive when an owner has a door to put
 * one on the clipboard; it lands here, beside this, and every caller is
 * unchanged.
 */
function clipboardNotes(kind: string, raw: string): Note[] | null {
    if (kind !== "text") return null;
    let flat: unknown;
    try {
        const document = JSON.parse(raw) as { content?: { text?: string } };
        flat = JSON.parse(document?.content?.text ?? "");
    } catch {
        return null;
    }
    if (!Array.isArray(flat) || flat.length === 0) return null;
    const values = flat.map((x) => Number(x));
    if (values.some((x) => !Number.isFinite(x))) return null;
    const stride = values.length % 5 === 0 ? 5 : 3;
    const notes: Note[] = [];
    for (let i = 0; i + stride <= values.length; i += stride) {
        notes.push(
            stride === 5
                ? [values[i], values[i + 1], values[i + 2], values[i + 3], values[i + 4]]
                : [values[i], values[i + 1], values[i + 2], 100, 0],
        );
    }
    return notes.length > 0 ? notes : null;
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
    tempo = 1.0,
): [Element, Member | null] {
    if (
        element instanceof Aggregate &&
        element.length > 1 &&
        element.temporalRelation(tempo) === SIMULTANEOUS
    ) {
        for (const h of element.handles) {
            if (automationOf(h.element, tempo) !== null) return [h.element, h];
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
function rollOwner(element: Element, tempo = 1.0): Element | null {
    if (
        element instanceof Aggregate &&
        element.length > 1 &&
        element.temporalRelation(tempo) === SIMULTANEOUS
    ) {
        for (const m of element.handles) {
            if (editableTimeline(m.element) !== null) return m.element;
        }
        return null;
    }
    return element;
}

/**
 * The buffer an element's samples are in, or `null` — what a signal view is
 * opened over.
 *
 * A `Vector` wraps one; a buffer handed straight to a {@link FormEditor} **is**
 * one. Anything else (a generator, an aggregate, a frozen source a reopened
 * session could not resolve) has no samples to write, which is the refusal
 * `openSignal` throws.
 */
function takeOf(element: Element | null): Buffer | null {
    const take = (element as { wraps?: unknown } | null)?.wraps ?? element;
    return isSamples(take) ? take : null;
}

/**
 * What a forward-only element produced, on a timeline of its own — the read-only
 * roll's data.
 *
 * The *change of state* happens right here: a pattern is bounced by `flatten`,
 * so the roll shows the notes the generator will play. Nothing is edited back
 * onto it, because writing this timeline would write a copy nobody plays; the
 * roll says so with the widget's own `notesEditable`, before the hand tries.
 */
function bounced(element: Element): Timeline {
    const timeline = new Timeline();
    if (element instanceof Aggregate || element instanceof Vector) return timeline;
    let events: [number, unknown][];
    try {
        events = flatten(element, 0.0, 1.0, null, false) as unknown as [number, unknown][];
    } catch {
        return timeline;
    }
    for (const [beat, item] of events) timeline.add(Number(beat), item as never);
    return timeline;
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
 * others (OSC items) are preserved. Uses only the public timeline API.
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
 * `OscItem`, a rest, an automation lane.
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
