/**
 * The arrangement: tracks, lanes, regions, and the timeline they sit on
 * (mirrors `clausters/arrangement.py`).
 *
 * This is the client's side of `clausters_document::arrangement` — the model a
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
 * - An {@link Arrangement} is the tracks plus what the **piece** has one of:
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
export class Arrangement {
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
        if (this.tracks.length) out.tracks = this.tracks.map((t) => t.write());
        if (this.tempo.length) out.tempo = this.tempo.map((t) => t.write());
        if (this.meter.length) out.meter = this.meter.map((m) => m.write());
        if (this.markers.length) out.markers = this.markers.map((m) => m.write());
        if (this.loopSpan) out.loop_span = this.loopSpan.write();
        if (this.punch) out.punch = this.punch.write();
        return { ...out, ...this.extra };
    }

    /** An arrangement from the crate's JSON. */
    static read(written: Extra): Arrangement {
        const piece = new Arrangement();
        piece.tracks = ((written.tracks as Extra[]) ?? []).map(Track.read);
        piece.tempo = ((written.tempo as Extra[]) ?? []).map(Tempo.read);
        piece.meter = ((written.meter as Extra[]) ?? []).map(Meter.read);
        piece.markers = ((written.markers as Extra[]) ?? []).map(Marker.read);
        if (written.loop_span) piece.loopSpan = Span.read(written.loop_span as Extra);
        if (written.punch) piece.punch = Span.read(written.punch as Extra);
        piece.extra = rest(written, "tracks", "tempo", "meter", "markers",
                           "loop_span", "punch");
        return piece;
    }
}
