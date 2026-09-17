// Static timelines and a playhead (mirrors `clausters/seq/timeline.py`).
//
// The counterpart to the generative layer (`Routine`, `Pbind`). A routine is a
// forward-only generator: its musical state lives in the generator's locals,
// so it cannot be *seeked*. A `Timeline` is the opposite — a **static,
// editable list of timed items kept sorted by beat**, with random access by
// time (`indexAt`, `range`). That is what makes DAW-style transport controls
// possible: a `Playhead` scans the timeline forward as the clock advances, and
// play / stop / locate / loop re-seek the cursor by time at the boundaries.
//
// An *item* is anything that can render itself on a destination — it has a
// `play(destination)` method. `Event` already is one, so a timeline of events
// renders to whatever destination the playhead holds, exactly like the rest of
// the client. `OscItem` wraps a raw OSC message, so a timeline can also be a
// plain, editable OSC score.
//
// This layer is **client-side**: each playhead has its own local transport
// over its own timeline, and several clients phase-align through `quant` and
// the shared `/transport_set` grid. A playhead can also *follow* the server's
// transport (`followTransport`), which is one conductor's play/stop/locate
// driving every client — the same local transport, driven from outside.

import { TempoClock } from "../base/clock.ts";
import type { Schedulable } from "../base/clock.ts";
import { TempoMap } from "../base/time.ts";
import { ManualTimebase, quantDelay } from "../base/timebase.ts";
import { currentRoutine } from "../base/context.ts";
import { main } from "../base/main.ts";
import { Routine, StopStream, Stream } from "../base/stream.ts";
import { Event } from "./event.ts";
import type { EventDestination } from "./event.ts";
import { Pattern } from "./pattern.ts";
import type { Server, TimedMessage } from "../defs/server/index.ts";
import type { MsgArg } from "../base/osc.ts";
import { OscFunc } from "../responders.ts";
import type { ResponderMessage } from "../responders.ts";

/** What a timeline can hold: anything that renders itself on a destination. */
export interface TimelineItem {
    play(destination: PlayDestination): unknown;
}

/** The destination a playhead renders on. `Server` satisfies it. */
export interface PlayDestination extends EventDestination {
    sendBundle(
        messages: readonly TimedMessage[],
        options?: { delayBeats?: number; clock?: TempoClock },
    ): void;
    /**
     * Raw MIDI at the playhead's beat, on a destination that carries MIDI
     * (`MidiServer`). Optional because most destinations do not: an
     * `OscItem` on a MIDI port and a `MidiItem` on an OSC server are both
     * mistakes, and each is reported by the destination that cannot answer.
     */
    sendMessage?(message: ArrayLike<number>): unknown;
}

/**
 * One timed item. A stable object, so it can be removed or moved by identity
 * after other edits have shifted positions.
 */
export class Entry {
    beat: number;
    readonly item: unknown;

    constructor(beat: number, item: unknown) {
        this.beat = beat;
        this.item = item;
    }
}

/**
 * A raw OSC message as a timeline item: rendering it sends the message at the
 * playhead's current logical beat.
 */
export class OscItem {
    readonly addr: string;
    readonly args: readonly MsgArg[];

    constructor(addr: string, ...args: MsgArg[]) {
        this.addr = addr;
        this.args = args;
    }

    play(destination: PlayDestination): void {
        destination.sendBundle([[this.addr, ...this.args]]);
    }
}

/**
 * Raw MIDI bytes as a timeline item: rendering it emits the message at the
 * playhead's current logical beat through a `MidiServer`.
 */
export class MidiItem {
    readonly message: Uint8Array;

    constructor(message: ArrayLike<number>) {
        this.message = Uint8Array.from(message);
    }

    play(destination: PlayDestination): void {
        if (typeof destination.sendMessage !== "function") {
            throw new TypeError(
                "a MidiItem needs a MIDI destination (a MidiServer), " +
                    "not one that carries OSC",
            );
        }
        destination.sendMessage(this.message);
    }
}

/**
 * The key that names a raw OSC message in an item's data, and the one that names
 * raw MIDI bytes. An `Event` carries neither — it is its own parameters — so
 * what an item *is* is told apart by which of the two keys is there, and by
 * neither being there.
 */
export const OSC_KEY = "osc";
/** @see {@link OSC_KEY} */
export const MIDI_KEY = "midi";

/**
 * One timeline item as plain, JSON-able data — or `null` for an item this has no
 * description of.
 *
 * **One description, because two seams need it.** A document writes a timeline's
 * items as the configuration of a placed clang, and the editing domain hands
 * them across the crate's `events` vocabulary as an event's opaque `data`; the
 * two are the same question — *what is this item, written down* — and answering
 * it twice is how a marker comes back from one of them as a note.
 *
 * An `Event` travels as its parameters. An {@link OscItem} and a
 * {@link MidiItem} are not parameters, and each names itself with its own key
 * (`OSC_KEY`, `MIDI_KEY`), which is what a reader tells them apart
 * by.
 */
export function itemData(item: unknown): Record<string, unknown> | null {
    if (item instanceof OscItem) return { [OSC_KEY]: String(item.addr), args: [...item.args] };
    if (item instanceof MidiItem) return { [MIDI_KEY]: [...item.message] };
    const props = (item as { props?: unknown } | null)?.props;
    if (props !== undefined && props !== null && typeof props === "object") {
        return { ...(props as Record<string, unknown>) };
    }
    if (item !== null && typeof item === "object") return { ...(item as Record<string, unknown>) };
    return null;
}

