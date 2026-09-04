// The free-standing `plot` — one verb for looking at a signal (mirrors
// `clausters/plot.py`).
//
// `plot` is the visual sibling of `play`: it plots whatever you hand it,
// resolving the ambient GUI host so a quick look never spells one out. Each
// call opens its **own window**. It dispatches by kind:
//
// - a **def** (`SynthDef` / `FaustDef` / `GraphDef`) is **rendered offline**
//   (an ephemeral NRT session: sent, instanced with `controls`, freed at
//   `dur`) and its output plotted, every channel in its own lane — the way to
//   eyeball what a def actually produces with no server and no audio device;
// - a bare **expression** takes the same offline path through the
//   ephemeral-def coercion `play` uses, so `plot(sine(440).mul(0.5))` shows
//   the signal directly. It plots as wide as it writes;
// - an `Env` is rendered through the engine's own `envGen`, so the drawn curve
//   is exactly what the engine plays — not a second evaluation of the same
//   break points. An `Automation` plots the same way (its curve *is* an
//   `Env`), labelled with the automation's control name;
// - a `Buffer` (or a buffer number) is fetched from the ambient **live**
//   server with its shape and rate — the way to check a buffer's contents;
// - any other **iterable of numbers** — an array, a `Float32Array`, a
//   `Pattern` — is read (up to `n` values for the endless ones) and
//   plotted as a sequence: index counts on the x axis, the value axis fitted
//   to the data.
//
// `view: "spectrum"` plots the averaged magnitude spectrum instead. Either way
// the window is static — no zoom, pan or editing — but measured: the rulers
// fit the data and hovering reads out the sample or bin under the cursor.
//
// **Asynchronous, where the reference verb is not.** Opening a host, fetching
// a buffer and running a render all wait, and a page may not block; so `plot`
// resolves with its window rather than returning it. That is this client's one
// standing difference, not a difference in the verb.
//
// ```ts
// await plot(sine(440.0).mul(0.2), { dur: 0.02 });
// await plot(Env.adsr(), { label: "adsr" });
// await plot(new seq.Pwhite(40.0, 4700.0), { n: 200 });
// ```

import { main } from "./base/main.ts";
import { loadCore } from "./base/core.ts";
import { asDef, exprChannels, isExpr } from "./defs/asdef.ts";
import { Buffer } from "./defs/buffer.ts";
import { FaustDef } from "./defs/faustdef.ts";
import { GraphDef } from "./defs/graphdef.ts";
import { Synth } from "./defs/node.ts";
import type { Controls } from "./defs/node.ts";
import { SynthDef } from "./defs/synthdef.ts";
import { Env, control, envGen, out } from "./defs/ugens/index.ts";
import { GuiHost, pageGuiConnection } from "./gui/host.ts";
import type { Stage } from "./gui/host.ts";
import type { PropValue } from "./gui/host.ts";
import { ambientHost } from "./gui/ambient.ts";
import * as guidef from "./gui/guidef.ts";
import { Automation } from "./seq/automation.ts";
import { Pattern } from "./seq/pattern.ts";
import { bounceDef } from "./render.ts";

/** The module's own host, opened lazily when no session brought one. */
let ownHost: GuiHost | null = null;

/**
 * One open plot window: its GUI `host`, the window `id` and the plot widget's
 * id, so the display stays adjustable after the fact.
 *
 * ```ts
 * const win = await plot(seq);
 * win.set({ view: "spectrum", freqScale: "mel" });   // live
 * win.close();
 * ```
 */
export class PlotWindow {
    readonly host: GuiHost;
    readonly id: number;
    readonly widgetId: number;

    constructor(host: GuiHost, id: number, widgetId: number) {
        this.host = host;
        this.id = id;
        this.widgetId = widgetId;
    }

    /**
     * Live-sets plot props (`view`, `min`/`max` — a number, or `"auto"` to
     * refit — `freqScale`, `dbFloor`/`dbCeil`, `ruler`/`rulerY`, `label`…)
     * through `/gui_set`.
     */
    set(props: Record<string, PropValue>): this {
        this.host.set(this.widgetId, props);
        return this;
    }

