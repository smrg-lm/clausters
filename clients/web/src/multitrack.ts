/**
 * The multitrack: tracks, lanes, regions, and the timeline they sit on
 * (mirrors `clausters/multitrack.py`).
 *
 * This is the client's side of `clausters_document::multitrack` — the model a
 * multitrack editor edits, and the one the three classic applications (audio
 * editor, multitrack editor, score editor) are built over. The crate defines
 * the format; this module is the idiomatic way to write one and read one back,
 * the same way `./document.ts` is the idiomatic way to reach an edit.
 *
 * The vocabulary is the field's own and not this project's invention:
 *
 * - A **source** is samples. It lives outside the arrangement — the session's
 *   table says where — and is never overwritten.
 * - A {@link Region} is **one placed thing**: a span of the timeline (where it
 *   starts, how long, its fades, which of the overlapping ones is on top) plus
 *   a {@link Content} saying what fills it. Six regions over one source are six
 *   identities and one source, referenced rather than copied. That is the whole
 *   of non-destructive editing.
 * - A {@link Lane} is one of a track's several contents, an ordered list of
 *   regions. Ardour's structure and our name.
 * - A {@link Track} holds several lanes and **plays one**, which is what
 *   comping is: record six passes into six lanes, then take from each.
 * - An {@link Automation} is a curve over one parameter, in the arrangement's
 *   time.
 * - An {@link Multitrack} is the tracks plus what the **piece** has one of:
 *   the tempo map, the meter map, the markers, the loop and punch spans. They
 *   are here and not on a track precisely so that no two tracks can disagree
 *   about them.
 *
 * ## A region is not a clip
 *
 * `Region` is the model's word; **clip** is the picture's. A clip, a lane row,
 * a waveform are what the host draws; a region is what an edit names. Keeping
 * them apart is deliberate — the multitrack's defects came from the thing drawn
 * and the thing addressed being one object.
 *
 * ## Time
 *
 * Everything placed here is placed in **beats**, because where a thing sits in
 * a piece is a musical decision. What fills a region is measured in its own
 * source's units — frames for samples, beats for a node — and the two are not
 * the same axis. The crate makes that a type; here it is a rule the field names
 * say (`position` and `length` are the region's, `start` and `duration` are its
 * window's), and the conversion between them needs the tempo map, which is why
 * the map is part of the piece.
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

import { multitrackPicture as corePicture, multitrackRead as coreRead } from "./core/clausters_core_web.js";
import { FIRST_VERSION } from "./document.ts";

/** Whatever a newer writer wrote and this build has no field for. */
export type Extra = Record<string, unknown>;

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
 * A fade's length in beats, and whatever the client says about its curve.
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
 * evaluation, for a window onto something generated — a function placed twice
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

    constructor(fill: string, fields: Partial<Content> = {}) {
        this.fill = fill;
        this.window = fields.window;
        this.playrate = fields.playrate ?? 1;
        this.args = fields.args;
        this.node = fields.node;
        this.other = fields.other;
    }

    /** A window onto a source. */
    static onto(window: Extra, options: { playrate?: number; args?: unknown } = {}): Content {
        return new Content("window", {
            window,
            playrate: options.playrate ?? 1,
            args: options.args,
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
            });
        }
        if (written.fill === "composite") {
            return new Content("composite", { node: written.node as Extra });
        }
        return new Content(String(written.fill), { other: { ...written } });
    }
}

/**
 * One placed thing on a lane: a span of the timeline, and what fills it.
 *
 * `position` and `length` are the region's own, in beats. They are **not** the
 * content's: a region may show part of what it holds, and trimming moves these
 * without touching the source.
 */
export class Region {
    id: number;
    position: number;
    length: number;
    content: Content;
    name?: string;
    /**
     * Which of the overlapping regions on this lane draws and plays on top.
     * Overlap is legal and ordinary — a crossfade *is* an overlap — so the
     * stack needs an order that survives a save.
     */
    layer: number;
    fadeIn?: Fade;
    fadeOut?: Fade;
    muted: boolean;
    extra: Extra;