/** The item {@link itemData} wrote: an `OscItem`, a `MidiItem`, or the `Event` anything else is. */
export function itemFromData(data: Record<string, unknown> | null | undefined): unknown {
    const held = { ...(data ?? {}) };
    if (OSC_KEY in held) {
        const addr = String(held[OSC_KEY]);
        const args = (held.args ?? []) as MsgArg[];
        return new OscItem(addr, ...args);
    }
    if (MIDI_KEY in held) {
        return new MidiItem((held[MIDI_KEY] ?? []) as ArrayLike<number>);
    }
    return new Event(held);
}

/**
 * How many events a bounce records before it decides the pattern is endless
 * (`Timeline.fromPattern`'s `maxEvents` default).
 *
 * A bounce holds every event in memory, so a million is already past any real
 * piece and nowhere near a legitimate one — which is what makes the cap honest
 * *here* and wrong inside `TempoClock.render`, where a long offline render of a
 * real score is exactly the thing that runs for a very long time on purpose.
 */
export const MAX_BOUNCED_EVENTS = 1_000_000;

/**
 * A plan in logical time: `(beat, item)` kept sorted by beat, with random
 * access by time, its own tempo map, and the verbs that play it.
 *
 * Items stay in beat order, and a stable insert preserves the order of items
 * added at the same beat (a note-off before a re-trigger). `add` returns a
 * handle you pass back to `remove`/`move`, so edits stay correct as other
 * inserts shift indices.
 *
 * **Its tempo is its own.** `map` is the timeline's `TempoMap`, the plan of how
 * its beats fall on seconds, and it is data like the items: edited on the map,
 * saved with the timeline. There is no `setTempo` here, because no clock is
 * handled: a timeline plays itself (`play`, `locate`, `pause`, `stop`, `loop`)
 * on a clock of its own that is born on its beat 0, so a clock beat *is* a
 * timeline beat.
 *
 * **An item is anything playable**: an `Event`, an `OscItem`/`MidiItem`, an
 * `Automation`, a pattern, a `Routine` — and **another timeline**, which its
 * parent plays when it reaches it. Each timeline keeps its own units: a
 * child's beats go to seconds through its own map, so siblings at different
 * tempi start together by construction. One tree plays on one engine, the
 * root's; a child stays an object of its own, and played by itself it plays on
 * its own clock.
 *
 * A timeline is stateful, so **one instance has at most one parent**: adding
 * one that already has a parent is refused (`copy` makes an independent one),
 * and so is adding an ancestor, which would be a cycle.
 */
export class Timeline {
    private entries: Entry[] = [];
    /** The timeline this one is an item of, or `null`. */
    parent: Timeline | null = null;
    // Built on first use, so a timeline can be written before the core is
    // loaded: only reading its time needs the map.
    private mapHeld: TempoMap | null;
    private readonly tempo: number;
    /** @internal */
    player: TimelinePlayer | null = null;

    constructor(
        items?: Iterable<readonly [number, unknown]>,
        { tempo = 1.0, tempoMap }: { tempo?: number; tempoMap?: TempoMap } = {},
    ) {
        this.mapHeld = tempoMap ?? null;
        this.tempo = tempo;
        if (items) for (const [beat, item] of items) this.add(beat, item);
    }

    /**
     * The timeline's tempo map: how its beats fall on seconds. Editable data —
     * write a tempo change on it (`push`, `ramp`, `env`) and what plays follows
     * it.
     */
    get map(): TempoMap {
        this.mapHeld ??= new TempoMap(this.tempo);
        return this.mapHeld;
    }

    set map(tempoMap: TempoMap) {
        this.mapHeld = tempoMap;
        if (this.player?.clock) this.player.clock.map = tempoMap;
    }

    // ---- editing ----

    /**
     * Inserts `item` at `beat` (kept sorted); returns the entry handle.
     *
     * A timeline as `item` becomes this one's child: refused if it already has
     * a parent (its `copy` has none) or if it is this timeline or one of its
     * ancestors.
     */
    add(beat: number, item: unknown): Entry {
        if (item instanceof Timeline) {
            this.checkChild(item, false);
            item.parent = this;
        }
        const entry = new Entry(beat, item);
        this.entries.splice(this.insertIndex(beat), 0, entry);
        return entry;
    }

    /** Removes an entry returned by `add` (by identity). */
    remove(entry: Entry): this {
        const i = this.entries.indexOf(entry);
        if (i >= 0) this.entries.splice(i, 1);
        if (entry.item instanceof Timeline) entry.item.parent = null;
        return this;
    }

    /** Moves an entry to `newBeat`, keeping the timeline sorted. */
    move(entry: Entry, newBeat: number): Entry {
        const i = this.entries.indexOf(entry);
        if (i >= 0) this.entries.splice(i, 1);
        entry.beat = newBeat;
        this.entries.splice(this.insertIndex(newBeat), 0, entry);
        return entry;
    }

    /** Drops every item. */
    clear(): this {
        for (const e of this.entries) if (e.item instanceof Timeline) e.item.parent = null;
        this.entries = [];
        return this;
    }