    /**
     * Calls `handler` when the viewer closes this window (a `/gui_closed`).
     * `null` clears it.
     */
    onClosed(handler: (() => void) | null): this {
        this.host.setClosedHandler(this.id, handler);
        return this;
    }

    /** Whether this window is gone — closed by a hand or by `close`. */
    get closed(): boolean {
        return !this.host.isOpen(this.id);
    }

    /**
     * Resolves when this window is closed, or on `timeout` seconds — `true` for
     * the first, `false` for the second. The same verb
     * {@link GuiHost.wait} and `WindowHandle.wait` carry.
     */
    wait(timeout?: number): Promise<boolean> {
        return this.host.waitWhile(() => !this.closed, timeout);
    }

    /** Closes the window (`/gui_free`). */
    close(): void {
        this.host.close(this.id);
    }
}

/**
 * One open patcher window — a def's **structure** (not its sound): its GUI
 * `host`, the window `id` and the `patch` widget's id.
 *
 * ```ts
 * const win = await myGraphdef.plotDef();
 * win.close();
 * ```
 */
export class PatchWindow {
    readonly host: GuiHost;
    readonly id: number;
    readonly widgetId: number;

    constructor(host: GuiHost, id: number, widgetId: number) {
        this.host = host;
        this.id = id;
        this.widgetId = widgetId;
    }

    /** Live-sets the patch widget's props (`label`, `boxes`, `cords`…) through `/gui_set`. */
    set(props: Record<string, PropValue>): this {
        this.host.set(this.widgetId, props);
        return this;
    }

    /**
     * Calls `handler` when the viewer closes this window (a `/gui_closed`).
     * `null` clears it.
     */
    onClosed(handler: (() => void) | null): this {
        this.host.setClosedHandler(this.id, handler);
        return this;
    }

    /** Whether this window is gone — closed by a hand or by `close`. */
    get closed(): boolean {
        return !this.host.isOpen(this.id);
    }

    /**
     * Resolves when this window is closed, or on `timeout` seconds — `true` for
     * the first, `false` for the second. The same verb
     * {@link GuiHost.wait} and `WindowHandle.wait` carry.
     */
    wait(timeout?: number): Promise<boolean> {
        return this.host.waitWhile(() => !this.closed, timeout);
    }

    /** Closes the window (`/gui_free`). */
    close(): void {
        this.host.close(this.id);
    }
}

/** What a def's `plotDef` takes. */
export interface PatchViewOptions {
    /** Captions the patch panel; absent, the caller's own default. */
    label?: string;
    /** Window width. */
    w?: number;
    /** Window height. */
    h?: number;
    /** Window title; absent, the def's name. */
    title?: string;
    /** An explicit host; absent, the ambient one. */
    host?: GuiHost;
    /**
     * Where a page draws it: the view takes this element's box and the canvas
     * inside it is made for you. Web-only — a script has an OS window, so the
     * Python client's counterpart of this verb takes no such argument (and a
     * host reached over a socket refuses one).
     */
    element?: Stage | null;
}

/**
 * Open a {@link GraphPatch} or {@link DefPatch} as a directed `patch` view in
 * its own window on the ambient GUI host — the structure opener behind the
 * `plotDef` methods. One window per call, the `plot` posture: the patch sits in
 * a `scroll` workspace (pan/zoom), no audio server involved. The **host lays
 * the boxes out** and sizes the scrollable canvas from the graph's own extent
 * (never below the window), so the model carries no geometry: a small graph
 * centres in the window, a large one fills the content and pans.
 *
 * @internal — exported for the def classes' `plotDef`, which is the surface.
 */
export async function openPatchView(
    model: { toWidget(): { boxes: Record<string, unknown>[]; cords: number[] } },
    options: PatchViewOptions = {},
): Promise<PatchWindow> {
    const { label, w = 1000, h = 700, title, host: explicitHost, element } = options;
    const host = explicitHost ?? await resolveHost();
    const widgetId = host.allocId();
    const view = guidef.patch({ id: widgetId, ...model.toWidget(), label });
    const workspace = guidef.scroll({ id: host.allocId() }, view);
    const tree = guidef.view({ title: title ?? label ?? "patch", w, h }, workspace);
    const handle = host.open(tree, { element });
    return new PatchWindow(host, handle.id, widgetId);
}

