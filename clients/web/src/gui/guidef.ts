// Building GuiDefs the way defs are built (mirrors `clausters/gui/guidef.py`).
//
// A GuiDef is the GUI analogue of a `SynthDef`/`GraphDef`: a tree of
// `{id, type, ...props, children}` nodes serialized to JSON and carried inside
// one OSC argument. These helpers compose that tree as plain objects — they
// are **host-agnostic**, just as building a `SynthDef` is server-agnostic;
// only `GuiHost` knows how to send one. The root node carries no `id` (it
// comes from the `/gui_def <id>` argument); every child carries its own.
//
// **The option names are this language's, the props are the wire's.** Each
// builder takes camelCase options (`textSize`, `baseBucket`, `selStart`) and
// writes the host's snake_case props — the same split the def builders keep,
// so the JSON is identical to the Python client's while the surface reads as
// TypeScript. A prop this client does not know yet (a newer host's) can be
// passed straight through under its wire name.
//
// **Ids and names.** Widget ids live in one namespace per host, across all
// windows. Leave `id` out and `GuiHost.open`/`define` assigns a host-unique
// one, writing it into your object in place; or pass small ints yourself (the
// allocator starts at 1000, so hand-picked ids below that never collide).
// Better still, pass `name: "cutoff"` to any builder and address the widget by
// that name through the window handle — the name is **client-only** and is
// stripped from the JSON by `toJson`.
//
// **Numbers.** JSON from JavaScript has one number type, so `480` and `480.0`
// serialize the same; the host reads every continuous prop as a float and
// every id/index prop as an integer, so the tree means the same thing it does
// from Python.

import { Env, envToPoints, pointsToEnv, resolveCurve } from "../defs/ugens/index.ts";
import type { Curve } from "../defs/ugens/index.ts";
import { samplesToBlob } from "../base/bulk.ts";
import type { GuiHost, PropValue, Stage } from "./host.ts";
import type { WindowHandle } from "./handle.ts";

export { Env, envToPoints, pointsToEnv };
export type { Curve };

/**
 * One node of a GuiDef tree: its `type`, its props, and (for a container)
 * its `children`. `name` is the client-only handle name, never on the wire.
 *
 * Every builder returns a {@link View}, which is this shape with a name index
 * and an `open` on it; the interface stays because a hand-written object
 * literal is still a legal tree everywhere one is taken.
 */
export interface GuiNode {
    type: string;
    id?: number;
    name?: string;
    children?: GuiNode[];
    [prop: string]: unknown;
}

/**
 * Inline `data` ceiling: at most this many floats ride the GuiDef JSON.
 * Anything longer travels as a **blob** beside it — the bulk path, where the
 * samples are bytes rather than JSON numbers. The same number the Python
 * client uses, so a source of the same length makes the same decision in both.
 */
export const INLINE_MAX = 2048;

/**
 * The sample prop names a {@link Source} may stand in for: the logical one
 * (`data`, "the samples") and the carriers themselves, so a tree written the
 * long way can adopt a source without moving the option.
 */
export const SOURCE_PROPS = ["data", "blob", "path", "cache", "buffer"] as const;

/**
 * The **structure** prop names a {@link Source} may stand in for — the heavy
 * props that carry a payload rather than a scalar and are not samples. Each
 * rides in the prop it is named by, so unlike the sample carriers there is
 * nothing to choose: what the source adds is that the payload stays
 * addressable after the definition is written. `STRUCTURES` (below, beside the
 * flat wire forms) says how each one normalizes.
 */
export const STRUCTURE_PROPS = [
    "points", "notes", "osc", "boxes", "cords", "display_list",
] as const;

/** How a payload travels: a sample carrier, or the structure prop it is. */
export type Carrier = (typeof SOURCE_PROPS)[number] | (typeof STRUCTURE_PROPS)[number];

/**
 * The payload a view draws, as something you hold — and can change.
 *
 * A widget's heavy props are the ones that carry a **payload rather than a
 * scalar**, and there are two families of them. The samples of a signal view,
 * which the wire spells several ways — `data` (a small array inline in the
 * JSON), `blob` (an index into the message's trailing binary arguments), `path`
 * (a mapped raw-f32 file, native hosts only), `cache` (a prebuilt peak
 * pyramid), `buffer` (a server buffer number) — where which one is right is a
 * question about **size and where the samples already are**, not about what is
 * being drawn, and `blob: 0` in particular is a correspondence kept by hand
 * between the widget and the `open` call that had better pass that blob first.
 * And the **structures**: a curve's `points`, a roll's `notes` and `osc`, a
 * patcher's `boxes` and `cords`, a score's `displayList` — each of which rides
 * in its own prop and has exactly one way to travel.
 *
 * A `Source` answers it and stays addressable:
 *
 * ```ts
 * const sig = source(samples);
 * const v = view({ title: "a take" }, waveform({ name: "wave", data: sig }));
 * const win = await v.open();
 *
 * sig.set(otherSamples);      // the definition and every open view follow
 *
 * const env = source(undefined, { points: [[0.0, 0.0], [1.0, 1.0, "exp"]] });
 * await view({}, bpf({ points: env })).open();
 * env.set([[0.0, 1.0], [1.0, 0.0]]);        // the same, for a structure
 * ```
 *
 * It is the same relation a `control` has with a knob: the entry point named
 * once, and referred to instead of copied. One source in two views is **one
 * payload and two references**, which is what makes the blobs interchangeable —
 * said by the program rather than by convention, and the index assigned for
 * you.
 *
 * **The carrier is decided once**, when the source is made. For samples it is
 * decided from what it first holds: a short array stays inline, a long one
 * becomes a blob, and `set` writes through that same carrier because a widget
 * already on screen was built around it. For a structure the carrier is the
 * **prop itself**: it is normalized to its flat wire form on the way into a
 * definition and on the way out of {@link Source.set}, which sends the whole
 * list (or the engraved page) through the `/gui_set` door that prop already
 * has.
 */
export class Source {
    readonly #carrier: Carrier;
    #value: unknown;
    /** Whether the payload is a structure (its own prop) rather than samples. */
    readonly #structure: boolean;
    readonly #fixed: Props;
    /**
     * `{node, prop}` for every node this source was placed in — the definitions
     * it feeds, rewritten by `set` so a later `open` sends what it holds now.
     */
    readonly #bound: { node: GuiNode; prop: string }[] = [];
    /**
     * `{host, id}` for every live widget drawing it, so `set` reaches what is
     * already on screen. An entry leaves when the host recycles the widget.
     */
    readonly #live: { host: GuiHost; id: number }[] = [];