    /**
     * Replaces the whole contents with `items` (`[beat, item]` pairs), **in one
     * step**.
     *
     * The step is what this is for. Clearing and re-adding leaves the timeline
     * empty in between, which nothing here can observe — a page has one thread —
     * and which the Python client's event loop very much can: a rebuild that
     * outlasts CPython's switch interval was read half-done in 87.7% of reads at
     * 4000 notes. The verb is the same in both clients because the surfaces are
     * one surface; what differs is only whether the language could ever have
     * noticed.
     */
    replace(items: Iterable<[number, unknown]>): this {
        const entries = [...items].map(([beat, item]) => new Entry(Number(beat), item));
        const children = entries.map((e) => e.item).filter((i) => i instanceof Timeline);
        for (const child of children) this.checkChild(child, true);
        entries.sort((a, b) => a.beat - b.beat);
        for (const e of this.entries) if (e.item instanceof Timeline) e.item.parent = null;
        for (const child of children) child.parent = this;
        this.entries = entries;
        return this;
    }

    /**
     * Refuses a child that already has another parent, or that is this
     * timeline or one of its ancestors.
     */
    private checkChild(item: Timeline, replacing: boolean): void {
        if (item.parent !== null && !(replacing && item.parent === this)) {
            throw new Error(
                "this timeline already has a parent; a timeline is stateful and " +
                    "plays in one place — add item.copy() instead",
            );
        }
        for (let node: Timeline | null = this; node !== null; node = node.parent) {
            if (node === item) {
                throw new Error("a timeline cannot contain itself or an ancestor");
            }
        }
    }

    /**
     * An independent timeline with the same plan: its own tempo map, its
     * children copied (recursively), and the other items shared — an event or a
     * message is a value, and a routine item is played fresh on every pass
     * anyway. It is stopped at beat 0 and has no parent.
     */
    copy(): Timeline {
        const twin = new Timeline(undefined, { tempoMap: this.map.copy() });
        twin.entries = this.entries.map((e) =>
            new Entry(e.beat, e.item instanceof Timeline ? e.item.copy() : e.item));
        for (const e of twin.entries) if (e.item instanceof Timeline) e.item.parent = twin;
        return twin;
    }

    /**
     * Snaps every placement to the nearest multiple of `grid` beats; a zero or
     * negative grid is a no-op.
     */
    quantize(grid: number): this {
        if (grid <= 0) return this;
        for (const entry of this.entries) {
            entry.beat = Math.max(0, Math.round(entry.beat / grid) * grid);
        }
        this.entries.sort((a, b) => a.beat - b.beat);
        return this;
    }

    // ---- random access by time ----

    /**
     * The index the *last* item at `beat` would be inserted after (a stable
     * insert), and the cursor of the first item strictly after it.
     */
    private insertIndex(beat: number): number {
        let lo = 0;
        let hi = this.entries.length;
        while (lo < hi) {
            const mid = (lo + hi) >> 1;
            if (this.entries[mid]!.beat <= beat) lo = mid + 1;
            else hi = mid;
        }
        return lo;
    }

    /**
     * The cursor (index) of the first item at or after `beat` — the seek
     * primitive a playhead starts and locates with.
     */
    indexAt(beat: number): number {
        let lo = 0;
        let hi = this.entries.length;
        while (lo < hi) {
            const mid = (lo + hi) >> 1;
            if (this.entries[mid]!.beat < beat) lo = mid + 1;
            else hi = mid;
        }
        return lo;
    }

    /** The `(beat, item)` pairs in the half-open beat window `[t0, t1)`. */
    range(t0: number, t1: number): [number, unknown][] {
        return this.entries
            .slice(this.indexAt(t0), this.indexAt(t1))
            .map((e) => [e.beat, e.item] as [number, unknown]);
    }

    /** The items exactly at `beat`. */
    at(beat: number): unknown[] {
        return this.entries.filter((e) => e.beat === beat).map((e) => e.item);
    }

    /**
     * The timeline's logical length, in its own beats: the beat of the last
     * item (0 when empty), **extended by any child that lasts longer**.
     *
     * A parent is never shorter than what it holds. A child's length is in the
     * child's beats, so it goes to seconds through the child's map and back to
     * this timeline's beats through this one's, from the beat it is placed at.
     * A looping child never ends.
     */
    duration(): number {
        let end = 0;
        for (const e of this.entries) {
            if (e.item instanceof Timeline) {
                const child = e.item;
                if (child.looping()) return Infinity;
                const secs = this.map.secsAt(e.beat) + child.map.secsAt(child.duration());
                end = Math.max(end, this.map.beatsAt(secs));
            } else {
                end = Math.max(end, e.beat);
            }
        }
        return end;
    }

    /** @internal */
    looping(): boolean {
        return this.player !== null && this.player.loop !== null;
    }

    /** @internal */
    get entryList(): readonly Entry[] {
        return this.entries;
    }

    get length(): number {
        return this.entries.length;
    }

    /** The `(beat, item)` pair at index `i`. */
    get(i: number): [number, unknown] | undefined {
        const entry = this.entries[i];
        return entry ? [entry.beat, entry.item] : undefined;
    }

    *[Symbol.iterator](): Generator<[number, unknown], void, undefined> {
        for (const entry of this.entries) yield [entry.beat, entry.item];
    }

    // ---- playing ----