/** What `plot` accepts. */
export type Plottable =
    | SynthDef
    | FaustDef
    | GraphDef
    | Env
    | Automation
    | Buffer
    | number
    | Iterable<number>
    | Iterable<Iterable<number>>
    | Pattern<unknown>
    // A bare expression (Ugen / ChannelList / Signal), which has no one type.
    | object;

export interface PlotOptions {
    /** Seconds a def or expression is held before it is freed. */
    dur?: number;
    /** Controls (ports, for a `GraphDef`) the instance is started with. */
    controls?: Controls;
    /** Extra defs the render needs first — a `GraphDef`'s member defs. */
    defs?: readonly (SynthDef | FaustDef | GraphDef)[];
    /** How many values to take from an endless sequence. */
    n?: number;
    /** The offline render's rate; also places a fetched buffer's time axis. */
    sampleRate?: number;
    /**
     * How many channels to show. Absent it is derived — a bare expression is
     * as wide as it writes, an already-built def defaults to 2. Ignored by the
     * other kinds (a buffer brings its own; a sequence infers it).
     */
    channels?: number;
    /** `"signal"` (default) or `"spectrum"`. */
    view?: string;
    /** Draw channels as overlaid traces instead of lanes. */
    overlay?: boolean;
    /** Value-axis sides of the signal view; absent, that side auto-fits. */
    min?: number;
    max?: number;
    /** Spectrum frequency axis: `"log"` (default), `"linear"`, `"mel"`, `"bark"`. */
    freqScale?: string;
    /** Spectrum analysis size (a power of two). */
    fftSize?: number;
    /** Spectrum dB window. */
    dbFloor?: number;
    dbCeil?: number;
    /** The signal view's time unit: `"time"`, `"samples"` or `"off"`. */
    ruler?: string;
    /** `"off"` hides the value-axis strip. */
    rulerY?: string;
    /** The plot's label strip; defaults to something sensible per kind. */
    label?: string;
    /** The window title (defaults to the label). */
    title?: string;
    /** Window width in px. */
    w?: number;
    /** Window height (default sized to the channel count). */
    h?: number;
    /** An explicit host; absent, the ambient one. */
    host?: GuiHost;
    /**
     * Where a page draws it: the view takes this element's box and the canvas
     * inside it is made for you. Web-only — a script has an OS window, so the
     * Python client's counterpart of this verb takes no such argument (and a
     * host reached over a socket refuses one).
     */
    element?: Stage | null;
}

/**
 * Plots `obj` in its own window on the ambient GUI host, and resolves with the
 * `PlotWindow` — `set(...)` retunes the display live, `close()` closes it.
 */
export async function plot(
    obj: Plottable,
    options: PlotOptions = {},
): Promise<PlotWindow> {
    const {
        dur = 1.0, controls, defs = [], n = 1024, sampleRate = 48_000.0,
        channels, view, overlay, min, max, freqScale, fftSize, dbFloor, dbCeil,
        ruler, rulerY, label, title, w = 760, h, host: explicitHost, element,
    } = options;

    const drawn = await resolve(obj, {
        dur, controls, defs, n, sampleRate, channels,
    });
    const text = label ?? drawn.label;
    const host = explicitHost ?? await resolveHost();

    // Widget ids live in the host's one namespace (every window, every script
    // on it), so each plot's widget takes a fresh one — a repeated id would be
    // skipped at define time and `set` would reach whichever widget claimed it
    // first.
    const widgetId = host.allocId();
    const props: Record<string, unknown> = {
        id: widgetId,
        channels: drawn.channels,
        view,
        overlay: overlay || undefined,
        min,
        max,
        freqScale,
        fftSize,
        dbFloor,
        dbCeil,
        ruler,
        rulerY,
        label: text,
    };
    if (drawn.sampleRate > 0) props.sampleRate = drawn.sampleRate;
    else if (ruler === undefined) props.ruler = "samples"; // no rate: read in counts

    let widget: guidef.GuiNode;
    const blobs: Uint8Array[] = [];
    if (drawn.samples.length <= guidef.INLINE_MAX) {
        widget = guidef.plot({ ...props, data: [...drawn.samples] });
    } else {
        // A page shares no filesystem with its host, so the samples travel
        // with the message — beside the JSON, not inside it.
        blobs.push(guidef.samplesToBlob(drawn.samples));
        widget = guidef.plot({ ...props, blob: 0 });
    }
    const height = h ?? (drawn.channels <= 1 ? 260 : 160 + 140 * drawn.channels);
    const tree = guidef.view({ title: title ?? text ?? "plot", w, h: height }, widget);
    const handle = host.open(tree, { blobs, element });
    return new PlotWindow(host, handle.id, widgetId);
}

