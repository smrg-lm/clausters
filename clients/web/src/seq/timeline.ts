// Timelines: a plan in logical time, played by itself (mirrors
// `clausters/seq/timeline.py`).
//
// The counterpart to the generative layer (`Routine`, `Pbind`). A routine is a
// forward-only generator: its musical state lives in the generator's locals,
// so it cannot be *seeked*. A `Timeline` is the opposite -- an **editable list
// of timed items kept sorted by beat**, with its own tempo map and random
// access by time (`indexAt`, `range`). That is what makes DAW-style transport
// controls possible, and they are the timeline's own: play / pause / stop /
// locate / loop, on a clock of its own or on a server's transport
// (`Timeline.transport`).
//
// An *item* is anything that can render itself on a destination -- it has a
// `play(destination)` method. `Event` already is one, so a timeline of events
// renders to whatever destination the timeline plays on, exactly like the rest
// of the client. `OscItem` wraps a raw OSC message, so a timeline can also be a
// plain, editable OSC score.
//
// This layer is **client-side** while a timeline plays on its own clock: each
// has its own local transport, and several clients phase-align through `quant`.
// On a **server transport** (`Timeline.transport`) the verbs are the
// transport's own commands and the plan rides the transport's clock, so one
// conductor's play/stop/locate drives every timeline on it.

import { TempoClock } from "../base/clock.ts";
import type { Schedulable } from "../base/clock.ts";
import { TempoMap } from "../base/time.ts";
import { quantDelay } from "../base/timebase.ts";
import { currentRoutine, setCurrentRoutine } from "../base/context.ts";
import { main } from "../base/main.ts";
import { Routine, StopStream, Stream } from "../base/stream.ts";
import { Event } from "./event.ts";
import type { EventDestination } from "./event.ts";
import { EventPattern, Pattern } from "./pattern.ts";
import type { Server, TimedMessage } from "../defs/server/index.ts";
import type { MsgArg } from "../base/osc.ts";
import { OscFunc } from "../responders.ts";
import type { ResponderMessage } from "../responders.ts";

/** What a timeline can hold: anything that renders itself on a destination. */
export interface TimelineItem {
    play(destination: PlayDestination): unknown;
}

/** The destination a timeline renders on. `Server` satisfies it. */
export interface PlayDestination extends EventDestination {
    sendBundle(
        messages: readonly TimedMessage[],
        options?: { delayBeats?: number; clock?: TempoClock },
    ): void;
    /**
     * Raw MIDI at the timeline's beat, on a destination that carries MIDI
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
 * timeline's current logical beat.
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
 * timeline's current logical beat through a `MidiServer`.
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
 * raw MIDI bytes. An `Event` carries neither -- it is its own parameters -- so
 * what an item *is* is told apart by which of the two keys is there, and by
 * neither being there.
 */
export const OSC_KEY = "osc";
/** @see {@link OSC_KEY} */
export const MIDI_KEY = "midi";

/**
 * One timeline item as plain, JSON-able data -- or `null` for an item this has no
 * description of.
 *
 * **One description, because two seams need it.** A document writes a timeline's
 * items as the configuration of a placed clang, and the editing domain hands
 * them across the crate's `events` vocabulary as an event's opaque `data`; the
 * two are the same question -- *what is this item, written down* -- and answering
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
 * an event pattern, a `Routine` -- and **another timeline**, which its
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
    private transportHeld: Server | null = null;
    /**
     * **Where this timeline's beat 0 falls on the transport**, in seconds of
     * the transport's position -- the transport's axis is physical, so the
     * offset is too. Only read in transport mode.
     */
    transportAt = 0;
    private readonly tempo: number;
    /** @internal */
    player: TimelinePlayer | TransportPlayer | null = null;

    constructor(
        items?: Iterable<readonly [number, unknown]>,
        { tempo = 1.0, tempoMap }: { tempo?: number; tempoMap?: TempoMap } = {},
    ) {
        this.mapHeld = tempoMap ?? null;
        this.tempo = tempo;
        if (items) for (const [beat, item] of items) this.add(beat, item);
    }

