/**
 * The multitrack: tracks, take lanes, regions, and the timeline they sit on
 * (mirrors `clausters/multitrack.py`).
 *
 * This is the client's side of `clausters_document::multitrack` -- the model a
 * multitrack editor edits, and the one the three classic applications (audio
 * editor, multitrack editor, score editor) are built over. The crate defines
 * the format; this module is the idiomatic way to write one and read one back,
 * the same way `./document.ts` is the idiomatic way to reach an edit.
 *
 * The vocabulary is the field's own and not this project's invention:
 *
 * - A **source** is samples. It lives outside the arrangement -- the session's
 *   table says where -- and is never overwritten.
 * - A {@link Region} is **one placed thing**: a span of the timeline (where it
 *   starts, how long, its fades, which of the overlapping ones is on top) plus
 *   a {@link Content} saying what fills it. Six regions over one source are six
 *   identities and one source, referenced rather than copied. That is the whole
 *   of non-destructive editing.
 * - A {@link TakeLane} is one of a track's several contents, an ordered list of
 *   regions. Ardour's structure and our name.
 * - A {@link Track} holds several take lanes and **plays one**, which is what
 *   comping is: record six passes into six take lanes, then take from each.
 * - An {@link Automation} is a curve over one parameter, in the arrangement's
 *   time.
 * - An {@link Multitrack} is the tracks plus what the **multitrack** has one of:
 *   the tempo map, the meter map, the markers, the loop and punch spans. They
 *   are here and not on a track precisely so that no two tracks can disagree
 *   about them.
 *
 * ## A region is not a clip
 *
 * `Region` is the model's word; **clip** is the picture's. A clip, a track row,
 * a waveform are what the host draws; a region is what an edit names. Keeping
 * them apart is deliberate -- the multitrack's defects came from the thing drawn
 * and the thing addressed being one object.
 *
 * ## Time
 *
 * Everything placed here is placed in **seconds**: a region's position, length
 * and fades, every automation point, the markers, the loop and the punch. A
 * multitrack is governed by physical time, the way the server and the clients
 * are, and no tempo change moves anything in it. What fills a region is measured
 * in its own source's units -- seconds of a recording, beats of a node -- and the
 * two are not the same axis. The crate makes that a type; here it is a rule the
 * field names say (`position` and `length` are the region's, `start` and
 * `duration` are its window's).
 *
 * The **tempo map and the meter map** are structures the multitrack holds, not
 * its axis: their entries are stated at beats, in beats per second, and what
 * reads them is a ruler drawing beats and bars over the seconds and a snap to
 * them. {@link Multitrack.tempoMap} is that map as a `TempoMap`, so a script that
 * wants a region on bar five asks it where bar five is.
 *
 * ## Spelling
 *
 * The fields are camelCase here and snake_case in the format, which is the same
 * `idiom` split every other module keeps: `fadeIn` is written as `fade_in`, and
 * a reader moving between the clients finds the same call in its own language's
 * shape.
 *
 * @module
 */

import {
    JsMultitrack,
    StepRunner,
    editingDefaultTempo,
    multitrackNames as coreNames,
    multitrackProps as coreProps,
    sessionMigrate,
} from "./core/clausters_core_web.js";
import { TempoMap } from "./base/time.ts";
import { Buffer } from "./defs/buffer.ts";
import { quads } from "./defs/ugens/env.ts";
import type { Curve as CurveSpec, PointsLike } from "./defs/ugens/env.ts";
import type { Server } from "./defs/server/index.ts";
import { resolveServer } from "./defs/wire.ts";
import { FIRST_VERSION, SESSION_FORMAT, editingLoad } from "./document.ts";
import { requireCore } from "./base/core.ts";
import { UndoHistory, contexts, keyOf } from "./history.ts";
import { readFileAt, writeFileAt } from "./base/files.ts";
import { EventSequence } from "./seq/sequence.ts";
import { runSteps } from "./steps.ts";

/** Whatever a newer writer wrote and this build has no field for. */
export type Extra = Record<string, unknown>;

/** A file's text, where `base/files.ts` keeps files. */
async function readText(path: string): Promise<string> {
    return new TextDecoder().decode(await readFileAt(path));
}

/** Writes a file's text where {@link readText} reads it. */
async function writeText(path: string, text: string): Promise<void> {
    await writeFileAt(path, new TextEncoder().encode(text));
}

/** The folder a path is in, `.` for a bare name. */
function folderOf(path: string): string {
    const cut = path.lastIndexOf("/");
    return cut < 0 ? "." : cut === 0 ? "/" : path.slice(0, cut);
}

/** Everything in `written` except the keys this build knows. */
function rest(written: Extra, ...known: string[]): Extra {
    const out: Extra = {};
    for (const [key, value] of Object.entries(written)) {
        if (!known.includes(key)) out[key] = value;
    }
    return out;
}