// ---- dispatch: turning the object into interleaved samples ----

interface Drawn {
    samples: Float32Array | number[];
    channels: number;
    /** 0 marks an index (sequence) axis rather than a time one. */
    sampleRate: number;
    label: string;
}

async function resolve(
    obj: Plottable,
    {
        dur, controls, defs, n, sampleRate, channels,
    }: {
        dur: number;
        controls?: Controls;
        defs: readonly (SynthDef | FaustDef | GraphDef)[];
        n: number;
        sampleRate: number;
        channels?: number;
    },
): Promise<Drawn> {
    if (obj instanceof Env) return renderEnv(obj, sampleRate, "env");
    if (obj instanceof Automation) {
        return renderEnv(obj.env, sampleRate, obj.name);
    }
    if (isExpr(obj)) {
        // Plot configures its render for what is being looked at, so the
        // expression is as wide as it writes (`exprChannels` is the one place
        // that knows). One that routes itself entirely says 0, and there is
        // nothing to infer: fall back to a stereo look.
        const width = channels ?? (exprChannels(obj) || 2);
        const stats = await bounceDef(asDef(obj), {
            dur, controls, defs, sampleRate, channels: width,
        });
        return { samples: stats.samples, channels: width, sampleRate, label: "expr" };
    }
    if (obj instanceof SynthDef || obj instanceof FaustDef || obj instanceof GraphDef) {
        const width = channels ?? 2;
        const stats = await bounceDef(obj, {
            dur, controls, defs, sampleRate, channels: width,
        });
        return { samples: stats.samples, channels: width, sampleRate, label: obj.name };
    }
    if (obj instanceof Buffer || typeof obj === "number") {
        return fetchBuffer(obj, sampleRate);
    }
    return sequence(obj as Iterable<number>, n);
}

/**
 * Renders an `Env` through the engine's own `envGen` — what you plot is what
 * an `envGen` plays, rather than a second evaluation of the same break points.
 * A sustained envelope (one with a `releaseNode`) has its gate closed at the
 * sustain point, so the release segments show too.
 */
/**
 * The samples `plot(Env)` draws — the envelope rendered through the engine's
 * own `envGen`, without the window around it. Exposed because it is the thing
 * worth comparing against the reference client: both render an envelope the
 * same way, so the drawn curve is comparable across clients rather than only
 * within one.
 */
export async function renderEnvSamples(
    env: Env,
    sampleRate = 48_000.0,
): Promise<{ samples: Float32Array; channels: number }> {
    const drawn = await renderEnv(env, sampleRate, "env");
    return { samples: drawn.samples as Float32Array, channels: drawn.channels };
}

async function renderEnv(
    env: Env,
    sampleRate: number,
    label: string,
): Promise<Drawn> {
    const { Session } = await import("./session.ts");
    const total = env.times.reduce((a, b) => a + b, 0) || 1.0;
    const session = await Session.nrt({ tempo: 1.0 });
    const server = session.server;
    const gate = control("gate", 1.0);
    const def = new SynthDef("_plot_env", out(0.0, envGen(env, { gate })));
    await def.send(server);
    const node = new Synth(def.name, undefined, { server });
    if (env.releaseNode !== undefined) {
        const sustainAt = env.times
            .slice(0, env.releaseNode)
            .reduce((a, b) => a + b, 0);
        server.sendBundleAfter(sustainAt, [
            ["/node_set", ["i", node.id], "gate", ["f", 0.0]],
        ]);
    }
    server.sendBundleAfter(total, [["/node_free", ["i", node.id]]]);
    const stats = await session.render({ sampleRate, channels: 1 });
    return { samples: stats.samples, channels: 1, sampleRate, label };
}