    /**
     * Plays the timeline from beat `at`, on a clock of its own.
     *
     * No `at` resumes where `pause` left it (beat 0 the first time). `quant`
     * starts it on the next multiple of `quant` beats of the **ambient** clock
     * (the routine's, the session's), which is how it lands on another clock's
     * bar. `destination` is where items play (a `Server`, a `MidiServer`);
     * without one the ambient server is resolved when an item needs one.
     *
     * Playing a child on its own plays only it, on its own clock; its parent is
     * not involved.
     */
    play(
        { at, quant, destination }: {
            at?: number;
            quant?: number;
            destination?: PlayDestination;
        } = {},
    ): this {
        const player = this.playerFor();
        if (destination !== undefined) player.destination = destination;
        if (at === undefined) {
            player.play(player.position(), quant);
        } else {
            player.mark = at;
            player.play(at, quant);
        }
        return this;
    }

    /**
     * Moves to `beat`. Playing, the timeline goes on from there: what is
     * sounding keeps its own release, children are entered at the beat that
     * corresponds, and an onset the new position has passed is not recovered.
     * Stopped, it is where the next `play` starts.
     */
    locate(beat: number): this {
        this.playerFor().locate(beat);
        return this;
    }

    /** Halts, holding the position: `play` with no `at` resumes there. */
    pause(): this {
        this.player?.halt();
        return this;
    }

    /**
     * Halts and goes back to the mark: the beat of the last `play` given an
     * `at`, or of the last `locate` made while stopped (a resume does not move
     * it).
     */
    stop(): this {
        if (this.player !== null) {
            this.player.halt();
            this.player.hold(this.player.mark);
        }
        return this;
    }

    /**
     * Loops the half-open beat window `[start, end)`: reaching `end` goes on
     * from `start`, with physical time running on. A child longer than the
     * window loops over its part that corresponds to it. Set before or during
     * play.
     */
    loop(start: number, end: number): this {
        this.playerFor().loop = [start, end];
        return this;
    }

    /** Stops looping. */
    unloop(): this {
        if (this.player !== null) this.player.loop = null;
        return this;
    }

    /** Where the timeline is, in its beats. */
    position(): number {
        return this.player === null ? 0 : this.player.position();
    }

    /**
     * Whether it is playing. False after `pause`/`stop` and once the end is
     * reached.
     */
    get playing(): boolean {
        return this.player !== null && this.player.running;
    }

    /** Whether it stopped because it reached its end (a loop never does). */
    get finished(): boolean {
        return this.player !== null && this.player.finished;
    }

    private playerFor(): TimelinePlayer {
        this.player ??= new TimelinePlayer(this);
        return this.player;
    }

    // ---- capture a pattern into a timeline ----

    /**
     * Bounces an event pattern into a static timeline by running it with no
     * pacing and recording each event at its logical beat. `dur` bounds an
     * open-ended pattern (in beats); leave it out to drain a finite one.
     *
     * The run is the clock's own **offline drive** (`TempoClock.render`), so it
     * is the same driver live playback uses with the waiting taken out.
     *
     * `tempo` is the tempo the pattern is run at, and the timeline's.
     *
     * `dur` bounds an endless pattern, in beats. Without one, an endless
     * pattern is **caught rather than run forever**: the bounce throws once it
     * has recorded `maxEvents` (`MAX_BOUNCED_EVENTS` by default). That guard is
     * this call's and not the clock's — a long offline `render` of a real score
     * is meant to run for a long time, where a bounce with no bound is a
     * mistake.
     */
    static fromPattern(
        pattern: Pattern<unknown>,
        {
            dur,
            tempo = 1.0,
            maxEvents = MAX_BOUNCED_EVENTS,
        }: { dur?: number; tempo?: number; maxEvents?: number } = {},
    ): Timeline {
        const timeline = new Timeline(undefined, { tempo });
        const recorder: EventDestination = {
            playEvent(event: Event) {
                timeline.add(currentRoutine()?.logicalBeat ?? 0, event);
                return null;
            },
            sendMsg() {},
        };
        // The offline drive, which is what a bounce is: no wall clock, no
        // ticker, no sleeping. The clock is deliberately **not started** —
        // `render` walks the queue in beat order itself, so the pattern is
        // queued and then drained, which is the same pair of calls the Python
        // client makes (`pattern.play(clock, recorder)`; `clock.render(dur)`).
        // Driving a `manualTicker` by hand here was a second driver for a job
        // this one already does.
        const clock = new TempoClock(tempo, { timebase: new ManualTimebase(0) });
        pattern.play(recorder, { clock });
        try {
            clock.render(dur, { maxSteps: dur === undefined ? maxEvents : undefined });
        } catch (cause) {
            throw new Error(
                `Timeline.fromPattern: the pattern did not end after ${maxEvents} ` +
                    "events — pass { dur } to bound an endless one",
                { cause },
            );
        }
        clock.close();
        return timeline;
    }
}

// ---- the engine: one tree, woken by its root's clock ----

/**
 * A timeline's clock as what it plays sees it, while the root's clock wakes the
 * tree.
 *
 * An item of a child measures in the **child's** beats — an event's sustain, an
 * automation's length, a routine's yields, a pattern's durations — but only the
 * root's clock runs. So the node hands each item this view: its beats and
 * conversions are the child's, placed on the root's time by the node's origin,
 * and what it schedules goes onto the root's clock at the beat that
 * corresponds. Timetags and sessions are the root clock's.
 */
class ClockView {
    private readonly node: TimelineNode;

    constructor(node: TimelineNode) {
        this.node = node;
    }

    private get root(): TempoClock {
        return this.node.player.clock!;
    }