    constructor(fields: {
        id: number;
        position: number;
        length: number;
        content: Content;
        name?: string;
        layer?: number;
        fadeIn?: Fade;
        fadeOut?: Fade;
        muted?: boolean;
        extra?: Extra;
    }) {
        this.id = fields.id;
        this.position = fields.position;
        this.length = fields.length;
        this.content = fields.content;
        this.name = fields.name;
        this.layer = fields.layer ?? 0;
        this.fadeIn = fields.fadeIn;
        this.fadeOut = fields.fadeOut;
        this.muted = fields.muted ?? false;
        this.extra = fields.extra ?? {};
    }

    /** Where it ends: its position plus its length. */
    get end(): number {
        return this.position + this.length;
    }

    /**
     * Whether the two occupy any of the same time. Half-open, so a region
     * ending exactly where the next begins does not overlap it — which is what
     * makes a cut into two regions not a crossfade.
     */
    overlaps(other: Region): boolean {
        return this.position < other.end && other.position < this.end;
    }

    write(): Extra {
        const out: Extra = {
            id: this.id,
            position: this.position,
            length: this.length,
            content: this.content.write(),
        };
        if (this.name !== undefined) out.name = this.name;
        if (this.layer) out.layer = this.layer;
        if (this.fadeIn) out.fade_in = this.fadeIn.write();
        if (this.fadeOut) out.fade_out = this.fadeOut.write();
        if (this.muted) out.muted = true;
        return { ...out, ...this.extra };
    }

    static read(written: Extra): Region {
        return new Region({
            id: num(written.id),
            position: num(written.position),
            length: num(written.length),
            content: Content.read((written.content as Extra) ?? {}),
            name: written.name as string | undefined,
            layer: num(written.layer),
            fadeIn: written.fade_in ? Fade.read(written.fade_in as Extra) : undefined,
            fadeOut: written.fade_out ? Fade.read(written.fade_out as Extra) : undefined,
            muted: Boolean(written.muted),
            extra: rest(written, "id", "position", "length", "content", "name",
                        "layer", "fade_in", "fade_out", "muted"),
        });
    }
}

/**
 * One of a track's several contents: an ordered list of regions.
 *
 * Ardour's structure and our name — its *playlist* is this, and that word is
 * spent on something else everywhere. {@link Lane.place} keeps the list in
 * position order, so a re-saved session is stable and a diff of two saves is
 * the edits rather than the iteration order.
 */
export class Lane {
    id: number;
    name?: string;
    regions: Region[];
    extra: Extra;

    constructor(fields: { id: number; name?: string; regions?: Region[]; extra?: Extra }) {
        this.id = fields.id;
        this.name = fields.name;
        this.regions = fields.regions ?? [];
        this.extra = fields.extra ?? {};
    }

    /**
     * Places a region and keeps the lane in position order. Returns it, so a
     * caller can go on holding what it just placed.
     */
    place(region: Region): Region {
        const at = this.regions.findIndex(
            (held) => held.position > region.position
                || (held.position === region.position && held.layer > region.layer),
        );
        if (at < 0) this.regions.push(region);
        else this.regions.splice(at, 0, region);
        return region;
    }

    /** The region with this id, if it is here. */
    region(id: number): Region | undefined {
        return this.regions.find((r) => r.id === id);
    }

    /** Where the last region ends, or zero when there are none. */
    get end(): number {
        return this.regions.reduce((most, r) => Math.max(most, r.end), 0);
    }

    write(): Extra {
        const out: Extra = { id: this.id };
        if (this.name !== undefined) out.name = this.name;
        if (this.regions.length) out.regions = this.regions.map((r) => r.write());
        return { ...out, ...this.extra };
    }