function num(value: unknown, fallback = 0): number {
    return value === undefined || value === null ? fallback : Number(value);
}

/**
 * A fade's length in seconds, and whatever the client says about its curve.
 *
 * The shape is carried and never interpreted: what an exponential fade *is*
 * belongs to whoever renders it. Losing it would straighten every fade on a
 * reopen, which is a different act from declining to interpret it.
 */
export class Fade {
    length: number;
    shape: unknown;

    constructor(length: number, shape?: unknown) {
        this.length = length;
        this.shape = shape;
    }

    write(): Extra {
        const out: Extra = { length: this.length };
        if (this.shape !== undefined) out.shape = this.shape;
        return out;
    }

    static read(written: Extra): Fade {
        return new Fade(num(written.length), written.shape);
    }
}

/**
 * What fills a region: a **window** onto a source, or a **composite** tree.
 *
 * Not the clipboard's content, which is what was *copied*. Two nouns in two
 * modules, each the right word where it stands: this one is a region's
 * `content` field, and the format tags it `fill`.
 *
 * Two shapes held as one class with a `fill` saying which, because that is how
 * the format writes it and a client that mirrored it as a class hierarchy would
 * spend an inheritance on a tag.
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
    node?: Extra;
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
        this.node = fields.node;
        this.other = fields.other;
        this.looping = fields.looping ?? false;
    }

    /**
     * A window onto a source: `{ source, start, duration }`.
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

    /** The general tree, placed as one region. */
    static composite(node: Extra): Content {
        return new Content("composite", { node });
    }

    write(): Extra {
        if (this.fill === "window") {
            const out: Extra = { fill: "window", window: this.window };
            if (this.playrate !== 1) out.playrate = this.playrate;
            if (this.args !== undefined) out.args = this.args;
            if (this.looping) out.loop = true;
            return out;
        }
        if (this.fill === "composite") return { fill: "composite", node: this.node };
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
        if (written.fill === "composite") {
            return new Content("composite", { node: written.node as Extra });
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
        return rest(written, ...CURVE_FIELDS);
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
            extra: rest(written, "id", "name", "target", "points", "visible", "enabled"),
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
        return rest(this.#region(), ...REGION_FIELDS);
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
        return rest(this.found(), ...TRACK_FIELDS);
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
 * One entry of the tempo map: from here on, this tempo.
 *
 * `at` is a beat and `tempo` is beats per **second**, the unit every tempo in
 * clausters is in; beats per minute is only how a ruler or a text field may show
 * it. `ramp` says the tempo runs from here to the next entry rather than
 * stepping. A ritardando is a ramp; a section change is a step.
 */
export class Tempo {
    at: number;
    tempo: number;
    ramp: boolean;
    extra: Extra;

    constructor(fields: { at: number; tempo: number; ramp?: boolean; extra?: Extra }) {
        this.at = fields.at;
        this.tempo = fields.tempo;
        this.ramp = fields.ramp ?? false;
        this.extra = fields.extra ?? {};
    }

    write(): Extra {
        const out: Extra = { at: this.at, tempo: this.tempo };
        if (this.ramp) out.ramp = true;
        return { ...out, ...this.extra };
    }

    static read(written: Extra): Tempo {
        return new Tempo({
            at: num(written.at),
            tempo: num(written.tempo),
            ramp: Boolean(written.ramp),
            extra: rest(written, "at", "tempo", "ramp"),
        });
    }
}

/**
 * One entry of the meter map: from here on, this time signature.
 *
 * It says how beats make **bars**, which is what a ruler draws and what a
 * snap-to-bar means. It never moves a beat: a meter change does not shift what
 * is placed, it re-bars it.
 */
export class Meter {
    at: number;
    beats: number;
    unit: number;
    extra: Extra;

    constructor(fields: { at: number; beats: number; unit: number; extra?: Extra }) {
        this.at = fields.at;
        this.beats = fields.beats;
        this.unit = fields.unit;
        this.extra = fields.extra ?? {};
    }

    write(): Extra {
        return { at: this.at, beats: this.beats, unit: this.unit, ...this.extra };
    }

    static read(written: Extra): Meter {
        return new Meter({
            at: num(written.at),
            beats: num(written.beats),
            unit: num(written.unit),
            extra: rest(written, "at", "beats", "unit"),
        });
    }
}

/**
 * A span of the timeline: the loop, the punch, a named region of the multitrack.
 *
 * Half-open, so two spans that meet cover no instant twice. In seconds.
 */
export class Span {
    start: number;
    end: number;

    constructor(start: number, end: number) {
        this.start = start;
        this.end = end;
    }

    get length(): number {
        return Math.max(0, this.end - this.start);
    }

    write(): Extra {
        return { start: this.start, end: this.end };
    }

    static read(written: Extra): Span {
        return new Span(num(written.start), num(written.end));
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
    readonly #mt: JsMultitrack;

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
     * A multitrack with a history -- one an editor is open on, or one a page
     * asked for {@link Multitrack.history} -- takes the change as a turn of
     * it: recorded, and every window over it redrawn. One with none just
     * changes.
     *
     * @internal
     */
    editIntent(intent: Extra, label: string): { applied: boolean; reason?: string } {
        const context = contexts.get(this);
        const answer = context === undefined
            ? this.applyIntent(intent, false)
            : context.scriptEdit(this, intent, label);
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
     * entry.
     */
    get history(): UndoHistory {
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
        const without = (curve: Extra, key: string) => JSON.stringify(rest(curve, key));
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
        return rest(this.write(), "version", "tracks", "channels", "tempo", "meter",
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

// ---- the session: the multitrack, and where its samples are ----
//
// Lifted out of `form/document.ts` rather than written again. What was worth
// keeping there was never the element-to-node conversion -- that is form's
// vocabulary and goes with it -- but this: a source table that says where
// samples are, a frozen reference for one this page does not hold, and a file
// that knows what it is missing. None of it was ever about form's primitives.

/**
 * One entry in a session's source table: where samples are, how long they live,
 * and what shape they have.
 *
 * The two fields a naive format leaves out and then cannot add are here:
 * `provenance`, a reference to whatever produced the samples, carried opaquely
 * so re-generating stays possible *without the document knowing how*; and
 * `editing`, a destructive edit that has not been confirmed -- a save never
 * blocks on a confirmation, so a saved session has to be able to say *this is a
 * working copy of that, and the person has not decided yet*.
 */
export class Source {
    /**
     * `{at: "file", path}`, `{at: "volatile"}` or `{at: "events"}` (the events
     * are {@link Source.sequence}). A relative path is resolved
     * against the session's own folder, which is what makes a session directory
     * movable; an absolute one names the user's own file, which a session must
     * never copy or rewrite.
     */
    location: Extra;
    /**
     * `"external"` (the user's own file), `"session"` (saved beside the
     * document) or `"temporary"` (a working copy that dies with the edit).
     */
    lifetime: string;
    /** Which generation of the content this is -- bumped by a destructive edit. */
    generation: number;
    channels?: number;
    frames?: number;
    /** Carried, never acted on: resampling is an edit. */
    sampleRate?: number;
    provenance?: unknown;
    /** `{from, confirmed}` while a destructive edit is open over these samples. */
    editing?: Extra;
    extra: Extra;
    /**
     * **The events, when the source is a sequence of them**: the
     * `EventSequence` itself, held rather than copied, so what an editor does
     * to it is what the next save writes.
     */
    sequence?: EventSequence;

    constructor(fields: {
        location: Extra;
        lifetime?: string;
        generation?: number;
        channels?: number;
        frames?: number;
        sampleRate?: number;
        provenance?: unknown;
        editing?: Extra;
        extra?: Extra;
        sequence?: EventSequence;
    }) {
        this.location = fields.location;
        this.lifetime = fields.lifetime ?? "session";
        this.generation = fields.generation ?? 0;
        this.channels = fields.channels;
        this.frames = fields.frames;
        this.sampleRate = fields.sampleRate;
        this.provenance = fields.provenance;
        this.editing = fields.editing;
        this.extra = fields.extra ?? {};
        this.sequence = fields.sequence;
    }

    /** Samples in a file. */
    static file(path: string, lifetime = "session"): Source {
        return new Source({ location: { at: "file", path }, lifetime });
    }

    /**
     * Samples that have not been written down. A session may hold one, because
     * saving must not be blocked by it, but a reader that finds one knows the
     * samples are not there and opens that element unresolved rather than
     * pretending.
     */
    static volatile(lifetime = "session"): Source {
        return new Source({ location: { at: "volatile" }, lifetime });
    }

    /**
     * A sequence of events -- an `EventSequence` -- held in the session file
     * itself. A region over it is a window onto its beats (in seconds, through
     * its tempo map), and it draws the notes.
     */
    static events(sequence: EventSequence, lifetime = "session"): Source {
        return new Source({ location: { at: "events" }, lifetime, sequence });
    }

    /** Its shape, for a caller that knows it. */
    shaped(channels: number, frames: number, sampleRate: number): Source {
        this.channels = channels;
        this.frames = frames;
        this.sampleRate = sampleRate;
        return this;
    }

    /** Where the file is, when the samples are in one. */
    get path(): string | undefined {
        if (this.location.at !== "file") return undefined;
        return (this.location.path as string) || undefined;
    }

    /**
     * Whether the samples are somewhere a reader could find them. A sequence
     * is in the file itself, so it always is.
     */
    get isResolvable(): boolean {
        return this.sequence !== undefined || this.path !== undefined;
    }

    /** Whether a destructive edit is open and undecided over these samples. */
    get isBeingEdited(): boolean {
        return Boolean(this.editing) && !this.editing!.confirmed;
    }

    write(): Extra {
        const out: Extra = {
            location:
                this.sequence === undefined
                    ? this.location
                    : { at: "events", sequence: this.sequence.data() },
            lifetime: this.lifetime,
            generation: this.generation,
        };
        if (this.channels !== undefined) out.channels = this.channels;
        if (this.frames !== undefined) out.frames = this.frames;
        if (this.sampleRate !== undefined) out.sample_rate = this.sampleRate;
        if (this.provenance !== undefined) out.provenance = this.provenance;
        if (this.editing !== undefined) out.editing = this.editing;
        return { ...out, ...this.extra };
    }

    static read(written: Extra): Source {
        const { sequence, ...location } = (written.location as Extra) ?? { at: "volatile" };
        return new Source({
            location,
            sequence: location.at === "events" ? EventSequence.fromData(sequence ?? {}) : undefined,
            lifetime: String(written.lifetime ?? "session"),
            generation: num(written.generation),
            channels: written.channels as number | undefined,
            frames: written.frames as number | undefined,
            sampleRate: written.sample_rate as number | undefined,
            provenance: written.provenance,
            editing: written.editing as Extra | undefined,
            extra: rest(written, "location", "lifetime", "generation", "channels",
                        "frames", "sample_rate", "provenance", "editing"),
        });
    }
}

/**
 * A source a session names and this page does not hold.
 *
 * Reading a session written elsewhere gives a reference and not an object.
 * Rather than losing it, a region's window holds this: the same `bufnum` a real
 * buffer answers with, plus what the table said about where the samples are and
 * what shape they have, so a re-save keeps every location it was given.
 *
 * Without it, a multitrack opened with no way to read its files would be written back
 * with every source marked volatile, which is a format that loses its own
 * contents on the second save.
 */
export class FrozenSource {
    bufnum: number;
    lifetime = "session";
    generation = 0;
    path?: string;
    frames = 0;
    channels = 0;
    sampleRate = 0;

    constructor(id: number, entry?: Source) {
        this.bufnum = id;
        this.locate(entry);
    }

    /** Take where and what this source is from a session's table entry. */
    locate(entry?: Source): void {
        if (!entry) return;
        this.path = entry.path;
        this.lifetime = entry.lifetime;
        this.generation = entry.generation;
        this.frames = entry.frames ?? 0;
        this.channels = entry.channels ?? 0;
        this.sampleRate = entry.sampleRate ?? 0;
    }
}

// ---- the presentation: what a window shows of a multitrack ----
//
// Parallel to the model and never inside it, which is Live's shape and
// deliberate: `Song.View`, `Track.View` and `Application.View` are objects
// *beside* their model objects rather than children. So a `TrackView` is looked
// up by the track's id, and an `Multitrack` round trips the same whether or
// not a view of it exists.

/** How one track is drawn. */
export class TrackView {
    /**
     * How tall its row is, in the window's own units. Absent is the window's
     * default, which is what a track nobody resized has.
     */
    height?: number;
    /** Whether the row is collapsed to its header. */
    collapsed = false;
    /**
     * Whether the track's other take lanes are shown under the one that plays --
     * comping open, in a word. Closed by default: a track with six takes on it
     * is one row until somebody asks to see them.
     */
    takeLanesShown = false;
    /** The colour the track is drawn in, carried and never read. */
    color?: string;
    extra: Extra = {};

    write(): Extra {
        const out: Extra = {};
        if (this.height !== undefined) out.height = this.height;
        if (this.collapsed) out.collapsed = true;
        if (this.takeLanesShown) out.take_lanes_shown = true;
        if (this.color !== undefined) out.color = this.color;
        return { ...out, ...this.extra };
    }

    static read(written: Extra): TrackView {
        const view = new TrackView();
        if (written.height !== undefined) view.height = num(written.height);
        view.collapsed = written.collapsed === true;
        view.takeLanesShown = written.take_lanes_shown === true;
        if (written.color !== undefined) view.color = String(written.color);
        view.extra = rest(written, "height", "collapsed", "take_lanes_shown", "color");
        return view;
    }
}

/** How one lane is drawn. */
export class TakeLaneView {
    /** How tall its row is when the track's lanes are shown. */
    height?: number;
    extra: Extra = {};

    write(): Extra {
        const out: Extra = {};
        if (this.height !== undefined) out.height = this.height;
        return { ...out, ...this.extra };
    }

    static read(written: Extra): TakeLaneView {
        const view = new TakeLaneView();
        if (written.height !== undefined) view.height = num(written.height);
        view.extra = rest(written, "height");
        return view;
    }
}

/**
 * One window's picture of one multitrack: where it is looking, how far it is zoomed,
 * what the hand is holding, how tall each track is drawn.
 *
 * None of that is what the multitrack *is* -- a selection and a zoom are each
 * window's and never the multitrack's -- and all of it is state a person loses
 * on a reopen unless something writes it down. A session carries a **list** of
 * these, because a multitrack drawn in two windows has two views and they disagree
 * on purpose.
 *
 * Nothing here ever reaches the document or the history: a view is not edited
 * through an intent, and an undo never puts a scroll back.
 */
export class View {
    /** What the window is called, when a person named it. */
    name?: string;
    /**
     * The stretch of the timeline on screen, in seconds -- the zoom and the
     * horizontal scroll, which are one fact and not two. Absent shows the whole
     * multitrack.
     */
    visible?: Span;
    /** How far down the tracks the window is scrolled, in its own units. */
    scroll = 0;
    /**
     * The grid this window snaps to, in beats. Zero snaps nothing. It is here
     * rather than in the multitrack because two windows over one multitrack may snap
     * differently -- the arranger to a bar, the editor below it to a sixteenth.
     * A musical grid over a multitrack in seconds, taken through its tempo map by
     * the window: the ruler's configuration, not a unit of the placement.
     */
    quant = 0;
    /**
     * Whether the window follows its content. `false` says the window is the
     * reader's, and nothing moves it, which is what an editor wants.
     */
    autofit = true;
    /** The time range the hand swept, in seconds, when it swept one. */
    selection?: Span;
    /**
     * What the hand is holding: regions, take lanes or tracks, by id. One list
     * rather than one per kind, because the multitrack has one id space.
     */
    selected: number[] = [];
    /** What a keystroke is aimed at, which is not the same as what is selected. */
    focused?: number;
    /** The region the detail editor below is showing, when the window has one. */
    detail?: number;
    /** How each track is drawn, by the track's id. */
    tracks = new Map<number, TrackView>();
    /** How each lane is drawn, by the lane's id. */
    takeLanes = new Map<number, TakeLaneView>();
    extra: Extra = {};

    /** How `track` is drawn, or the default when nobody touched it. */
    track(track: Track): TrackView {
        return this.tracks.get(track.ident) ?? new TrackView();
    }

    /**
     * How `track` is drawn, to be edited -- created on first use, which is
     * what makes "nobody has touched it" cost nothing to store.
     */
    trackView(track: Track): TrackView {
        let view = this.tracks.get(track.ident);
        if (!view) {
            view = new TrackView();
            this.tracks.set(track.ident, view);
        }
        return view;
    }

    /** How `lane` is drawn, or the default. */
    takeLane(lane: TakeLane): TakeLaneView {
        return this.takeLanes.get(lane.ident) ?? new TakeLaneView();
    }

    /** How `lane` is drawn, to be edited. See {@link View.trackView}. */
    takeLaneView(lane: TakeLane): TakeLaneView {
        const id = lane.ident;
        let view = this.takeLanes.get(id);
        if (!view) {
            view = new TakeLaneView();
            this.takeLanes.set(id, view);
        }
        return view;
    }

    /**
     * Drops everything this view says about objects the multitrack no longer holds,
     * and answers whether anything went.
     *
     * **State goes when the thing goes.** Keeping it is worse than losing it: a
     * height kept for a track that is not the same track is a defect that looks
     * like a feature.
     */
    prune(multitrack: Multitrack): boolean {
        const held = new Set<number>();
        for (const track of multitrack.tracksWritten()) {
            held.add(num(track.id));
            for (const lane of (track.take_lanes as Extra[] | undefined) ?? []) {
                held.add(num(lane.id));
                for (const region of (lane.regions as Extra[] | undefined) ?? []) held.add(num(region.id));
            }
            for (const curve of (track.automation as Extra[] | undefined) ?? []) held.add(num(curve.id));
        }
        const before = [this.tracks.size, this.takeLanes.size, this.selected.length,
                        this.focused, this.detail].join(",");
        for (const id of [...this.tracks.keys()]) {
            if (!held.has(id)) this.tracks.delete(id);
        }
        for (const id of [...this.takeLanes.keys()]) {
            if (!held.has(id)) this.takeLanes.delete(id);
        }
        this.selected = this.selected.filter((id) => held.has(id));
        if (this.focused !== undefined && !held.has(this.focused)) {
            this.focused = undefined;
        }
        if (this.detail !== undefined && !held.has(this.detail)) {
            this.detail = undefined;
        }
        return before !== [this.tracks.size, this.takeLanes.size,
                           this.selected.length, this.focused,
                           this.detail].join(",");
    }

    /** The view as the crate's JSON. Nothing said is nothing written. */
    write(): Extra {
        const out: Extra = {};
        if (this.name !== undefined) out.name = this.name;
        if (this.visible) out.visible = this.visible.write();
        if (this.scroll) out.scroll = this.scroll;
        if (this.quant) out.quant = this.quant;
        if (!this.autofit) out.autofit = false;
        if (this.selection) out.selection = this.selection.write();
        if (this.selected.length) out.selected = [...this.selected];
        if (this.focused !== undefined) out.focused = this.focused;
        if (this.detail !== undefined) out.detail = this.detail;
        if (this.tracks.size) {
            const table: Extra = {};
            for (const id of [...this.tracks.keys()].sort((a, b) => a - b)) {
                table[String(id)] = this.tracks.get(id)!.write();
            }
            out.tracks = table;
        }
        if (this.takeLanes.size) {
            const table: Extra = {};
            for (const id of [...this.takeLanes.keys()].sort((a, b) => a - b)) {
                table[String(id)] = this.takeLanes.get(id)!.write();
            }
            out.take_lanes = table;
        }
        return { ...out, ...this.extra };
    }

    /** A view from the crate's JSON. */
    static read(written: Extra): View {
        const view = new View();
        if (written.name !== undefined) view.name = String(written.name);
        if (written.visible) view.visible = Span.read(written.visible as Extra);
        view.scroll = num(written.scroll);
        view.quant = num(written.quant);
        view.autofit = written.autofit !== false;
        if (written.selection) view.selection = Span.read(written.selection as Extra);
        view.selected = ((written.selected as number[]) ?? []).map(Number);
        if (written.focused !== undefined) view.focused = num(written.focused);
        if (written.detail !== undefined) view.detail = num(written.detail);
        for (const [id, entry] of Object.entries((written.tracks as Extra) ?? {})) {
            view.tracks.set(Number(id), TrackView.read(entry as Extra));
        }
        for (const [id, entry] of Object.entries((written.take_lanes as Extra) ?? {})) {
            view.takeLanes.set(Number(id), TakeLaneView.read(entry as Extra));
        }
        view.extra = rest(written, "name", "visible", "scroll", "quant",
                          "autofit", "selection", "selected", "focused",
                          "detail", "tracks", "take_lanes");
        return view;
    }
}

/**
 * A session: the multitrack, saved, and where its samples are.
 *
 * An {@link Multitrack} says *what plays when* and deliberately does not say
 * where a source lives, because inside a running system a source is a server
 * buffer, a mapped file or a rendered result and the multitrack has no business
 * knowing which. A session is the multitrack plus exactly that missing half.
 *
 * Not the client's `Session`, which is a connection to a running server. Two
 * nouns, two modules; this one is a **file**.
 */
export class Session {
    /**
     * The format version this build writes -- {@link SESSION_FORMAT}, the crate's
     * `session::FORMAT`. A session **read** in an older format is migrated to
     * this one first ({@link Session.read}), since its numbers are read
     * differently: format 2 placed the multitrack in beats.
     */
    format = SESSION_FORMAT;
    /**
     * The multitrack. Always present, possibly empty -- which mirrors the crate,
     * where an absent arrangement reads as an empty one rather than as nothing.
     */
    multitrack = new Multitrack();
    /**
     * How the multitrack was being **looked at**: one entry per window. Carried for
     * the reason every program in the field carries it -- reopening a multitrack into
     * the window it was left in is what a person expects -- and a reader that
     * ignores it opens the same multitrack.
     */
    views: View[] = [];
    /**
     * **Where a pass over the multitrack ends**, as a playback's `end` says
     * it: `null` (it rolls on, the default), `"contents"` (where the last
     * region ends) or a number of seconds, an end marker. The transport's own
     * switch, kept so a session opened again stops where it stopped before --
     * not in a view, since it changes what is heard. It bounds no axis: how
     * far a window shows or zooms out reads nothing here.
     */
    end: "contents" | number | null = null;
    /**
     * The general tree, for what is not an arrangement. The leg being walked
     * off: a composite region carries that same tree, placed.
     */
    document?: Extra;
    /** Where each source is, keyed by source id. */
    sources = new Map<number, Source>();
    /**
     * What produced the session as a whole -- the scripts behind it -- carried
     * opaquely.
     */
    provenance?: unknown;
    extra: Extra = {};
    /**
     * The file this session was opened from or last saved to, or `null`. Not
     * written into the file: where a session is is not what it says.
     */
    path: string | null = null;

    /**
     * A session read from the file at `path`, remembering where it came from:
     * {@link Session.load} reads relative paths against that file's folder, and
     * {@link Session.save} writes back to it.
     *
     * The path is on the filesystem `Buffer.read` names -- the disk under node,
     * and the page's own storage (`opfs`) in a tab.
     */
    static async open(path: string): Promise<Session> {
        const session = Session.read(JSON.parse(await readText(path)) as Extra);
        session.path = path;
        return session;
    }

    /**
     * Writes the session to `path`, or back to the file it was opened from or
     * last saved to, and answers where it went. Rejects when there is no `path`
     * and the session was never opened or saved.
     */
    async save(path?: string): Promise<string> {
        const target = path ?? this.path;
        if (target === null) {
            throw new Error("clausters: this session has nowhere to save to: give it a path");
        }
        await writeText(target, JSON.stringify(this.write(), null, 1));
        this.path = target;
        return target;
    }

    /** The source a reference names, if the table has it. */
    source(id: number): Source | undefined {
        return this.sources.get(id);
    }

    /**
     * The sources that are sequences of events: source id -> `EventSequence`,
     * the handles the table holds. With what {@link Session.load} answers, the
     * whole table a multitrack editor is opened with.
     */
    sequences(): Map<number, EventSequence> {
        const out = new Map<number, EventSequence>();
        for (const [id, source] of this.sources) {
            if (source.sequence !== undefined) out.set(id, source.sequence);
        }
        return out;
    }

    /**
     * Sources whose samples are not written down anywhere -- what a save
     * consults before promising the file is complete.
     */
    volatile(): number[] {
        return [...this.sources.entries()]
            .filter(([, source]) => !source.isResolvable)
            .map(([id]) => id)
            .sort((a, b) => a - b);
    }

    /** Sources with a destructive edit still open and undecided. */
    openEdits(): number[] {
        return [...this.sources.entries()]
            .filter(([, source]) => source.isBeingEdited)
            .map(([id]) => id)
            .sort((a, b) => a - b);
    }

    /**
     * Sources the multitrack names but the table does not hold -- what an opening
     * reader reports rather than discovering one element at a time.
     *
     * **Every** take lane is walked and not only the ones that play: an alternate
     * take names its source whether or not anyone has chosen it yet.
     */
    dangling(): number[] {
        const missing: number[] = [];
        for (const region of this.multitrack.regions()) {
            const named = (region.content.window ?? {}).source as Extra | undefined;
            const id = named?.source;
            if (typeof id === "number" && !this.sources.has(id) && !missing.includes(id)) {
                missing.push(id);
            }
        }
        return missing;
    }

    /**
     * Promotes a temporary working copy to one saved beside the document,
     * **leaving the edit open**. What a save mid-edit does: auto-confirming
     * would turn a save into an edit, and refusing until the edit is settled
     * would make the safest habit in the program the one that is blocked.
     */
    promote(id: number): boolean {
        const source = this.sources.get(id);
        if (!source || source.lifetime !== "temporary") return false;
        source.lifetime = "session";
        return true;
    }

    /**
     * Confirms the edit open over a source: the working copy becomes the
     * samples, and there is nothing left undecided about it.
     */
    confirm(id: number): boolean {
        const source = this.sources.get(id);
        if (!source || !source.editing) return false;
        source.editing = { ...source.editing, confirmed: true };
        return true;
    }

    /** The session as the crate's JSON. */
    write(): Extra {
        const out: Extra = { format: this.format };
        const multitrack = this.multitrack.write();
        if (Object.keys(multitrack).length) out.multitrack = multitrack;
        if (this.views.length) out.views = this.views.map((v) => v.write());
        if (this.end !== null) out.end = this.end;
        if (this.document !== undefined) out.document = this.document;
        if (this.sources.size) {
            const table: Extra = {};
            for (const id of [...this.sources.keys()].sort((a, b) => a - b)) {
                table[String(id)] = this.sources.get(id)!.write();
            }
            out.sources = table;
        }
        if (this.provenance !== undefined) out.provenance = this.provenance;
        return { ...out, ...this.extra };
    }

    /**
     * A session from the crate's JSON, migrated first when it was written in an
     * older format.
     *
     * The migration is the crate's (`sessionMigrate`), so an old file opens the
     * same here, in the Python client and in the GUI host.
     */
    static read(written: Extra): Session {
        if (num(written.format, 1) < SESSION_FORMAT) {
            written = JSON.parse(sessionMigrate(JSON.stringify(written))) as Extra;
        }
        const session = new Session();
        session.format = num(written.format, 1);
        // `arrangement` is what the field was called before 2026-09-08, read so a
        // session written then still opens.
        session.multitrack = Multitrack.read(
            (written.multitrack as Extra) ?? (written.arrangement as Extra) ?? {});
        session.views = ((written.views as Extra[]) ?? []).map(View.read);
        session.end = (written.end as "contents" | number | null | undefined) ?? null;
        if (written.document !== undefined) session.document = written.document as Extra;
        for (const [id, entry] of Object.entries((written.sources as Extra) ?? {})) {
            session.sources.set(Number(id), Source.read(entry as Extra));
        }
        if (written.provenance !== undefined) session.provenance = written.provenance;
        session.extra = rest(written, "format", "multitrack", "arrangement", "views",
                             "end", "document", "sources", "provenance");
        return session;
    }

    /**
     * Loads the sources the multitrack names into `server`: every take read from its
     * file, and every join stitched from the takes it is made of once those are
     * there.
     *
     * What each source *is* in a running system is not the document's to decide,
     * and loading is the half of reopening a session that says it: a server
     * buffer per source. What is read and in what order is the shared crate's
     * (`editingLoad`), so a session opens the same here, in the Python client
     * and in the GUI host.
     *
     * `beside` is the folder a relative path is read against -- the session
     * file's own, on **the server's** filesystem; when omitted, the folder of
     * the file the session was opened from or saved to, else the current one.
     * The answer maps source id to
     * {@link Buffer} for every source that loaded (a sequence of events takes
     * none: {@link Session.sequences} hands those over); a source that cannot -- a
     * volatile one, one the table does not hold, a join over a take that did not
     * load -- is left out and named in a warning. Rejects when the server refuses
     * a read or a stitch (a file that is not there), after freeing what the load
     * had made.
     */
    async load(
        server?: Server,
        { beside, timeout }: { beside?: string; timeout?: number } = {},
    ): Promise<Map<number, Buffer>> {
        beside ??= this.path === null ? "." : folderOf(this.path);
        const srv = resolveServer(server);
        const aside = [...this.sources.keys()].map(() => srv.buffers.alloc());
        const plan = editingLoad(this.write(), beside, aside);
        if (plan.error !== undefined) {
            for (const bufnum of aside) srv.buffers.free(bufnum);
            throw new Error(`clausters: ${plan.error}`);
        }
        for (const bufnum of plan.unused) srv.buffers.free(bufnum);
        for (const [id, why] of plan.unresolved) {
            console.warn(`clausters: source ${id} is not loadable: ${why}`);
        }
        const buffers = new Map<number, Buffer>();
        for (const [id, take] of Object.entries(plan.takes)) {
            const buffer = new Buffer(take.buffer, take.frames, Math.max(1, take.channels), 0.0, srv);
            if (take.path !== undefined) buffer.path = take.path;
            buffers.set(Number(id), buffer);
        }
        try {
            await runSteps(srv, new StepRunner(), plan.steps, { to: "samples", timeout });
        } catch (error) {
            for (const buffer of buffers.values()) buffer.free();
            throw error;
        }
        // The shape is the file's, so it is read back rather than trusted.
        for (const buffer of buffers.values()) await buffer.info(timeout);
        return buffers;
    }
}

/**
 * One **curve** of a multitrack view: an automation, and where it hangs.
 *
 * The same shape for both places a curve lives, because it is the same curve: a
 * track's automation is drawn as a **row of its own** under that track and runs
 * the whole timeline, a region's is drawn as a **layer inside that box** and
 * runs as long as the box does. `owner` says which -- a track's id for a row, a
 * region's for a layer.
 */
export interface Curve {
    /** The automation it draws -- its identity, and its name on the wire. */
    automation: number;
    /** What it hangs from: a track (a row) or a region (a layer). */
    owner: number;
    label: string;
    /** What it automates, in the page's own terms: the value domain is decided
     * from this, and which domain a parameter has is the page's to know. */
    target?: unknown;
    /** The break-points; `at` is on the musical axis. */
    points?: { at: number; value: number; data?: unknown }[];
    visible: boolean;
    enabled: boolean;
}

/**
 * **A multitrack as the props the multitrack widget is drawn with.**
 *
 * The rows, the boxes, the automations over both, their break-points, which of
 * them are hidden and which boxes loop -- everything a multitrack has from the
 * document alone, in the flat shapes the wire carries. What a caller adds is
 * what is a function of something *else*: the position cursor, the meter buses,
 * the widget's own chrome.
 *
 * `sources` is the same table the instance plan takes, because what a box
 * is drawn from and what it is played from are the same samples.
 */
export function multitrackProps(
    multitrack: string,
    rate: number,
    sources: string,
): string {
    return coreProps(multitrack, rate, sources);
}

/**
 * **What a multitrack calls its rows, its boxes and its curves**, by the names the
 * wire carries
 * them under.
 *
 * The minting correction's half that is a fact about the multitrack: a host that
 * made a track or split a box minted the *word* while the document minted the
 * *id*, so a view keeps what it was last told and answers with the picture when
 * the two stop agreeing. Reading it here rather than striding the props is what
 * keeps a flat array's shape out of a call site.
 */
export function multitrackNames(
    multitrack: Multitrack | unknown,
): { rows: string[]; boxes: string[]; curves: string[] } {
    const body = multitrack instanceof Multitrack ? multitrack.write() : multitrack;
    const answer = coreNames(JSON.stringify(body));
    if (!answer) return { rows: [], boxes: [], curves: [] };
    return JSON.parse(answer) as {
        rows: string[];
        boxes: string[];
        curves: string[];
    };
}