    beats2secs(beats: number): number {
        return this.node.origin + this.node.timeline.map.secsAt(beats);
    }

    secs2beats(secs: number): number {
        return this.node.timeline.map.beatsAt(secs - this.node.origin);
    }

    rootBeat(beats: number): number {
        if (this.node.isRoot) return beats;
        return this.node.player.rootBeat(this.beats2secs(beats));
    }

    localBeat(rootBeat: number): number {
        if (this.node.isRoot) return rootBeat;
        return this.secs2beats(this.node.player.rootSecs(rootBeat));
    }

    beats(): number {
        const routine = currentRoutine();
        if (routine !== null && (routine.clock as unknown) === this) return routine.logicalBeat;
        return this.localBeat(this.root.beats());
    }

    get tempo(): number {
        return this.node.timeline.map.tempoAt(this.beats());
    }

    get map(): TempoMap {
        return this.node.timeline.map;
    }

    sched(delayBeats: number, item: Schedulable): this {
        this.schedule(this.beats() + delayBeats, item);
        return this;
    }

    schedAbs(beat: number, item: Schedulable): this {
        this.schedule(beat, item);
        return this;
    }

    play<T extends Schedulable>(item: T): T {
        this.schedule(this.beats(), item);
        return item;
    }

    unsched(item: Schedulable): this {
        const wrapper = this.node.player.wrappers.get(item);
        if (wrapper !== undefined) {
            this.node.player.wrappers.delete(item);
            this.root.unsched(wrapper);
        }
        return this;
    }

    private schedule(beat: number, item: Schedulable): void {
        const player = this.node.player;
        const wrapper = new Routine(translated(item, this));
        player.wrappers.set(item, wrapper);
        player.owned.push([this.node, wrapper]);
        this.root.schedAbs(this.rootBeat(beat), wrapper);
    }

    // What a Server and a session read, from the root clock.
    get timebase() { return this.root.timebase; }
    get pacingOrigin() { return this.root.pacingOrigin; }
    get startTime() { return this.root.startTime; }
    get session() { return this.root.session; }
    get name() { return this.root.name; }
    get rolling() { return this.root.rolling; }
    get frozen() { return this.root.frozen; }
}

/**
 * The body of the routine the root clock wakes for `item`, an item scheduled
 * on a child's view: each wake runs `item` at the child's beat that
 * corresponds, and turns the child beats it yields into root beats.
 */
function translated(item: Schedulable, view: ClockView) {
    return function* (): Generator<number, void, unknown> {
        for (;;) {
            const me = currentRoutine()!;
            const rootBeat = me.logicalBeat;
            const local = view.localBeat(rootBeat);
            const saved: [TempoClock | null, number] = [me.clock, me.logicalBeat];
            me.clock = view as unknown as TempoClock;
            me.logicalBeat = local;
            let delta: unknown;
            try {
                if (item instanceof Stream) {
                    item.clock = view as unknown as TempoClock;
                    item.logicalBeat = local;
                    try {
                        delta = item.next(view);
                    } catch (error) {
                        if (error instanceof StopStream) return;
                        throw error;
                    }
                } else {
                    delta = (item as () => unknown)();
                }
            } finally {
                [me.clock, me.logicalBeat] = saved;
            }
            if (typeof delta !== "number") return;
            yield view.rootBeat(local + delta) - rootBeat;
        }
    };
}

type Due = [number, () => void, number | null];

/**
 * One timeline of a playing tree: where its beat 0 falls on the root's axis of
 * seconds, its cursor, and the children it has entered.
 */
class TimelineNode {
    readonly player: TimelinePlayer;
    readonly timeline: Timeline;
    origin: number;
    readonly isRoot: boolean;
    readonly view: ClockView;
    cursor = 0;
    children: TimelineNode[] = [];

    constructor(player: TimelinePlayer, timeline: Timeline, origin: number, beat: number) {
        this.player = player;
        this.timeline = timeline;
        this.origin = origin;
        this.isRoot = timeline === player.timeline;
        this.view = new ClockView(this);
        this.enter(beat);
    }

    /**
     * Places the cursor at `beat`: the next onset at or after it, and the
     * children the beat is inside, entered at the beat that corresponds.
     */
    enter(beat: number): void {
        const tl = this.timeline;
        this.cursor = tl.indexAt(beat);
        this.children = [];
        const secs = this.origin + tl.map.secsAt(beat);
        for (const e of tl.entryList.slice(0, this.cursor)) {
            if (!(e.item instanceof Timeline)) continue;
            const child = e.item;
            const origin = this.origin + tl.map.secsAt(e.beat);
            const local = child.map.beatsAt(secs - origin);
            if (local < child.duration() || child.looping()) {
                this.children.push(new TimelineNode(this.player, child, origin, local));
            }
        }
    }

    /**
     * `[seconds, action, beat]` of the next thing to do in this subtree, or
     * `null` when it has ended. `beat` is the root's exact beat when the action
     * is the root's own, and `null` when it has to be read through the maps.
     */
    nextDue(): Due | null {
        const tl = this.timeline;
        let best: Due | null = null;
        const entry = tl.entryList[this.cursor];
        if (entry !== undefined) {
            best = [this.origin + tl.map.secsAt(entry.beat), () => this.onset(),
                this.isRoot ? entry.beat : null];
        }
        const loop = this.isRoot ? null : tl.player?.loop ?? null;
        if (loop !== null) {
            const endSecs = this.origin + tl.map.secsAt(loop[1]);
            if (best === null || best[0] >= endSecs) best = [endSecs, () => this.wrap(), null];
        }
        for (const child of this.children) {
            const due = child.nextDue();
            if (due !== null && (best === null || due[0] < best[0])) best = due;
        }
        return best;
    }