    static read(written: Extra): Lane {
        return new Lane({
            id: num(written.id),
            name: written.name as string | undefined,
            regions: ((written.regions as Extra[]) ?? []).map(Region.read),
            extra: rest(written, "id", "name", "regions"),
        });
    }
}

/**
 * A curve over one parameter, in the arrangement's own time.
 *
 * `target` says **what this automates** in the client's terms and is never read
 * here — a control name, a bus, a plugin's parameter index — the same door a
 * leaf's configuration is, and for the same reason.
 */
export class Automation {
    id: number;
    target?: unknown;
    name?: string;
    points: Extra[];
    /**
     * Whether the lane is shown. The **view's**, and kept here because which
     * curves a person had open is part of reopening the piece as they left it.
     */
    visible: boolean;
    /**
     * Whether the curve is being applied. A curve can be kept and switched off
     * without being deleted, which is what an arm or a bypass is.
     */
    enabled: boolean;
    extra: Extra;

    constructor(fields: {
        id: number;
        target?: unknown;
        name?: string;
        points?: Extra[];
        visible?: boolean;
        enabled?: boolean;
        extra?: Extra;
    }) {
        this.id = fields.id;
        this.target = fields.target;
        this.name = fields.name;
        this.points = fields.points ?? [];
        this.visible = fields.visible ?? false;
        this.enabled = fields.enabled ?? true;
        this.extra = fields.extra ?? {};
    }