    /**
     * The timeline's tempo map: how its beats fall on seconds. Editable data --
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
     * ancestors. A value pattern is refused: it is the definition of a
     * generator and does not play -- an `EventPattern` does.
     */
    add(beat: number, item: unknown): Entry {
        if (item instanceof Pattern && !(item instanceof EventPattern)) {
            throw new TypeError(
                `a ${item.constructor.name} of values does not play, so it is not a timeline `
                + "item: a pattern plays when its values are events (a Pbind, or a list "
                + "pattern of event patterns only)",
            );
        }
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
     * empty in between, which nothing here can observe -- a page has one thread --
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
                    "plays in one place -- add item.copy() instead",
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
     * children copied (recursively), and the other items shared -- an event or a
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
     * The cursor (index) of the first item at or after `beat` -- the seek
     * primitive `play({ at })` and `locate` start from.
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
     * The server whose **transport** plays this timeline, or `null` -- the
     * ordinary case -- for its own clock.
     *
     * One mode per root, and the same verbs in both: `play`, `pause`, `stop`
     * and `locate` are the transport's own commands here, exactly as the
     * multitrack's playback uses them, and the timeline's items are planned
     * onto the transport's clock (`/sched_atTransport`) from the position it is
     * at. Setting it needs a **governed group** bound (`Server.transportGroup`),
     * since that is what makes a transport own the nodes it plays -- and what
     * the timeline's synths are placed under, so a pause freezes them with the
     * transport.
     *
     * Assigning halts whatever was playing: a timeline plays in one place. It
     * also starts listening to the transport's broadcasts, because the mode
     * **is** the following: a conductor's roll, freeze and locate drive this
     * timeline from then on.
     */
    get transport(): Server | null {
        return this.transportHeld;
    }

    set transport(server: Server | null) {
        if (this.player !== null) {
            this.player.halt();
            this.player.close();
            this.player = null;
        }
        this.transportHeld = server;
        if (server !== null) (this.playerFor() as TransportPlayer).begin();
    }

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
            player.resume(quant);
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

    /**
     * Asks where the transport is and keeps the answer; the promise settles
     * once every verb already asked for has been sent.
     *
     * Only a timeline **on a transport** has anything to ask: on its own clock
     * the position is here. (The reference client blocks instead of answering a
     * promise, for the reason every request there does.)
     */
    async refresh(): Promise<this> {
        if (this.player !== null) await this.player.refresh();
        return this;
    }

    private playerFor(): TimelinePlayer | TransportPlayer {
        this.player ??= this.transportHeld === null
            ? new TimelinePlayer(this)
            : new TransportPlayer(this);
        return this.player;
    }
}

// ---- the engine: one tree, woken by its root's clock ----

/**
 * A timeline's clock as what it plays sees it, while the root's clock wakes the
 * tree.
 *
 * An item of a child measures in the **child's** beats -- an event's sustain, an
 * a routine's yields, a pattern's durations -- but only the
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

    private get root(): TempoClock | null {
        return this.node.player.clock;
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
        return this.localBeat(this.root!.beats());
    }

    /**
     * What a `Server` stamps a bundle with when the timeline plays on a
     * **server transport**: seconds of this node's axis -> a sample of the
     * transport's clock. `null` on a timeline playing on its own clock, where
     * the ordinary timetag or `/sched_at` path applies.
     */
    get schedAxis(): ((secs: number) => number) | null {
        return this.node.player.schedAxis;
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
            this.root?.unsched(wrapper);
        }
        return this;
    }

    private schedule(beat: number, item: Schedulable): void {
        const player = this.node.player;
        const wrapper = new Routine(translated(item, this));
        player.wrappers.set(item, wrapper);
        player.owned.push([this.node, wrapper]);
        this.root!.schedAbs(this.rootBeat(beat), wrapper);
    }

    // What a Server and a session read, from the root clock -- `null` where
    // there is no clock behind the view: a timeline on a server transport has
    // none, and what it needs instead is the axis above.
    get timebase() { return this.root?.timebase ?? null; }
    get pacingOrigin() { return this.root?.pacingOrigin ?? null; }
    get startTime() { return this.root?.startTime ?? null; }
    get session() { return this.root?.session ?? null; }
    get name() { return this.root?.name ?? null; }
    get rolling() { return this.root?.rolling ?? false; }
    get frozen() { return this.root?.frozen ?? false; }
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
 * What a tree of nodes is driven by: the two players (a clock's and a
 * transport's) answer the same handful of questions, which is what keeps
 * nesting, the entry rule and the units in one implementation.
 */
interface TreeDriver {
    readonly timeline: Timeline;
    destination: PlayDestination | null;
    readonly clock: TempoClock | null;
    readonly schedAxis: ((secs: number) => number) | null;
    rootSecs(beat: number): number;
    rootBeat(secs: number): number;
    render(node: TimelineNode, beat: number, item: unknown): void;
    release(node: TimelineNode | null): void;
    readonly wrappers: Map<Schedulable, Routine>;
    owned: [TimelineNode, Routine][];
}

/**
 * One timeline of a playing tree: where its beat 0 falls on the root's axis of
 * seconds, its cursor, and the children it has entered.
 */
class TimelineNode {
    readonly player: TreeDriver;
    readonly timeline: Timeline;
    origin: number;
    readonly isRoot: boolean;
    readonly view: ClockView;
    cursor = 0;
    children: TimelineNode[] = [];

    constructor(player: TreeDriver, timeline: Timeline, origin: number, beat: number) {
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
 * A timeline played on a **server transport**: the same verbs, carried out as
 * the transport's own commands, and the tree planned onto the transport's clock
 * instead of woken on a clock of its own.
 *
 * The transport is state in physical time -- frozen nodes, a locate and a loop
 * exact in the engine -- and a timeline is a plan of discrete events in logical
 * time. So nothing here drives time: `play`, `pause`, `stop` and `locate` are
 * `/transport_play`, `/transport_stop` and `/transport_locateSample`, and what
 * this adds is the **plan**: every item from a position, stamped on the
 * transport's clock through the timeline's own map, so a pause holds the queue
 * with the sound. A locate clears the transport queue
 * (`schedClear("transport")`) and re-plans from the new position, `latency`
 * ahead so nothing regenerated is late.
 *
 * **Its verbs are queued, not awaited.** The transport's commands are requests,
 * and a page waits for an answer instead of blocking on one, so each verb goes
 * onto one chain in the order it was called and `refresh` is what settles with
 * it. The reference client, whose requests block, simply sends them.
 *
 * @internal
 */
export class TransportPlayer implements TreeDriver {
    readonly timeline: Timeline;
    destination: PlayDestination | null = null;
    mark = 0;
    finished = false;
    root: TimelineNode | null = null;
    owned: [TimelineNode, Routine][] = [];
    readonly wrappers = new Map<Schedulable, Routine>();
    readonly clock = null;
    /**
     * The transport as this client last heard it -- from a verb it sent, a
     * broadcast, or `refresh`. Read rather than asked for, the way
     * `PlayheadSync` reads the transport: asking is a round trip and reading a
     * position is not.
     */
    private reported: { playing: boolean; positionSample: number } =
        { playing: false, positionSample: 0 };
    /**
     * `[baseSample, baseSecs, rate]` while a plan is being written: what turns
     * a node's seconds into a sample of the transport's clock.
     */
    private stamp: [number, number, number] | null = null;
    private held = 0;
    private rate = 0;
    /**
     * The position sample this client itself cued, so its own locate's
     * broadcast is not read as somebody else's.
     */
    private cued: number | null = null;
    private following: OscFunc | null = null;
    private chain: Promise<unknown> = Promise.resolve();

    /**
     * The server whose transport this plays on, held rather than read from the
     * timeline: leaving the mode stops the transport, and the verb that stops
     * it is queued behind whatever was still in flight.
     */
    private readonly server: Server;

    constructor(timeline: Timeline) {
        this.timeline = timeline;
        this.server = timeline.transport!;
    }

    // the root's axis, as the clock player's
    rootSecs(beat: number): number {
        return this.timeline.map.secsAt(beat);
    }

    rootBeat(secs: number): number {
        return this.timeline.map.beatsAt(secs);
    }

    get loop(): [number, number] | null {
        return null;
    }

    set loop(_span: [number, number] | null) {
        throw new Error(
            "a loop on a transport is the engine's, and a timeline's events would "
                + "have to be re-cued on every wrap: loop it on its own clock "
                + "(timeline.transport = null) or loop the transport itself",
        );
    }

    get schedAxis(): ((secs: number) => number) | null {
        if (this.stamp === null) return null;
        const [baseSample, baseSecs, rate] = this.stamp;
        return (secs: number) => Math.round(baseSample + (secs - baseSecs) * rate);
    }

    get running(): boolean {
        return this.reported.playing;
    }

    /**
     * Where the transport is, in this timeline's beats, as this client last heard
     * it -- a wrap inside the transport's loop and a locate some other client
     * sent are both where it says, since both are broadcast. `refresh` asks
     * again.
     */
    position(): number {
        return this.rootBeat(Math.max(this.timelineSecs(this.reported.positionSample), 0));
    }

    private timelineSecs(positionSample: number): number {
        const rate = this.rate || 48_000;
        return positionSample / rate - this.timeline.transportAt;
    }

    /** Asks the server where the transport is, and keeps it. */
    async refresh(): Promise<void> {
        await this.chain;
        const state = await this.server.transportState();
        this.reported = {
            playing: state.playing,
            positionSample: Number(state.positionSample),
        };
    }

    private queue(work: () => Promise<void>): void {
        this.chain = this.chain.then(work);
        // A step nobody is waiting for must not become an **unhandled
        // rejection**: most of these are queued by a verb whose caller will
        // `refresh`, but a re-cue is queued by a *broadcast* -- a responder
        // callback with no caller at all -- and the one that lands while the
        // server is closing fails with every request it had in flight. The
        // chain keeps the failure for the next `refresh`, which is where a
        // refusal is meant to reach the caller; this handler only says it was
        // observed, so a page's console and a test run stay quiet.
        this.chain.catch(() => {});
    }

    private async rateOf(): Promise<number> {
        this.rate ||= Number((await this.server.queryInfo()).nominalSampleRate);
        return this.rate;
    }

    resume(quant?: number): void {
        refuseQuant(quant);
        this.reported = { ...this.reported, playing: true };
        this.queue(async () => {
            // Nothing is re-planned: a pause froze the transport's queue with
            // the transport, so what was queued is still queued in its exact
            // relative place -- the whole difference between a resume and a play.
            await this.server.transportPlay();
        });
    }

    play(at: number, quant?: number): void {
        refuseQuant(quant);
        this.reported = { ...this.reported, playing: true };
        this.locate(at, false);
        this.queue(async () => {
            await this.server.transportPlay();
            await this.plan(at);
        });
    }

    locate(beat: number, cue = true): void {
        this.held = beat;
        this.queue(async () => {
            const rate = await this.rateOf();
            const sample = Math.round((this.timeline.transportAt + this.rootSecs(beat)) * rate);
            this.server.schedClear("transport");
            await this.server.transportLocateSample(sample);
            this.reported = { ...this.reported, positionSample: sample };
            this.cued = sample;
            if (cue && this.reported.playing) await this.plan(beat);
        });
    }

    halt(): void {
        this.held = this.position();
        this.reported = { ...this.reported, playing: false };
        this.queue(async () => {
            await this.server.transportStop();
        });
    }

    hold(beat: number): void {
        this.locate(beat);
    }

    /**
     * Nothing is owned here: a plan holds no routines, and what is queued is
     * the transport's (cleared by a locate).
     */
    release(_node: TimelineNode | null): void {
        this.owned = [];
    }

    render(node: TimelineNode, beat: number, item: unknown): void {
        if (item instanceof Routine || item instanceof Pattern) {
            throw new Error(
                `${(item as object).constructor.name} at beat ${beat}: a routine or a `
                    + "pattern cannot be planned from a position",
            );
        }
        const stub = { clock: node.view as unknown as TempoClock, logicalBeat: beat };
        const previous = setCurrentRoutine(stub as unknown as Stream);
        try {
            // The transport's own server when nothing else was named: a plan a
            // broadcast writes runs where no session is ambient, and the server
            // whose transport this is is the one place the items can be meant for.
            const destination = this.destination
                ?? (this.server as unknown as PlayDestination);
            (item as TimelineItem).play(destination);
        } finally {
            setCurrentRoutine(previous);
        }
    }

    /**
     * Writes the whole tree from `at` onto the transport's clock.
     *
     * The walk is the clock player's -- the same nodes, the same entry rule, the
     * same units -- with the waiting taken out: there is no time to pass here,
     * since every item names a sample of a clock the engine is running.
     */
    private async plan(at: number): Promise<void> {
        const rate = await this.rateOf();
        const state = await this.server.transportState();
        this.stamp = [Number(state.transportSample), this.rootSecs(at), rate];
        const stub = { clock: null as unknown, logicalBeat: 0 };
        const previous = setCurrentRoutine(stub as unknown as Stream);
        try {
            this.root = new TimelineNode(this, this.timeline, 0, at);
            this.finished = false;
            for (;;) {
                const due = this.root.nextDue();
                if (due === null) break;
                stub.logicalBeat = due[2] ?? this.rootBeat(due[0]);
                due[1]();
                this.root.prune();
            }
        } finally {
            setCurrentRoutine(previous);
            this.stamp = null;
        }
    }

    // ---- following whoever drives the transport ----

    /**
     * Listens to the transport's broadcasts, so a **conductor** drives this
     * timeline too: whoever calls the transport's verbs -- this client, a second
     * one, the multitrack editor next door -- makes it roll, freeze and re-plan.
     *
     * That is the whole of following now: the plan rides the transport's own
     * clock, so a roll and a freeze need nothing from here; what a locate needs
     * is the re-cue, and this is where a locate somebody else sent arrives.
     */
    /**
     * Takes up the transport: the group check and the broadcast listener, both
     * queued, so a `refresh` settles with them and a refusal reaches the caller
     * rather than the console.
     */
    begin(): void {
        this.queue(async () => {
            await this.follow();
        });
    }

    async follow(): Promise<this> {
        if (this.following !== null) return this;
        const state = await this.server.transportState();
        if (state.group === null) {
            throw new Error(
                "a timeline on a transport needs a governed group: bind one with "
                    + "server.transportGroup(group) -- the transport owns the nodes it plays",
            );
        }
        await this.server.notify(true);
        this.following = new OscFunc(
            (msg) => this.broadcast(msg),
            "/transport_query.reply",
            { recv: this.server.receiver },
        );
        return this;
    }

    /** Gives up the broadcast listener, if any. */
    close(): void {
        this.following?.free();
        this.following = null;
    }

    private broadcast(msg: ResponderMessage): void {
        // /transport_query.reply originSample tempo defined playing position
        // group transportSample positionSample ...
        if (msg.length < 9) return;
        const playing = Boolean(Number(msg[4]));
        const position = Number(msg[8]);
        this.reported = { playing, positionSample: position };
        if (!playing) return;
        const rate = this.rate || 48_000;
        // What this client cued itself is not news: its own locate broadcasts
        // too, and re-planning on the echo would write the plan twice.
        if (this.cued !== null && Math.abs(position - this.cued) < 0.05 * rate) return;
        this.cued = position;
        // Somebody else drove it -- a conductor's play or locate, a loop's wrap.
        // The engine does not clear the queue on a locate (a client's own does),
        // so the re-cue is the pair: clear what was queued for where we were,
        // and plan again from where it says.
        this.queue(async () => {
            this.server.schedClear("transport");
            await this.plan(this.rootBeat(Math.max(this.timelineSecs(position), 0)));
        });
    }
}

function refuseQuant(quant?: number): void {
    if (quant) {
        throw new Error(
            "quant is the client clock's: on a transport the start is the transport's "
                + "own, so locate where you want it and roll",
        );
    }
}

/**
 * The engine of a timeline played as a root: its hidden clock, born on the
 * timeline's beat 0, and the routine that wakes the whole tree on it.
 *
 * @internal
 */
export class TimelinePlayer implements TreeDriver {
    clock: TempoClock | null = null;
    /**
     * A timeline on its own clock stamps on the ordinary axis (a timetag, or
     * `/sched_at` under a sample timebase): there is no transport to name.
     */
    readonly schedAxis = null;
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

    /** Plays on from where `halt` left it: on a clock, a play from the held beat. */
    resume(quant?: number): void {
        this.play(this.position(), quant);
    }

    /** Nothing to ask: on its own clock the position is here. */
    refresh(): Promise<void> {
        return Promise.resolve();
    }

    /** Nothing to give up: a timeline on its own clock listens to nothing. */
    close(): void {}

    rootSecs(beat: number): number {
        return this.timeline.map.secsAt(beat);
    }

    rootBeat(secs: number): number {
        return this.timeline.map.beatsAt(secs);
    }

    /**
     * The hidden clock, which belongs to the session the timeline sounds in:
     * made there, on that session's timebase, the first time it plays in it --
     * and made again, at the position it stopped at, when it plays in another.
     * A timeline sounding in one session is refused in another.
     */
    private clockFor(): TempoClock {
        const session = main.ambientSession();
        if (this.clock !== null && this.clock.session !== session) {
            if (this.running) {
                throw new Error(
                    "this timeline is sounding in another session; stop it there before "
                    + "playing it in this one",
                );
            }
            const held = this.position();
            this.clock.stop();
            const owner = this.clock.session as { clock?: TempoClock; release?(c: TempoClock): unknown } | null;
            if (owner?.release && owner.clock !== this.clock) owner.release(this.clock);
            this.clock = null;
            this.held = held;
        }
        if (this.clock === null) {
            this.clock = new TempoClock(1, { tempoMap: this.timeline.map });
            this.clock.locate(this.held);
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
        if (item instanceof EventPattern) {
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