    private wrap(): void {
        const [start, end] = this.timeline.player!.loop!;
        const map = this.timeline.map;
        this.player.release(this);
        this.origin += map.secsAt(end) - map.secsAt(start);
        this.enter(start);
    }

    private onset(): void {
        const tl = this.timeline;
        const e = tl.entryList[this.cursor]!;
        this.cursor += 1;
        if (e.item instanceof Timeline) {
            const origin = this.origin + tl.map.secsAt(e.beat);
            this.children.push(new TimelineNode(this.player, e.item, origin, 0));
            return;
        }
        this.player.render(this, e.beat, e.item);
    }

    prune(): void {
        this.children = this.children.filter((c) => c.nextDue() !== null);
        for (const c of this.children) c.prune();
    }

    contains(node: TimelineNode): boolean {
        return node === this || this.children.some((c) => c.contains(node));
    }
}

/**
 * The engine of a timeline played as a root: its hidden clock, born on the
 * timeline's beat 0, and the routine that wakes the whole tree on it.
 *
 * @internal
 */
export class TimelinePlayer {
    clock: TempoClock | null = null;
    destination: PlayDestination | null = null;
    loop: [number, number] | null = null;
    mark = 0;
    running = false;
    finished = false;
    root: TimelineNode | null = null;
    owned: [TimelineNode, Routine][] = [];
    readonly wrappers = new Map<Schedulable, Routine>();
    private engine: Routine | null = null;
    private epoch = 0;
    private held = 0;

    readonly timeline: Timeline;

    constructor(timeline: Timeline) {
        this.timeline = timeline;
    }

    rootSecs(beat: number): number {
        return this.timeline.map.secsAt(beat);
    }

    rootBeat(secs: number): number {
        return this.timeline.map.beatsAt(secs);
    }

    private clockFor(): TempoClock {
        if (this.clock === null) {
            const ambient = main.resolveClock();
            this.clock = new TempoClock(1, {
                tempoMap: this.timeline.map,
                timebase: ambient?.timebase,
            });
        }
        return this.clock;
    }

    position(): number {
        if (this.running && this.clock !== null) return this.clock.beats();
        return this.held;
    }

    hold(beat: number): void {
        this.held = beat;
        this.clock?.locate(beat);
    }

    play(at: number, quant?: number): void {
        const clock = this.clockFor();
        this.halt();
        this.finished = false;
        let delay = 0;
        if (quant) {
            const ambient = main.resolveClock();
            if (ambient !== null && ambient !== clock) {
                const now = ambient.beats();
                delay = ambient.beats2secs(now + quantDelay(ambient.gridBeat(), quant)) -
                    ambient.beats2secs(now);
            }
        }
        clock.locate(this.rootBeat(this.rootSecs(at) - delay));
        this.running = true;
        this.epoch += 1;
        const epoch = this.epoch;
        const player = this;
        this.engine = new Routine(function* () {
            yield* player.run(epoch, at);
        });
        clock.schedAbs(clock.beats(), this.engine);
        clock.start();
    }

    locate(beat: number): void {
        if (this.running) {
            this.play(beat);
        } else {
            this.mark = beat;
            this.hold(beat);
            this.finished = false;
        }
    }

    halt(): void {
        if (this.clock === null) return;
        this.held = this.clock.beats();
        this.running = false;
        this.epoch += 1;
        if (this.engine !== null) {
            this.clock.unsched(this.engine);
            this.engine = null;
        }
        this.release(null);
    }

    /**
     * Unschedules the routines a pass started, in `node`'s subtree (all of them
     * for `null`).
     */
    release(node: TimelineNode | null): void {
        const keep: [TimelineNode, Routine][] = [];
        for (const [owner, routine] of this.owned) {
            if (node === null || node.contains(owner)) this.clock!.unsched(routine);
            else keep.push([owner, routine]);
        }
        this.owned = keep;
        if (node === null) this.wrappers.clear();
    }

    /**
     * Plays one item of `node` at its `beat`, with the moment and the clock set
     * to the node's: an item measures in its own timeline's beats.
     */
    render(node: TimelineNode, beat: number, item: unknown): void {
        const me = currentRoutine()!;
        const saved: [TempoClock | null, number] = [me.clock, me.logicalBeat];
        me.clock = node.view as unknown as TempoClock;
        me.logicalBeat = beat;
        try {
            this.renderOn(node.view, item);
        } finally {
            [me.clock, me.logicalBeat] = saved;
        }
    }

    private renderOn(view: ClockView, item: unknown): void {
        const clock = view as unknown as TempoClock;
        if (item instanceof Routine) {
            // Fresh on every pass: the item may still be sounding from the last
            // one, or on another clock.
            view.play(new Routine(item.func));
            return;
        }
        const destination = this.destination ?? (main.resolveServer() as unknown as PlayDestination);
        if (item instanceof Pattern) {
            item.play(destination, { clock });
            return;
        }
        (item as TimelineItem).play(destination);
    }