    write(): Extra {
        const out: Extra = { id: this.id };
        if (this.name !== undefined) out.name = this.name;
        if (this.target !== undefined) out.target = this.target;
        if (this.points.length) out.points = [...this.points];
        if (this.visible) out.visible = true;
        if (!this.enabled) out.enabled = false;
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
 * A row of the arrangement: several lanes, one of them playing, the curves over
 * it, and whatever the client says it is.
 *
 * **What a track *is* — an instrument, a bus, a folder — is not here.** That is
 * `config`, carried and never interpreted, for the reason a leaf is opaque: a
 * def is code in the language of whoever wrote it. What the document owns is
 * the structure: which lanes, which one plays, what is placed on them.
 */
export class Track {
    id: number;
    name?: string;
    lanes: Lane[];
    /** Which lane plays, as an index into {@link Track.lanes}. */
    active: number;
    automation: Automation[];
    muted: boolean;
    /**
     * Marked as soloed. Whether a solo anywhere silences everything else is the
     * mixer's rule and not the document's.
     */
    soloed: boolean;
    config?: unknown;
    extra: Extra;

    constructor(fields: {
        id: number;
        name?: string;
        lanes?: Lane[];
        active?: number;
        automation?: Automation[];
        muted?: boolean;
        soloed?: boolean;
        config?: unknown;
        extra?: Extra;
    }) {
        this.id = fields.id;
        this.name = fields.name;
        this.lanes = fields.lanes ?? [];
        this.active = fields.active ?? 0;
        this.automation = fields.automation ?? [];
        this.muted = fields.muted ?? false;
        this.soloed = fields.soloed ?? false;
        this.config = fields.config;
        this.extra = fields.extra ?? {};
    }

    /**
     * The lane that plays, or `undefined` when {@link Track.active} names one
     * that is not there.
     */
    get activeLane(): Lane | undefined {
        return this.lanes[this.active];
    }

    /**
     * Where the track's last region ends, across **every** lane — what it spans
     * rather than what it plays, since an alternate take is still part of the
     * piece.
     */
    get end(): number {
        return this.lanes.reduce((most, lane) => Math.max(most, lane.end), 0);
    }

    write(): Extra {
        const out: Extra = { id: this.id };
        if (this.name !== undefined) out.name = this.name;
        if (this.lanes.length) out.lanes = this.lanes.map((lane) => lane.write());
        if (this.active) out.active = this.active;
        if (this.automation.length) out.automation = this.automation.map((a) => a.write());
        if (this.muted) out.muted = true;
        if (this.soloed) out.soloed = true;
        if (this.config !== undefined) out.config = this.config;
        return { ...out, ...this.extra };
    }

    static read(written: Extra): Track {
        return new Track({
            id: num(written.id),
            name: written.name as string | undefined,
            lanes: ((written.lanes as Extra[]) ?? []).map(Lane.read),
            active: num(written.active),
            automation: ((written.automation as Extra[]) ?? []).map(Automation.read),
            muted: Boolean(written.muted),
            soloed: Boolean(written.soloed),
            config: written.config,
            extra: rest(written, "id", "name", "lanes", "active", "automation",
                        "muted", "soloed", "config"),
        });
    }
}

/**
 * One entry of the tempo map: from here on, this tempo.
 *
 * `ramp` says the tempo runs from here to the next entry rather than stepping.
 * A ritardando is a ramp; a section change is a step.
 */
export class Tempo {
    at: number;
    bpm: number;
    ramp: boolean;
    extra: Extra;

    constructor(fields: { at: number; bpm: number; ramp?: boolean; extra?: Extra }) {
        this.at = fields.at;
        this.bpm = fields.bpm;
        this.ramp = fields.ramp ?? false;
        this.extra = fields.extra ?? {};
    }

    write(): Extra {
        const out: Extra = { at: this.at, bpm: this.bpm };
        if (this.ramp) out.ramp = true;
        return { ...out, ...this.extra };
    }

    static read(written: Extra): Tempo {
        return new Tempo({
            at: num(written.at),
            bpm: num(written.bpm),
            ramp: Boolean(written.ramp),
            extra: rest(written, "at", "bpm", "ramp"),
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

/** A named point on the timeline. */
export class Marker {
    id: number;
    at: number;
    name?: string;
    extra: Extra;

    constructor(fields: { id: number; at: number; name?: string; extra?: Extra }) {
        this.id = fields.id;
        this.at = fields.at;
        this.name = fields.name;
        this.extra = fields.extra ?? {};
    }

    write(): Extra {
        const out: Extra = { id: this.id, at: this.at };
        if (this.name !== undefined) out.name = this.name;
        return { ...out, ...this.extra };
    }

    static read(written: Extra): Marker {
        return new Marker({
            id: num(written.id),
            at: num(written.at),
            name: written.name as string | undefined,
            extra: rest(written, "id", "at", "name"),
        });
    }
}

/**
 * A span of the timeline: the loop, the punch, a named region of the piece.
 *
 * Half-open, so two spans that meet cover no beat twice.
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
 * The tracks, and the timeline they are placed on.
 *
 * What is here rather than on a track is what the **piece** has one of: the
 * tempo map, the meter map, the markers, the loop. A track has none of them and
 * never disagrees with another track about them, which is the whole argument
 * for where they live.
 */
export class Multitrack {
    /**
     * What this piece is *at*, and the whole of what a stale edit is stale
     * against — the twin of the document's own version, and deliberately a
     * second counter: an editor of the piece is not editing the tree, so one
     * number would make every edit to either look like a change to both.
     */
    version = FIRST_VERSION;
    tracks: Track[] = [];
    tempo: Tempo[] = [];
    meter: Meter[] = [];
    markers: Marker[] = [];
    loopSpan?: Span;
    punch?: Span;
    extra: Extra = {};

    /** The track with this id. */
    track(id: number): Track | undefined {
        return this.tracks.find((t) => t.id === id);
    }

    /**
     * Where the last region ends, across every track and every lane — how long
     * the piece is.
     */
    get end(): number {
        return this.tracks.reduce((most, t) => Math.max(most, t.end), 0);
    }

    /**
     * Every region, in track then lane then position order — **every** lane,
     * not only the ones that play, because an alternate take still names the
     * source it plays.
     */
    *regions(): Generator<Region> {
        for (const track of this.tracks) {
            for (const lane of track.lanes) yield* lane.regions;
        }
    }

    /**
     * The tempo entry in force at `at`, or `undefined` when the map says
     * nothing.
     *
     * The **entry**, not a converted position: turning a beat into seconds
     * needs the whole map walked and a ramp integrated, and the shape of a ramp
     * is not something the document names.
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
     * — two tempos at one position is a state the map should not hold.
     */
    setTempo(tempo: Tempo): void {
        this.tempo = this.tempo.filter((t) => t.at !== tempo.at);
        this.tempo.push(tempo);
        this.tempo.sort((a, b) => a.at - b.at);
    }

    /** Adds a meter entry, on the same rule. */
    setMeter(meter: Meter): void {
        this.meter = this.meter.filter((m) => m.at !== meter.at);
        this.meter.push(meter);
        this.meter.sort((a, b) => a.at - b.at);
    }

    /**
     * Adds a marker, in position order. Several may share a beat: unlike a
     * tempo, two names for one moment is a thing people do.
     */
    addMarker(marker: Marker): void {
        this.markers.push(marker);
        this.markers.sort((a, b) => a.at - b.at);
    }

    /** The arrangement as the crate's JSON. Nothing said is nothing written. */
    write(): Extra {
        const out: Extra = {};
        // Out of the file while it is the first version, so an unedited piece
        // still writes an empty object: the reader defaults back to the same
        // number, so nothing is lost by leaving it out.
        if (this.version !== FIRST_VERSION) out.version = this.version;
        if (this.tracks.length) out.tracks = this.tracks.map((t) => t.write());
        if (this.tempo.length) out.tempo = this.tempo.map((t) => t.write());
        if (this.meter.length) out.meter = this.meter.map((m) => m.write());
        if (this.markers.length) out.markers = this.markers.map((m) => m.write());
        if (this.loopSpan) out.loop_span = this.loopSpan.write();
        if (this.punch) out.punch = this.punch.write();
        return { ...out, ...this.extra };
    }

    /** An arrangement from the crate's JSON. */
    static read(written: Extra): Multitrack {
        const piece = new Multitrack();
        piece.version = (written.version as number) ?? FIRST_VERSION;
        piece.tracks = ((written.tracks as Extra[]) ?? []).map(Track.read);
        piece.tempo = ((written.tempo as Extra[]) ?? []).map(Tempo.read);
        piece.meter = ((written.meter as Extra[]) ?? []).map(Meter.read);
        piece.markers = ((written.markers as Extra[]) ?? []).map(Marker.read);
        if (written.loop_span) piece.loopSpan = Span.read(written.loop_span as Extra);
        if (written.punch) piece.punch = Span.read(written.punch as Extra);
        piece.extra = rest(written, "version", "tracks", "tempo", "meter",
                           "markers", "loop_span", "punch");
        return piece;
    }
}

// ---- the session: the piece, and where its samples are ----
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
 * `editing`, a destructive edit that has not been confirmed — a save never
 * blocks on a confirmation, so a saved session has to be able to say *this is a
 * working copy of that, and the person has not decided yet*.
 */
export class Source {
    /**
     * `{at: "file", path}` or `{at: "volatile"}`. A relative path is resolved
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
    /** Which generation of the content this is — bumped by a destructive edit. */
    generation: number;
    channels?: number;
    frames?: number;
    /** Carried, never acted on: resampling is an edit. */
    sampleRate?: number;
    provenance?: unknown;
    /** `{from, confirmed}` while a destructive edit is open over these samples. */
    editing?: Extra;
    extra: Extra;

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

    /** Whether the samples are somewhere a reader could find them. */
    get isResolvable(): boolean {
        return this.path !== undefined;
    }

    /** Whether a destructive edit is open and undecided over these samples. */
    get isBeingEdited(): boolean {
        return Boolean(this.editing) && !this.editing!.confirmed;
    }

    write(): Extra {
        const out: Extra = {
            location: this.location,
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
        return new Source({
            location: (written.location as Extra) ?? { at: "volatile" },
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
 * Without it, a piece opened with no way to read its files would be written back
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

// ---- the presentation: what a window shows of a piece ----
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
     * Whether the track's other lanes are shown under the one that plays —
     * comping open, in a word. Closed by default: a track with six takes on it
     * is one row until somebody asks to see them.
     */
    lanesShown = false;
    /** The colour the track is drawn in, carried and never read. */
    color?: string;
    extra: Extra = {};

    write(): Extra {
        const out: Extra = {};
        if (this.height !== undefined) out.height = this.height;
        if (this.collapsed) out.collapsed = true;
        if (this.lanesShown) out.lanes_shown = true;
        if (this.color !== undefined) out.color = this.color;
        return { ...out, ...this.extra };
    }

    static read(written: Extra): TrackView {
        const view = new TrackView();
        if (written.height !== undefined) view.height = num(written.height);
        view.collapsed = written.collapsed === true;
        view.lanesShown = written.lanes_shown === true;
        if (written.color !== undefined) view.color = String(written.color);
        view.extra = rest(written, "height", "collapsed", "lanes_shown", "color");
        return view;
    }
}

/** How one lane is drawn. */
export class LaneView {
    /** How tall its row is when the track's lanes are shown. */
    height?: number;
    extra: Extra = {};

    write(): Extra {
        const out: Extra = {};
        if (this.height !== undefined) out.height = this.height;
        return { ...out, ...this.extra };
    }

    static read(written: Extra): LaneView {
        const view = new LaneView();
        if (written.height !== undefined) view.height = num(written.height);
        view.extra = rest(written, "height");
        return view;
    }
}

/**
 * One window's picture of one piece: where it is looking, how far it is zoomed,
 * what the hand is holding, how tall each track is drawn.
 *
 * None of that is what the piece *is* — a selection and a zoom are each
 * window's and never the composition's — and all of it is state a person loses
 * on a reopen unless something writes it down. A session carries a **list** of
 * these, because a piece drawn in two windows has two views and they disagree
 * on purpose.
 *
 * Nothing here ever reaches the document or the history: a view is not edited
 * through an intent, and an undo never puts a scroll back.
 */
export class View {
    /** What the window is called, when a person named it. */
    name?: string;
    /**
     * The stretch of the timeline on screen — the zoom and the horizontal
     * scroll, which are one fact and not two. Absent shows the whole piece.
     */
    visible?: Span;
    /** How far down the tracks the window is scrolled, in its own units. */
    scroll = 0;
    /**
     * The grid this window snaps to, in beats. Zero snaps nothing. It is here
     * rather than in the piece because two windows over one piece may snap
     * differently — the arranger to a bar, the editor below it to a sixteenth.
     */
    quant = 0;
    /**
     * Whether the window follows its content. `false` says the window is the
     * reader's, and nothing moves it, which is what an editor wants.
     */
    autofit = true;
    /** The time range the hand swept, when it swept one. */
    selection?: Span;
    /**
     * What the hand is holding: regions, lanes or tracks, by id. One list
     * rather than one per kind, because the piece has one id space.
     */
    selected: number[] = [];
    /** What a keystroke is aimed at, which is not the same as what is selected. */
    focused?: number;
    /** The region the detail editor below is showing, when the window has one. */
    detail?: number;
    /** How each track is drawn, by the track's id. */
    tracks = new Map<number, TrackView>();
    /** How each lane is drawn, by the lane's id. */
    lanes = new Map<number, LaneView>();
    extra: Extra = {};

    /** How this track is drawn, or the default when nobody touched it. */
    track(id: number): TrackView {
        return this.tracks.get(id) ?? new TrackView();
    }

    /**
     * How this track is drawn, to be edited — created on first use, which is
     * what makes "nobody has touched it" cost nothing to store.
     */
    trackView(id: number): TrackView {
        let view = this.tracks.get(id);
        if (!view) {
            view = new TrackView();
            this.tracks.set(id, view);
        }
        return view;
    }

    /** How this lane is drawn, or the default. */
    lane(id: number): LaneView {
        return this.lanes.get(id) ?? new LaneView();
    }

    /** How this lane is drawn, to be edited. See {@link View.trackView}. */
    laneView(id: number): LaneView {
        let view = this.lanes.get(id);
        if (!view) {
            view = new LaneView();
            this.lanes.set(id, view);
        }
        return view;
    }

    /**
     * Drops everything this view says about objects the piece no longer holds,
     * and answers whether anything went.
     *
     * **State goes when the thing goes.** Keeping it is worse than losing it: a
     * height kept for a track that is not the same track is a defect that looks
     * like a feature.
     */
    prune(piece: Multitrack): boolean {
        const held = new Set<number>();
        for (const track of piece.tracks) {
            held.add(track.id);
            for (const lane of track.lanes) {
                held.add(lane.id);
                for (const region of lane.regions) held.add(region.id);
            }
            for (const curve of track.automation) held.add(curve.id);
        }
        const before = [this.tracks.size, this.lanes.size, this.selected.length,
                        this.focused, this.detail].join(",");
        for (const id of [...this.tracks.keys()]) {
            if (!held.has(id)) this.tracks.delete(id);
        }
        for (const id of [...this.lanes.keys()]) {
            if (!held.has(id)) this.lanes.delete(id);
        }
        this.selected = this.selected.filter((id) => held.has(id));
        if (this.focused !== undefined && !held.has(this.focused)) {
            this.focused = undefined;
        }
        if (this.detail !== undefined && !held.has(this.detail)) {
            this.detail = undefined;
        }
        return before !== [this.tracks.size, this.lanes.size,
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
        if (this.lanes.size) {
            const table: Extra = {};
            for (const id of [...this.lanes.keys()].sort((a, b) => a - b)) {
                table[String(id)] = this.lanes.get(id)!.write();
            }
            out.lanes = table;
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
        for (const [id, entry] of Object.entries((written.lanes as Extra) ?? {})) {
            view.lanes.set(Number(id), LaneView.read(entry as Extra));
        }
        view.extra = rest(written, "name", "visible", "scroll", "quant",
                          "autofit", "selection", "selected", "focused",
                          "detail", "tracks", "lanes");
        return view;
    }
}

/**
 * A composition, saved: the arrangement, and where its samples are.
 *
 * An {@link Multitrack} says *what plays when* and deliberately does not say
 * where a source lives, because inside a running system a source is a server
 * buffer, a mapped file or a rendered result and the piece has no business
 * knowing which. A session is the piece plus exactly that missing half.
 *
 * Not the client's `Session`, which is a connection to a running server. Two
 * nouns, two modules; this one is a **file**.
 */
export class Session {
    /** The format version. */
    format = 1;
    /**
     * The piece. Always present, possibly empty — which mirrors the crate,
     * where an absent arrangement reads as an empty one rather than as nothing.
     */
    multitrack = new Multitrack();
    /**
     * How the piece was being **looked at**: one entry per window. Carried for
     * the reason every program in the field carries it — reopening a piece into
     * the window it was left in is what a person expects — and a reader that
     * ignores it opens the same piece.
     */
    views: View[] = [];
    /**
     * The general tree, for what is not an arrangement. The leg being walked
     * off: a composite region carries that same tree, placed.
     */
    document?: Extra;
    /** Where each source is, keyed by source id. */
    sources = new Map<number, Source>();
    /**
     * What produced the session as a whole — the scripts behind it — carried
     * opaquely.
     */
    provenance?: unknown;
    extra: Extra = {};

    /** The source a reference names, if the table has it. */
    source(id: number): Source | undefined {
        return this.sources.get(id);
    }

    /**
     * Sources whose samples are not written down anywhere — what a save
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
     * Sources the piece names but the table does not hold — what an opening
     * reader reports rather than discovering one element at a time.
     *
     * **Every** lane is walked and not only the ones that play: an alternate
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
        const piece = this.multitrack.write();
        if (Object.keys(piece).length) out.multitrack = piece;
        if (this.views.length) out.views = this.views.map((v) => v.write());
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

    /** A session from the crate's JSON. */
    static read(written: Extra): Session {
        const session = new Session();
        session.format = num(written.format, 1);
        // `arrangement` is what the field was called before 2026-09-08, read so a
        // session written then still opens.
        session.multitrack = Multitrack.read(
            (written.multitrack as Extra) ?? (written.arrangement as Extra) ?? {});
        session.views = ((written.views as Extra[]) ?? []).map(View.read);
        if (written.document !== undefined) session.document = written.document as Extra;
        for (const [id, entry] of Object.entries((written.sources as Extra) ?? {})) {
            session.sources.set(Number(id), Source.read(entry as Extra));
        }
        if (written.provenance !== undefined) session.provenance = written.provenance;
        session.extra = rest(written, "format", "multitrack", "arrangement", "views",
                             "document", "sources", "provenance");
        return session;
    }
}

/** One row of a multitrack view: a track, and the strip drawn beside it. */
export interface Row {
    /** The track it draws — its identity, and its name on the wire. */
    track: number;
    /** The lane of that track whose regions it shows. */
    lane: number;
    label: string;
    mute: boolean;
    solo: boolean;
    gain: number;
}

/** One box of a multitrack view: a region, on the row that plays it. */
export interface Box {
    region: number;
    row: number;
    /** In **beats**. */
    position: number;
    /** In **beats**, and it is the difference of two positions. */
    length: number;
    /** Where in the source it starts, in **seconds**. */
    start: number;
    /** How much there is to show, in **seconds**. */
    content: number;
    source?: number;
    label: string;
    muted: boolean;
}

/** A box as a hand left it, for {@link multitrackRead} to make sense of. */
export interface Placed {
    /** The region's id, or a name no region has — which is how a **new** box is
     * told from a moved one, since a split names its halves after the box they
     * came from. */
    name: string;
    row: number;
    position: number;
    length: number;
    start: number;
    content: number;
    source?: number;
}

/**
 * **The rows and boxes a piece draws as** — the multitrack view's own mapping,
 * and there is one of it (`clausters_document::multitrack::picture`), so a page,
 * the Python client and the standalone host draw the same picture of the same
 * piece rather than each deriving one.
 *
 * **In beats and seconds.** A timeline axis counts sample frames and the crate
 * has no tempo function; a page crosses to its own axis with the tempo-map calls
 * it already binds, and a *length* is the difference of two positions there.
 * `source` is the document's source id and not a server buffer: which buffer a
 * source was read into is the page's own table.
 */
export function multitrackPicture(piece: unknown): { rows: Row[]; boxes: Box[] } {
    const answer = corePicture(JSON.stringify(piece));
    return answer
        ? (JSON.parse(answer) as { rows: Row[]; boxes: Box[] })
        : { rows: [], boxes: [] };
}

/**
 * **What a report of a multitrack's boxes means**, in the piece's own
 * vocabulary.
 *
 * The reader every multitrack view needs and none should write: the report is
 * the *piece* rather than the gesture, so a move, a block drag, a trim, a split,
 * a delete and a paste all arrive as one list, and telling them apart is one
 * rule written once.
 */
export function multitrackRead(piece: unknown, placed: readonly Placed[]): unknown[] {
    const answer = coreRead(JSON.stringify(piece), JSON.stringify(placed));
    if (!answer) return [];
    return ((JSON.parse(answer) as { intents?: unknown[] }).intents ?? []);
}
