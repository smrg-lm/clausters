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

import { Env, envToPoints, pointsToEnv, resolveCurve } from "../defs/ugens.ts";
import type { Curve } from "../defs/ugens.ts";

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
    options: ContainerOptions & { title?: string } = {},
    ...children: GuiNode[]
): GuiNode {
    const { title, layout, margin, gap, cols, theme, ...rest } = options;
    return node("window", {
        ...rest,
        ...drop([
            ["title", title],
            ["layout", layout],
            ["margin", margin],
            ["gap", gap],
            ["cols", cols],
            ["theme", theme],
        ]),
        children: [...(options.children ?? []), ...children],
    });
}

/**
 * A nestable `panel` container. As a child it takes the same place props as
 * any widget; `theme` makes it a theme group over its whole subtree.
 */
export function panel(options: ContainerOptions = {}, ...children: GuiNode[]): GuiNode {
    const { layout, margin, gap, cols, theme, ...rest } = options;
    return node("panel", {
        ...rest,
        ...drop([
            ["layout", layout],
            ["margin", margin],
            ["gap", gap],
            ["cols", cols],
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
         * corner, and device pixels per content unit. Live via `set`, and
         * emitted as `"view" x y zoom` when a gesture moves them.
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
    return node("scroll", {
        ...rest,
        ...drop([
            ["axis", axis],
            ["zoom", flag(zoom)],
            ["content_w", contentW],
            ["content_h", contentH],
            ["view_x", viewX],
            ["view_y", viewY],
            ["view_zoom", viewZoom],
            ["layout", layout],
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
 * Static `label` text. `textSize` is the glyph scale over the host's embedded
 * 5x7 font (default 2.0 — every text-bearing widget takes it); `wrap` word-
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
    return node("slider", { ...rest, ...props, ...drop([["vertical", vertical]]) });
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
 * A `menu` selector over `options` (strings); a click cycles to the next and
 * emits the chosen `index`.
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
    } = {},
): GuiNode {
    const {
        cache, path, buffer, data, blob, channels, baseBucket, overlay,
        rulerY, bitDepth, ...timeline
    } = options;
    return node("waveform", {
        ...timelineProps(timeline),
        ...sourceProps({ cache, path, buffer, data, blob, channels }),
        ...drop([
            ["base_bucket", baseBucket],
            ["overlay", flag(overlay)],
            ["ruler_y", rulerY],
            ["bit_depth", bitDepth],
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
        colormap?: number;
        /** The frequency ruler: `"hz"` (the default) or `"off"`. */
        rulerY?: string;
    } = {},
): GuiNode {
    const {
        cache, path, buffer, data, blob, channels, windowSize, hop,
        dbFloor, dbCeil, freqScale, colormap, rulerY, ...timeline
    } = options;
    return node("spectrogram", {
        ...timelineProps(timeline),
        ...sourceProps({ cache, path, buffer, data, blob, channels }),
        ...drop([
            ["window_size", windowSize],
            ["hop", hop],
            ["db_floor", dbFloor],
            ["db_ceil", dbCeil],
            ["freq_scale", freqScale],
            ["colormap", colormap],
            ["ruler_y", rulerY],
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
    return node("plot", {
        ...rest,
        ...sourceProps({ cache, path, buffer, data, blob, channels }),
        ...drop([
            ["view", view],
            ["overlay", flag(overlay)],
            ["sample_rate", sampleRate],
            ["min", min],
            ["max", max],
            ["ruler", ruler],
            ["ruler_y", rulerY],
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
    return node("scope", {
        ...rest,
        bus,
        rate,
        ...drop([
            ["channels", channels],
            ["overlay", flag(overlay)],
            ["window_ms", windowMs],
            ["trigger", trigger],
            ["hold", flag(hold)],
            ["min", min],
            ["max", max],
            ["ruler", strip(ruler)],
            ["ruler_y", strip(rulerY)],
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
    return node("phasescope", {
        ...rest,
        bus,
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
 */
export function spectrum(
    bus = 0,
    options: WidgetOptions & {
        channels?: number;
        fftSize?: number;
        dbFloor?: number;
        dbCeil?: number;
        freqScale?: string;
        averaging?: number;
        peakHold?: boolean;
        ruler?: boolean | string;
        rulerY?: boolean | string;
        label?: string;
    } = {},
): GuiNode {
    const {
        channels, fftSize, dbFloor, dbCeil, freqScale, averaging, peakHold,
        ruler, rulerY, label: text, ...rest
    } = options;
    return node("spectrum", {
        ...rest,
        bus,
        ...drop([
            ["channels", channels],
            ["fft_size", fftSize],
            ["db_floor", dbFloor],
            ["db_ceil", dbCeil],
            ["freq_scale", freqScale],
            ["averaging", averaging],
            ["peak_hold", flag(peakHold)],
            ["ruler", strip(ruler)],
            ["ruler_y", strip(rulerY)],
            ["label", text],
        ]),
    });
}

/**
 * A live `nodetree` view of the audio server's node tree rooted at `group`
 * (default the root group). The host mirrors the server's tree over its
 * client leg, so creations, deaths and `/n_set` edits show live. `controls`
 * (default true) shows each synth's control name/value pairs. Read-only.
 */
export function nodetree(
    options: WidgetOptions & { group?: number; controls?: boolean; label?: string } = {},
): GuiNode {
    const { group = 0, controls, label: text, ...rest } = options;
    return node("nodetree", {
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
    return node("bpf", {
        ...rest,
        ...drop([
            ["points", points === undefined ? undefined : flatPoints(points)],
            ["min", min],
            ["max", max],
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
        /** A static cursor, for a located, stopped transport. */
        playhead?: number;
        label?: string;
    } = {},
): GuiNode {
    const {
        notes, osc, min, max, snap, velocity, oscLane, midiIn, playhead,
        label: text, ...timeline
    } = options;
    return node("pianoroll", {
        ...timelineProps(timeline),
        ...drop([
            ["notes", notes === undefined ? undefined : flatNotes(notes)],
            ["osc", osc === undefined ? undefined : flatOsc(osc)],
            ["min", min],
            ["max", max],
            ["snap", snap],
            ["velocity", flag(velocity)],
            ["osc_lane", flag(oscLane)],
            ["midi_in", flag(midiIn)],
            ["playhead", playhead],
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
        /** Extra `[name, value]` control pairs for the host's `/s_new`. */
        voiceArgs?: readonly (readonly [string, number])[];
        label?: string;
    } = {},
): GuiNode {
    const {
        min, max, activeMin, activeMax, pan, overview, velocity, channel,
        voice, voiceArgs, label: text, ...rest
    } = options;
    return node("piano", {
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
 * It reads the axis of the group named by `link` — give it the lanes' link id —
 * and its ticks are indented by a lane's header width, so they stand over the
 * samples they label. A press locates the transport, Shift+drag pans and the
 * wheel zooms: you scrub on the ruler. `h` is its thickness in device pixels.
 */
export function timeruler(options: TimelineOptions = {}): GuiNode {
    const { h = 20, ...timeline } = options;
    return node("timeruler", { ...timelineProps(timeline), h });
}

/**
 * A multitrack `track` lane holding `clip` children on a shared time axis —
 * the DAW-style editor's lane. `label` names it in a left header, `height` is
 * its lane weight, and `snap` is the drag grid a clip's move/resize rounds
 * to. The lanes of a window navigate as one (the same `link` group the heavy
 * views use), and the lane carries the same time chrome.
 */
export function track(
    options: TimelineOptions & {
        label?: string;
        height?: number;
        snap?: number;
        theme?: Record<string, string>;
        children?: readonly GuiNode[];
    } = {},
    ...clips: GuiNode[]
): GuiNode {
    const { label: text, height, snap, theme, children, ...timeline } = options;
    return node("track", {
        ...timelineProps(timeline),
        ...drop([
            ["label", text],
            ["height", height],
            ["snap", snap],
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
 * are — `cache`/`path`/`buffer`/`data`/`blob`, summarized through the take's
 * peak pyramid to fit the rectangle), a **piano-roll** of `notes`, or an
 * **automation curve** of `points` editable in place. Dragging the clip
 * (move) or its edge (resize) flows back as a `"clip"` event carrying the new
 * `offset`/`dur`.
 */
export function clip(
    options: SourceOptions & {
        /** The clip's start on the shared timeline (samples). */
        offset?: number;
        /** Its duration (samples) — a clip with no duration draws nothing. */
        dur: number;
        baseBucket?: number;
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
        baseBucket, notes, points, exp, min, max, label: text, ...rest
    } = options;
    return node("clip", {
        ...rest,
        dur,
        offset,
        ...sourceProps({ cache, path, buffer, data, blob, channels }),
        ...drop([
            ["base_bucket", baseBucket],
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
        sampleRate?: number;
        selected?: string;
        editable?: boolean;
    } = {},
): GuiNode {
    const {
        displayList, playhead, playheadAt, sampleRate, selected, editable, ...rest
    } = options;
    const dl = displayList ?? {};
    return node("score", {
        ...rest,
        ...drop([
            ["playhead", playhead],
            ["playhead_at", playheadAt],
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
    return node("patch", {
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
function timelineProps(options: TimelineOptions): Props {
    const {
        ruler, sampleRate, tempo, beatAt, quant, selStart, selLen,
        playheadAt, playhead, playheadLoopStart, playheadLoopLen,
        yStart, yLen, link, ...rest
    } = options;
    return {
        ...rest,
        ...drop([
            ["ruler", ruler],
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
            ["y_start", yStart],
            ["y_len", yLen],
            ["link", link],
        ]),
    };
}

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
export function samplesToBlob(samples: Iterable<number>): Uint8Array {
    const floats = Float32Array.from(samples);
    const bytes = new Uint8Array(floats.buffer, floats.byteOffset, floats.byteLength);
    if (LITTLE_ENDIAN) return bytes;
    const view = new DataView(bytes.buffer.slice(0));
    for (let i = 0; i < floats.length; i++) view.setFloat32(i * 4, floats[i]!, true);
    return new Uint8Array(view.buffer);
}

/**
 * Whether this runtime's typed arrays are already little-endian (every
 * browser and node target in practice; the check keeps the blob correct
 * wherever they are not).
 */
const LITTLE_ENDIAN = new Uint8Array(Uint16Array.of(1).buffer)[0] === 1;