    *run(epoch: number, at: number): Generator<number, void, unknown> {
        this.root = new TimelineNode(this, this.timeline, 0, at);
        const me = currentRoutine()!;
        if (me.logicalBeat < at) yield at - me.logicalBeat;
        while (this.running && epoch === this.epoch) {
            const loop = this.loop;
            const due = this.root.nextDue();
            if (loop !== null && (due === null || due[0] >= this.rootSecs(loop[1]))) {
                const wait = loop[1] - me.logicalBeat;
                if (wait > 0) {
                    yield wait;
                    if (!(this.running && epoch === this.epoch)) return;
                }
                this.release(null);
                this.clock!.locate(loop[0]);
                this.root = new TimelineNode(this, this.timeline, 0, loop[0]);
                continue;
            }
            if (due === null) {
                this.held = me.logicalBeat;
                this.running = false;
                this.finished = true;
                return;
            }
            const beat = due[2] ?? this.rootBeat(due[0]);
            const wait = beat - me.logicalBeat;
            if (wait > 0) {
                yield wait;
                if (!(this.running && epoch === this.epoch)) return;
            }
            due[1]();
            this.root.prune();
        }
    }
}

/**
 * A transport over a `Timeline`: play / stop / locate / loop, and a song
 * `position`.
 *
 * The playhead scans the timeline forward as a clock advances, rendering each
 * item on a destination. The forward scan is what `play` runs; the random
 * access lives at the boundaries — `play({ at })` and `locate(beat)` re-seek
 * the cursor by time, which a forward-only routine could never do.
 *
 * Timing rides the clock's logical time like everything else, so a playhead
 * inherits `quant` and a sample-exact timebase for free.
 *
 * A pass ends on its own when the scan reaches the end of the timeline:
 * `playing` goes false and `finished` says the end is why, so a transport
 * reads the end off the playhead instead of timing it.
 */
export class Playhead {
    readonly timeline: Timeline;
    readonly clock: TempoClock;
    readonly destination: PlayDestination;

    private running = false;
    private ended = false;
    private epoch = 0;
    private routine: Routine | null = null;
    private loopWindow: [number, number] | null = null;
    private startBeat = 0;
    private posBeat = 0;
    private posClock: number | null = null;
    /** The responder obeying a server's transport, while following one. */
    private following: OscFunc | null = null;

    constructor(timeline: Timeline, clock: TempoClock, destination: PlayDestination) {
        this.timeline = timeline;
        this.clock = clock;
        this.destination = destination;
    }

    // ---- transport ----

    /**
     * Starts (or restarts) playback from beat `at`, snapping the start to a
     * `quant` boundary of the clock's grid. Re-seeks the cursor to `at`, so it
     * doubles as a locate-and-play.
     */
    play({ at = 0, quant }: { at?: number; quant?: number } = {}): this {
        this.startBeat = at;
        this.posBeat = at;
        this.posClock = null;
        this.running = true;
        this.ended = false;
        this.epoch += 1;
        const epoch = this.epoch;
        if (this.routine !== null) this.clock.unsched(this.routine);
        this.routine = new Routine(() => this.feed(epoch));
        this.clock.play(this.routine, quant);
        return this;
    }

    /**
     * Halts the playhead. Items already rendered keep sounding (their releases
     * are scheduled); no further items are played.
     */
    stop(): this {
        this.posBeat = this.position();
        this.running = false;
        this.ended = false; // halted by hand, not ended
        this.posClock = null;
        this.epoch += 1;
        if (this.routine !== null) {
            this.clock.unsched(this.routine);
            this.routine = null;
        }
        return this;
    }

    /**
     * Seeks to `beat`. While playing, restarts the scan from there (random
     * access); while stopped, sets where the next `play` begins.
     */
    locate(beat: number): this {
        if (this.running) {
            this.play({ at: beat });
        } else {
            this.startBeat = beat;
            this.posBeat = beat;
            this.ended = false; // seeking away from the end leaves it behind
        }
        return this;
    }

    /**
     * Loops the half-open beat window `[start, end)`: when the scan reaches
     * `end` it wraps back to `start`. Set before or during play.
     */
    loop(start: number, end: number): this {
        this.loopWindow = [start, end];
        return this;
    }

    /** Stops looping; the scan plays through to the end. */
    unloop(): this {
        this.loopWindow = null;
        return this;
    }

    // ---- following the server's shared transport ----

