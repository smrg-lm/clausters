// `scope` — watch live audio buses in a window (mirrors `clausters/scope.py`).
//
// The real-time sibling of `./plot.ts`: one call opens a window that follows
// `channels` consecutive audio buses of the running server, frame by frame,
// with no per-frame messages from the script. Everything is wired for you —
// the ambient server and GUI host are resolved, and the host asks the server
// to record the buses it draws — so you name a bus and nothing else.
//
// ```ts
// const win = await scope();                     // hardware out 0, oscilloscope
// await scope(0, { channels: 2 });               // outs 0/1, one lane each
// await scope(bus);                              // a Bus watches all its channels
// await scope(0, { view: "phase" });             // stereo field of outs 0/1
// await scope(0, { view: "spectrum", channels: 2, freqScale: "mel" });
// ```
//
// **The three views** (`view`):
//
// - `"signal"` — a triggered **oscilloscope**. Each channel is a lane (or a
//   color-coded trace with `overlay`); the x ruler reads milliseconds of the
//   `windowMs` display window, the y ruler signal value over `[min, max]`. The
//   trace is *phase-locked*: every frame is aligned on a rising crossing of
//   the `trigger` level (marked by a faint line) found in the **first**
//   channel, so a periodic signal stands still and the channels keep their
//   true relative phase. The corner read-out says `lock` (the trigger fired)
//   or `free` (no crossing — silence or DC — so the window free-runs).
// - `"phase"` — a **phasescope** (goniometer) of the stereo pair `bus` /
//   `bus + 1`: mono draws a vertical line, anti-phase horizontal, a wide field
//   fills the lozenge; the bar underneath is the correlation.
// - `"spectrum"` — a live **spectrum**: one FFT per channel per frame, one
//   color-coded curve each; the x ruler reads hertz on `freqScale`
//   (log/linear/mel/bark), the y ruler dB over `[dbFloor, dbCeil]`.
//
// Adjust it live with `win.set({...})` — any prop of the open view — and close
// it with `win.close()`, which closes the window and lets the host stop
// recording whatever no open view is drawing any more.
//
// **What does not port: the shared-memory requirement.** The reference client
// refuses a server with no `shm`, because the native host reads the taps out
// of that segment. The browser host has no segment to map and streams the taps
// over its own server leg instead, so there is nothing here to demand — which
// is why this module has no check where its sibling has one.
//
// **Asynchronous, where the reference verb is not**, the same standing
// difference `plot` carries: resolving a host may open one, and a page may not
// block, so `scope` resolves with its window rather than returning it.

import { main } from "./base/main.ts";
import { Bus } from "./defs/bus.ts";
import type { Server } from "./defs/server/index.ts";
import * as guidef from "./gui/guidef.ts";
import type { GuiHost, PropValue } from "./gui/host.ts";
import { resolveHost } from "./plot.ts";

/** Which of the three views a scope window draws. */
export type ScopeView = "signal" | "phase" | "spectrum";

// Written out rather than inferred from the array: a type derived from a
// `const` array carries the array into the API reference, and the array is an
// implementation detail of the argument check below.
const VIEWS: readonly ScopeView[] = ["signal", "phase", "spectrum"];

/**
 * One open scope window: the GUI `host`, the window `id` and the scope
 * widget's id. `set` retunes the display live; `close` closes the window, and
 * the host stops the recording nothing is drawing any more.
 *
 * ```ts
 * const win = await scope(bus, { view: "spectrum" });
 * win.set({ freqScale: "mel", fftSize: 4096 });   // /gui_set, live
 * win.close();                                    // /gui_free
 * ```
 */
export class ScopeWindow {
    readonly host: GuiHost;
    readonly id: number;
    readonly widgetId: number;
    readonly server: Server;
    /** First bus of the adjacent run this window watches (`channels` of them). */
    readonly bus: number;
    readonly channels: number;
    #closed = false;

    constructor(
        host: GuiHost,
        id: number,
        widgetId: number,
        server: Server,
        bus: number,
        channels: number,
    ) {
        this.host = host;
        this.id = id;
        this.widgetId = widgetId;
        this.server = server;
        this.bus = bus;
        this.channels = channels;
    }

    /**
     * Live-sets the scope widget's props through `/gui_set` — per view:
     * `windowMs`/`trigger`/`hold`/`min`/`max`/`overlay` (signal),
     * `windowMs`/`hold` (phase), `fftSize`/`freqScale`/`dbFloor`/`dbCeil`/
     * `averaging`/`peakHold` (spectrum); `ruler`/`rulerY` (`"off"` hides an
     * axis strip) and `label` on any. `bus` and `channels` retarget it.
     */
    set(props: Record<string, PropValue>): this {
        this.host.set(this.widgetId, props);
        return this;
    }

