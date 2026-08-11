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

export { Env, envToPoints, pointsToEnv };
export type { Curve };

/**
 * One node of a GuiDef tree: its `type`, its props, and (for a container)
 * its `children`. `name` is the client-only handle name, never on the wire.
 */
export interface GuiNode {
    type: string;
    id?: number;
    name?: string;
    children?: GuiNode[];
    [prop: string]: unknown;
}

/**
 * The options every widget takes: the client-side `id`/`name`, the place
 * props the container's layout applies (all device pixels, all live via
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
     * a view always pans that axis whatever the table says.
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
    /** Labels clock time, and places a spectral frequency axis. */
    sampleRate?: number;
    /** Musical time: beats per second, the beat at sample 0, beats per bar. */
    tempo?: number;
    beatAt?: number;
    quant?: number;
    /** The time selection, in samples. */
    selStart?: number;
    selLen?: number;
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
}

/** Where a heavy view's samples come from, in the host's precedence order. */
export interface SourceOptions extends WidgetOptions {
    /**
     * A prebuilt peak-pyramid file (fetched in the browser); the most compact
     * bulk path — the raw samples are never loaded.
     */
    cache?: string;
    /** A file of raw little-endian `f32` samples the host maps (fetches). */
    path?: string;
    /** A server buffer number, pulled over the host's client leg. */
    buffer?: number;
    /** A short signal inline in the JSON. */
    data?: readonly number[];
    /**
     * The index of a binary blob carried beside the JSON (see
     * `samplesToBlob` and `GuiHost.define`).
     */
    blob?: number;
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
 * The axis pair `{x, y}` the chrome of a two-axis container belongs to, as
 * the one `axes` prop it rides under (or nothing, when neither side was
 * named). `x`/`y` are already the free-placement props, which is why the pair
 * nests rather than sitting bare on the node.
 */
function axes(x: Props, y: Props): Props {
    const out: Props = {};
    if (Object.keys(x).length > 0) out.x = x;
    if (Object.keys(y).length > 0) out.y = y;
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
): GuiNode {
    const { id, name, children, ...props } = options;
    const out: GuiNode = { type };
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
    return out;
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
        boxes?: readonly unknown[];
        cords?: readonly number[];
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
            ["boxes", boxes === undefined ? undefined : [...boxes]],
            ["cords", cords === undefined ? undefined : cords.map((n) => Math.trunc(n))],
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
        axes?: { x?: Props; y?: Props };
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
        selectable?: boolean;
        editable?: boolean;
        overlay?: boolean;
        axes?: { x?: Props; y?: Props };
        label?: string;
    } = {},
): GuiNode {
    const {
        view, cache, path, buffer, data, blob, channels, bus, rate, retention,
        baseBucket, navigable, selectable, editable, overlay, axes: pair,
        label: text, ...rest
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
            ["selectable", flag(selectable)],
            ["editable", flag(editable)],
            ["overlay", flag(overlay)],
            ["label", text],
        ]),
    });
}


/**
 * A top-level `window` container (a GuiDef root). It takes no id — the root's
 * id is the `/gui_def` argument.
 *
 * `w`/`h` size the OS window (the canvas, in the browser); `layout` places
 * the children, tuned by `margin`/`gap`/`cols`. A fixed-height bar over a
 * weighted content area over a fixed status strip — the application shell —
 * is just `window({ layout: "col" }, bar({ h: 28 }), content(), status({ h: 20 }))`.
 */
export function window(
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
): GuiNode {
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
        layout, margin, gap, cols, theme, ...rest
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
            ["flow", layout],
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
    text: string,
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
    value?: number;
    textSize?: number;
}

function rangeProps(options: RangeOptions): [Props, Props] {
    const { label: text, min, max, value, textSize, ...rest } = options;
    return [
        rest,
        drop([
            ["label", text],
            ["min", min],
            ["max", max],
            ["value", value],
            ["text_size", textSize],
        ]),
    ];
}

/** A rotary `knob` over a continuous range. */
export function knob(options: RangeOptions = {}): GuiNode {
    const [rest, props] = rangeProps(options);
    return node("knob", { ...rest, ...props });
}