    /**
     * Makes this playhead obey a `server`'s shared transport: when a conductor
     * calls `transportPlay` / `transportStop` / `transportLocate`, the server
     * broadcasts the new state and this playhead rolls / halts / seeks to
     * match — so several clients run in lockstep on one grid.
     *
     * It registers for the server's pushes (`notify`, which `boot`/`attach`
     * already does) and subscribes to the `/transport_query.reply` broadcasts
     * through `server.onReply`, then applies the current state once. `quant`
     * snaps each rolling start to a beat boundary, so every follower lands
     * together — with the clock joined to the same grid
     * (`TempoClock.joinTransport`) that boundary is the *shared* bar line.
     * Release with `unfollowTransport`.
     *
     * Beat-aligned in plain wall-clock mode; sample-exact when the clock is
     * also locked to the server (`Session.lockToServer`).
     *
     * `tempoMap`: the piece's {@link TempoMap}, for a piece whose tempo changes
     * along the way. The shared grid is a contract between clients and can only
     * state **one** tempo (`/transport_set` is an origin and a scalar), so the
     * beat position the server broadcasts is a reading of that nominal grid, not
     * of this piece. Given a map, the position is taken from the transport's
     * **sample** spelling and converted here — the same seam an editor drives
     * the transport through — and the broadcast beat is ignored. It needs
     * `sampleRate` (the engine's) to read that axis; without either, nothing
     * changes.
     *
     * The responder is an `OscFunc` on the server's receiver, as in the
     * reference client — with the receiver the page already has (the server's
     * connection) rather than a socket opened for the purpose, which a browser
     * has no way to bind.
     */
    async followTransport(
        server: Server,
        {
            quant,
            timeout,
            tempoMap,
            sampleRate = 0,
        }: {
            quant?: number;
            timeout?: number;
            tempoMap?: TempoMap | null;
            sampleRate?: number;
        } = {},
    ): Promise<this> {
        this.unfollowTransport();
        await server.notify(true, timeout);
        const rate = Number(sampleRate) || 0;
        /**
         * The song position as a beat **of this piece**.
         *
         * The broadcast field (index 5) reads the shared grid, which is one
         * tempo by construction. When the piece has a map, the truthful spelling
         * is the sample position (index 7) put through it; the two agree exactly
         * whenever the piece is affine.
         */
        const beatOf = (msg: ResponderMessage): number =>
            tempoMap && rate > 0 && msg.length >= 8
                ? tempoMap.beatsAt(Number(msg[7]) / rate)
                : Number(msg[5]);
        this.following = new OscFunc(
            (msg) => {
                // /transport_query.reply originSample tempo defined playing
                // position group transportSample positionSample ...
                if (msg.length < 7 || !Number(msg[3])) return;
                const position = beatOf(msg);
                if (Number(msg[4])) {
                    this.play({ at: position, quant });
                } else {
                    this.stop();
                    this.locate(position);
                }
            },
            "/transport_query.reply",
            { recv: server.receiver },
        );
        const state = await server.transportState(timeout);
        // Gated on the **grid**, not on the state: the state is always there
        // now, but a playhead runs on beats, and `position` is 0 until a grid
        // says what a beat is. Applying that would locate to 0 on a server
        // whose transport is being driven in samples.
        if (state.tempo !== null) {
            const at =
                tempoMap && rate > 0
                    ? tempoMap.beatsAt(Number(state.positionSample) / rate)
                    : state.position;
            if (state.playing) this.play({ at, quant });
            else this.locate(at);
        }
        return this;
    }

    /**
     * Stops following a server transport (see `followTransport`): drops the
     * subscription, leaving the playhead wherever the last broadcast left it.
     */
    unfollowTransport(): this {
        this.following?.free();
        this.following = null;
        return this;
    }

    /**
     * The current song position, in beats. Interpolated from the clock between
     * items while playing; the start or last-seek beat while stopped.
     */
    /**
     * The **clock** beat the scan last woke on — the origin `position`
     * interpolates from, and, once the scan has drained, the beat at which its
     * last item was rendered. `null` before the first wake.
     *
     * A transport reads it to keep a cursor moving after the scan is over: the
     * piece ends where the last item does, which is a stretch of time later.
     */
    get scannedAt(): number | null {
        return this.posClock;
    }

    position(): number {
        if (!this.running || this.posClock === null) return this.posBeat;
        let pos = this.posBeat + (this.clock.beats() - this.posClock);
        if (this.loopWindow !== null) {
            const [start, end] = this.loopWindow;
            const span = end - start;
            if (span > 0 && pos >= end) pos = start + ((pos - start) % span);
        }
        return pos;
    }

    /**
     * Whether the scan is running. It goes false on `stop` **and** when the
     * scan reaches the end of the timeline, so a transport polls this one flag
     * instead of comparing `position` against a length of its own.
     */
    get playing(): boolean {
        return this.running;
    }

    /**
     * Whether the scan ran off the end, as opposed to being halted by hand or
     * still playing. It is the *scan* that ended: a loop never ends, and the
     * last item keeps sounding for its own length — the playhead schedules
     * items, it does not wait for them.
     */
    get finished(): boolean {
        return this.ended;
    }

    // ---- the feeder: a cursor walk fed to the clock ----

    private *feed(epoch: number): Generator<number | undefined, void, unknown> {
        const tl = this.timeline;
        let cursor = tl.indexAt(this.startBeat);
        let prev = this.startBeat;
        while (this.running && epoch === this.epoch) {
            this.posBeat = prev;
            this.posClock = this.clock.beats();
            if (this.loopWindow !== null) {
                const [start, end] = this.loopWindow;
                const next = tl.get(cursor);
                if (next === undefined || next[0] >= end) {
                    const tail = end - prev;
                    if (tail > 0) yield tail;
                    cursor = tl.indexAt(start);
                    prev = start;
                    continue;
                }
            }
            const entry = tl.get(cursor);
            if (entry === undefined) {
                // Drained: the pass is over, and the transport driving it has
                // to know without polling a length of its own. The feeder runs
                // on the clock, so it records the end rather than announcing
                // it — `playing` goes false, `position` freezes on the last
                // item.
                this.running = false;
                this.ended = true;
                return;
            }
            const [beat, item] = entry;
            const wait = beat - prev;
            if (wait > 0) {
                yield wait;
                if (!(this.running && epoch === this.epoch)) return;
                prev = beat;
                this.posBeat = prev;
                this.posClock = this.clock.beats();
            }
            (item as { play(destination: PlayDestination): unknown }).play(
                this.destination,
            );
            cursor += 1;
        }
    }
}
