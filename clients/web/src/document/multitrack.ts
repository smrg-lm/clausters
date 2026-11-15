/**
 * The multitrack: tracks, take lanes, regions, automation and markers.
 *
 * Private: the names are `clausters/document`'s.
 *
 * @module
 */

import {
    JsMultitrack,
    editingDefaultTempo,
} from "../core/clausters_core_web.js";
import { TempoMap } from "../base/time.ts";
import { quads } from "../defs/ugens/env.ts";
import type { Curve as CurveSpec, PointsLike } from "../defs/ugens/env.ts";
import { resolveServer } from "../defs/wire.ts";
import { FIRST_VERSION, } from "./format.ts";
import { requireCore } from "../base/core.ts";
import { UndoHistory, contexts, keyOf } from "./history.ts";

import { type Extra, num, unknownFields } from "./format.ts";
import { Meter, Span, Tempo } from "./timeline.ts";


/**
 * **The window a multitrack is open in**, as the multitrack sees it: the
 * multitrack editor, which a change made through the multitrack's objects
 * is forwarded to.
 *
 * @internal
 */
export interface WindowOver {
    /** Whether the window is gone. */
    readonly closed: boolean;
    /** The window's history. */
    readonly history: WindowHistory;
    /** A change made here, `current` the edit that puts it back, as an edit of the window's. */
    forward(intent: Extra, label: string, current: unknown): void;
}

/**
 * **The window's history, as a page's multitrack reads it** while the
 * multitrack application has it open: the undo order the window walks,
 * which the page's changes are entries of. Its steps and its reads are
 * promises, since the window's history is in the host's process.
 */
export interface WindowHistory {
    /** Runs `run`, and makes every change inside it **one** entry, labelled `label`. */
    entry<T>(label: string, run: () => T): T;
    /** Steps back over the last entry -- a page's or a window's -- and resolves whether anything moved. */
    undo(): Promise<boolean>;
    /** Steps forward again after `undo`; whether anything moved. */
    redo(): Promise<boolean>;
    /** Whether there is an entry to step back over. */
    canUndo(): Promise<boolean>;
    /** Whether there is an undone entry to step forward into. */
    canRedo(): Promise<boolean>;
    /** What an undo would take back, as a menu names it. */
    undoLabel(): Promise<string | null>;
    /** What a redo would bring back. */
    redoLabel(): Promise<string | null>;
}

/**
 * **A region's fade**: how long it lasts, in seconds, and the shape of the one
 * envelope segment it is.
 *
 * A region's two fades make a trapezoid over its box -- from silence to full
 * level, full level, back to silence -- and each sloped side is a segment like
 * any other envelope's: `shape` is one of the names an `Env` segment takes
 * (`"lin"`, `"exp"`, `"sin"`, `"wel"`, `"curve"` ...) and `curve` the
 * curvature for `"curve"`. The default is `"wel"`, **equal power**: quick at
 * first and slow at the end of a rise, so a fade out against a fade in of the
 * same length keeps the summed power constant.
 */
export class Fade {
    length: number;
    shape: string;
    curve: number;

    constructor(length: number, shape = "wel", curve = 0.0) {
        this.length = length;
        this.shape = shape;
        this.curve = curve;
    }

    write(): Extra {
        const out: Extra = { length: this.length };
        if (this.shape !== "wel") out.shape = this.shape;
        if (this.curve !== 0) out.curve = this.curve;
        return out;
    }

    static read(written: Extra): Fade {
        const shape = typeof written.shape === "string" ? written.shape : "wel";
        return new Fade(num(written.length), shape, written.curve === undefined ? 0 : num(written.curve));
    }
}

/**
 * What fills a region: a **window** onto a source.
 *
 * Not the clipboard's content, which is what was *copied*. Two nouns in two
 * modules, each the right word where it stands: this one is a region's
 * `content` field, and the format tags it `fill`.
 *
 * One class with a `fill` saying which shape it is, because that is how the
 * format writes it. A window is the one fill this build knows; any other is
 * carried whole and written back as it came, so a session a newer writer saved
 * is not lost by passing through here.
 *
 * A window carries its `playrate` (a property of *this* placement: two regions
 * over one source may play it at two rates) and the `args` of **this**
 * evaluation, for a window onto something generated -- a function placed twice
 * is two evaluations, possibly with different arguments, and the document
 * carries them without reading them.
 */
export class Content {
    fill: string;
    window?: Extra;
    playrate: number;
    args?: unknown;
    other?: Extra;
    /**
     * Whether the window **wraps**: past the end of the source it begins again,
     * and before the beginning it shows the source's own tail. What a box longer
     * than what it reads means -- the alternative being that it simply stops,
     * which is what a box that does not loop does. A property of *this*
     * placement, like `playrate`: two regions over one recording may loop and
     * not loop.
     */
    looping: boolean;

    constructor(fill: string, fields: Partial<Content> = {}) {
        this.fill = fill;
        this.window = fields.window;
        this.playrate = fields.playrate ?? 1;
        this.args = fields.args;
        this.other = fields.other;
        this.looping = fields.looping ?? false;
    }

    /**
     * A window onto a source: `{ source, start, duration }`, the source being
     * one of the session's table.
     *
     * `duration` is how much of the source the window **reaches** -- the whole
     * take, or the sum of a join's segments -- and not how much the region
     * shows: the region's own `length` says that, and a trim that hides part of
     * the source leaves `duration` alone so the edge can be pulled back.
     */
    static onto(
        window: Extra,
        options: { playrate?: number; args?: unknown; looping?: boolean } = {},
    ): Content {
        return new Content("window", {
            window,
            playrate: options.playrate ?? 1,
            args: options.args,
            looping: options.looping ?? false,
        });
    }

    write(): Extra {
        if (this.fill === "window") {
            const out: Extra = { fill: "window", window: this.window };
            if (this.playrate !== 1) out.playrate = this.playrate;
            if (this.args !== undefined) out.args = this.args;
            if (this.looping) out.loop = true;
            return out;
        }
        // A fill this build does not know, carried whole.
        return { ...(this.other ?? {}) };
    }

    static read(written: Extra): Content {
        if (written.fill === "window") {
            return new Content("window", {
                window: written.window as Extra,
                playrate: num(written.playrate, 1),
                args: written.args,
                looping: written.loop === true,
            });
        }
        return new Content(String(written.fill), { other: { ...written } });
    }
}

/**
 * A break-point list as the **document's** points: `{ at, value, data }`, with
 * the segment's shape in the point's own `data`.
 *
 * The one place the two vocabularies meet in this direction. Reads every
 * spelling {@link quads} does, curve names included, and the crate carries the
 * `data` without ever reading it -- which is what lets a shape survive an undo
 * instead of coming back straight.
 */
export function cratePoints(points: PointsLike, curve?: CurveSpec | readonly CurveSpec[]): Extra[] {
    return quads(points, curve).map(([at, value, shape, curvature]) => ({
        at,
        value,
        data: { shape, curve: curvature },
    }));
}

/**
 * The document's points back as the flat `[t, v, shape, curve, ...]` quads the
 * `bpf` view and a curve both speak -- the other direction.
 *
 * A point that says nothing about its segment is linear, which is what a curve
 * drawn somewhere that has no shapes means.
 */
export function flatPoints(
    points: readonly { at?: unknown; value?: unknown; data?: unknown }[],
): number[] {
    const out: number[] = [];
    for (const point of points) {
        const data = (point.data ?? {}) as { shape?: number; curve?: number };
        out.push(
            Number(point.at ?? 0.0),
            Number(point.value ?? 0.0),
            Math.trunc(Number(data.shape ?? 1)),
            Number(data.curve ?? 0.0),
        );
    }
    return out;
}