    /**
     * Closes the window (`/gui_free`). Idempotent. The recording behind it is
     * the host's business: it stops what no open view reads.
     */
    close(): void {
        if (this.#closed) return;
        this.#closed = true;
        this.host.close(this.id);
    }
}

/** What `scope` takes besides the bus. */
export interface ScopeOptions {
    /** `"signal"` (oscilloscope, default), `"phase"` or `"spectrum"`. */
    view?: ScopeView;
    /**
     * How many consecutive buses to monitor. Absent: a `Bus`'s own channel
     * count, else 1; the phase view is fixed at 2.
     */
    channels?: number;
    /** Signal view — color-coded traces in one field instead of stacked lanes. */
    overlay?: boolean;
    /**
     * The display window — signal (default 20 ms) and phase (trail
     * persistence, default 30 ms) views.
     */
    windowMs?: number;
    /**
     * Signal view — the rising-crossing trigger level (default 0; searched in
     * the first channel, marked by a faint line).
     */
    trigger?: number;
    /** Freeze the trace (signal/phase; also live through `set`). */
    hold?: boolean;
    /** Vertical range of the signal view (default −1 / 1). */
    min?: number;
    max?: number;
    /** Spectrum analysis size (a power of two, 256..4096, default 2048). */
    fftSize?: number;
    /** Spectrum dB window (default −100 / 0). */
    dbFloor?: number;
    dbCeil?: number;
    /** Spectrum frequency axis: `"log"` (default), `"linear"`, `"mel"`, `"bark"`. */
    freqScale?: string;
    /** Spectrum per-bin exponential smoothing, 0..1 (default 0.5). */
    averaging?: number;
    /** Spectrum — overlay a slowly decaying peak trace. */
    peakHold?: boolean;
    /**
     * The x axis strip (ms / Hz per view), shown by default; `false` or
     * `"off"` hides it. The phase view has no rulers.
     */
    ruler?: boolean | string;
    /** The y axis strip (value / dB), likewise. */
    rulerY?: boolean | string;
    /** The widget's label strip (defaults to the buses, per view). */
    label?: string;
    /** The window title (defaults to the label). */
    title?: string;
    /** Window width in px. */
    w?: number;
    /** Window height (default sized per view and channel count). */
    h?: number;
    /** An explicit server; absent, the ambient one. */
    server?: Server;
    /** An explicit host; absent, the ambient one. */
    host?: GuiHost;
}

/**
 * Watches `channels` consecutive audio buses from `bus` in a window, and
 * resolves with the `ScopeWindow` — `set(...)` retunes the display live,
 * `close()` closes it.
 *
 * The signal and spectrum views monitor `bus .. bus + channels - 1`; the phase
 * view is the two-channel case and always reads the pair `bus` / `bus + 1`.
 */
export async function scope(
    bus: Bus | number = 0,
    options: ScopeOptions = {},
): Promise<ScopeWindow> {
    const {
        view = "signal", overlay, windowMs, trigger, hold, min, max, fftSize,
        dbFloor, dbCeil, freqScale, averaging, peakHold, ruler, rulerY, label,
        title, w = 480, h, server: explicitServer, host: explicitHost,
    } = options;

    if (!VIEWS.includes(view)) {
        throw new TypeError(
            `unknown view '${String(view)}' (one of ${VIEWS.join(", ")})`,
        );
    }
    let channels = options.channels;
    if (view === "phase") {
        if (channels !== undefined && channels !== 2) {
            throw new TypeError(
                "view 'phase' reads exactly 2 channels (bus and bus + 1), got " +
                    `channels=${String(channels)}`,
            );
        }
        channels = 2;
    } else {
        channels ??= bus instanceof Bus ? bus.channels : 1;
    }
    if (channels < 1) {
        throw new TypeError(`channels must be >= 1, got ${String(channels)}`);
    }

    const server = main.resolveServer(explicitServer);
    const host = explicitHost ?? await resolveHost();
    const index = bus instanceof Bus ? bus.index : Math.trunc(bus);

    const text = label ?? (
        view === "phase" ? `bus ${String(index)}/${String(index + 1)}`
            : channels > 1 ? `bus ${String(index)}-${String(index + channels - 1)}`
            : `bus ${String(index)}`
    );

    // Widget ids live in the host's one namespace, so each scope's widget
    // takes a fresh one — the same rule `plot` follows and for the same
    // reason: a repeated id is skipped at define time and `set` would then
    // reach whichever widget claimed it first.
    const widgetId = host.allocId();
    let widget: guidef.GuiNode;
    let height: number;
    if (view === "signal") {
        widget = guidef.scope(index, {
            id: widgetId, channels, overlay, windowMs, trigger, hold, min, max,
            ruler, rulerY, label: text,
        });
        height = h ?? 200 + 90 * (overlay ? 1 : channels);
    } else if (view === "phase") {
        widget = guidef.phasescope(index, {
            id: widgetId, windowMs, hold, label: text,
        });
        height = h ?? 420;
    } else {
        widget = guidef.spectrum(index, {
            id: widgetId, channels, fftSize, dbFloor, dbCeil, freqScale,
            averaging, peakHold, ruler, rulerY, label: text,
        });
        height = h ?? 280;
    }

    const tree = guidef.window({ title: title ?? text, w, h: height }, widget);
    const handle = host.open(tree);
    return new ScopeWindow(host, handle.id, widgetId, server, index, channels);
}