    constructor(
        samples?: ArrayLike<number> | Iterable<number> | null,
        {
            buffer,
            path,
            cache,
            points,
            notes,
            osc,
            boxes,
            cords,
            displayList,
            channels,
            sampleRate,
            baseBucket,
        }: SourceInput = {},
    ) {
        const held = ([
            ["points", points], ["notes", notes], ["osc", osc], ["boxes", boxes],
            ["cords", cords], ["display_list", displayList],
        ] as const).filter(([, v]) => v !== undefined && v !== null);
        const named = [samples, buffer, path, cache].filter((x) => x !== undefined && x !== null);
        if (named.length + held.length !== 1) {
            throw new TypeError(
                "a source names its payload exactly one way: an iterable of " +
                    "floats (or buffer/path/cache for samples that already live " +
                    "somewhere the host can reach), or one of " +
                    `${STRUCTURE_PROPS.join("/")} for a structure that rides in ` +
                    "its own prop",
            );
        }
        this.#fixed = drop([
            ["channels", channels],
            ["sample_rate", sampleRate],
            ["base_bucket", baseBucket],
        ]);
        this.#structure = held.length > 0;
        if (this.#structure && Object.keys(this.#fixed).length > 0) {
            throw new TypeError(
                "channels/sampleRate/baseBucket describe samples, and this " +
                    `source carries a ${held[0]![0]} structure`,
            );
        }
        if (this.#structure) {
            this.#carrier = held[0]![0];
            this.#value = held[0]![1];
            this.props(); // normalized now, so a malformed structure is caught here
        } else if (buffer !== undefined && buffer !== null) {
            this.#carrier = "buffer";
            this.#value = buffer;
        } else if (path !== undefined && path !== null) {
            this.#carrier = "path";
            this.#value = path;
        } else if (cache !== undefined && cache !== null) {
            this.#carrier = "cache";
            this.#value = cache;
        } else {
            const values = [...(samples as Iterable<number>)].map(Number);
            if (values.length <= INLINE_MAX) {
                this.#carrier = "data";
                this.#value = values;
            } else {
                this.#carrier = "blob";
                this.#value = samplesToBlob(values);
            }
        }
    }

    /**
     * How this payload travels: for samples `"data"`, `"blob"`, `"path"`,
     * `"cache"` or `"buffer"`; for a structure the **prop itself**
     * (`"points"`, `"notes"`, `"osc"`, `"boxes"`, `"cords"`,
     * `"display_list"`), which is the only way it can travel. Decided when the
     * source is made and fixed for its life.
     */
    get carrier(): Carrier {
        return this.#carrier;
    }

    /** The blob bytes this source rides in, for a `blob` carrier. @internal */
    get bytes(): Uint8Array | null {
        return this.#carrier === "blob" ? (this.#value as Uint8Array) : null;
    }

    /**
     * The wire props this source expands to — what a builder puts into the node
     * in place of the prop it was passed as. A `blob` source expands to its
     * fixed props only: the index is the position its bytes take in the
     * `/gui_def` message, which `GuiHost.open` assigns. A structure is
     * normalized here, so a definition carries the same flat form it would have
     * carried written out by hand.
     */
    props(): Props {
        if (this.#structure) return STRUCTURES[this.#carrier]!.props(this.#value);
        if (this.#carrier === "blob") return { ...this.#fixed };
        return { [this.#carrier]: this.#value, ...this.#fixed } as Props;
    }

    /**
     * The node keys this source's expansion occupies, so a rewrite clears
     * exactly what the last one wrote — the five carriers for samples (they are
     * one slot spelled five ways), the prop's own keys for a structure.
     */
    slots(): readonly string[] {
        return this.#structure ? STRUCTURES[this.#carrier]!.slots : SOURCE_PROPS;
    }

    /** @internal Records a node this source feeds, so `set` rewrites it. */
    bindTo(node: GuiNode, prop: string): void {
        this.#bound.push({ node, prop });
    }

    /** @internal Records a live widget drawing this source. */
    addLive(host: GuiHost, id: number): void {
        this.#live.push({ host, id });
    }

    /** @internal Forgets a widget the host recycled. */
    dropLive(host: GuiHost, id: number): void {
        for (let i = this.#live.length - 1; i >= 0; i--) {
            const held = this.#live[i]!;
            if (held.host === host && held.id === id) this.#live.splice(i, 1);
        }
    }

    /**
     * Point this source at another payload: the **definitions** that hold it
     * are rewritten, and every widget already drawing it is told to redraw.
     *
     * A view open twice is updated twice — the payload belongs to the
     * definition, so both instances follow; the per-instance door stays
     * `win.widget("wave").set(...)`.
     *
     * A **structure** always takes this: it is normalized the way its builder
     * would have normalized it, and rides out through the `/gui_set` door its
     * prop already has (a flat list as the JSON string a scalar-only wire
     * needs, an engraved page as the whole `display_list`). Of the sample
     * carriers only the two that **hold** the samples take it — the inline
     * `data` and the `blob` a long payload spills to, which are this platform's
     * pair for the reference client's `data` and `path`. The carrier is fixed
     * when the source is made, there and here: a widget on screen was built
     * around it. A source that *names* samples somebody else owns (a server
     * buffer, a cache) is a reference: change them where they live and call
     * {@link Source.reload}.
     */
    set(payload: ArrayLike<number> | Iterable<number> | unknown): this {
        if (this.#structure) {
            this.#value = payload;
            const props = this.props();
            for (const { node } of this.#bound) rewriteSource(node, props, this.slots());
            const live = this.liveProps();
            for (const { host, id } of [...this.#live]) host.set(id, live);
            return this;
        }
        const samples = payload as ArrayLike<number> | Iterable<number>;
        if (this.#carrier !== "data" && this.#carrier !== "blob") {
            throw new TypeError(
                `this source names a ${this.#carrier}, which it does not own — ` +
                    "change the samples where they live and call reload()",
            );
        }
        const values = [...(samples as Iterable<number>)].map(Number);
        if (this.#carrier === "data" && values.length > INLINE_MAX) {
            throw new RangeError(
                `${values.length} samples do not fit the inline carrier this ` +
                    `source was made with (at most ${INLINE_MAX}). A source's ` +
                    "carrier is fixed when it is made, because a widget on screen " +
                    "was built around it — make this one from a long array, so it " +
                    "spills from the start",
            );
        }
        // A blob is rewritten **as itself**, which is what the reference client
        // does with the file it spilled to: same carrier, new bytes, and every
        // widget drawing it is pushed the samples that now hold.
        this.#value = this.#carrier === "blob" ? samplesToBlob(values) : values;
        for (const { node } of this.#bound) rewriteSource(node, this.props(), this.slots());
        const live: Record<string, PropValue> = this.#carrier === "blob"
            ? { data: this.#value as Uint8Array }
            : { data: values };
        for (const { host, id } of [...this.#live]) host.set(id, live);
        return this;
    }

    /**
     * Tell every widget drawing this source to read it again — the samples are
     * where they were and they moved (a server buffer recorded into, a cache
     * rebuilt, a file rewritten from outside).
     */
    reload(): this {
        if (this.#structure) {
            throw new TypeError(
                `a ${this.#carrier} source holds its own payload, so there is ` +
                    "nowhere for it to have moved — call set() with the structure " +
                    "it should draw now",
            );
        }
        for (const { host, id } of [...this.#live]) host.set(id, { reload: 1 });
        return this;
    }

    /**
     * What a `/gui_set` carries for this structure. The same props a definition
     * takes, except for an engraved page: the wire spells the page as five
     * props in a definition and as the one `display_list` live, which is the
     * host's door for replacing a drawing in place.
     */
    private liveProps(): Record<string, PropValue> {
        if (this.#carrier === "display_list") {
            // `props()` is already the drawing layers, which is exactly what a
            // live page replaces — the client-side keys of a display list (its
            // `notes`) never ride the wire, here or in a definition.
            return { display_list: this.props() } as Record<string, PropValue>;
        }
        return this.props() as Record<string, PropValue>;
    }
}

/** What a {@link Source} may be made of: samples, or one structure. */
export interface SourceInput {
    /** A server buffer number the host reads the samples from. */
    buffer?: number;
    /** A raw little-endian f32 file the host maps (native hosts only). */
    path?: string;
    /** A prebuilt peak pyramid. */
    cache?: string;
    /** A `bpf`'s break-points. */
    points?: PointSpec;
    /** A roll's notes. */
    notes?: NoteSpec;
    /** A roll's OSC markers. */
    osc?: OscMarkSpec;
    /** A patcher's boxes. */
    boxes?: readonly unknown[];
    /** A patcher's cords. */
    cords?: readonly number[];
    /** An engraved `score` page. */
    displayList?: Record<string, unknown>;
    /** De-interleaves the samples into this many channels. */
    channels?: number;
    /** The samples' rate, which gives a time ruler its unit. */
    sampleRate?: number;
    /** The peak pyramid's base bucket. */
    baseBucket?: number;
}

/**
 * The payload a view draws, as a {@link Source} — held, referred to, and
 * changed in place of being copied into a prop.
 *
 * For **samples**, give it an iterable of floats, or name samples that already
 * live somewhere the host can reach: `buffer` (a server buffer number), `path`
 * (a raw little-endian f32 file it maps) or `cache` (a prebuilt peak pyramid).
 * `channels` de-interleaves it, `sampleRate` gives the time ruler its unit and
 * `baseBucket` sizes the peak pyramid.
 *
 * For a **structure**, name the prop it is: `points` (a `bpf`'s break-points),
 * `notes` / `osc` (a roll's notes and markers), `boxes` / `cords` (a
 * patcher's boxes and wires) or `displayList` (an engraved `score` page). It
 * takes the same form the builder's own option takes, is normalized the same
 * way, and {@link Source.set} replaces it live:
 *
 * ```ts
 * const page = source(undefined, { displayList: engraved });
 * await view({}, score({ displayList: page, editable: true })).open();
 * page.set(reEngraved);              // after applying an edit
 * ```
 */
export function source(
    samples?: ArrayLike<number> | Iterable<number> | null,
    options: SourceInput = {},
): Source {
    return new Source(samples, options);
}

/**
 * Put `props` into `node` in place of the keys the source it belongs to
 * occupies — its {@link Source.slots}, not every key a source could ever
 * write, so one heavy prop's source never clears another's.
 */
function rewriteSource(node: GuiNode, props: Props, slots: readonly string[]): void {
    for (const key of slots) delete node[key];
    Object.assign(node, props);
}

/** The node types that scope the names inside them (a nested view). */
const SCOPES = new Set(["window"]);

/**
 * One node of a GuiDef tree, as the thing a program composes and then opens —
 * the GUI's counterpart of a `SynthDef`.
 *
 * Built by the builders in this module, never directly: `knob(...)`,
 * `layout(a, b)` and `view(...)` all return one. Composition is nesting, and
 * the object *is* the document — its own properties are what
 * `JSON.stringify` writes, so nothing on the wire changes:
 *
 * ```ts
 * const v = view({}, layout({ flow: "col" }, knob({ name: "freq" }), slider({ name: "amp" })));
 * v.find("freq").min = 110.0;          // still the plain node underneath
 * const w = v.open();                  // a live window
 * w.widget("freq").set({ value: 440.0 });
 * ```
 *
 * `v.type` and `v["min"]` are the document, as they have always been; the
 * *name* index is {@link View.find} / {@link View.names}, which keeps the two
 * addressings from colliding. On the live side the index is the name
 * (`w.widget("freq")`), because a `WindowHandle` has no document to index.
 */
export class View implements GuiNode {
    declare type: string;
    declare id?: number;
    declare name?: string;
    declare children?: GuiNode[];
    [prop: string]: unknown;

    /**
     * `name -> node` for this view's own scope, built once at construction
     * from the children's already-built scopes — so composing a tree costs one
     * pass over each node, not one per lookup.
     */
    readonly #scope: Map<string, GuiNode>;

    /**
     * The def control this widget was built from (`knob(freq)`), so
     * `WindowHandle.bind` knows what it drives. Client-side only; the name is
     * all that reaches the wire.
     */
    #control: unknown = null;

    /**
     * The {@link Source} objects feeding this node's props, so opening it can
     * tell each which live widget draws it — and, for a blob source, where its
     * bytes went in the message. Client-side only.
     */
    readonly #sources: { prop: string; source: Source }[] = [];

    /** Build a view from the document `props`, in the order they were written. */
    constructor(props: Record<string, unknown>) {
        Object.assign(this, props);
        this.#scope = View.#scopeOf(this);
    }

    /**
     * `name -> node` for one node's scope: every named descendant, stopping
     * the descent at a nested view (which is registered by its own name and
     * keeps the names inside it).
     *
     * The node's *own* name is not in its scope — a view is not found inside
     * itself; it is found in the scope of whatever contains it. A child built
     * as a `View` already has its scope, so a tree costs one pass per node.
     */
    static #scopeOf(node: GuiNode): Map<string, GuiNode> {
        const scope = new Map<string, GuiNode>();
        for (const child of node.children ?? []) {
            const inner = child instanceof View ? child.#scope : View.#scopeOf(child);
            if (typeof child.name === "string" && child.name) {
                View.#claim(scope, child.name, child);
            }
            if (SCOPES.has(child.type)) continue; // a nested view keeps its names
            for (const [innerName, innerNode] of inner) View.#claim(scope, innerName, innerNode);
        }
        return scope;
    }

    /** Record `name -> node`, refusing a name already taken in this scope. */
    static #claim(scope: Map<string, GuiNode>, name: string, node: GuiNode): void {
        if (scope.has(name)) {
            throw new Error(
                `duplicate widget name "${name}" in one view — a name is how this ` +
                    "client addresses a widget, so two widgets cannot share one. " +
                    "Rename one, or put them in nested views, which scope their names.",
            );
        }
        scope.set(name, node);
    }

    /**
     * The names reachable in this view's scope, in tree order. A nested view
     * contributes its own name, not the names inside it.
     */
    names(): string[] {
        return [...this.#scope.keys()];
    }

    /**
     * The named widget in this view's scope.
     *
     * Throws if nothing carries that name here — including when the name is
     * inside a nested view, which is a scope of its own:
     * `v.find("osc1").find("freq")`.
     */
    find(name: string): GuiNode {
        const found = this.#scope.get(name);
        if (found === undefined) {
            const here = this.names().join(", ") || "none";
            throw new Error(`no widget named "${name}" in this view (names here: ${here})`);
        }
        return found;
    }

    /** The def control this widget was built from, or `null`. */
    get control(): unknown {
        return this.#control;
    }

    /** @internal Records the control a widget builder read its props off. */
    setControl(control: unknown): this {
        this.#control = control;
        return this;
    }

    /** The sources feeding this node's props. @internal */
    get sources(): readonly { prop: string; source: Source }[] {
        return this.#sources;
    }

    /** @internal Records a source this node's prop was built from. */
    addSource(prop: string, held: Source): this {
        this.#sources.push({ prop, source: held });
        return this;
    }

    /** This view as the GuiDef document `/gui_def` takes (names stripped). */
    toJson(): string {
        return toJson(this);
    }

    /**
     * Open this view on a GUI host and return its `WindowHandle`.
     *
     * The resource is the subject: `view(...).open()` rather than
     * `host.open(view(...))`. `host` follows the ambient rule every other
     * visual verb follows (`plot`, `scope`) — the one registered with
     * `setAmbientHost`, else the current or default session's host when one is
     * up, else a host the ambient layer opens on this page and owns.
     *
     * `element` is where a page draws it: the view takes that element's box,
     * and the canvas inside it is made for you. It is the browser's own
     * argument — the Python client's `View.open` has no counterpart for it,
     * because a script gets an OS window — and a host reached over a socket
     * refuses one for the same reason. A page that names none gets a canvas of
     * its own, appended to the document: *a view with no element is a canvas*,
     * which is how a page finishes *a view with no parent is a window*.
     *
     * It is `async` because the page's host boots asynchronously (the core
     * wasm, the GPU device) — the one difference from the Python client's
     * `View.open`, which has a process to talk to and nothing to await.
     */
    async open(
        element?: Stage | null,
        {
            id,
            blobs = [],
            host,
        }: { id?: number; blobs?: readonly Uint8Array[]; host?: GuiHost } = {},
    ): Promise<WindowHandle> {
        const target = host ?? (await (await import("../plot.ts")).resolveHost());
        return target.open(this, { id, blobs, element });
    }
}



/**
 * The options every widget takes: the client-side `id`/`name`, the place
 * props the container's layout applies (all logical pixels, all live via
 * `set`), the leaf style prop, and any wire prop this client does not name.
 */
export interface WidgetOptions {
    /** The widget's id; omitted, `GuiHost.open`/`define` assigns one. */
    id?: number;
    /**
     * A client-only handle name — `win.widget("cutoff")` — stripped from the
     * JSON.
     */
    name?: string;
    /**
     * A fixed main-axis size in a `row`/`col` (`w` in a row, `h` in a col);
     * in a `free` container, the widget's size.
     */
    w?: number;
    h?: number;
    /**
     * The share of the leftover a child takes in a `row`/`col`, and the way
     * to stretch a control past the size it asks for.
     *
     * The main axis resolves in one order: a fixed `w`/`h`, else an explicit
     * `weight`, else the widget's **natural size** (how big that kind of
     * widget wants to be — a control knows, a view does not), else a share of
     * the leftover at weight 1. The cross axis always fills. A natural size
     * follows the host's sizing table, never the widget's data.
     */
    weight?: number;
    /**
     * The position inside a `free` container (a child with none of these
     * overlays the whole area).
     */
    x?: number;
    y?: number;
    /**
     * One `"#rrggbb[aa]"` re-seeding the roles that carry this widget's
     * function: the accent family, the trace, a series' first color, a clip's
     * body. An empty string clears it.
     */
    color?: string;
    /**
     * How opaque this widget draws, `0`–`1`. Like a theme group it is a
     * **group's** property: it multiplies down the whole subtree, so a control
     * at `0.5` inside a panel at `0.5` draws at `0.25`. A negative number
     * clears it.
     *
     * It fades the flat drawing — the chrome, the controls and the text. A
     * heavy view's picture (a waveform's trace, a spectrogram's texture, a
     * `canvas` shader) is drawn by its own pipeline and keeps its own opacity.
     */
    opacity?: number;
    /**
     * The corner radius of the boxes this widget draws, in logical pixels.
     * Unlike `opacity` it applies to this widget alone — a rounded panel says
     * nothing about the controls in it. Each box clamps it to half its shorter
     * side, so the widget's own frame rounds while the hairlines inside it (a
     * divider, a tick, a track edge) keep their shape. A negative number
     * clears it.
     */
    radius?: number;
    /**
     * A **container's** gesture table: what a drag on it does, by modifier
     * modifier (`drag` for the plain drag, `shift`, `ctrl`, `alt`), each value an
     * ordered plan of steps — `element` (hand the press to whatever is under
     * the cursor: a clip, a note, a box; it may decline), `pan`, `select`,
     * `locate`, `none`.
     *
     * Panning, sweeping a selection and locating the transport belong to the
     * coordinate system a container gives its contents, which is why
     * Shift+drag pans the same way over a `waveform`, a `track` lane, a
     * `pianoroll` and a `timeruler`. A plan that consumes nothing falls
     * outward to the container around it; a table names only the modifiers it
     * changes (`{ drag: "pan", shift: "select" }`), and the vertical strip of
     * a view always pans that axis whatever the table says. The steps are
     * `element`, `pan`, `select` (the time span), `select_box` (the same sweep
     * restricted to the band of values it covered, declining where the picture
     * measures only time), `locate` and `none`.
     */
    gestures?: Record<string, string>;
    [prop: string]: unknown;
}

/**
 * A container's own options: how it places its children, and the theme group
 * it opens over its whole subtree.
 */
export interface ContainerOptions extends WidgetOptions {
    /** `"row"`, `"col"`, `"grid"` or `"free"`. */
    layout?: string;
    /** The inset before the children (default 6). */
    margin?: number;
    /** The space between children (default 6). */
    gap?: number;
    /** A fixed `grid` column count (default near-square). */
    cols?: number;
    /**
     * A partial color-role table (`{"role": "#rrggbb[aa]"}`) overlaying the
     * parent's theme for the whole subtree — a **theme group**, recursive by
     * construction. An empty table clears it.
     */
    theme?: Record<string, string>;
    children?: readonly GuiNode[];
}

/**
 * The chrome every timeline view shares: the rulers, the selection, the
 * playhead and the shared navigation group.
 */
export interface TimelineOptions extends WidgetOptions {
    /** The time ruler: `"time"`, `"samples"`, `"beats"` or `"off"`. */
    ruler?: string;
    /**
     * **The labelled points on the time axis**: `[time, label, color]` triples
     * (label and colour optional), drawn as an **arrow into the ruler's
     * ticks** — never a line down the picture, which is what a playhead and a
     * selection band are. **Ctrl+click** on the ruler adds one, numbered, or
     * removes the one under the pointer, and a **click** on one puts the
     * transport at the exact time it was placed at rather than at the pixel
     * the hand landed on. **What is clicked is the arrow**, with the usual
     * slop around it: the label is text on the tick row, and making a word the
     * target would give a marker called `intro` ten times the reach of one
     * called `2`. An edit flows back as a flat `"markers"` event (`time label
     * color …`): the time and the text are what the owner is handed, and what
     * it keeps against its own document.
     */
    markers?: MarkerSpec;
    /** Labels clock time, and places a spectral frequency axis. */
    sampleRate?: number;
    /** Musical time: beats per second, the beat at sample 0, beats per bar. */
    tempo?: number;
    beatAt?: number;
    quant?: number;
    /**
     * The time selection: `selLen` is a **count of samples** and `selStart` the
     * first of them. The host snaps both, set from here or swept with the
     * pointer, so a selection never stands between two samples; a sweep takes
     * the samples it passed over, so one joins when the cursor reaches it.
     */
    selStart?: number;
    selLen?: number;
    /**
     * The selection's **second axis**: the band of values it is restricted to,
     * in the view's own domain (`min`/`max`), or an empty/inverted pair — the
     * default — for no restriction. A sweep with height sets them and reports
     * them as two further arguments of the `"selection"` event; a sweep along
     * one height leaves them alone and reports the two numbers it always did.
     */
    selMin?: number;
    selMax?: number;
    /**
     * The engine sample-clock value at timeline position 0 — the playhead
     * sweeps on its own from there (negative = none).
     */
    playheadAt?: number;
    /**
     * A **static** playhead: where the transport's cursor stands while
     * nothing is sweeping (negative = none). `playheadAt` wins while it is
     * set, so a transport parks the line here when it pauses or locates.
     */
    playhead?: number;
    /**
     * The sweep's **loop region**, in the same sample units as `playhead`:
     * with a positive length the swept line wraps inside it instead of running
     * straight past, which is what a looping playback does — so a looped
     * region is followed on the same one anchor, still with no message per
     * frame. A non-positive length is the straight pass.
     */
    playheadLoopStart?: number;
    playheadLoopLen?: number;
    /** The vertical display window (normalized; `0, 1` is the full axis). */
    yStart?: number;
    yLen?: number;
    /**
     * The shared navigation group: views declaring the same id zoom, pan,
     * select and locate as one (negative unlinks).
     */
    link?: number;
    /**
     * Whether this view's window **follows its content**. The default (`true`,
     * and what every view did before there was a switch) refits a window that
     * was showing the whole timeline when the content changes, so a view that
     * grows goes on showing all of it — right for a monitor.
     *
     * `false` says the window is the **reader's**: the extent is still
     * registered, so the axis knows how far it can go, and nothing moves it.
     * That is what an editor wants, because there the content change is mostly
     * the reader's own edit — undoing a trim, splitting a clip, dragging one
     * onto another lane — and an edit that re-frames the view is the window
     * starting over under the hand that made it. It is the axis' own property,
     * so **one view asking to be left alone leaves the whole navigation group
     * alone**.
     */
    autofit?: boolean;
    /**
     * The axis pair written the long way, for a property this client does not
     * name flat yet — `{ x: { unit: "beats", tempo: 2.0 }, y: { bit_depth: 16 } }`.
     * Merged over the flat options per axis, so what it names wins.
     */
    axes?: AxisPair;
}

/** Where a heavy view's samples come from, in the host's precedence order. */
export interface SourceOptions extends WidgetOptions {
    // Every carrier prop also takes a {@link Source}: the logical `data` is
    // where one usually goes, and the carriers themselves accept it so a tree
    // written the long way can adopt a source without moving the option.
    /**
     * A prebuilt peak-pyramid file (fetched in the browser); the most compact
     * bulk path — the raw samples are never loaded.
     */
    cache?: string | Source;
    /** A file of raw little-endian `f32` samples the host maps (fetches). */
    path?: string | Source;
    /** A server buffer number, pulled over the host's client leg. */
    buffer?: number | Source;
    /** A short signal inline in the JSON. */
    data?: readonly number[] | Source;
    /**
     * The index of a binary blob carried beside the JSON (see
     * `samplesToBlob` and `GuiHost.define`).
     */
    blob?: number | Source;
    /**
     * The interleaved channel count of `path`/`data`/`blob` (default 1);
     * every channel is kept and drawn.
     */
    channels?: number;
}

/**
 * A props object under wire names, with the options that were left out
 * dropped — the shape every builder assembles.
 */
export type Props = Record<string, unknown>;

/** The given `[wireKey, value]` pairs, minus the ones left `undefined`. */
function drop(pairs: readonly (readonly [string, unknown])[]): Props {
    const out: Props = {};
    for (const [key, value] of pairs) {
        if (value !== undefined) out[key] = value;
    }
    return out;
}

/**
 * A boolean as the `1`/`0` the wire carries (OSC and the host have no bool),
 * or `undefined` when it was not given.
 */
function flag(value: boolean | undefined): number | undefined {
    return value === undefined ? undefined : value ? 1 : 0;
}

/**
 * A ruler switch: a named strip (`"time"`, `"hz"`, `"off"`, …) or a boolean
 * shorthand, as the scope-family widgets accept it.
 */
function strip(value: boolean | string | undefined): string | number | undefined {
    if (value === undefined) return undefined;
    return typeof value === "string" ? value : value ? 1 : "off";
}

/**
 * A `plot`'s `view` option as the model's presentation name: the static plot
 * spelled the same choice its own way, which is the clearest single sign that
 * the six view names were points of one product all along.
 */
const PLOT_VIEW: Record<string, string> = { signal: "trace", spectrum: "spectrum" };

/**
 * The axis pair a two-axis container's chrome belongs to, written the long
 * way. Every builder that takes the chrome flat also takes this, and what is
 * named here **wins** over the flat option describing the same property — the
 * flat keywords are the shorthand, this is the pair itself.
 */
export interface AxisPair {
    x?: Props;
    y?: Props;
}

/**
 * The axis pair `{x, y}` the chrome of a two-axis container belongs to, as
 * the one `axes` prop it rides under (or nothing, when neither side was
 * named). `x`/`y` are already the free-placement props, which is why the pair
 * nests rather than sitting bare on the node.
 *
 * `given` is the caller's own `axes` argument, merged over the flat chrome per
 * axis: an axis it names contributes its properties on top of the ones the
 * flat options wrote, and an axis it leaves out is the flat one untouched.
 */
function axes(x: Props, y: Props, given?: AxisPair): Props {
    const out: Props = {};
    const merged = {
        x: { ...x, ...(given?.x ?? {}) },
        y: { ...y, ...(given?.y ?? {}) },
    };
    if (Object.keys(merged.x).length > 0) out.x = merged.x;
    if (Object.keys(merged.y).length > 0) out.y = merged.y;
    return Object.keys(out).length > 0 ? { axes: out } : {};
}

/**
 * The children of a container, as a plain array (or absent when there are
 * none — an empty `children` key would be noise on the wire).
 */
function kids(children: readonly GuiNode[] | undefined): GuiNode[] | undefined {
    return children && children.length > 0 ? [...children] : undefined;
}

/**
 * A generic widget node `{id?, type, ...props, children?}` — the building
 * block every other builder wraps, and the escape hatch for a widget type
 * this client does not name yet. Everything but `id`/`name`/`children` is a
 * property, kept verbatim under the key you write.
 */
export function node(
    type: string,
    options: { id?: number; name?: string; children?: readonly GuiNode[] } & Props = {},
): View {
    const { id, name, children, ...props } = options;
    const held = new Map<string, Source>();
    for (const key of [...SOURCE_PROPS, ...STRUCTURE_PROPS]) {
        const value = props[key];
        if (value instanceof Source) {
            held.set(key, value);
            delete props[key];
        }
    }
    for (const [key, value] of Object.entries(props)) {
        if (value instanceof Source) {
            throw new TypeError(
                `${type}: a source names a view's payload, so it goes in a prop ` +
                    `that is one of ${[...SOURCE_PROPS, ...STRUCTURE_PROPS].join(", ")}` +
                    ` — not "${key}"`,
            );
        }
    }
    const out: Record<string, unknown> = { type };
    if (id !== undefined) {
        if (!Number.isInteger(id)) {
            throw new TypeError(
                `widget id must be an integer, got ${String(id)} — omit it to ` +
                    "let GuiHost.open assign one",
            );
        }
        out.id = id;
    }
    if (name !== undefined) out.name = name;
    Object.assign(out, props);
    const list = kids(children);
    if (list) out.children = list;
    const built = new View(out);
    for (const [key, value] of held) {
        rewriteSource(built, value.props(), value.slots());
        built.addSource(key, value);
        value.bindTo(built, key);
    }
    return built;
}

// ---- containers ----

// A GuiDef names three kinds of thing: a **container** owning 0, 1 or 2 axes,
// an **element** drawn against them, and a **control**, which is an element
// with a value and no axis. The four builders here name that model; the ones
// below (`panel`, `waveform`, `track`, ...) are shortcuts that build the same
// nodes with a familiar name and the props of one common case.

/**
 * A container with **no axes**, arranging its children by `flow`:
 * `"row"`, `"col"` (the default), `"grid"`, `"free"` — or `"stack"`, which
 * shows one child at a time, the one `index` names, and lays out and draws
 * none of the others. A stack is not a different container: it is this one
 * with a selection instead of an arrangement.
 */
export function layout(
    options: ContainerOptions & {
        /** With `flow: "stack"`, the child shown (from 0). */
        index?: number;
        /** The arrangement; `layout` is accepted as its old name. */
        flow?: string;
    /**
     * Size to the content instead of to the share the layout offers: a `row`
     * adds its children up along its axis and takes the tallest across it, a
     * `col` the other way round, a `grid` counts its cells. The question
     * reaches the whole subtree, and an axis a child leaves elastic (a plane,
     * a lane, a heavy view) is one the container hands back.
     */
    hug?: boolean;
    } = {},
    ...children: GuiNode[]
): GuiNode {
    const { flow, index, layout: arrangement, margin, gap, cols, hug, theme, ...rest } = options;
    return node("layout", {
        ...rest,
        ...drop([
            ["flow", flow ?? arrangement],
            ["index", index],
            ["margin", margin],
            ["gap", gap],
            ["cols", cols],
            ["hug", flag(hug)],
            ["theme", theme],
        ]),
        children: [...(options.children ?? []), ...children],
    });
}

/**
 * A container with **two axes locked to one scale**: a pannable, zoomable
 * plane in content units. `axis`/`zoom` constrain it (see `scroll`), and with
 * `boxes`/`cords` it is the **patcher** — the boxes are what the plane places
 * and the cords the wires between them, which is all `patch` ever added.
 */
export function plane(
    options: ContainerOptions & {
        axis?: string;
        zoom?: boolean;
        contentW?: number;
        contentH?: number;
        viewX?: number;
        viewY?: number;
        viewZoom?: number;
        flow?: string;
        boxes?: readonly unknown[] | Source;
        cords?: readonly number[] | Source;
    } = {},
    ...children: GuiNode[]
): GuiNode {
    const {
        axis, zoom, contentW, contentH, viewX, viewY, viewZoom, boxes, cords,
        flow, layout: arrangement, margin, gap, cols, theme, ...rest
    } = options;
    return node("plane", {
        ...rest,
        ...drop([
            ["axis", axis],
            ["zoom", flag(zoom)],
            ["content_w", contentW],
            ["content_h", contentH],
            ["view_x", viewX],
            ["view_y", viewY],
            ["view_zoom", viewZoom],
            ["boxes", held(boxes, (v) => [...v])],
            ["cords", held(cords, flatCords)],
            ["flow", flow ?? arrangement],
            ["margin", margin],
            ["gap", gap],
            ["cols", cols],
            ["theme", theme],
        ]),
        children: [...(options.children ?? []), ...children],
    });
}

/**
 * A container with **two independent axes** — the time/value container.
 *
 * One container, told apart by what is on it: holding other fields it is a
 * **lane** (with the header options), carrying `offset`/`dur` it is a **clip**
 * placed on its parent's x axis, and a bare strip of a given `h` with nothing
 * on it is the free-standing **ruler** over its navigation group. `track`,
 * `clip` and `timeruler` are those three cases.
 *
 * `axes` is the pair the chrome belongs to — on `x`: `unit`
 * (`"time"`/`"samples"`/`"beats"`/`"off"`), `start`/`len`, `tempo`/`beatAt`
 * as `beat_at`/`quant`, `sample_rate`, `link`, `sel_start`/`sel_len` and the
 * playhead family; on `y`: `unit`, `start`/`len`, `min`/`max`, `bit_depth`.
 */
export function field(
    options: WidgetOptions & {
        axes?: AxisPair;
        offset?: number;
        dur?: number;
        label?: string;
        height?: number;
        snap?: number;
        headerW?: number;
        mute?: boolean;
        solo?: boolean;
        level?: number;
        h?: number;
        theme?: Record<string, string>;
        children?: readonly GuiNode[];
    } = {},
    ...children: GuiNode[]
): GuiNode {
    const {
        axes: pair, offset, dur, label: text, height, snap, headerW,
        mute, solo, level, theme, ...rest
    } = options;
    return node("field", {
        ...rest,
        ...(pair === undefined ? {} : { axes: pair }),
        ...drop([
            ["offset", offset],
            ["dur", dur],
            ["label", text],
            ["height", height],
            ["snap", snap],
            ["header_w", headerW],
            ["mute", mute],
            ["solo", solo],
            ["level", level],
            ["theme", theme],
        ]),
        children: [...(options.children ?? []), ...children],
    });
}

/**
 * **Every view of a signal**, as the one element they are: a presentation of
 * a source, with the capabilities offered over it.
 *
 * `view` is the presentation — `"trace"` (the default), `"spectrum"`,
 * `"spectrogram"` or `"phase"`. The source is either `bus` (with `rate`),
 * read forward-only, or the addressable `data`/`blob`/`buffer`/`path`/`cache`,
 * which is what lets a view navigate, slice and select. `navigable`,
 * `selectable` and `editable` are the capabilities over it. So
 * `signal({ view: "trace", path: take })` is the heavy waveform and
 * `signal({ view: "trace", bus: 0 })` the oscilloscope. Over
 * `view: "spectrum"` the navigable axis is **frequency**, not time: it is a
 * window the element carries alone (`axes: { x: { start, len } }`, normalized
 * over `[0, Nyquist]`, reported as `"view_x"`) and joins no navigation group,
 * and it is the one view where `navigable` is off unless asked for.
 *
 * The presentation's own parameters (`fft_size`/`window_size`, `hop`,
 * `db_floor`/`db_ceil`, `freq_scale`, `colormap`, `window_ms`, `trigger`,
 * `hold`, `averaging`, `peak_hold`) ride through under their wire names;
 * `waveform`, `plot`, `scope`, `spectrum`, `spectrogram` and `phasescope`
 * name and document the six common points of the product.
 */
export function signal(
    options: SourceOptions & {
        view?: string;
        bus?: number;
        rate?: "audio" | "control";
        /**
         * Seconds of history the host keeps of a `bus` (0 = none, the
         * default). A forward-only source has no addressable past, which is
         * what stops it being navigable: there is nothing behind the newest
         * window to zoom out to. This supplies one, so
         * `signal({ view: "spectrogram", bus: 0, retention: 8, navigable: true })`
         * is a **waterfall** — eight seconds of live spectrum you can zoom and
         * pan like a file. It is a policy of the axis, not of the drawing: the
         * same seconds mean the same seconds at any frame rate, FFT size or
         * hop, and a `GuiHost.set` of it resizes the history live.
         */
        retention?: number;
        baseBucket?: number;
        navigable?: boolean;
        /**
         * The samples are **being written into as they are drawn** — a take you
         * are recording. The view draws it up to the buffer's write frontier
         * and leaves the axis past it empty, rather than drawing a flat line
         * across the buffer's own zeros: past the frontier there is no
         * silence, there is nothing yet. The host cannot infer it — a
         * frontier alone does not tell a recording from a loaded take one
         * write touched — so the client that allocated the buffer says so.
         * Clear it when the take is finished.
         */
        fills?: boolean;
        selectable?: boolean;
        editable?: boolean;
        overlay?: boolean;
        /**
         * **What the picture measures**: `"peak"` (the default, the min/max
         * envelope the signal reached), `"rms"` (the symmetric body of the
         * level it held), or both as one space-separated string —
         * `"peak rms"`, the classic editor picture, the level drawn inside the
         * envelope. A factor of the view rather than a widget of its own: one
         * body, drawn once per measure by the one renderer, which is what keeps
         * the axis, the ruler, the selection and the upload single. A peak
         * cache built before the measure existed draws no body rather than
         * zeros.
         */
        measure?: "peak" | "rms" | "peak rms" | "rms peak";
        /**
         * **Inside a `clip`**, and only there: where on the clip's own time
         * this body sits (`at`) and how much of it it covers (`dur`). A clip
         * holding three segments of three files holds three takes, each over
         * its own third. A body that names neither fills the clip.
         */
        at?: number;
        dur?: number;
        /**
         * **Inside a `clip`**: this body's own window onto its buffer — the
         * source frame it reads from, and whether that window wraps. A body
         * that names neither reads through the clip's own window, which is
         * every take written as a clip prop.
         */
        start?: number;
        loop?: boolean;
        axes?: AxisPair;
        label?: string;
    } = {},
): GuiNode {
    const {
        view, cache, path, buffer, data, blob, channels, bus, rate, retention,
        baseBucket, navigable, selectable, editable, overlay, measure, fills, at, dur,
        start, loop, axes: pair, label: text, ...rest
    } = options;
    return node("signal", {
        ...rest,
        ...(pair === undefined ? {} : { axes: pair }),
        ...sourceProps({ cache, path, buffer, data, blob, channels }),
        ...drop([
            ["view", view],
            ["bus", bus],
            ["rate", rate],
            ["retention", retention],
            ["base_bucket", baseBucket],
            ["navigable", flag(navigable)],
            ["fills", flag(fills)],
            ["selectable", flag(selectable)],
            ["editable", flag(editable)],
            ["overlay", flag(overlay)],
            ["measure", measure],
            ["fills", flag(fills)],
            ["at", at],
            ["dur", dur],
            ["start", start],
            ["loop", flag(loop)],
            ["label", text],
        ]),
    });
}


/**
 * A view's **root**: a container that becomes an OS window (a canvas, in the
 * browser) when nothing holds it, and an ordinary component when something
 * does. It takes no id — a root's id is the `/gui_def` argument.
 *
 * There is one node type, not two. A view with no parent is a window, so this
 * is the container to reach for when the thing being built is the whole of
 * what a window shows; nested in another view it is a panel that **scopes its
 * names** ({@link View.find}), which is what lets two copies of one strip each
 * hold their own `freq`.
 *
 * Any node opens ({@link View.open}): `knob({}).open()` is a window that is a
 * knob. Use `view()` when the window's own properties matter — a title, a
 * size, a theme — since a root that is not one is framed in a window that hugs
 * whatever it holds.
 *
 * `w`/`h` size the OS window (the canvas, in the browser); `layout` places
 * the children, tuned by `margin`/`gap`/`cols`. A fixed-height bar over a
 * weighted content area over a fixed status strip — the application shell —
 * is just `view({ layout: "col" }, bar({ h: 28 }), content(), status({ h: 20 }))`.
 *
 * `window` is the older spelling of this builder and still works.
 */
export function view(
    options: ContainerOptions & {
        title?: string;
        flow?: string;
    /**
     * Size to the content instead of to the share the layout offers: a `row`
     * adds its children up along its axis and takes the tallest across it, a
     * `col` the other way round, a `grid` counts its cells. The question
     * reaches the whole subtree, and an axis a child leaves elastic (a plane,
     * a lane, a heavy view) is one the container hands back.
     */
    hug?: boolean;
    } = {},
    ...children: GuiNode[]
): View {
    const { title, flow, layout, margin, gap, cols, hug, theme, ...rest } = options;
    return node("window", {
        ...rest,
        ...drop([
            ["title", title],
            ["flow", flow ?? layout],
            ["margin", margin],
            ["gap", gap],
            ["cols", cols],
            ["hug", flag(hug)],
            ["theme", theme],
        ]),
        children: [...(options.children ?? []), ...children],
    });
}

/**
 * The older spelling of {@link view}, kept because it is what every tree
 * written before the root rule says. One node type, one builder.
 */
export const window = view;

/**
 * A nestable `panel` container. As a child it takes the same place props as
 * any widget; `theme` makes it a theme group over its whole subtree.
 */
export function panel(
    options: ContainerOptions & {
        flow?: string;
    /**
     * Size to the content instead of to the share the layout offers: a `row`
     * adds its children up along its axis and takes the tallest across it, a
     * `col` the other way round, a `grid` counts its cells. The question
     * reaches the whole subtree, and an axis a child leaves elastic (a plane,
     * a lane, a heavy view) is one the container hands back.
     */
    hug?: boolean;
    } = {},
    ...children: GuiNode[]
): GuiNode {
    const { flow, layout, margin, gap, cols, hug, theme, ...rest } = options;
    return node("layout", {
        ...rest,
        ...drop([
            ["flow", flow ?? layout],
            ["margin", margin],
            ["gap", gap],
            ["cols", cols],
            ["hug", flag(hug)],
            ["theme", theme],
        ]),
        children: [...(options.children ?? []), ...children],
    });
}

/**
 * A `stack` container showing **one child at a time**: the one at `index`.
 *
 * The shown page fills the container (`margin` insets it); the hidden ones are
 * not laid out and not drawn, so a page costs nothing while it is away — but
 * they stay in the tree, so a heavy view keeps its GPU slot across a switch and
 * comes back without re-uploading anything.
 *
 * `index` is live via `set`, and it is the prop a control **binds** to: a
 * toggle or a menu bound to it (`GuiHost.bindWidget`, or an inline
 * `bind: ["widget", stackId, "index"]`) flips the page with no round-trip
 * through this script — which is what makes tabs, a pager and a
 * waveform/spectrogram switch composition rather than widgets. An `index`
 * outside the children shows nothing: a blank page rather than a clamped one.
 */
export function stack(
    options: WidgetOptions & {
        /** The child shown, from 0 (the default). */
        index?: number;
        /** The inset before the shown page (default 6). */
        margin?: number;
        /** A theme group over the whole subtree, hidden pages included. */
        theme?: Record<string, string>;
        /**
         * Size to the **largest** page rather than to the shown one, so
         * flipping a pager does not resize it.
         */
        hug?: boolean;
        children?: readonly GuiNode[];
    } = {},
    ...children: GuiNode[]
): GuiNode {
    const { index, margin, hug, theme, ...rest } = options;
    return node("layout", {
        ...rest,
        flow: "stack",
        ...drop([
            ["index", index],
            ["margin", margin],
            ["hug", flag(hug)],
            ["theme", theme],
        ]),
        children: [...(options.children ?? []), ...children],
    });
}

/**
 * A `scroll` container: a 2D workspace onto a virtual content area.
 *
 * The children lay out into a content area larger than the widget, seen
 * through a window that pans and zooms — dragging the empty plane pans it,
 * the wheel zooms anchored at the cursor. The constrained scroll views are
 * this same widget configured down: `{ axis: "y", zoom: false }` is a plain
 * vertical scroll view, `{ axis: "x", zoom: false }` a horizontal strip, the
 * default the free plane. `layout` defaults to `"free"` here, so a child's
 * `x`/`y`/`w`/`h` place it in **content units**.
 */
export function scroll(
    options: ContainerOptions & {
        /** The arrangement inside the content area; `layout` is its old name. */
        flow?: string;
        /** `"both"` (the default), `"x"` or `"y"`. */
        axis?: string;
        /** The wheel zoom (on by default). */
        zoom?: boolean;
        /** The content area, when the children's extents should not size it. */
        contentW?: number;
        contentH?: number;
        /**
         * The view state: the content coordinates at the widget's top-left
         * corner, and physical pixels per content unit. Live via `set`, and
         * emitted as `"view" x y zoom` when a gesture moves them.
         *
         * Omitting `viewZoom` is not the same as passing `1`: a plane with no
         * zoom of its own starts at the **display's scale**, so one content unit
         * is one logical pixel and the boxes come up the size they are meant to
         * look. Pass a number (or turn the wheel) and it is literal from then
         * on; `set({viewZoom: 0})` clears it again — how a script says "back to
         * the default" for a number it cannot name.
         */
        viewX?: number;
        viewY?: number;
        viewZoom?: number;
    } = {},
    ...children: GuiNode[]
): GuiNode {
    const {
        axis, zoom, contentW, contentH, viewX, viewY, viewZoom,
        flow, layout, margin, gap, cols, theme, ...rest
    } = options;
    return node("plane", {
        ...rest,
        ...drop([
            ["axis", axis],
            ["zoom", flag(zoom)],
            ["content_w", contentW],
            ["content_h", contentH],
            ["view_x", viewX],
            ["view_y", viewY],
            ["view_zoom", viewZoom],
            ["flow", flow ?? layout],
            ["margin", margin],
            ["gap", gap],
            ["cols", cols],
            ["theme", theme],
        ]),
        children: [...(options.children ?? []), ...children],
    });
}

// ---- the light controls ----

/**
 * Static `label` text. `textSize` is the glyph scale over the host's font
 * (default 2.0 — every text-bearing widget takes it; a host drawing with its
 * embedded 5x7 face quantizes it to half-steps, one built with a rasterizer
 * takes it as sent); `wrap` word-
 * wraps to the label's width (off, an overflowing line clips with an
 * ellipsis); `align` places each line: `"start"` (the default), `"center"` or
 * `"end"`.
 */
export function label(
    text = "",
    options: WidgetOptions & { textSize?: number; wrap?: boolean; align?: string } = {},
): GuiNode {
    const { textSize, wrap, align, ...rest } = options;
    return node("label", {
        ...rest,
        text,
        ...drop([["text_size", textSize], ["wrap", flag(wrap)], ["align", align]]),
    });
}

/** The options the continuous controls share: a range, a value and a label. */
export interface RangeOptions extends WidgetOptions {
    label?: string;
    min?: number;
    max?: number;
    /**
     * The bend of the range the handle travels: `0` (the default) is linear,
     * negative spends most of the range on the first half of the travel and
     * positive on the last half — the fine-at-the-bottom feel a frequency or
     * an amplitude control wants. The same bend `lincurve` runs, read by the
     * host out of the shared core.
     */
    curve?: number;
    /**
     * The grid a **drag** lands on, in the value's own units: `1` over
     * `0..127` is the integers a MIDI note number wants, and a FaustDef's
     * parameter arrives with the one its `hslider` declared. Counted from
     * `min` and never past `max`: a grid that does not divide the range
     * (`0..10` by `3`) stops on the last whole step, `9`, rather than on an
     * off-grid `10`, and a reversed range (`min > max`) steps from its own
     * `min` downward. A value *you* send is drawn as sent — the step is a rule
     * about the hand, not a constraint on the document.
     */
    step?: number;
    value?: number;
    textSize?: number;
}

function rangeProps(options: RangeOptions): [Props, Props] {
    const { label: text, min, max, curve, step, value, textSize, ...rest } = options;
    return [
        rest,
        drop([
            ["label", text],
            ["min", min],
            ["max", max],
            ["curve", curve],
            ["step", step],
            ["value", value],
            ["text_size", textSize],
        ]),
    ];
}


/**
 * Anything a control widget can be built from: the graph's own `Control`
 * object, or the `ControlInfo` every def family answers with
 * (`sd.control("freq")`, `fd.control("cutoff")`, `gd.control("mix")`).
 */
export interface ControlLike {
    name: string;
    default: number;
    /**
     * A `ControlInfo`'s range, which only a **Faust** parameter fills — read
     * structurally, because on a graph `Control` `min`/`max` are the binary
     * operators and naming the fields here would collide with them.
     */
    range?: [number, number] | null;
    step?: number | null;
    /**
     * The control type the def declared (`"kr"`, `"tr"`, `"ir"`), which
     * {@link button} reads: only a trigger returns to zero on its own, so only
     * a trigger can be driven by a button that sends nothing on release.
     */
    rate?: string | null;
}

/**
 * Whether `x` is a control rather than an option bag.
 *
 * Both carry a `name` — a control's is the one the server addresses, an option
 * bag's is the handle index — so the tell is `default`, which every control has
 * (a `Control` object and a `ControlInfo` alike) and no widget option is. In
 * Python the two are told apart by position, one positional and the rest
 * keywords; TypeScript has one positional slot, so it reads the shape.
 */
function isControl(x: unknown): x is ControlLike {
    return x !== null && typeof x === "object"
        && typeof (x as ControlLike).name === "string"
        && typeof (x as ControlLike).default === "number";
}

/**
 * A control's own props for a widget built from it: its name and its default as
 * the value — plus a range only where the control genuinely has one.
 *
 * The **entry point named once**, for what a def actually knows: what the
 * control is called (which is what `/node_set` addresses) and what it starts at.
 *
 * **The range is the widget's**, and it is spelled on the call:
 * `knob(freq, { min: 110.0, max: 880.0 })`. A control is a signal in a graph and
 * a GraphDef port is a name the server takes any float for; neither says how a
 * knob should be drawn. The one control that arrives with a range is a **Faust**
 * parameter, because `hslider(label, init, min, max, step)` cannot be written
 * without one and the compiled DSP reports it back — Faust's syntax showing
 * through, not a range this client declares.
 *
 * Explicit options win: the control says what it is, the call says how to draw
 * it. That includes `name`, which is the handle's index — the two are usually
 * the same string and need not be.
 */
function fromControl(
    control: ControlLike,
    given: Props,
    { needsRange }: { needsRange: boolean },
): Props {
    // Only a number counts: on a graph `Control` `min`/`max` are the binary
    // operators, so reading them finds a function rather than a range.
    const info = control as { min?: unknown; max?: unknown };
    const [lo, hi] = control.range
        ?? (typeof info.min === "number" ? [info.min, info.max as number] : [undefined, undefined]);
    if (needsRange && lo === undefined && given.min === undefined) {
        throw new Error(
            `control '${control.name}' has no range to be drawn over — spell one ` +
                `on the widget (knob(${control.name}, { min: …, max: … })). Only a ` +
                "FaustDef's parameter brings its own, from the hslider that " +
                "declared it",
        );
    }
    const own = drop([
        ["name", control.name],
        ["label", control.name],
        ["value", control.default],
        ["min", lo],
        ["max", hi],
        ["step", control.step ?? undefined],
    ]);
    for (const [key, value] of Object.entries(given)) {
        if (value !== undefined) own[key] = value;
    }
    return own;
}

/**
 * Splits a control widget's arguments: `knob(freq, { … })` or `knob({ … })`.
 * Returns the control (or `null`) and the option bag.
 */
function controlArgs<T>(
    first: ControlLike | T | undefined,
    second: T | undefined,
): [ControlLike | null, T] {
    if (first !== undefined && isControl(first)) {
        return [first, (second ?? {}) as T];
    }
    return [null, ((first ?? second ?? {}) as T)];
}

/**
 * A rotary `knob` over a continuous range.
 *
 * Takes a def's control first — `knob(freq)`, `knob(sd.control("freq"))` — and
 * reads its **name** and its **default** off it, so the widget and the graph
 * cannot disagree about what `"freq"` is. The **range is the widget's**:
 * `knob(freq, { min: 110.0, max: 880.0 })`. Only a Faust parameter arrives with
 * one, and an option still wins over it.
 */
export function knob(control?: ControlLike | RangeOptions, options?: RangeOptions): View {
    const [source_, opts] = controlArgs<RangeOptions>(control, options);
    const [rest, props] = rangeProps(opts);
    // `rest` last: the control says what it is, the call says how to draw it,
    // and `name` is the one key both carry (the handle's index against the
    // control's own).
    const built = source_ === null
        ? { ...rest, ...props }
        : { ...fromControl(source_, props, { needsRange: true }), ...rest };
    return node("knob", built).setControl(source_);
}

/**
 * A continuous `slider`. `vertical` lays it out along the y axis (min at the
 * bottom) instead of horizontally.
 *
 * Takes a def's control first, like {@link knob}.
 */
export function slider(
    control?: ControlLike | (RangeOptions & { vertical?: boolean }),
    options?: RangeOptions & { vertical?: boolean },
): View {
    const [source_, opts] = controlArgs<RangeOptions & { vertical?: boolean }>(control, options);
    const { vertical, ...plain } = opts;
    const [rest, props] = rangeProps(plain);
    const built = source_ === null
        ? { ...rest, ...props }
        : { ...fromControl(source_, props, { needsRange: true }), ...rest };
    // Only a vertical slider says so: the host reads the prop's absence as
    // horizontal, and the Python builder emits nothing for a false one.
    return node("slider", {
        ...built, ...drop([["vertical", vertical || undefined]]),
    }).setControl(source_);
}

/** A draggable numeric read-out over a range. Takes a control, like {@link knob}. */
export function number(control?: ControlLike | RangeOptions, options?: RangeOptions): View {
    const [source_, opts] = controlArgs<RangeOptions>(control, options);
    const [rest, props] = rangeProps(opts);
    const built = source_ === null
        ? { ...rest, ...props }
        : { ...fromControl(source_, props, { needsRange: true }), ...rest };
    return node("number", built).setControl(source_);
}

/**
 * A `button`'s options: which primitive reaches the server, and the two values
 * it sends.
 */
export interface ButtonOptions extends WidgetOptions {
    label?: string;
    /**
     * Which pointer primitive reaches the server: `"gate"` (the default) sends
     * `on` at the press and `off` at the release, `"press"` sends `on` and
     * nothing after it.
     */
    mode?: "gate" | "press";
    /** What the press sends, and what the release sends under `"gate"`. */
    on?: number;
    off?: number;
    textSize?: number;
}

/**
 * A push `button`, whose **press is the event**.
 *
 * `mode` says which of the two pointer primitives reaches the server:
 *
 * - `"gate"` (the default) sends `on` at the press and `off` when the button is
 *   let go, so the value lasts exactly as long as the button is held — what an
 *   envelope's gate reads, and what a trigger control ignores the tail of by
 *   definition.
 * - `"press"` sends `on` at the press and nothing after it: the bang.
 *
 * Takes a def's control first, like {@link knob} — a gate needs no range, so
 * none is required here:
 *
 * ```ts
 * button(gate, { label: "hold" });               // 1 while held, 0 on release
 * button(trig, { mode: "press", label: "fire" }); // one message, the bang
 * ```
 *
 * **A widget cannot make a value instantaneous**: what is sent is held by
 * whoever receives it. So `mode: "press"` against a def's control is only a
 * bang where the control returns to zero on its own — a `rate: "tr"`, which the
 * server resets after one block — and building it over any other control
 * throws, since it would leave `on` standing forever. A button that drives no
 * control has no such trouble: it emits a `/gui_event` and one message *is* an
 * event.
 *
 * `on`/`off` are the two values it sends (`1`/`0` by default). Press and
 * release are the primitives, and a **click** — a press and a release that
 * landed inside — is a composed gesture rather than a mode.
 */
export function button(
    control?: ControlLike | ButtonOptions,
    options?: ButtonOptions,
): View {
    const [source_, opts] = controlArgs<ButtonOptions>(control, options);
    const { label: text, mode, on, off, textSize, ...rest } = opts;
    if (mode !== undefined && mode !== "gate" && mode !== "press") {
        throw new Error(
            `unknown button mode '${mode}'; use "gate" (on while held) or ` +
                '"press" (one message, the bang)',
        );
    }
    const props = drop([
        ["label", text], ["mode", mode], ["on", on], ["off", off],
        ["text_size", textSize],
    ]);
    let built;
    if (source_ === null) {
        built = { ...rest, ...props };
    } else {
        if (mode === "press" && source_.rate !== "tr" && source_.rate !== "trigger") {
            throw new Error(
                `button(mode: "press") drives '${source_.name}', which holds what it ` +
                    "is sent: the press would leave it at `on` forever. Declare the " +
                    'control a trigger (rate: "tr"), which the server resets after one ' +
                    "block, or use the default gate mode",
            );
        }
        // A button holds no value between presses, so the control's default has
        // nothing to seed here: `on`/`off` are what it sends, and its own name
        // is what the binding addresses.
        const { min: _min, max: _max, step: _step, value: _value, ...own } =
            fromControl(source_, props, { needsRange: false });
        built = { ...own, ...rest };
    }
    return node("button", built).setControl(source_);
}

/** A `toggle`'s options: its state, and the two values that state stands for. */
export interface ToggleOptions extends WidgetOptions {
    label?: string;
    /** The state the box is drawn in. */
    value?: boolean;
    /** The two values that state stands for on the wire. */
    on?: number;
    off?: number;
    textSize?: number;
}

/**
 * A boolean `toggle`. `value` is its state; what it *sends* is `on` or `off`,
 * `1`/`0` by default (OSC has no bool).
 *
 * The state is a boolean; **the two values it stands for need not be**. A
 * bypass lives at `0.0`/`0.7` and a mode at `1`/`2`, and neither is a span a
 * widget could be drawn over — which is why they are a pair and not a
 * `min`/`max`:
 *
 * ```ts
 * toggle(bypass, { on: 0.7, off: 0.0, label: "wet" });
 * ```
 */
export function toggle(
    control?: ControlLike | ToggleOptions,
    options?: ToggleOptions,
): View {
    const [source_, opts] = controlArgs<ToggleOptions>(control, options);
    const { label: text, value, on, off, textSize, ...rest } = opts;
    const props = drop([
        ["label", text], ["value", flag(value)], ["on", on], ["off", off],
        ["text_size", textSize],
    ]);
    // A toggle needs no range: it is 0/1 whatever the control declares.
    const built = source_ === null
        ? { ...rest, ...props }
        : { ...fromControl(source_, props, { needsRange: false }), ...rest };
    return node("toggle", built).setControl(source_);
}

/**
 * An editable `text` field. The entered string is emitted on **every** edit —
 * like a slider's value, never gated on Enter. `multiline` allows embedded
 * newlines and a growing field; `value` seeds the contents (and sets them
 * live).
 */
export function text(
    options: WidgetOptions & {
        value?: string;
        label?: string;
        multiline?: boolean;
        textSize?: number;
    } = {},
): GuiNode {
    const { value, label: name, multiline, textSize, ...rest } = options;
    return node("text", {
        ...rest,
        ...drop([
            ["value", value],
            ["label", name],
            // A true boolean here, as on a slider's `vertical`: the host reads
            // both forms, and these two props have always ridden as bools.
            ["multiline", multiline],
            ["text_size", textSize],
        ]),
    });
}

/**
 * A `menu` over `options` (strings), emitting the chosen `index`.
 *
 * A press **opens the list** over the window — the field grown downward by a
 * row per option, flipped above it near the bottom edge — and a press on a row
 * picks it; a press anywhere else dismisses it and picks nothing. The list is
 * the host's, so a bound menu drives its target with no round trip through the
 * page.
 */
export function menu(
    options: readonly string[] = [],
    rest: WidgetOptions & { index?: number; label?: string; textSize?: number } = {},
): GuiNode {
    const { index, label: text, textSize, ...others } = rest;
    return node("menu", {
        ...others,
        options: [...options],
        ...drop([["index", index], ["label", text], ["text_size", textSize]]),
    });
}

// ---- the heavy views ----

/**
 * The editor-grade `waveform` view, fed its samples by `cache`/`path`/
 * `buffer`/`data`/`blob` (the host's precedence order).
 *
 * Every channel is drawn — stacked lanes sharing the time axis, or per-color
 * overlaid traces with `overlay`. The rulers, the selection, the playhead and
 * the navigation group are the shared timeline chrome; `rulerY` labels the
 * amplitude axis (`"norm"`, `"db"`, `"bits"`, `"percent"`, `"off"`).
 * Dragging on the view selects (and emits `"selection" start len`), Shift+drag
 * pans, the wheel zooms.
 *
 * `rulerY` **labels** the axis and does not map it: the picture is linear in
 * amplitude whichever unit is named, and `"db"` is a ladder of rungs drawn at
 * the amplitudes those decibels are. So what a reading names and what an edit
 * writes at a height are one value, and editing is in linear amplitude and only
 * there.
 */
export function waveform(
    options: SourceOptions & TimelineOptions & {
        /** The peak-pyramid bucket size (default 256). */
        baseBucket?: number;
        /** Draw the channels as overlaid traces instead of stacked lanes. */
        overlay?: boolean;
        /** The amplitude-axis ruler: it labels the axis, it does not map it. */
        rulerY?: string;
        /** The integer resolution `rulerY: "bits"` labels (default 16). */
        bitDepth?: number;
        /**
         * The value domain the trace is drawn over, `[-1, 1]` (full-scale
         * audio) when omitted. A named domain is ruled with its own numbers,
         * since `db`/`bits`/`percent` are units of full scale.
         *
         * A column is the min/max of what the signal did in that pixel, never
         * extended to the zero line — the body of a zoomed-out waveform is the
         * data filling it, not a fill the drawing adds. Zoomed in far enough,
         * each sample is marked with a dot.
         */
        min?: number;
        /** The top of the value domain (see `min`). */
        max?: number;
        /**
         * What the picture measures: `"peak"` (the default envelope), `"rms"`
         * (the level body), or **both as one space-separated string** —
         * `"peak rms"`, the classic editor picture, the level drawn inside the
         * envelope. A stack is a prop of *one* view and not two views layered:
         * a view paints its own field before it draws, so the second would hide
         * the first (see `signal`, and the multitrack editor's signal view,
         * whose `layers` is this prop).
         */
        measure?: "peak" | "rms" | "peak rms" | "rms peak";
        /**
         * The samples are **being written into as they are drawn** — a take you
         * are recording. The picture stops at the buffer's write frontier and
         * the axis past it stays empty; see `signal`.
         */
        fills?: boolean;
    } = {},
): GuiNode {
    const {
        cache, path, buffer, data, blob, channels, baseBucket, overlay,
        rulerY, bitDepth, min, max, measure, fills, ...timeline
    } = options;
    return node("signal", {
        view: "trace",
        ...timelineProps(
            timeline,
            drop([["unit", rulerY], ["bit_depth", bitDepth], ["min", min], ["max", max]]),
        ),
        ...sourceProps({ cache, path, buffer, data, blob, channels }),
        ...drop([
            ["base_bucket", baseBucket],
            ["overlay", flag(overlay)],
            ["measure", measure],
            ["fills", flag(fills)],
        ]),
    });
}

/**
 * The editor-grade `spectrogram` (STFT time-frequency) view, fed like the
 * `waveform` and carrying the same chrome — here `yStart`/`yLen` slice the
 * **frequency** display axis.
 *
 * The analysis: `windowSize` is the FFT size (a power of two, default 1024)
 * and `hop` the frame advance (default half the window). The display is live:
 * the dB window `[dbFloor, dbCeil]` sets the contrast, `freqScale` picks the
 * frequency axis (`"log"` — the default — `"linear"`, `"mel"` or `"bark"`)
 * and `colormap` picks 0 viridis / 1 magma / 2 grayscale.
 */
export function spectrogram(
    options: SourceOptions & TimelineOptions & {
        windowSize?: number;
        hop?: number;
        dbFloor?: number;
        dbCeil?: number;
        freqScale?: string;
        /** The legacy boolean alias of `freqScale`: log against linear. */
        logFreq?: boolean;
        colormap?: number;
        /** The frequency ruler: `"hz"` (the default) or `"off"`. */
        rulerY?: string;
    } = {},
): GuiNode {
    const {
        cache, path, buffer, data, blob, channels, windowSize, hop,
        dbFloor, dbCeil, freqScale, logFreq, colormap, rulerY, ...timeline
    } = options;
    return node("signal", {
        view: "spectrogram",
        ...timelineProps(timeline, drop([["unit", rulerY]])),
        ...sourceProps({ cache, path, buffer, data, blob, channels }),
        ...drop([
            ["window_size", windowSize],
            ["hop", hop],
            ["db_floor", dbFloor],
            ["db_ceil", dbCeil],
            ["freq_scale", freqScale],
            ["log_freq", flag(logFreq)],
            ["colormap", colormap],
        ]),
    });
}

/**
 * A static `plot` of a signal — measurement without navigation: it does not
 * zoom, pan or edit. `view` picks the presentation: `"signal"` (the default;
 * value against time, the whole sequence always drawn) or `"spectrum"` (the
 * averaged magnitude spectrum, analyzed host-side with the shared-core FFT).
 * Omit a side of `[min, max]` and the value axis auto-fits the data; the
 * string `"auto"` releases a side live.
 */
export function plot(
    options: SourceOptions & {
        view?: string;
        overlay?: boolean;
        sampleRate?: number;
        min?: number | string;
        max?: number | string;
        ruler?: string;
        rulerY?: string;
        fftSize?: number;
        dbFloor?: number;
        dbCeil?: number;
        freqScale?: string;
        /** What the columns measure — see `signal`. */
        measure?: "peak" | "rms";
        label?: string;
        /**
         * The axis pair written the long way, merged over the flat chrome
         * options per axis (what it names wins).
         */
        axes?: AxisPair;
    } = {},
): GuiNode {
    const {
        cache, path, buffer, data, blob, channels, view, overlay, sampleRate,
        min, max, ruler, rulerY, fftSize, dbFloor, dbCeil, freqScale, measure,
        label: text, axes: pair, ...rest
    } = options;
    // A plot is the trace (or the spectrum) of a signal that does **not**
    // navigate — the capability, not a different element.
    return node("signal", {
        ...rest,
        view: PLOT_VIEW[view ?? "signal"] ?? view,
        navigable: 0,
        ...sourceProps({ cache, path, buffer, data, blob, channels }),
        ...axes(
            drop([["unit", ruler], ["sample_rate", sampleRate]]),
            drop([["unit", rulerY], ["min", min], ["max", max]]),
            pair,
        ),
        ...drop([
            ["overlay", flag(overlay)],
            ["fft_size", fftSize],
            ["db_floor", dbFloor],
            ["db_ceil", dbCeil],
            ["freq_scale", freqScale],
            ["measure", measure],
            ["label", text],
        ]),
    });
}

// ---- the live views (the audio server's data) ----

/**
 * A level `meter` on `bus`, read from the audio server's shared segment every
 * frame — no OSC per frame at all.
 *
 * At `rate` `"audio"` (the default) it meters an audio bus — bus 0 is the first
 * hardware output, so `meter()` is the console meter on the left out — reading
 * the level the server publishes per block: a peak held with a decay, so a
 * transient is caught even though the display refreshes far slower than the
 * engine. It costs the server nothing to set up, so a mixer's worth of meters
 * is fine. At `"control"` it reads a control bus's value instead. `min`/`max`
 * scale the bar (default `0`/`1`).
 */
export function meter(
    bus = 0,
    options: WidgetOptions & {
        rate?: "audio" | "control";
        min?: number;
        max?: number;
        label?: string;
    } = {},
): GuiNode {
    const { rate = "audio", min, max, label: text, ...rest } = options;
    return node("meter", {
        ...rest,
        bus,
        rate,
        ...drop([["min", min], ["max", max], ["label", text]]),
    });
}

/**
 * A time-domain `scope` over `channels` **adjacent** buses starting at `bus`
 * (bus 0 is the first hardware output), in one of two rates.
 *
 * At `rate` `"audio"` (the default) it is a real **oscilloscope**: a `windowMs`
 * display window of each bus's samples, aligned on a rising crossing of
 * `trigger` found in the first channel, so a periodic signal draws a stable
 * trace and the channels keep their true relative phase. Naming the bus is all
 * a script does — the GUI host has the server record it and stops when nothing
 * draws it. At `"control"` it plots the control buses' recent history instead,
 * one sample per frame tick. `hold` freezes the trace.
 */
export function scope(
    bus = 0,
    options: WidgetOptions & {
        rate?: "audio" | "control";
        channels?: number;
        overlay?: boolean;
        windowMs?: number;
        trigger?: number;
        hold?: boolean;
        min?: number;
        max?: number;
        /**
         * The x ruler (ms of the window) and the y ruler (value): shown by
         * default on the audio-rate form, hidden with `false` or `"off"`.
         */
        ruler?: boolean | string;
        rulerY?: boolean | string;
        /** What the columns measure — see `signal`. */
        measure?: "peak" | "rms";
        label?: string;
        /**
         * The axis pair written the long way, merged over the flat chrome
         * options per axis (what it names wins).
         */
        axes?: AxisPair;
    } = {},
): GuiNode {
    const {
        rate = "audio", channels, overlay, windowMs, trigger, hold, min, max,
        ruler, rulerY, measure, label: text, axes: pair, ...rest
    } = options;
    return node("signal", {
        ...rest,
        bus,
        rate,
        view: "trace",
        ...axes(
            drop([["unit", strip(ruler)]]),
            drop([["unit", strip(rulerY)], ["min", min], ["max", max]]),
            pair,
        ),
        ...drop([
            ["channels", channels],
            ["overlay", flag(overlay)],
            ["window_ms", windowMs],
            ["trigger", trigger],
            ["hold", flag(hold)],
            ["measure", measure],
            ["label", text],
        ]),
    });
}

/**
 * A `phasescope` (goniometer) of the stereo pair `bus` (left) and `bus + 1`
 * (right) — the adjacent-channel layout the whole family uses — drawn as the
 * 45°-rotated Lissajous figure: vertical is the mid, horizontal the side, so
 * mono reads as a vertical line and anti-phase as horizontal. An age-faded
 * trail spans the last `windowMs` and a correlation read-out sits under the
 * field. Audio rate only.
 */
export function phasescope(
    bus = 0,
    options: WidgetOptions & {
        windowMs?: number;
        hold?: boolean;
        label?: string;
    } = {},
): GuiNode {
    const { windowMs, hold, label: text, ...rest } = options;
    return node("signal", {
        ...rest,
        bus,
        view: "phase",
        ...drop([
            ["window_ms", windowMs],
            ["hold", flag(hold)],
            ["label", text],
        ]),
    });
}

/**
 * A live `spectrum` (spectroscope): one forward FFT per frame over the newest
 * window of each of `channels` **adjacent** audio buses starting at `bus`, one
 * magnitude curve per channel. `averaging` (0..1) smooths each bin and
 * `peakHold` overlays a decaying peak trace; `freqScale` is the spectrogram's
 * set. Audio rate only.
 *
 * `navigable` turns the **frequency axis** into one you can move: drag it to
 * pan, wheel over it to zoom under the cursor, `R` to see all of it again. It
 * needs no history behind it — unlike a live time axis, every bin is there
 * every frame — so it is one window the view carries alone, in normalized
 * units over `[0, Nyquist]`: `viewStart`/`viewLen` (`0, 1` = the whole axis),
 * live via `GuiHost.set` and reported as a `"view_x"` event. It is off by
 * default; without it this is the watching spectroscope it has always been.
 */
export function spectrum(
    bus = 0,
    options: WidgetOptions & {
        channels?: number;
        fftSize?: number;
        dbFloor?: number;
        dbCeil?: number;
        freqScale?: string;
        /** The legacy boolean alias of `freqScale`: log against linear. */
        logFreq?: boolean;
        averaging?: number;
        peakHold?: boolean;
        /** Whether the frequency axis zooms and pans (off by default). */
        navigable?: boolean;
        /** The visible slice of the frequency axis, normalized (`0, 1` = all). */
        viewStart?: number;
        viewLen?: number;
        ruler?: boolean | string;
        rulerY?: boolean | string;
        label?: string;
        /**
         * The axis pair written the long way, merged over the flat chrome
         * options per axis (what it names wins).
         */
        axes?: AxisPair;
    } = {},
): GuiNode {
    const {
        channels, fftSize, dbFloor, dbCeil, freqScale, logFreq, averaging,
        peakHold, navigable, viewStart, viewLen, ruler, rulerY,
        label: text, axes: pair, ...rest
    } = options;
    return node("signal", {
        ...rest,
        bus,
        view: "spectrum",
        ...axes(
            drop([
                ["unit", strip(ruler)],
                ["start", viewStart],
                ["len", viewLen],
            ]),
            drop([["unit", strip(rulerY)]]),
            pair,
        ),
        ...drop([
            ["channels", channels],
            ["fft_size", fftSize],
            ["db_floor", dbFloor],
            ["db_ceil", dbCeil],
            ["freq_scale", freqScale],
            ["log_freq", flag(logFreq)],
            ["averaging", averaging],
            ["peak_hold", flag(peakHold)],
            ["navigable", flag(navigable)],
            ["label", text],
        ]),
    });
}

/**
 * A live `nodetree` view of the audio server's node tree rooted at `group`
 * (default the root group). The host mirrors the server's tree over its
 * client leg, so creations, deaths and `/node_set` edits show live. `controls`
 * (default true) shows each synth's control name/value pairs. Read-only.
 */
export function nodetree(
    options: WidgetOptions & { group?: number; controls?: boolean; label?: string } = {},
): GuiNode {
    const { group = 0, controls, label: text, ...rest } = options;
    return node("nodes", {
        ...rest,
        group,
        ...drop([["controls", flag(controls)], ["label", text]]),
    });
}

// ---- the editors ----

/**
 * A drawable `bpf` break-point function — the envelope editor.
 *
 * Break-points `(time, value)` plus a per-segment shape using the server's
 * own envelope shape numbers, evaluated host-side through the same shared
 * math its `EnvGen` plays — what you draw is what you hear. `points` takes
 * either the flat wire quads `[t, v, shape, curve, …]` or a list of
 * `[time, value]` / `[time, value, curve]` tuples whose curve is an `Env`
 * shape name or a numeric curvature (see `envToPoints`/`pointsToEnv` for the
 * `Env` round trip). Editing flows back as `"points"` with the flat list.
 *
 * The widget is general on purpose (the automation-lane shape): values live
 * in `[min, max]` — unipolar, bipolar or any parameter span — and `exp` gives
 * a frequency-like range a geometric display scale.
 */
export function bpf(
    options: WidgetOptions & {
        points?: PointSpec | Source;
        min?: number;
        max?: number;
        duration?: number;
        exp?: boolean;
        label?: string;
        /**
         * The axis pair written the long way, merged over the flat chrome
         * options per axis (what it names wins).
         */
        axes?: AxisPair;
    } = {},
): GuiNode {
    const {
        points, min, max, duration, exp, label: text, axes: pair, ...rest
    } = options;
    return node("curve", {
        ...rest,
        ...axes({}, drop([["min", min], ["max", max]]), pair),
        ...drop([
            ["points", held(points, flatPoints)],
            ["duration", duration],
            ["exp", flag(exp)],
            ["label", text],
        ]),
    });
}

/**
 * The editor-grade `pianoroll`: a keyboard gutter, a note grid, a velocity
 * lane and an OSC lane — the timeline sibling of the compact `clip`
 * roll, drawing the same notes with editing, rulers and navigation.
 *
 * `notes` are `[start, dur, pitch]` or `[start, dur, pitch, velocity,
 * channel]` MIDI notes (times in timeline samples, pitch drawn over
 * `[min, max]`); `osc` are `[time, label]` (or bare `time`) markers, one per
 * OSC or raw-MIDI timeline item. An
 * edit flows back as a flat `"notes"` or `"osc"` event. `midiIn` arms live
 * MIDI painting in the native host.
 *
 * **A plain drag over the grid sweeps the notes** the rectangle covered — the
 * rectangles the notes *are*, the same gesture a patcher's canvas has over its
 * boxes and a lane has over its clips — and it writes **no time span**. A
 * *time range* over the same grid is the other selection, asked for by name
 * (`gestures: { drag: "select" }`), exactly as on a lane.
 */
export function pianoroll(
    options: TimelineOptions & {
        notes?: NoteSpec | Source;
        osc?: OscMarkSpec | Source;
        min?: number;
        max?: number;
        snap?: number;
        velocity?: boolean;
        oscLane?: boolean;
        midiIn?: boolean;
        label?: string;
    } = {},
): GuiNode {
    const {
        notes, osc, min, max, snap, velocity, oscLane, midiIn,
        label: text, ...timeline
    } = options;
    return node("notes", {
        ...timelineProps(timeline, drop([["min", min], ["max", max]])),
        ...drop([
            ["notes", held(notes, flatNotes)],
            ["osc", held(osc, flatOsc)],
            ["snap", snap],
            ["velocity", flag(velocity)],
            ["osc_lane", flag(oscLane)],
            ["midi_in", flag(midiIn)],
            ["label", text],
        ]),
    });
}

/**
 * The playable `piano` virtual keyboard: keys with real piano proportions,
 * resizing freely with the widget.
 *
 * `min`/`max` are the visible MIDI range (default 36–96; `min` snaps down to
 * a white key), `activeMin`/`activeMax` the mapped range (keys outside draw
 * grayed and are inert), and the `overview` strip pans and zooms the window
 * (`pan: false` freezes all navigation). Playing emits **MIDI-shaped**
 * `"note" pitch velocity state channel` events; setting `voice` to a def name
 * instead has the *host* manage one server voice per held key, so the
 * keyboard plays with no script in the loop.
 */
export function piano(
    options: WidgetOptions & {
        min?: number;
        max?: number;
        activeMin?: number;
        activeMax?: number;
        pan?: boolean;
        overview?: boolean;
        velocity?: number;
        channel?: number;
        voice?: string;
        /** Extra `[name, value]` control pairs for the host's `/synth_new`. */
        voiceArgs?: readonly (readonly [string, number])[];
        label?: string;
    } = {},
): GuiNode {
    const {
        min, max, activeMin, activeMax, pan, overview, velocity, channel,
        voice, voiceArgs, label: text, ...rest
    } = options;
    return node("keys", {
        ...rest,
        ...drop([
            ["min", min],
            ["max", max],
            ["active_min", activeMin],
            ["active_max", activeMax],
            ["pan", flag(pan)],
            ["overview", flag(overview)],
            ["velocity", velocity],
            ["channel", channel],
            ["voice", voice],
            ["voice_args", voiceArgs?.flatMap(([n, v]) => [n, v])],
            ["label", text],
        ]),
    });
}

/**
 * A free-standing **time ruler**: the shared axis drawn as a strip the document
 * places — a DAW's ruler above its tracks.
 *
 * A `track`'s own `ruler` is reserved out of *that lane's* height, so ruling a
 * stack of lanes means picking one to carry it and to pay for it, and the strip
 * then sits wherever that lane sits. This widget owns its box instead: put it
 * above the lanes and no lane loses a pixel.
 *
 * It reads the axis of the group named by `link`; with **no** `link` it joins
 * the window's lanes on its own, since a free-standing ruler exists to rule
 * them —
 * and its ticks are indented by the **group's** gutter — the widest any member
 * asks for — so they stand over the samples they label.
 *
 * **The ruler is where the time range is swept.** Two selections live at once
 * over a stack of lanes or a roll — the **data** one (the clips, the boxes, the
 * notes a rectangle covered: what gets edited) and the **time range** (a span
 * the group keeps, drawn as a band, looped by the transport: what gets played)
 * — and they are told apart by where the gesture began, not by a mode: the body
 * sweeps the first, the ruler the second. So a **drag scrolls** the axis,
 * **Alt+drag sweeps the range**, the wheel zooms, and a **click locates** — a
 * drag that never left the slop is where the hand pointed. On a signal the
 * range is not a second thing: the frames and the span are one selection there,
 * so the ruler is another hand onto the one the view already has. And a lane's
 * own `ruler`, a roll's and a signal's are **strips** rather than widgets — the
 * press lands on the view — so the table is read from where the press landed:
 * the bottom of a view that has a ruler answers with the ruler's table, whoever
 * drew it.
 * `h` is this one's thickness in logical pixels.
 */
export function timeruler(
    options: TimelineOptions & {
        /** A theme group over the strip. */
        theme?: Record<string, string>;
    } = {},
): GuiNode {
    const { h = 20, theme, ...timeline } = options;
    return node("field", {
        ...timelineProps(timeline),
        h,
        ...drop([["theme", theme]]),
    });
}

/**
 * A multitrack `track` lane holding `clip` children on a shared time axis —
 * the DAW-style editor's lane. `label` names it in a left header, `height` is
 * its lane weight, and `snap` is the drag grid a clip's move/resize rounds
 * to. The lanes of a window navigate as one (the same `link` group the heavy
 * views use), and the lane carries the same time chrome.
 *
 * The **header** is the band left of the axis, and it is sizeable: it holds
 * the `label` and, when asked for, the lane's controls — `mute` and `solo` each
 * add a toggle (pass the initial state), `level` adds a fader over `[0, 1]`.
 * Working one sends a `/gui_event` naming the prop it changed (`"mute" 0|1`,
 * `"solo" 0|1`, `"level" f`), so a driver mirrors the edit by echoing it back.
 * `headerW` overrides the width outright; without it the header sizes itself to
 * what it carries. That width is the **axis'**, not the lane's: every member of
 * a navigation group starts its body at the widest gutter any of them asks for.
 */
export function track(
    options: TimelineOptions & {
        label?: string;
        height?: number;
        snap?: number;
        /** The header's width in logical pixels; omitted sizes it naturally. */
        headerW?: number;
        /** Offer a mute toggle, with this initial state. */
        mute?: boolean;
        /** Offer a solo toggle, with this initial state. */
        solo?: boolean;
        /** Offer a level fader, over `[0, 1]`, at this initial value. */
        level?: number;
        theme?: Record<string, string>;
        children?: readonly GuiNode[];
    } = {},
    ...clips: GuiNode[]
): GuiNode {
    const {
        label: text,
        height,
        snap,
        headerW,
        mute,
        solo,
        level,
        theme,
        children,
        ...timeline
    } = options;
    return node("field", {
        ...timelineProps(timeline),
        ...drop([
            ["label", text],
            ["height", height],
            ["snap", snap],
            ["header_w", headerW],
            ["mute", mute],
            ["solo", solo],
            ["level", level],
            ["theme", theme],
        ]),
        children: [...(children ?? []), ...clips],
    });
}

/**
 * One `clip` on a `track`: a placed rectangle spanning `[offset, offset +
 * dur]` in timeline sample units (the graphic unit — length = duration).
 *
 * Its body is a **take** (reached exactly as the heavy `waveform`'s samples
 * are — `cache`/`path`/`buffer`/`data`/`blob`), a **piano-roll** of `notes`,
 * or an **automation curve** of `points` editable in place. Dragging the clip
 * (move) or its edge (a trim) flows back as a `"clip"` event carrying the new
 * `offset`/`dur`.
 *
 * The take is drawn in the presentation `view` names: `"trace"` (the default)
 * summarizes it through the peak pyramid to fit the rectangle,
 * `"spectrogram"` draws its STFT as the time-frequency texture — the same
 * signal seen the other way, and still a clip: placed at `offset`, ending at
 * `dur`, dragged and resized on the lane's axis. The spectral parameters are
 * the `spectrogram` view's own (`windowSize`, `hop`, `dbFloor`, `dbCeil`,
 * `freqScale`, `colormap`); the presentation and the analysis are read when
 * the clip is built, the display props are live via `set`.
 */
export function clip(
    options: SourceOptions & {
        /** The clip's start on the shared timeline (samples). */
        offset?: number;
        /** Its duration (samples) — a clip with no duration draws nothing. */
        dur: number;
        baseBucket?: number;
        /** The take's presentation: `"trace"` (default) or `"spectrogram"`. */
        view?: string;
        windowSize?: number;
        hop?: number;
        dbFloor?: number;
        dbCeil?: number;
        freqScale?: string;
        colormap?: number;
        notes?: NoteSpec | Source;
        points?: PointSpec | Source;
        exp?: boolean;
        min?: number;
        max?: number;
        /**
         * Whether a hand may edit this clip's **bodies** (default true), *all*
         * of them: it is a statement about the clip. False where the body draws
         * a *rendering* rather than the thing itself — the notes of a pattern, a
         * curve this editor cannot write — so the roll or the curve refuses the
         * press instead of offering a drag it will unwind. The refusal is
         * visible and consumes the press; the clip's own move and resize are
         * untouched.
         */
        editable?: boolean;
        /**
         * The same answer for **one** body, overriding {@link clip}'s
         * `editable` where it is given. A clip whose bodies layer needs it: an
         * envelope over a pattern's notes is the ordinary case, and there the
         * roll is a rendering that cannot be written while the curve over it is
         * the thing itself. It is the split `min`/`max` already has from
         * `pointsMin`/`pointsMax`, for the same reason — two bodies, one props
         * map.
         */
        notesEditable?: boolean;
        /** The curve body's own editability; see `notesEditable`. */
        pointsEditable?: boolean;
        /**
         * The source frame this clip's own time zero reads (default 0). A clip
         * is a **window onto a segment of its buffer**: one timeline sample
         * is one source frame, so trimming one hides frames rather than
         * compressing them, and opening the window again brings them back.
         */
        start?: number;
        /**
         * Whether that window **wraps** around the buffer: past the last
         * frame it begins again, and before the first comes the buffer's own
         * tail. It is what lets an edge be pulled past the buffer at all.
         */
        loop?: boolean;
        /**
         * Draw the samples **fitted** to the clip's span instead of read frame
         * for sample — the picture a time stretch would make, which nothing
         * here makes yet. Off by default.
         */
        fit?: boolean;
        /**
         * The **edit layer** a hand is on: `"clip"` (the placement — where it
         * sits, how long it is), `"take"`, `"notes"` or `"points"`. Only the
         * active layer acts or shows an affordance, so a clip whose curve is
         * being edited shows no grips. A press picks the layer under it and
         * reports the change as a `"layer"` event.
         */
        layer?: string;
        /**
         * The layers that are **not drawn**, space-separated (`"notes
         * points"`); empty draws them all. What is hidden is not edited either.
         */
        hidden?: string;
        label?: string;
    },
): GuiNode {
    const {
        offset = 0.0, dur, cache, path, buffer, data, blob, channels,
        baseBucket, view, windowSize, hop, dbFloor, dbCeil, freqScale, colormap,
        notes, points, exp, min, max, editable, notesEditable, pointsEditable,
        start, loop, fit, layer, hidden,
        label: text, ...rest
    } = options;
    return node("field", {
        ...rest,
        dur,
        offset,
        ...sourceProps({ cache, path, buffer, data, blob, channels }),
        ...drop([
            ["base_bucket", baseBucket],
            ["view", view],
            ["window_size", windowSize],
            ["hop", hop],
            ["db_floor", dbFloor],
            ["db_ceil", dbCeil],
            ["freq_scale", freqScale],
            ["colormap", colormap],
            ["notes", held(notes, flatNotes)],
            ["points", held(points, flatPoints)],
            ["exp", flag(exp)],
            ["min", min],
            ["max", max],
            ["editable", flag(editable)],
            ["notes_editable", flag(notesEditable)],
            ["points_editable", flag(pointsEditable)],
            ["start", start],
            ["loop", flag(loop)],
            ["fit", flag(fit)],
            ["layer", layer],
            ["hidden", hidden],
            ["label", text],
        ]),
    });
}

/**
 * An engraved music-notation `score` page. The host is only the renderer: it
 * fits the engraved page into the widget and tessellates its primitives.
 *
 * `displayList` is the semantic engraving — `vb` (the page-unit viewBox),
 * `glyphs` (the SMuFL outline table), `prims` (the placed primitives),
 * `cursors` (the engraved timemap) and `step` (page units per diatonic step)
 * — which a client produces from its own score. A click emits `"element"`
 * with the primitive's `xml:id`; `editable` turns on the drag that emits
 * `"transpose" id position` — the diatonic staff position the note *reaches*,
 * from its staff's top line, positive upward — a *request* the driver applies
 * and answers with a re-engraved page. The position is absolute rather than a
 * displacement, so a resend cannot move the note twice and a page re-engraved
 * under the gesture needs no rebasing.
 *
 * `entry` turns on **note entry**, and it is its own flag rather than a second
 * meaning for `editable` because it takes over a gesture that already does
 * something: on any other page, pressing blank paper clears the selection. With
 * it on, a press on blank paper inside a staff emits
 * `"insert" <after-xml:id> <position> <staff>` — the element the new note would
 * *follow* on that staff (empty before everything on it), the staff position,
 * and which staff from the top. The host names a place and nothing more: a
 * staff position is not a pitch until something knows the clef and the key, and
 * a duration is a choice nobody made by clicking.
 *
 * The playback cursor works exactly as a timeline view's:
 * `playheadAt` anchors it to the engine clock, `playhead` is a static time in
 * milliseconds.
 */
export function score(
    options: WidgetOptions & {
        displayList?: Record<string, unknown> | Source;
        playhead?: number;
        playheadAt?: number;
        /** The loop region the sweeping cursor wraps inside, in ms. */
        playheadLoopStart?: number;
        playheadLoopLen?: number;
        sampleRate?: number;
        selected?: string;
        editable?: boolean;
        entry?: boolean;
    } = {},
): GuiNode {
    const {
        displayList, playhead, playheadAt, playheadLoopStart, playheadLoopLen,
        sampleRate, selected, editable, entry, ...rest
    } = options;
    return node("score", {
        ...rest,
        ...drop([
            ["playhead", playhead],
            ["playhead_at", playheadAt],
            ["playhead_loop_start", playheadLoopStart],
            ["playhead_loop_len", playheadLoopLen],
            ["sample_rate", sampleRate],
            ["selected", selected],
            ["editable", editable],
            ["entry", entry],
        ]),
        // A source goes in as itself and `node` expands it into the five;
        // anything else is expanded here.
        ...(displayList instanceof Source
            ? { display_list: displayList }
            : scorePage(displayList ?? {})),
    });
}

/**
 * A `patch` **patcher**: a directed, typed signal graph drawn as boxes with
 * inlets on top and outlets on the bottom, and a **cord** per `outlet ->
 * inlet` connection. The buses are not drawn — a cord *is* a bus.
 *
 * `boxes` and `cords` are the widget's split schema: each box is
 * `{def, inlets, outlets, x?, y?}` (a port is a bare name for audio, or
 * `{name, rate}`), and `cords` is the flat `[fromBox, outlet, toBox, inlet,
 * …]` list of indices. Dragging a box flows back as `"move"`, and dragging an
 * outlet onto an inlet as `"wire"` — the driver owns the geometry and the
 * graph, and re-renders.
 */
export function patch(
    options: WidgetOptions & {
        boxes?: readonly unknown[] | Source;
        cords?: readonly number[] | Source;
        label?: string;
    } = {},
): GuiNode {
    const { boxes, cords, label: text, ...rest } = options;
    return node("plane", {
        ...rest,
        ...drop([
            ["boxes", held(boxes, (v) => [...v])],
            ["cords", held(cords, flatCords)],
            ["label", text],
        ]),
    });
}

/**
 * A `canvas` running a script-supplied WGSL shader over the widget area —
 * custom visuals.
 *
 * `shader` is the body of a `shade` function the host wraps and runs:
 * `fn shade(uv: vec2<f32>, frag: vec4<f32>) -> vec4<f32>`. Inside it the host
 * exposes `u.resolution`, `u.time` and `u.params` — four values driven either
 * from the script (`set(id, { param0: … })` lands in `u.params.x`) or from a
 * control bus per slot (`buses`), read every frame, so a shader animates from
 * OSC parameters and from live server audio at once.
 */
export function canvas(
    shader?: string,
    options: WidgetOptions & {
        params?: readonly number[];
        buses?: readonly number[];
        label?: string;
    } = {},
): GuiNode {
    const { params, buses, label: text, ...rest } = options;
    return node("canvas", {
        ...rest,
        ...drop([
            ["shader", shader],
            ["params", params === undefined ? undefined : [...params]],
            ["buses", buses === undefined ? undefined : buses.map((b) => Math.trunc(b))],
            ["label", text],
        ]),
    });
}

// ---- the shared prop groups ----

/** The timeline chrome (and the generic options riding with it) as wire props. */
function timelineProps(options: TimelineOptions, y: Props = {}): Props {
    const {
        ruler, sampleRate, tempo, beatAt, quant, selStart, selLen,
        selMin, selMax, markers,
        playheadAt, playhead, playheadLoopStart, playheadLoopLen,
        yStart, yLen, link, autofit, axes: pair, ...rest
    } = options;
    return {
        ...rest,
        ...axes(
            drop([
                ["unit", ruler],
                ["sample_rate", sampleRate],
                ["tempo", tempo],
                ["beat_at", beatAt],
                ["quant", quant],
                ["sel_start", selStart],
                ["sel_len", selLen],
                ["playhead_at", playheadAt],
                ["playhead", playhead],
                ["playhead_loop_start", playheadLoopStart],
                ["playhead_loop_len", playheadLoopLen],
                ["link", link],
                ["autofit", autofit],
                ["markers", markers === undefined ? undefined : flatMarkers(markers)],
            ]),
            {
                ...drop([
                    ["start", yStart],
                    ["len", yLen],
                    ["sel_min", selMin],
                    ["sel_max", selMax],
                ]),
                ...y,
            },
            pair,
        ),
    };
}

/**
 * The model's names for the four elements the catalog named after the thing
 * they show rather than for what they are: a piano-roll is the **notes**
 * element, a break-point envelope a **curve**, the server's graph **nodes**
 * and a keyboard **keys**. The same builder under both names.
 */
export const notes = pianoroll;
export const curve = bpf;
export const nodes = nodetree;
export const keys = piano;

/** A heavy view's data source as wire props. */
function sourceProps(options: Pick<SourceOptions,
    "cache" | "path" | "buffer" | "data" | "blob" | "channels">): Props {
    const { cache, path, buffer, data, blob, channels } = options;
    return drop([
        ["cache", cache],
        ["path", path],
        ["buffer", buffer],
        // A `Source` passes through untouched — `node` expands it into the
        // carrier it picked; anything else is read into an array here.
        ["data", data === undefined || data instanceof Source ? data : [...data]],
        ["blob", blob],
        ["channels", channels],
    ]);
}

// ---- the flat wire forms ----

/**
 * Break-points: either the flat wire quads `[t, v, shape, curve, …]` or
 * `[time, value]` / `[time, value, curve]` tuples.
 */
export type PointSpec =
    | readonly number[]
    | readonly (readonly [number, number] | readonly [number, number, Curve])[];

/** Notes: `[start, dur, pitch]`, optionally with `velocity` and `channel`. */
export type NoteSpec = readonly (readonly number[])[];

/** OSC markers: `[time, label]` pairs, or a bare `time`. */
export type OscMarkSpec = readonly (number | readonly [number] | readonly [number, string])[];

/**
 * A `points` argument as the flat quad list: a flat list is validated (whole
 * quads, shapes truncated to ints), tuples become `t, v, shape, curve` with
 * the shape resolved like an `Env` curve spec (linear by default).
 */
export function flatPoints(points: PointSpec): number[] {
    const list = [...points];
    if (list.length === 0) return [];
    if (typeof list[0] === "number") {
        const flat = list as number[];
        if (flat.length % 4 !== 0) {
            throw new TypeError("a flat points list must be [t, v, shape, curve, …] quads");
        }
        return flat.map((x, i) => (i % 4 === 2 ? Math.trunc(x) : x));
    }
    const out: number[] = [];
    for (const point of list as readonly (readonly [number, number, Curve?])[]) {
        const [shape, curve] =
            point.length > 2 ? resolveCurve(point[2] as Curve) : [1, 0.0];
        out.push(point[0], point[1], shape, curve);
    }
    return out;
}

/**
 * A `notes` argument as the flat quintuples `start dur pitch velocity
 * channel` (the canonical form the host reads for both the `pianoroll` and a
 * `clip`'s roll). A missing velocity defaults to 100, a missing channel to 0.
 */
export function flatNotes(notes: NoteSpec): number[] {
    const out: number[] = [];
    for (const note of notes) {
        out.push(
            note[0]!,
            note[1]!,
            note[2]!,
            note.length > 3 ? Math.trunc(note[3]!) : 100,
            note.length > 4 ? Math.trunc(note[4]!) : 0,
        );
    }
    return out;
}

/** A patcher's `cords` as the flat `[fromBox, outlet, toBox, inlet, …]` ints. */
export function flatCords(cords: readonly number[]): number[] {
    return cords.map((n) => Math.trunc(n));
}

/**
 * An engraved page as the props a **definition** carries it in: the wire spells
 * a `score`'s drawing as six keys, one per part of the display list, and
 * `GuiHost.set` spells the same page as the one `display_list`.
 *
 * `elements` is the last of them and the odd one: not a drawing layer but the
 * list of ids that name a **sounding element**, which the engraving walk knows
 * and a renderer cannot re-derive — to the host an id is an id, and a staff's
 * lines carry the staff's. It is what lets a press on blank paper say which
 * element it fell after.
 */
export function scorePage(displayList: Record<string, unknown>): Props {
    const dl = displayList ?? {};
    return drop([
        ["vb", dl.vb],
        ["glyphs", dl.glyphs],
        ["prims", dl.prims],
        ["cursors", dl.cursors],
        ["step", dl.step],
        ["elements", dl.elements],
    ]);
}

/**
 * The {@link STRUCTURE_PROPS}, each with how a value of it normalizes to the
 * props a definition carries and the node keys that expansion occupies (all but
 * the engraved page write the prop they are named by).
 */
const STRUCTURES: Record<string, { props: (v: unknown) => Props; slots: readonly string[] }> = {
    points: { props: (v) => ({ points: flatPoints(v as PointSpec) }), slots: ["points"] },
    notes: { props: (v) => ({ notes: flatNotes(v as NoteSpec) }), slots: ["notes"] },
    osc: { props: (v) => ({ osc: flatOsc(v as OscMarkSpec) }), slots: ["osc"] },
    boxes: { props: (v) => ({ boxes: [...(v as readonly unknown[])] }), slots: ["boxes"] },
    cords: { props: (v) => ({ cords: flatCords(v as readonly number[]) }), slots: ["cords"] },
    display_list: {
        props: (v) => scorePage(v as Record<string, unknown>),
        slots: ["vb", "glyphs", "prims", "cursors", "step", "elements"],
    },
};

/**
 * A structure argument as it goes into the node: a {@link Source} passes
 * through untouched (`node` expands it into the props it carries), anything
 * else is normalized here — the same call the source would have made.
 */
function held<T>(value: T | Source | undefined, flatten: (v: T) => unknown): unknown {
    if (value === undefined || value instanceof Source) return value;
    return flatten(value);
}

/**
 * A `markers` argument: `[time, label, color]`, `[time, label]` or a bare
 * `time` per marker — the label and the colour both optional.
 */
export type MarkerSpec = readonly (
    | number
    | readonly [number]
    | readonly [number, string]
    | readonly [number, string, string]
)[];

/** A `markers` argument as the flat `time, label, color` triples the host reads. */
export function flatMarkers(markers: MarkerSpec): (number | string)[] {
    const out: (number | string)[] = [];
    for (const marker of markers) {
        if (typeof marker === "number") out.push(marker, "", "");
        else {
            out.push(
                marker[0],
                marker.length > 1 ? String(marker[1]) : "",
                marker.length > 2 ? String(marker[2]) : "",
            );
        }
    }
    return out;
}

/** An `osc` argument as the flat `time, label` pairs the host reads. */
export function flatOsc(marks: OscMarkSpec): (number | string)[] {
    const out: (number | string)[] = [];
    for (const mark of marks) {
        if (typeof mark === "number") out.push(mark, "");
        else out.push(mark[0], mark.length > 1 ? String(mark[1]) : "");
    }
    return out;
}

// ---- serialization and bulk data ----

/**
 * A GuiDef tree as the JSON string carried in `/gui_def`.
 *
 * The client-only `name` key is stripped from every node: it labels the
 * widget for the host client's name → handle map and never rides the wire.
 */
export function toJson(tree: GuiNode): string {
    return JSON.stringify(stripNames(tree));
}

/**
 * A shallow copy of `node` (and its subtree) without the client-only `name`,
 * so serialization never leaks it to the host — whether or not the tree went
 * through `GuiHost`'s id/name walk.
 */
function stripNames(tree: GuiNode): GuiNode {
    const out: GuiNode = { type: tree.type };
    for (const [key, value] of Object.entries(tree)) {
        if (key !== "name" && key !== "children") out[key] = value;
    }
    if (tree.children && tree.children.length > 0) {
        out.children = tree.children.map(stripNames);
    }
    return out;
}

/**
 * Samples packed as a little-endian `f32` blob — the bulk form a `waveform`
 * reads through `blob`. Flat bytes at the boundary, the rule the rest of the
 * client follows.
 */
export { samplesToBlob };