/** A point of a curve as a script may spell it: the document's, or `[at, value]`. */
export type PointLike = Extra | readonly [number, number];

/** Points as the document writes them: a pair becomes `{at, value}`. */
function documentPoints(points: Iterable<PointLike>): Extra[] {
    return [...points].map((point) =>
        Array.isArray(point)
            ? { at: Number(point[0]), value: Number(point[1]) }
            : { ...(point as Extra) },
    );
}

/**
 * What holds a curve that is not a free value -- a sequence and the event id
 * or `null`, a multitrack and the track or the region -- which reads a curve
 * for its view, writes it back and removes it.
 *
 * @internal
 */
export interface CurveHolder {
    /** Curve `id` as written, while `scope` holds it; `null` when it does not. */
    writtenCurve(scope: never, id: number): Extra | null;
    /** Writes a curve whole, and answers its id. */
    writeCurve(scope: never, written: Extra, label: string): number;
    /** Removes curve `id` from `scope`. */
    removeCurve(scope: never, id: number): void;
}

/** The fields a curve writes under their own names. */
const CURVE_FIELDS = ["id", "target", "name", "points", "visible", "enabled", "extra"];

/**
 * A curve over one parameter, in its holder's time.
 *
 * `target` says **what this automates** in the client's terms and is never read
 * here -- a control name, a bus, a plugin's parameter index -- the same door a
 * leaf's configuration is, and for the same reason. The points are `{at, value,
 * data}`, the shape the document's points vocabulary carries; `[at, value]`
 * pairs are read as such points. What `at` counts is the holder's: seconds on
 * a {@link Track}, seconds from the region's start on a {@link Region}, beats
 * on an `EventSequence`, beats from the note's start on one of its events.
 *
 * **One class, free or held.** Built by a script, a curve is a **value** that
 * nothing holds. Added to a holder -- a sequence or one of its events
 * (`seq.automation.add`, `event.automation.add`), a track or a region of a
 * multitrack (`track.automation.add`, `region.automation.add`) -- it is a
 * **live view** of the curve the holder keeps: reading a field asks the
 * holder, so it reads what an editor left there, writing one writes the curve
 * back, and the same curve read twice is the same object. A curve its holder
 * no longer keeps -- removed, or undone away -- is **detached**:
 * {@link Automation.held} is `false` and reading it throws, until an undo
 * brings it back.
 */
/** What plays a channel's curve: a `Server`, or what stands where one would. */
interface CurveDestination {
    playAutomation(curve: Automation): number | null;
    curves?: { stop(curve: number): void };
}