/**
 * Fetches a buffer's interleaved samples and shape from the ambient **live**
 * server — the buffer-contents check.
 */
async function fetchBuffer(
    target: Buffer | number,
    fallbackRate: number,
): Promise<Drawn> {
    const buffer = target instanceof Buffer
        ? target
        : new Buffer(target, 0, 1, 0.0, main.resolveServer());
    await buffer.info();
    const samples = await buffer.getSamples();
    const rate = buffer.sampleRate > 0 ? buffer.sampleRate : fallbackRate;
    return {
        samples,
        channels: Math.max(1, buffer.channels),
        sampleRate: rate,
        label: `buffer ${buffer.bufnum}`,
    };
}

/**
 * Takes up to `n` values from an iterable of numbers (or of per-channel rows,
 * interleaved). The rate is 0: the x axis reads in
 * index counts and the value range auto-fits.
 */
function sequence(obj: Iterable<number>, n: number): Drawn {
    const first = take(obj, n);
    if (first.length > 0 && isRow(first[0])) {
        const rows = (first as unknown[]).map((row) => take(row as Iterable<number>, n));
        const frames = Math.min(...rows.map((row) => row.length));
        const interleaved: number[] = [];
        for (let f = 0; f < frames; f++) {
            for (const row of rows) interleaved.push(Number(row[f]));
        }
        return {
            samples: interleaved,
            channels: rows.length,
            sampleRate: 0,
            label: "sequence",
        };
    }
    return {
        samples: first.map(Number),
        channels: 1,
        sampleRate: 0,
        label: "sequence",
    };
}

/** The first `n` items of anything iterable — a `Pattern` included. */
function take(obj: Iterable<unknown>, n: number): unknown[] {
    const out: unknown[] = [];
    for (const value of obj) {
        out.push(value);
        if (out.length === n) break;
    }
    return out;
}

/** A per-channel row: iterable, but not a number. */
function isRow(value: unknown): boolean {
    return typeof value !== "number" && typeof value !== "string"
        && value !== null && typeof value === "object"
        && typeof (value as { [Symbol.iterator]?: unknown })[Symbol.iterator] === "function";
}

/**
 * The GUI host the ambient visual verbs open windows on: one registered
 * through `setAmbientHost` if there is one, else the current (else default)
 * session's host when one is already up, else a host this module opens once
 * and owns.
 *
 * A registered host wins outright: it is a front this module could not have
 * opened itself, which is the whole reason it was registered.
 *
 * @internal — exported for `./scope.ts`, the other ambient visual verb, which
 * resolves through the same ladder and shares this module's own host. The
 * Python client shares it the same way, as `plot._ambient_host`.
 */
export async function resolveHost(): Promise<GuiHost> {
    const registered = ambientHost();
    if (registered) return registered;
    const session = main.currentSession as { guiHost?: GuiHost | null } | null;
    const sessionHost = session?.guiHost ?? null;
    if (sessionHost) return sessionHost;
    // A host of our own: the core wasm has to be in before one exists (its id
    // allocators are core registries), and the ambient verbs are exactly the
    // surface that resolves what it needs rather than asking for it. A page
    // that opened a `Session` first has already paid this; one that only wants
    // to look at something has not.
    await loadCore();
    // Not the ambient one: this host is the *fallback* of the ambient ladder,
    // and registering it would make the fallback outrank a session opened
    // afterwards. The reference client draws the same line
    // (`plot._ambient_host` boots with `adopt_ambient=False`).
    ownHost ??= await new GuiHost(await pageGuiConnection())
        .boot({ adoptAmbient: false });
    return ownHost;
}