/**
 * A continuous `slider`. `vertical` lays it out along the y axis (min at the
 * bottom) instead of horizontally.
 */
export function slider(options: RangeOptions & { vertical?: boolean } = {}): GuiNode {
    const { vertical, ...plain } = options;
    const [rest, props] = rangeProps(plain);
    // Only a vertical slider says so: the host reads the prop's absence as
    // horizontal, and the Python builder emits nothing for a false one.
    return node("slider", {
        ...rest, ...props, ...drop([["vertical", vertical || undefined]]),
    });
}

/** A draggable numeric read-out over a range. */
export function number(options: RangeOptions = {}): GuiNode {
    const [rest, props] = rangeProps(options);
    return node("number", { ...rest, ...props });
}

/** A momentary push `button` (emits `1` on press, `0` on release). */
export function button(
    options: WidgetOptions & { label?: string; textSize?: number } = {},
): GuiNode {
    const { label: text, textSize, ...rest } = options;
    return node("button", { ...rest, ...drop([["label", text], ["text_size", textSize]]) });
}

/** A boolean `toggle`. `value` rides as `1`/`0` (OSC has no bool). */
export function toggle(
    options: WidgetOptions & { label?: string; value?: boolean; textSize?: number } = {},
): GuiNode {
    const { label: text, value, textSize, ...rest } = options;
    return node("toggle", {
        ...rest,
        ...drop([["label", text], ["value", flag(value)], ["text_size", textSize]]),
    });
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
 */
export function waveform(
    options: SourceOptions & TimelineOptions & {
        /** The peak-pyramid bucket size (default 256). */
        baseBucket?: number;
        /** Draw the channels as overlaid traces instead of stacked lanes. */
        overlay?: boolean;
        /** The amplitude-axis ruler. */
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
    } = {},
): GuiNode {
    const {
        cache, path, buffer, data, blob, channels, baseBucket, overlay,
        rulerY, bitDepth, min, max, ...timeline
    } = options;
    return node("signal", {
        view: "trace",
        ...timelineProps(
            timeline,
            drop([["unit", rulerY], ["bit_depth", bitDepth], ["min", min], ["max", max]]),
        ),
        ...sourceProps({ cache, path, buffer, data, blob, channels }),
        ...drop([["base_bucket", baseBucket], ["overlay", flag(overlay)]]),
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
        label?: string;
    } = {},
): GuiNode {
    const {
        cache, path, buffer, data, blob, channels, view, overlay, sampleRate,
        min, max, ruler, rulerY, fftSize, dbFloor, dbCeil, freqScale,
        label: text, ...rest
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
        ),
        ...drop([
            ["overlay", flag(overlay)],
            ["fft_size", fftSize],
            ["db_floor", dbFloor],
            ["db_ceil", dbCeil],
            ["freq_scale", freqScale],
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
        label?: string;
    } = {},
): GuiNode {
    const {
        rate = "audio", channels, overlay, windowMs, trigger, hold, min, max,
        ruler, rulerY, label: text, ...rest
    } = options;
    return node("signal", {
        ...rest,
        bus,
        rate,
        view: "trace",
        ...axes(
            drop([["unit", strip(ruler)]]),
            drop([["unit", strip(rulerY)], ["min", min], ["max", max]]),
        ),
        ...drop([
            ["channels", channels],
            ["overlay", flag(overlay)],
            ["window_ms", windowMs],
            ["trigger", trigger],
            ["hold", flag(hold)],
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
    } = {},
): GuiNode {
    const {
        channels, fftSize, dbFloor, dbCeil, freqScale, logFreq, averaging,
        peakHold, navigable, viewStart, viewLen, ruler, rulerY,
        label: text, ...rest
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
        points?: PointSpec;
        min?: number;
        max?: number;
        duration?: number;
        exp?: boolean;
        label?: string;
    } = {},
): GuiNode {
    const { points, min, max, duration, exp, label: text, ...rest } = options;
    return node("curve", {
        ...rest,
        ...axes({}, drop([["min", min], ["max", max]])),
        ...drop([
            ["points", points === undefined ? undefined : flatPoints(points)],
            ["duration", duration],
            ["exp", flag(exp)],
            ["label", text],
        ]),
    });
}

/**
 * The editor-grade `pianoroll`: a keyboard gutter, a note grid, a velocity
 * lane and an OSC-event lane — the timeline sibling of the compact `clip`
 * roll, drawing the same notes with editing, rulers and navigation.
 *
 * `notes` are `[start, dur, pitch]` or `[start, dur, pitch, velocity,
 * channel]` MIDI notes (times in timeline samples, pitch drawn over
 * `[min, max]`); `osc` are `[time, label]` (or bare `time`) event flags. An
 * edit flows back as a flat `"notes"` or `"osc"` event. `midiIn` arms live
 * MIDI painting in the native host.
 */
export function pianoroll(
    options: TimelineOptions & {
        notes?: NoteSpec;
        osc?: OscEventSpec;
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
            ["notes", notes === undefined ? undefined : flatNotes(notes)],
            ["osc", osc === undefined ? undefined : flatOsc(osc)],
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
 * asks for — so they stand over the samples they label. A press locates the transport, Shift+drag pans and the
 * wheel zooms: you scrub on the ruler. `h` is its thickness in device pixels.
 */
export function timeruler(options: TimelineOptions = {}): GuiNode {
    const { h = 20, ...timeline } = options;
    return node("field", { ...timelineProps(timeline), h });
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
 * (move) or its edge (resize) flows back as a `"clip"` event carrying the new
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
        notes?: NoteSpec;
        points?: PointSpec;
        exp?: boolean;
        min?: number;
        max?: number;
        label?: string;
    },
): GuiNode {
    const {
        offset = 0.0, dur, cache, path, buffer, data, blob, channels,
        baseBucket, view, windowSize, hop, dbFloor, dbCeil, freqScale, colormap,
        notes, points, exp, min, max, label: text, ...rest
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
            ["notes", notes === undefined ? undefined : flatNotes(notes)],
            ["points", points === undefined ? undefined : flatPoints(points)],
            ["exp", flag(exp)],
            ["min", min],
            ["max", max],
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
 * `"transpose" id steps`, a *request* the driver applies and answers with a
 * re-engraved page. The playback cursor works exactly as a timeline view's:
 * `playheadAt` anchors it to the engine clock, `playhead` is a static time in
 * milliseconds.
 */
export function score(
    options: WidgetOptions & {
        displayList?: Record<string, unknown>;
        playhead?: number;
        playheadAt?: number;
        /** The loop region the sweeping cursor wraps inside, in ms. */
        playheadLoopStart?: number;
        playheadLoopLen?: number;
        sampleRate?: number;
        selected?: string;
        editable?: boolean;
    } = {},
): GuiNode {
    const {
        displayList, playhead, playheadAt, playheadLoopStart, playheadLoopLen,
        sampleRate, selected, editable, ...rest
    } = options;
    const dl = displayList ?? {};
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
            ["vb", dl.vb],
            ["glyphs", dl.glyphs],
            ["prims", dl.prims],
            ["cursors", dl.cursors],
            ["step", dl.step],
        ]),
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
        boxes?: readonly unknown[];
        cords?: readonly number[];
        label?: string;
    } = {},
): GuiNode {
    const { boxes, cords, label: text, ...rest } = options;
    return node("plane", {
        ...rest,
        ...drop([
            ["boxes", boxes === undefined ? undefined : [...boxes]],
            ["cords", cords === undefined ? undefined : cords.map((n) => Math.trunc(n))],
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
        playheadAt, playhead, playheadLoopStart, playheadLoopLen,
        yStart, yLen, link, ...rest
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
            ]),
            { ...drop([["start", yStart], ["len", yLen]]), ...y },
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
        ["data", data === undefined ? undefined : [...data]],
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

/** OSC event flags: `[time, label]` pairs, or a bare `time`. */
export type OscEventSpec = readonly (number | readonly [number] | readonly [number, string])[];

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

/** An `osc` argument as the flat `time, label` pairs the host reads. */
export function flatOsc(events: OscEventSpec): (number | string)[] {
    const out: (number | string)[] = [];
    for (const event of events) {
        if (typeof event === "number") out.push(event, "");
        else out.push(event[0], event.length > 1 ? String(event[1]) : "");
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
export { samplesToBlob } from "../base/bulk.ts";