export class Automation {
    #holder: [CurveHolder, unknown] | null = null;
    #id: number;
    #value: {
        target?: unknown;
        name?: string;
        points: Extra[];
        visible: boolean;
        enabled: boolean;
        extra: Extra;
    } | null;

    constructor(fields: {
        target?: unknown;
        points?: Iterable<PointLike>;
        name?: string;
        visible?: boolean;
        enabled?: boolean;
        extra?: Extra;
        id?: number;
    } = {}) {
        this.#id = Math.trunc(fields.id ?? 0);
        this.#value = {
            target: fields.target,
            name: fields.name,
            points: documentPoints(fields.points ?? []),
            visible: fields.visible ?? false,
            enabled: fields.enabled ?? true,
            extra: { ...(fields.extra ?? {}) },
        };
    }

    /** The view of curve `id` that `holder` keeps in `scope`. @internal */
    static heldBy(holder: CurveHolder, scope: unknown, id: number): Automation {
        const curve = new Automation({ id });
        curve.bind(holder, scope, id);
        return curve;
    }

    /** Becomes the view of curve `id` the holder now keeps. @internal */
    bind(holder: CurveHolder, scope: unknown, id: number): void {
        this.#holder = [holder, scope];
        this.#id = Math.trunc(id);
        this.#value = null;
    }

    /** @internal */
    get holder(): [CurveHolder, unknown] | null {
        return this.#holder;
    }

    /**
     * The structure that holds the curve -- a sequence, a multitrack -- whose
     * history it shares (`Editing.of`), or `null` for a free value.
     *
     * @internal
     */
    get historyOwner(): object | null {
        return this.#holder === null ? null : this.#holder[0];
    }

    // ---- the fields ----

    #written(): Extra {
        if (this.#holder === null) {
            const value = this.#value!;
            return { id: this.#id, ...value, points: [...value.points] };
        }
        const [holder, scope] = this.#holder;
        const found = holder.writtenCurve(scope as never, this.#id);
        if (found === null) throw new Error("its holder no longer keeps this curve");
        return found;
    }

    /**
     * Writes one field: into the value, or -- for a held curve -- the whole
     * curve back into its holder, which keeps its id.
     */
    #write(name: string, value: unknown): void {
        if (this.#holder === null) {
            (this.#value as Record<string, unknown>)[name] = value;
            return;
        }
        let written: Extra = { ...this.#written() };
        if (name === "extra") {
            written = Object.fromEntries(Object.entries(written).filter(([k]) => CURVE_FIELDS.includes(k)));
            Object.assign(written, value as Extra);
        } else {
            written[name] = value;
        }
        const [holder, scope] = this.#holder;
        holder.writeCurve(scope as never, written, "edit a curve");
    }

    /**
     * Removes the curve from what holds it. This object is left detached, and
     * an undo that brings the curve back brings it back too. A free curve is
     * held by nothing, so there is nothing to remove it from: that throws.
     */
    remove(): void {
        if (this.#holder === null) throw new Error("a free curve is held by nothing");
        this.#written();
        const [holder, scope] = this.#holder;
        holder.removeCurve(scope as never, this.#id);
    }

    /**
     * Whether something holds the curve: `false` for a free value, and for a
     * view whose curve was removed.
     */
    get held(): boolean {
        if (this.#holder === null) return false;
        try {
            this.#written();
            return true;
        } catch {
            return false;
        }
    }

    /** The curve's identity in its holder; `0` for a value nothing holds yet. */
    get id(): number {
        return this.#id;
    }

    set id(value: number) {
        if (this.#holder !== null) throw new Error("a held curve keeps the id its holder gave it");
        this.#id = Math.trunc(value);
    }

    /** What the curve drives, in its holder's vocabulary. */
    get target(): unknown {
        return this.#written().target ?? undefined;
    }

    set target(value: unknown) {
        this.#write("target", value);
    }

    /** What a reader calls the curve. */
    get name(): string | undefined {
        return (this.#written().name as string | null | undefined) ?? undefined;
    }

    set name(value: string | undefined) {
        this.#write("name", value);
    }

    /**
     * The points, `{at, value, data}`, in order. A copy: change a curve by
     * assigning its points, or with {@link Automation.setPoints}.
     */
    get points(): Extra[] {
        return [...((this.#written().points as Extra[] | undefined) ?? [])];
    }

    set points(value: Iterable<PointLike>) {
        this.#write("points", documentPoints(value));
    }

    /**
     * Whether the curve is shown. The **view's**, and kept here because which
     * curves a person had open is part of reopening the work as they left it.
     */
    get visible(): boolean {
        return Boolean(this.#written().visible);
    }

    set visible(value: boolean) {
        this.#write("visible", Boolean(value));
    }

    /**
     * Whether the curve is being applied. A curve can be kept and switched off
     * without being deleted, which is what an arm or a bypass is.
     */
    get enabled(): boolean {
        return this.#written().enabled !== false;
    }

    set enabled(value: boolean) {
        this.#write("enabled", Boolean(value));
    }

    /** Fields a newer writer wrote, carried as they are. */
    get extra(): Extra {
        const written = this.#written();
        if (this.#holder === null) return { ...(written.extra as Extra) };
        return unknownFields(written, ...CURVE_FIELDS);
    }

    set extra(value: Extra) {
        this.#write("extra", { ...value });
    }

    // ---- the curve protocol ----

    /**
     * The curve as the flat `[t, v, shape, curve, ...]` break points the `bpf`
     * view and a `"points"` event speak -- the curve protocol `gui.edit` opens
     * a curve by, shared with `Env` and `Bpf`.
     *
     * A point that says nothing about its segment is linear, which is what a
     * curve drawn somewhere that has no shapes means.
     */
    toPoints(): number[] {
        return flatPoints(this.points);
    }

    /**
     * Take the curve's break points from a break-point list, in place -- the
     * other half of {@link Automation.toPoints}, and what an editor writes back
     * through. Reads every spelling {@link quads} does, curve names included.
     *
     * The shape of each segment travels in the point's own `data`, which the
     * document crate carries and never reads.
     */
    setPoints(points: PointsLike, curve?: CurveSpec | readonly CurveSpec[]): this {
        this.points = cratePoints(points, curve);
        return this;
    }

    // ---- as a channel's curve ----

    #playing: [CurveDestination, number] | null = null;

    /**
     * **Plays it as a channel's curve** on `destination` (the ambient server
     * when omitted): from now on it sets the control its `target` names on
     * the notes of its channel -- `{ control: "amp", channel: 0 }`, or every
     * channel without one -- those sounding and those to come, its points
     * counted in beats from this moment, and it holds its last value past its
     * end (`Server.playAutomation`).
     *
     * The timeline-item protocol, so a curve is placed on a `Timeline` like
     * an event. A note's own curve is not played: it is the event's
     * (`new Event({ automation: [curve] })`). Returns the curve, whose `stop`
     * ends it.
     */
    play(destination?: unknown): this {
        const target = (destination ?? resolveServer()) as Partial<CurveDestination>;
        if (typeof target?.playAutomation !== "function") {
            throw new TypeError(
                "a curve plays on a server, where it sets a control of the notes of its "
                    + `channel; ${(target as object | null)?.constructor?.name ?? typeof target} `
                    + "does not play one",
            );
        }
        if (this.enabled) {
            this.stop();
            const made = target.playAutomation(this);
            this.#playing = made === null ? null : [target as CurveDestination, made];
        }
        return this;
    }

    /**
     * Stops it as a channel's curve: the notes it reached keep the last value
     * it set. Nothing when it is not playing.
     */
    stop(): void {
        const playing = this.#playing;
        this.#playing = null;
        playing?.[0].curves?.stop(playing[1]);
    }

    // ---- as data ----

    write(): Extra {
        const written = this.#written();
        const out: Extra = { id: this.#id };
        if (written.name !== undefined && written.name !== null) out.name = written.name;
        if (written.target !== undefined && written.target !== null) out.target = written.target;
        const points = (written.points as Extra[] | undefined) ?? [];
        if (points.length) out.points = [...points];
        if (written.visible) out.visible = true;
        if (written.enabled === false) out.enabled = false;
        return { ...out, ...this.extra };
    }

    static read(written: Extra): Automation {
        return new Automation({
            id: num(written.id),
            target: written.target,
            name: written.name as string | undefined,
            points: [...((written.points as Extra[]) ?? [])],
            visible: Boolean(written.visible),
            enabled: written.enabled === undefined ? true : Boolean(written.enabled),
            extra: unknownFields(written, "id", "name", "target", "points", "visible", "enabled"),
        });
    }
}

/**
 * **An object that stands for one structure of a multitrack**: the multitrack
 * and the structure's id, never a copy. Reading a field asks the multitrack, so
 * it reads what an editor left there; writing one is an edit in the
 * multitrack's vocabulary. The same structure read twice is the same object
 * (the multitrack keeps an identity map), and one the multitrack no longer
 * holds -- removed, or undone away -- is **detached**: `held` is `false` and
 * reading it throws, until an undo brings it back.
 */
abstract class Held {
    /** @internal */
    readonly owner: Multitrack;
    /** @internal */
    readonly ident: number;

    /** @internal */
    constructor(owner: Multitrack, id: number) {
        this.owner = owner;
        this.ident = Math.trunc(id);
    }

    /** The door's verb that reads one of these, and what it is called. */
    protected abstract get verb(): string;
    protected abstract get noun(): string;

    /** What the door answers for this structure; throws when it is gone. @internal */
    found(): Extra {
        const found = this.owner.call(this.verb, { id: this.ident }) as Extra | null;
        if (found === null) throw new Error(`clausters: the multitrack no longer holds this ${this.noun}`);
        return found;
    }

    /** Whether the multitrack still holds it. */
    get held(): boolean {
        return this.owner.call(this.verb, { id: this.ident }) !== null;
    }

    /** The multitrack it belongs to. */
    get multitrack(): Multitrack {
        return this.owner;
    }

    /** The multitrack, whose history it shares (`Editing.of`). @internal */
    get historyOwner(): object {
        return this.owner;
    }
}

/** The fields a region writes under their own names. */
const REGION_FIELDS = ["id", "position", "length", "content", "name", "layer",
    "fade_in", "fade_out", "muted", "automation"];

/** The fields a track writes under their own names. */
const TRACK_FIELDS = ["id", "name", "take_lanes", "active", "automation", "muted",
    "soloed", "level", "channels", "config"];

/**
 * **One placed thing on a take lane**: a span of the timeline, and what fills
 * it -- a view of a region the multitrack holds, made by `lane.regions.add`.
 *
 * `position` and `length` are the region's own, in seconds. They are **not**
 * the content's: a region may show part of what it holds, and trimming moves
 * these without touching the source. Each field is written through the
 * multitrack's own verb: the position and the layer are where it is placed,
 * the length and the content what it shows, the fades its fades, and the rest
 * a rewrite of its take lane.
 */
export class Region extends Held {
    protected get verb(): string {
        return "region";
    }

    protected get noun(): string {
        return "region";
    }

    #region(): Extra {
        return this.found().region as Extra;
    }

    /** The take lane it sits on. */
    get takeLane(): TakeLane {
        return this.owner.viewOf(TakeLane, num(this.found().takeLane));
    }

    /** The track it belongs to. */
    get track(): Track {
        return this.owner.viewOf(Track, num(this.found().track));
    }

    /** Where it starts on the timeline, in seconds. */
    get position(): number {
        return num(this.#region().position);
    }

    set position(value: number) {
        this.place(undefined, { position: value });
    }

    /** Which of the overlapping regions on its take lane is on top -- higher is nearer the front. */
    get layer(): number {
        return num(this.#region().layer);
    }

    set layer(value: number) {
        this.place(undefined, { layer: value });
    }

    /**
     * **Places it**: on `takeLane` -- of this track or of another -- at
     * `position`, on `layer`, each left as it is when not given. One edit,
     * whatever moved, so it undoes in one step.
     */
    place(takeLane?: TakeLane, { position, layer }: { position?: number; layer?: number } = {}): void {
        const found = this.found();
        const region = found.region as Extra;
        this.owner.editIntent({
            intent: "placeregion",
            region: this.ident,
            track: takeLane === undefined ? num(found.track) : num(takeLane.found().track),
            take_lane: takeLane === undefined ? num(found.takeLane) : takeLane.ident,
            position: position ?? num(region.position),
            layer: Math.trunc(layer ?? num(region.layer)),
        }, "move a region");
    }

    /** How long it occupies, in seconds -- not the content's length. */
    get length(): number {
        return num(this.#region().length);
    }

    set length(value: number) {
        this.trim({ length: value });
    }

    /** What fills it, as a value: change it by assigning one. */
    get content(): Content {
        return Content.read((this.#region().content as Extra) ?? {});
    }

    set content(value: Content) {
        this.trim({ content: value });
    }

    /**
     * **How much of it shows, and from where**: a right-hand trim moves the
     * length, a left-hand one the position, the length and the window into the
     * source -- which is why the content is part of it. What is not given is
     * left as it is.
     */
    trim({ position, length, content }: { position?: number; length?: number; content?: Content } = {}): void {
        const region = this.#region();
        const intent: Extra = {
            intent: "trimregion",
            region: this.ident,
            position: position ?? num(region.position),
            length: length ?? num(region.length),
        };
        if (content !== undefined) intent.content = content.write();
        this.owner.editIntent(intent, "trim a region");
    }

    /** Its fade in, as a value, or `undefined`. */
    get fadeIn(): Fade | undefined {
        const fade = this.#region().fade_in as Extra | undefined;
        return fade ? Fade.read(fade) : undefined;
    }

    set fadeIn(value: Fade | undefined) {
        this.#fades(value, this.fadeOut);
    }

    /** Its fade out, as a value, or `undefined`. */
    get fadeOut(): Fade | undefined {
        const fade = this.#region().fade_out as Extra | undefined;
        return fade ? Fade.read(fade) : undefined;
    }

    set fadeOut(value: Fade | undefined) {
        this.#fades(this.fadeIn, value);
    }

    #fades(fadeIn: Fade | undefined, fadeOut: Fade | undefined): void {
        const intent: Extra = { intent: "faderegion", region: this.ident };
        if (fadeIn !== undefined) intent.fade_in = fadeIn.write();
        if (fadeOut !== undefined) intent.fade_out = fadeOut.write();
        this.owner.editIntent(intent, "fade a region");
    }

    /** What a reader calls it -- a label, never a second identity. */
    get name(): string | undefined {
        return (this.#region().name as string | undefined) ?? undefined;
    }

    set name(value: string | undefined) {
        this.#write("name", value, "rename a region");
    }

    /** Silenced without being removed. The region's own, not its track's. */
    get muted(): boolean {
        return Boolean(this.#region().muted);
    }

    set muted(value: boolean) {
        this.#write("muted", Boolean(value), "mute a region");
    }

    /** Fields a newer writer wrote, carried as they are. */
    get extra(): Extra {
        return unknownFields(this.#region(), ...REGION_FIELDS);
    }

    /**
     * **The curves that act on this placement alone**, as a live collection:
     * drawn inside the region, their points in seconds from its start.
     */
    get automation(): Curves {
        return new Curves(this.owner, ["region", this.ident]);
    }

    /** Where it ends: its position plus its length. */
    get end(): number {
        const region = this.#region();
        return num(region.position) + num(region.length);
    }

    /**
     * Whether the two occupy any of the same time. Half-open, so a region
     * ending exactly where the next begins does not overlap it -- which is what
     * makes a cut into two regions not a crossfade.
     */
    overlaps(other: Region): boolean {
        return this.position < other.end && other.position < this.end;
    }

    /** One field written by rewriting its take lane, which keeps every region's identity. */
    #write(field: string, value: unknown, label: string): void {
        const lane = num(this.found().takeLane);
        const regions = this.owner.laneRegions(lane);
        for (const region of regions) {
            if (num(region.id) !== this.ident) continue;
            if (value === undefined || value === null) delete region[field];
            else region[field] = value;
        }
        this.owner.editIntent({ intent: "settakelane", take_lane: lane, regions }, label);
    }

    /**
     * Takes it off its take lane. This object is left detached, and an undo
     * that brings the region back brings it back too.
     */
    remove(): void {
        const lane = num(this.found().takeLane);
        const regions = this.owner.laneRegions(lane).filter((r) => num(r.id) !== this.ident);
        this.owner.editIntent({ intent: "settakelane", take_lane: lane, regions }, "remove a region");
    }

    /** The region as the crate writes it. */
    write(): Extra {
        return { ...this.#region() };
    }
}

/**
 * **The regions a take lane holds, as a live collection**, in position order:
 * iterate it, index it with `item`, and `add` one.
 */
export class Regions {
    readonly #lane: TakeLane;

    /** @internal */
    constructor(lane: TakeLane) {
        this.#lane = lane;
    }

    #ids(): number[] {
        const ids = this.#lane.owner.idsOf("regions", { takeLane: this.#lane.ident });
        if (ids === null) throw new Error("clausters: the multitrack no longer holds this take lane");
        return ids;
    }

    /** How many regions it holds. */
    get length(): number {
        return this.#ids().length;
    }

    [Symbol.iterator](): IterableIterator<Region> {
        return this.#ids().map((id) => this.#lane.owner.viewOf(Region, id))[Symbol.iterator]();
    }

    /** The region at index `i` in position order (negative counts from the end). */
    item(i: number): Region {
        const id = this.#ids().at(i);
        if (id === undefined) throw new RangeError(`the take lane holds no region at index ${i}`);
        return this.#lane.owner.viewOf(Region, id);
    }

    /**
     * **Places a region** at `position` for `length` seconds, filled with
     * `content`, and answers it. The multitrack names it.
     */
    add(position: number, length: number, content: Content, {
        name, layer = 0, fadeIn, fadeOut, muted = false,
    }: { name?: string; layer?: number; fadeIn?: Fade; fadeOut?: Fade; muted?: boolean } = {}): Region {
        const owner = this.#lane.owner;
        const [id] = owner.mint(1);
        const written: Extra = { id, position, length, content: content.write() };
        if (name !== undefined) written.name = String(name);
        if (layer) written.layer = Math.trunc(layer);
        if (fadeIn !== undefined) written.fade_in = fadeIn.write();
        if (fadeOut !== undefined) written.fade_out = fadeOut.write();
        if (muted) written.muted = true;
        owner.editIntent({
            intent: "settakelane",
            take_lane: this.#lane.ident,
            regions: [...owner.laneRegions(this.#lane.ident), written],
        }, "add a region");
        return owner.viewOf(Region, id!);
    }
}

/**
 * **One of a track's several contents**: an ordered list of regions -- a view
 * of a take lane the multitrack holds, made by `track.takeLanes.add` (a track
 * starts with one).
 */
export class TakeLane extends Held {
    protected get verb(): string {
        return "takeLane";
    }

    protected get noun(): string {
        return "take lane";
    }

    #lane(): Extra {
        return this.found().takeLane as Extra;
    }

    /** The track that holds it. */
    get track(): Track {
        return this.owner.viewOf(Track, num(this.found().track));
    }

    /** What a reader calls it. */
    get name(): string | undefined {
        return (this.#lane().name as string | undefined) ?? undefined;
    }

    set name(value: string | undefined) {
        const track = num(this.found().track);
        this.owner.rewriteTrack(track, (t) => {
            for (const lane of (t.take_lanes as Extra[] | undefined) ?? []) {
                if (num(lane.id) !== this.ident) continue;
                if (value === undefined) delete lane.name;
                else lane.name = value;
            }
        }, "rename a take lane");
    }

    /** Its regions, as a live collection in position order. */
    get regions(): Regions {
        return new Regions(this);
    }

    /** Where its last region ends, or zero when there are none. */
    get end(): number {
        return ((this.#lane().regions as Extra[] | undefined) ?? [])
            .reduce((most, r) => Math.max(most, num(r.position) + num(r.length)), 0);
    }

    /** Takes it off its track, with its regions. This object is left detached. */
    remove(): void {
        const track = num(this.found().track);
        this.owner.rewriteTrack(track, (t) => {
            t.take_lanes = ((t.take_lanes as Extra[] | undefined) ?? [])
                .filter((lane) => num(lane.id) !== this.ident);
        }, "remove a take lane");
    }

    /** The take lane as the crate writes it. */
    write(): Extra {
        return { ...this.#lane() };
    }
}

/**
 * **The take lanes a track holds, as a live collection**: iterate it, index it
 * with `item`, and `add` one.
 */
export class TakeLanes {
    readonly #track: Track;

    /** @internal */
    constructor(track: Track) {
        this.#track = track;
    }

    /** @internal */
    ids(): number[] {
        const ids = this.#track.owner.idsOf("takeLanes", { track: this.#track.ident });
        if (ids === null) throw new Error("clausters: the multitrack no longer holds this track");
        return ids;
    }

    /** How many take lanes it holds. */
    get length(): number {
        return this.ids().length;
    }

    [Symbol.iterator](): IterableIterator<TakeLane> {
        return this.ids().map((id) => this.#track.owner.viewOf(TakeLane, id))[Symbol.iterator]();
    }

    /** The take lane at index `i` (negative counts from the end). */
    item(i: number): TakeLane {
        const id = this.ids().at(i);
        if (id === undefined) throw new RangeError(`the track holds no take lane at index ${i}`);
        return this.#track.owner.viewOf(TakeLane, id);
    }

    /** **Adds an empty take lane** under the others, and answers it. */
    add(name?: string): TakeLane {
        const owner = this.#track.owner;
        const [id] = owner.mint(1);
        const written: Extra = { id };
        if (name !== undefined) written.name = String(name);
        owner.rewriteTrack(this.#track.ident, (t) => {
            t.take_lanes = [...((t.take_lanes as Extra[] | undefined) ?? []), written];
        }, "add a take lane");
        return owner.viewOf(TakeLane, id!);
    }
}

/**
 * **A row of the multitrack**: several take lanes, one of them playing, the
 * curves over it, and whatever the client says it is -- a view of a track the
 * multitrack holds, made by `mt.tracks.add`.
 *
 * **What a track *is* -- an instrument, a bus, a folder -- is not here.** That
 * is `config`, carried and never interpreted, for the reason a leaf is opaque:
 * a def is code in the language of whoever wrote it. What the document owns is
 * the structure: which take lanes, which one plays, what is placed on them.
 */
export class Track extends Held {
    protected get verb(): string {
        return "track";
    }

    protected get noun(): string {
        return "track";
    }

    #set(field: string, value: unknown, label: string): void {
        this.owner.rewriteTrack(this.ident, (t) => {
            if (value === undefined || value === null) delete t[field];
            else t[field] = value;
        }, label);
    }

    /** What a reader calls it. */
    get name(): string | undefined {
        return (this.found().name as string | undefined) ?? undefined;
    }

    set name(value: string | undefined) {
        this.#set("name", value, "rename a track");
    }

    /** Silenced. */
    get muted(): boolean {
        return Boolean(this.found().muted);
    }

    set muted(value: boolean) {
        this.#set("muted", Boolean(value), "mute a track");
    }

    /**
     * Marked as soloed. Whether a solo anywhere silences everything else is the
     * mixer's rule and not the document's.
     */
    get soloed(): boolean {
        return Boolean(this.found().soloed);
    }

    set soloed(value: boolean) {
        this.#set("soloed", Boolean(value), "solo a track");
    }

    /** Where its fader is, as a linear gain. */
    get level(): number {
        const level = this.found().level;
        return level === undefined ? 1 : num(level);
    }

    set level(value: number) {
        this.#set("level", Number(value), "set a track's level");
    }

    /** How wide it is, in channels -- two unless it says otherwise. */
    get channels(): number {
        const channels = this.found().channels;
        return channels === undefined ? 2 : num(channels);
    }

    set channels(value: number) {
        this.#set("channels", Math.trunc(value), "set a track's width");
    }

    /** What the client says the track is, carried and never read. */
    get config(): unknown {
        return this.found().config;
    }

    set config(value: unknown) {
        this.#set("config", value, "configure a track");
    }

    /** Fields a newer writer wrote, carried as they are. */
    get extra(): Extra {
        return unknownFields(this.found(), ...TRACK_FIELDS);
    }

    /** Its take lanes, as a live collection. */
    get takeLanes(): TakeLanes {
        return new TakeLanes(this);
    }

    /**
     * Which take lane plays, as an index into {@link Track.takeLanes}. Setting it
     * is comping's one verb.
     */
    get active(): number {
        return num(this.found().active);
    }

    set active(index: number) {
        this.activeTakeLane = this.takeLanes.item(index);
    }

    /** The take lane that plays, or `undefined` when `active` names one that is not there. */
    get activeTakeLane(): TakeLane | undefined {
        const ids = this.takeLanes.ids();
        const id = ids[this.active];
        return id === undefined ? undefined : this.owner.viewOf(TakeLane, id);
    }

    set activeTakeLane(lane: TakeLane | undefined) {
        if (lane === undefined) return;
        this.owner.editIntent(
            { intent: "setactivetakelane", track: this.ident, take_lane: lane.ident },
            "choose a take",
        );
    }

    /**
     * **The curves over the track**, as a live collection: drawn in rows beside
     * it, their points in the multitrack's seconds.
     */
    get automation(): Curves {
        return new Curves(this.owner, ["track", this.ident]);
    }

    /**
     * Where its last region ends, across **every** take lane -- what it spans
     * rather than what it plays, since an alternate take is still part of the
     * multitrack.
     */
    get end(): number {
        let end = 0;
        for (const lane of (this.found().take_lanes as Extra[] | undefined) ?? []) {
            for (const r of (lane.regions as Extra[] | undefined) ?? []) {
                end = Math.max(end, num(r.position) + num(r.length));
            }
        }
        return end;
    }

    /** Takes it out of the multitrack, with everything on it. This object is left detached. */
    remove(): void {
        const tracks = this.owner.tracksWritten().filter((t) => num(t.id) !== this.ident);
        this.owner.editIntent({ intent: "settracks", tracks }, "remove a track");
    }

    /** The track as the crate writes it. */
    write(): Extra {
        return { ...this.found() };
    }
}

/**
 * **The multitrack's tracks, as a live collection** in the order shown:
 * iterate it, index it with `item`, and `add` one.
 */
export class Tracks {
    readonly #owner: Multitrack;

    /** @internal */
    constructor(owner: Multitrack) {
        this.#owner = owner;
    }

    #ids(): number[] {
        return this.#owner.idsOf("tracks") ?? [];
    }

    /** How many tracks there are. */
    get length(): number {
        return this.#ids().length;
    }

    [Symbol.iterator](): IterableIterator<Track> {
        return this.#ids().map((id) => this.#owner.viewOf(Track, id))[Symbol.iterator]();
    }

    /** The track at index `i` (negative counts from the end). */
    item(i: number): Track {
        const id = this.#ids().at(i);
        if (id === undefined) throw new RangeError(`the multitrack holds no track at index ${i}`);
        return this.#owner.viewOf(Track, id);
    }

    /** **Adds a track** under the others, with one empty take lane, and answers it. */
    add(name?: string, {
        muted = false, soloed = false, level = 1, channels = 2, config,
    }: { muted?: boolean; soloed?: boolean; level?: number; channels?: number; config?: unknown } = {}): Track {
        const [trackId, laneId] = this.#owner.mint(2);
        const written: Extra = { id: trackId, take_lanes: [{ id: laneId }] };
        if (name !== undefined) written.name = String(name);
        if (muted) written.muted = true;
        if (soloed) written.soloed = true;
        if (level !== 1) written.level = Number(level);
        if (channels !== 2) written.channels = Math.trunc(channels);
        if (config !== undefined) written.config = config;
        this.#owner.editIntent(
            { intent: "settracks", tracks: [...this.#owner.tracksWritten(), written] },
            "add a track",
        );
        return this.#owner.viewOf(Track, trackId!);
    }
}

/**
 * **A named point on the timeline**, at a second -- a view of a marker the
 * multitrack holds, made by `mt.markers.add`.
 */
export class Marker extends Held {
    protected get verb(): string {
        return "marker";
    }

    protected get noun(): string {
        return "marker";
    }

    /** Where it is, in seconds. */
    get at(): number {
        return num(this.found().at);
    }

    set at(value: number) {
        this.#set(value, this.name);
    }

    /** What it is called. */
    get name(): string | undefined {
        return (this.found().name as string | undefined) ?? undefined;
    }

    set name(value: string | undefined) {
        this.#set(this.at, value);
    }

    #set(at: number, name: string | undefined): void {
        const intent: Extra = { intent: "setmarker", marker: this.ident, at };
        if (name !== undefined) intent.name = String(name);
        this.owner.editIntent(intent, "set a marker");
    }

    /** Takes it off the timeline. This object is left detached. */
    remove(): void {
        this.found();
        this.owner.editIntent({ intent: "removemarker", marker: this.ident }, "remove a marker");
    }

    /** The marker as the crate writes it. */
    write(): Extra {
        return { ...this.found() };
    }
}

/**
 * **The multitrack's markers, as a live collection** in position order:
 * iterate it, index it with `item`, and `add` one. Several may share an
 * instant: unlike a tempo, two names for one moment is a thing people do.
 */
export class Markers {
    readonly #owner: Multitrack;

    /** @internal */
    constructor(owner: Multitrack) {
        this.#owner = owner;
    }

    #ids(): number[] {
        return this.#owner.idsOf("markers") ?? [];
    }

    /** How many markers there are. */
    get length(): number {
        return this.#ids().length;
    }

    [Symbol.iterator](): IterableIterator<Marker> {
        return this.#ids().map((id) => this.#owner.viewOf(Marker, id))[Symbol.iterator]();
    }

    /** The marker at index `i` in position order (negative counts from the end). */
    item(i: number): Marker {
        const id = this.#ids().at(i);
        if (id === undefined) throw new RangeError(`the multitrack holds no marker at index ${i}`);
        return this.#owner.viewOf(Marker, id);
    }

    /** **Places a marker** at `at` seconds, and answers it. */
    add(at: number, name?: string): Marker {
        const [id] = this.#owner.mint(1);
        const intent: Extra = { intent: "setmarker", marker: id, at };
        if (name !== undefined) intent.name = String(name);
        this.#owner.editIntent(intent, "add a marker");
        return this.#owner.viewOf(Marker, id!);
    }
}

/** Which holds a multitrack's curve: a track or a region, by id. */
type CurveScope = ["track" | "region", number];

/**
 * **Curves a track or a region holds, as a live collection** of
 * {@link Automation} views: iterate it, index it with `item`, and `add` one.
 */
export class Curves {
    readonly #owner: Multitrack;
    readonly #scope: CurveScope;

    /** @internal */
    constructor(owner: Multitrack, scope: CurveScope) {
        this.#owner = owner;
        this.#scope = scope;
    }

    #ids(): number[] {
        const [kind, id] = this.#scope;
        const ids = this.#owner.idsOf("automation", { [kind]: id });
        if (ids === null) throw new Error(`clausters: the multitrack no longer holds this ${kind}`);
        return ids;
    }

    /** How many curves there are. */
    get length(): number {
        return this.#ids().length;
    }

    [Symbol.iterator](): IterableIterator<Automation> {
        return this.#ids().map((id) => this.#owner.curveView(this.#scope, id))[Symbol.iterator]();
    }

    /** The curve at index `i` (negative counts from the end). */
    item(i: number): Automation {
        const id = this.#ids().at(i);
        if (id === undefined) throw new RangeError(`there is no curve at index ${i}`);
        return this.#owner.curveView(this.#scope, id);
    }

    /**
     * **Adds a curve** and answers it, held: `target` says what it moves, in
     * the client's terms (`{ port: "gain" }`), `points` are `[second, value]`
     * pairs or the document's points, `name` labels it. Or `target` is a free
     * {@link Automation}, which is added as it is and becomes the view.
     */
    add(target: unknown, {
        points = [], name, visible = false, enabled = true,
    }: { points?: Iterable<PointLike>; name?: string; visible?: boolean; enabled?: boolean } = {}): Automation {
        let curve: Automation | null = null;
        let written: Extra;
        if (target instanceof Automation) {
            curve = target;
            if (curve.holder !== null) {
                throw new Error("clausters: this curve is held already: add a copy of it "
                    + "(Automation.read(curve.write()))");
            }
            written = curve.write();
        } else {
            written = { target, points: documentPoints(points) };
            if (name !== undefined) written.name = String(name);
            if (visible) written.visible = true;
            if (!enabled) written.enabled = false;
        }
        const [id] = this.#owner.mint(1);
        written.id = id;
        this.#owner.withCurves(this.#scope, (curves) => [...curves, written], "add a curve");
        if (curve === null) return this.#owner.curveView(this.#scope, id!);
        this.#owner.adoptCurve(curve, this.#scope, id!);
        return curve;
    }
}

/**
 * **The tracks, and the timeline they are placed on** -- a handle over the
 * multitrack the shared crate holds, which a multitrack editor opened on it
 * edits in place.
 *
 * What is here rather than on a track is what the **multitrack** has one of:
 * the tempo map, the meter map, the markers, the loop. A track has none of them
 * and never disagrees with another track about them, which is the whole
 * argument for where they live.
 *
 * **What a page reads and writes is objects**: {@link Multitrack.tracks}, a
 * track's {@link Track.takeLanes}, a take lane's {@link TakeLane.regions}, the
 * curves of a track or a region and the {@link Multitrack.markers} are live
 * collections whose `add` answers the object it made, and every object is a
 * view of what the multitrack holds -- so a change is made through the object
 * it changes (`region.position = 2.0`, `track.muted = true`,
 * `region.remove()`), and no call takes or answers an id. A tempo entry, a
 * meter entry, a span, a fade and a region's content have no identity of their
 * own: they are read as values and written whole through what holds them.
 */
export class Multitrack implements CurveHolder {
    #mt: JsMultitrack;
    /**
     * The multitrack editor whose window has this multitrack open, which a
     * change made through its objects is forwarded to. @internal
     */
    window: WindowOver | null = null;

    /** What {@link Multitrack.read} hands the constructor, for the one call it makes. */
    static #reading: Extra | null = null;

    /** @param options `channels`: how wide the multitrack is -- the master's own width. */
    constructor({ channels = 2 }: { channels?: number } = {}) {
        requireCore("a Multitrack");
        const reading = Multitrack.#reading;
        Multitrack.#reading = null;
        const written = reading ?? (channels === 2 ? null : { channels: Math.trunc(channels) });
        this.#mt = new JsMultitrack(written === null ? "" : JSON.stringify(written));
    }

    /** A multitrack from the crate's JSON -- what `write` and a session file hold. */
    static read(written: Extra): Multitrack {
        Multitrack.#reading = { ...(written ?? {}) };
        return new Multitrack();
    }

    /** The multitrack as the crate's JSON. Nothing said is nothing written. */
    write(): Extra {
        return this.call("state") as Extra;
    }

    // ---- the door, and the identity map ----

    /** The handle a multitrack editor opens over. @internal */
    get handle(): JsMultitrack {
        return this.#mt;
    }

    /**
     * **Holds `written` from now on**, the document's JSON: what the window
     * that had it open says it is now. The objects a page holds stay the
     * same objects.
     *
     * @internal
     */
    holdWritten(written: Extra): void {
        this.#mt = new JsMultitrack(JSON.stringify(written ?? {}));
    }

    /** One verb of the multitrack's door; throws with the crate's error. @internal */
    call(verb: string, args: Extra = {}): unknown {
        const answer = JSON.parse(this.#mt.call(JSON.stringify({ verb, ...args }))) as unknown;
        if (answer !== null && typeof answer === "object" && "error" in (answer as Extra)) {
            throw new Error(`clausters: ${String((answer as Extra).error)}`);
        }
        return answer;
    }

    /** `kind:id` to the one object that stands for that structure, while anything holds it. */
    readonly #objects = new Map<string, WeakRef<object>>();

    #object<T extends object>(key: string, make: () => T): T {
        const found = this.#objects.get(key)?.deref();
        if (found !== undefined) return found as T;
        const made = make();
        this.#objects.set(key, new WeakRef(made));
        return made;
    }

    /** The object of the structure `id` is, of class `cls` -- the same one every time. @internal */
    viewOf<T extends Held>(cls: new (owner: Multitrack, id: number) => T, id: number): T {
        return this.#object(`${cls.name}:${id}`, () => new cls(this, id));
    }

    /** The object of curve `id`, held by `scope` -- the same one every time. @internal */
    curveView(scope: CurveScope, id: number): Automation {
        return this.#object(`curve:${id}`, () => Automation.heldBy(this, scope, id));
    }

    /** Makes a free curve the view of curve `id`, in the identity map. @internal */
    adoptCurve(curve: Automation, scope: CurveScope, id: number): void {
        curve.bind(this, scope, id);
        this.#objects.set(`curve:${id}`, new WeakRef(curve));
    }

    /** The ids a container holds, in the order shown, or `null` when it is not there. @internal */
    idsOf(of: string, where: Extra = {}): number[] | null {
        const ids = (this.call("ids", { of, ...where }) as { ids: number[] | null }).ids;
        return ids === null ? null : ids.map(Number);
    }

    /** `count` ids nothing in the multitrack names. @internal */
    mint(count: number): number[] {
        return ((this.call("mint", { count }) as { ids: number[] }).ids).map(Number);
    }

    /**
     * Applies one edit in the multitrack's vocabulary and answers `{applied,
     * current?, reason?}`. The door the objects write through.
     *
     * @internal
     */
    applyIntent(intent: Extra, inverse = true): { applied: boolean; current?: unknown; reason?: string } {
        return this.call("apply", { intent, inverse }) as { applied: boolean; current?: unknown; reason?: string };
    }

    /**
     * **One change a page makes through an object**: applied, and answered as
     * {@link Multitrack.applyIntent} answers; throws with the multitrack's
     * reason when it refuses. `label` is what an undo would call it.
     *
     * A multitrack the multitrack application has open is the window's: the
     * change is made here and forwarded to the window, an edit of its
     * history, in the order the page made them. One with a history of this
     * page's -- one it asked for {@link Multitrack.history} -- takes the
     * change as a turn of it: recorded, and every view over it redrawn. One
     * with none just changes.
     *
     * @internal
     */
    editIntent(intent: Extra, label: string): { applied: boolean; reason?: string } {
        const context = contexts.get(this);
        const window = this.window !== null && !this.window.closed ? this.window : null;
        const answer = window !== null || context === undefined
            ? this.applyIntent(intent, window !== null)
            : context.scriptEdit(this, intent, label);
        // the window's is the multitrack: the change is an edit of the window's
        if (window !== null && answer.applied) window.forward(intent, label, (answer as { current?: unknown }).current);
        if (!answer.applied && answer.reason) throw new Error(`clausters: ${answer.reason}`);
        return answer;
    }

    /** The tracks as written. @internal */
    tracksWritten(): Extra[] {
        return [...((this.write().tracks as Extra[] | undefined) ?? [])];
    }

    /** The regions of take lane `lane`, as written. @internal */
    laneRegions(lane: number): Extra[] {
        const found = this.call("takeLane", { id: lane }) as { takeLane: Extra } | null;
        if (found === null) throw new Error("clausters: the multitrack no longer holds this take lane");
        return [...((found.takeLane.regions as Extra[] | undefined) ?? [])];
    }

    /**
     * Rewrites one track with `change` -- a function over its written form -- as
     * the tracks stated whole, which keeps every identity.
     *
     * @internal
     */
    rewriteTrack(id: number, change: (track: Extra) => void, label: string): void {
        const tracks = this.tracksWritten();
        for (const track of tracks) if (num(track.id) === id) change(track);
        this.editIntent({ intent: "settracks", tracks }, label);
    }

    // ---- what a history asks of it ----

    /** The key and the domain it joins a history under -- a multitrack editor's. @internal */
    scriptKey(): [string, string] {
        return [keyOf("multitrack", this), "multitrack"];
    }

    /**
     * The edit a redo applies: the intent itself, which already names every
     * identity it makes -- the ids are minted before it is sent.
     *
     * @internal
     */
    forwardOf(intent: Extra): Extra {
        return intent;
    }

    /**
     * **The multitrack's history**: the undo order its editors share, made on
     * first ask. From then on every change made through the multitrack's
     * objects is an entry of it, and a turn the windows over it see;
     * `mt.history.entry("tidy", () => ...)` makes everything inside it one
     * entry. While the multitrack application has it open it is the window's
     * history ({@link WindowHistory}), which its Ctrl+Z walks.
     */
    get history(): UndoHistory | WindowHistory {
        if (this.window !== null && !this.window.closed) return this.window.history;
        return new UndoHistory(this);
    }

    // ---- the curve holder ----

    /** Curve `id` as written, while `scope` holds it. @internal */
    writtenCurve(scope: CurveScope, id: number): Extra | null {
        const found = this.call("automation", { id }) as Extra | null;
        if (found === null || num(found[scope[0]]) !== scope[1] || found[scope[0]] === undefined) return null;
        return found.automation as Extra;
    }

    /**
     * Rewrites the curves `scope` holds with `change`, through the edit that
     * states their holder: the tracks for a track's, the take lane for a
     * region's.
     *
     * @internal
     */
    withCurves(scope: CurveScope, change: (curves: Extra[]) => Extra[], label: string): void {
        const [kind, id] = scope;
        if (kind === "track") {
            this.rewriteTrack(id, (t) => {
                t.automation = change([...((t.automation as Extra[] | undefined) ?? [])]);
            }, label);
            return;
        }
        const found = this.call("region", { id }) as Extra | null;
        if (found === null) throw new Error("clausters: the multitrack no longer holds this region");
        const lane = num(found.takeLane);
        const regions = this.laneRegions(lane);
        for (const region of regions) {
            if (num(region.id) === id) {
                region.automation = change([...((region.automation as Extra[] | undefined) ?? [])]);
            }
        }
        this.editIntent({ intent: "settakelane", take_lane: lane, regions }, label);
    }

    /**
     * Writes a curve whole and answers its id. Its points alone are the
     * multitrack's own curve verb, the one a curve drawn in a row is, and
     * whether it is shown alone is the one a menu's check is.
     *
     * @internal
     */
    writeCurve(scope: CurveScope, written: Extra, label: string): number {
        const id = num(written.id);
        const current = this.writtenCurve(scope, id);
        const without = (curve: Extra, key: string) => JSON.stringify(unknownFields(curve, key));
        if (current !== null && without(current, "visible") === without(written, "visible")) {
            this.editIntent({ intent: "showautomation", automation: id,
                              visible: Boolean(written.visible) }, label);
            return id;
        }
        if (current !== null && without(current, "points") === without(written, "points")) {
            this.editIntent({ intent: "setautomation", automation: id,
                              points: [...((written.points as Extra[] | undefined) ?? [])] }, label);
            return id;
        }
        this.withCurves(scope, (curves) => curves.map((c) => (num(c.id) === id ? written : c)), label);
        return id;
    }

    /** Removes curve `id` from `scope`. @internal */
    removeCurve(scope: CurveScope, id: number): void {
        this.withCurves(scope, (curves) => curves.filter((c) => num(c.id) !== id), "remove a curve");
    }

    // ---- reading ----

    /** What this multitrack is *at*, and the whole of what a stale edit is stale against. */
    get version(): number {
        const version = this.write().version;
        return version === undefined ? FIRST_VERSION : num(version);
    }

    /**
     * How wide the multitrack is, in channels -- the master's own width, and
     * what a track's output is mixed into.
     */
    get channels(): number {
        const channels = this.write().channels;
        return channels === undefined ? 2 : num(channels);
    }

    /** Fields a newer writer wrote, carried as they are. */
    get extra(): Extra {
        return unknownFields(this.write(), "version", "tracks", "channels", "tempo", "meter",
                    "markers", "loop_span", "punch");
    }

    /** The tracks, as a live collection in the order shown. */
    get tracks(): Tracks {
        return new Tracks(this);
    }

    /** The named points, as a live collection in position order. */
    get markers(): Markers {
        return new Markers(this);
    }

    /**
     * Where the last region ends, across every track and every take lane -- how
     * long the multitrack is.
     */
    get end(): number {
        let end = 0;
        for (const track of this.tracksWritten()) {
            for (const lane of (track.take_lanes as Extra[] | undefined) ?? []) {
                for (const r of (lane.regions as Extra[] | undefined) ?? []) {
                    end = Math.max(end, num(r.position) + num(r.length));
                }
            }
        }
        return end;
    }

    /**
     * Every region, in track then take lane then position order -- **every**
     * take lane, not only the ones that play, because an alternate take still
     * names the source it plays.
     */
    *regions(): Generator<Region> {
        for (const track of this.tracksWritten()) {
            for (const lane of (track.take_lanes as Extra[] | undefined) ?? []) {
                for (const region of (lane.regions as Extra[] | undefined) ?? []) {
                    yield this.viewOf(Region, num(region.id));
                }
            }
        }
    }

    // ---- the timeline's own: the two maps and the two spans ----

    /** The tempo map's entries, in position order, as values. Change it with {@link Multitrack.setTempo}. */
    get tempo(): Tempo[] {
        return ((this.write().tempo as Extra[] | undefined) ?? []).map(Tempo.read);
    }

    /** The meter map's entries, in position order, as values. Change it with {@link Multitrack.setMeter}. */
    get meter(): Meter[] {
        return ((this.write().meter as Extra[] | undefined) ?? []).map(Meter.read);
    }

    /**
     * The tempo map this multitrack holds, as a `TempoMap`: where its beats and
     * bars fall over its seconds, with the reader's default of one beat a second
     * where it states no tempo.
     *
     * It places nothing -- every position here is already seconds -- and is what
     * a ruler draws from and what a script asks to put something on a bar. Built
     * afresh on each call, so an edited tempo is the one read.
     */
    tempoMap(): TempoMap {
        const fallback = editingDefaultTempo();
        return (
            TempoMap.fromChanges(
                this.tempo.map((t) => ({ beats: t.at, tempo: t.tempo, ramp: t.ramp })),
                fallback,
            ) ?? new TempoMap(fallback)
        );
    }

    /**
     * The tempo entry in force at beat `at`, or `undefined` when the map says
     * nothing.
     *
     * The **entry**, not a converted position; {@link Multitrack.tempoMap} is the
     * map.
     */
    tempoAt(at: number): Tempo | undefined {
        return [...this.tempo].reverse().find((t) => t.at <= at);
    }

    /** The meter entry in force at `at`, or `undefined`. */
    meterAt(at: number): Meter | undefined {
        return [...this.meter].reverse().find((m) => m.at <= at);
    }

    /**
     * Adds a tempo entry, in position order, replacing any already at that beat
     * -- two tempos at one position is a state the map should not hold.
     */
    setTempo(tempo: Tempo): void {
        const entries = [...this.tempo.filter((t) => t.at !== tempo.at), tempo]
            .sort((a, b) => a.at - b.at);
        this.editIntent({ intent: "settempomap", tempo: entries.map((t) => t.write()) }, "set the tempo");
    }

    /** Adds a meter entry, on the same rule. */
    setMeter(meter: Meter): void {
        const entries = [...this.meter.filter((m) => m.at !== meter.at), meter]
            .sort((a, b) => a.at - b.at);
        this.editIntent({ intent: "setmetermap", meter: entries.map((m) => m.write()) }, "set the meter");
    }

    /**
     * **Whether an edit by hand that makes two regions overlap crossfades
     * them**: on, the edit writes the crossfade into the earlier one's fade out
     * and the later one's fade in; off, it writes none and the overlap is the
     * two summed. It acts on the edits that follow: turning it off or on
     * changes no region. A default of the multitrack, on unless turned off,
     * and set like any edit -- recorded, and drawn by every window over it.
     */
    get crossfade(): boolean {
        const defaults = (this.write().defaults ?? {}) as Extra;
        return defaults.crossfade === undefined ? true : Boolean(defaults.crossfade);
    }

    set crossfade(on: boolean) {
        const defaults: Extra = { ...((this.write().defaults ?? {}) as Extra), crossfade: on };
        this.editIntent({ intent: "setdefaults", defaults }, "crossfade overlaps");
    }

    /**
     * **The fade every region's edge has unless the region states one**: 10 ms
     * of equal power when nothing was set, and `undefined` for none at all in
     * this multitrack, where a region that states nothing meets the silence
     * beside it as a step. A region's own {@link Region.fadeIn} and
     * {@link Region.fadeOut} win over it; one left `undefined` follows it, and
     * so is an edge the crossfade writes over. Set like any edit.
     */
    get defaultFade(): Fade | undefined {
        const defaults = (this.write().defaults ?? {}) as Extra;
        if (!("fade" in defaults)) return new Fade(0.01);
        const fade = defaults.fade as Extra | null;
        return fade === null ? undefined : Fade.read(fade);
    }

    set defaultFade(fade: Fade | undefined) {
        const defaults: Extra = {
            ...((this.write().defaults ?? {}) as Extra),
            fade: fade === undefined ? null : fade.write(),
        };
        this.editIntent({ intent: "setdefaults", defaults }, "set the default fade");
    }

    /**
     * Where the loop is, or `undefined`. Whether looping is *on* is the
     * transport's; what the multitrack holds is where.
     */
    get loopSpan(): Span | undefined {
        const span = this.write().loop_span as Extra | undefined;
        return span ? Span.read(span) : undefined;
    }

    set loopSpan(span: Span | undefined) {
        this.#range("loop", span);
    }

    /** Where recording punches in and out, or `undefined`. */
    get punch(): Span | undefined {
        const span = this.write().punch as Extra | undefined;
        return span ? Span.read(span) : undefined;
    }

    set punch(span: Span | undefined) {
        this.#range("punch", span);
    }

    #range(which: "loop" | "punch", span: Span | undefined): void {
        const intent: Extra = { intent: "setrange", range: which };
        if (span !== undefined) intent.span = span.write();
        this.editIntent(intent, `set the ${which}`);
    }
}

