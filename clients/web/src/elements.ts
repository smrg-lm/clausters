// The components: a bundle as an element of the document.
//
// `<clausters-bundle src="<bundle url>">` mounts a bundle into a canvas of its
// own. On the desktop `clausters-gui` opens a window per GuiDef and the window
// manager places it; here the canvas is an element and **the document places
// it** — CSS, the order of the markup, the flow of the page. That is the whole
// substitution: canvases interleave with prose and images, so one page can be
// an interactive text with the instrument sounding beside the paragraph that
// explains it.
//
// A bundle's declared parameters are attributes on the tag, with `preset`
// beside them; resolution is **attribute → preset → declared default**:
//
//     <fm-voice></fm-voice>                        the defaults
//     <fm-voice freq="440" title="voice 2"></fm-voice>
//     <fm-voice preset="bright" amp="0.1"></fm-voice>
//
// `defineComponent` is what gives a bundle its own tag; `<clausters-bundle
// src="...">` is the generic form for a page that would rather not generate a
// module.
//
// Mounting is two phases, because the host does not need audio and the engine
// does — and the AudioContext is page-wide, so N power buttons would be wrong:
// the GuiDef opens and draws on connect, and the engine half goes out on the
// first gesture **anywhere on the page**, whichever component received it.
// Until then a component shows its power affordance over its canvas;
// `<clausters-power>` is that affordance alone, for pages driving the raw
// singletons.
//
// Unmounting is the mirror of that, and it is **not** two phases: an element
// removed from the DOM gives back everything it took at once — its window, its
// nodes, its ids — and what the page shares (the engine, the host, the defs
// and the buffers) stays. Removing a component is therefore a complete
// undoing, and an element connected again mounts afresh from the same bundle,
// with a new allocation, rather than resuming the one it had. The other
// direction closes too: a window the *host* closes (a `/gui_closed`, which is
// what a native `--ws` host sends when the user closes the window a component
// mounted into) reaches the element, which unmounts and says so — no live tag
// over a freed def.
//
// Failures stay local: a component that cannot fetch or resolve its bundle
// shows the error on itself and emits `clausters-error`, and the rest of the
// page comes up. `clausters-ready` (detail: `{ id }`) fires per component with
// its resolved def id, and `clausters-closed` (detail: `{ id }`) when one is
// unmounted by its host.

import { server } from "./engine/server.ts";
import { canvasBox, guiHost, onScaleChange } from "./gui/page.ts";
import { decodePacket } from "./base/osc.ts";
import { freeBundle, openBundle, startBundle } from "./bundle.ts";
import type { Mounted } from "./bundle.ts";

const BUTTON_STYLE = `
    button {
        font: 14px system-ui, sans-serif;
        background: #22222c; color: #ddd; border: 1px solid #444;
        padding: 8px 18px; cursor: pointer;
    }
    button:disabled { opacity: 0.6; cursor: default; }
`;

/**
 * The components waiting for the page's first gesture, and whether it came.
 * The AudioContext is page-wide, so any component's power affordance — or a
 * `<clausters-power>` — starts every one of them.
 */
const waiting = new Set<ClaustersBundle>();
let gestured = false;

/** Starts every mounted component's engine half. Call from a user gesture. */
export async function startPage(): Promise<void> {
    gestured = true;
    const components = [...waiting];
    waiting.clear();
    const engine = await server();
    await engine.resume();
    // Per component, so one failure does not hold the others back.
    await Promise.allSettled(components.map((c) => c.start()));
}

export class ClaustersBundle extends HTMLElement {
    /**
     * The declared parameters, once the manifest is read — the attributes
     * this element answers to. `preset` and `src` are always among them.
     */
    private canvas: HTMLCanvasElement;
    private overlay: HTMLDivElement;
    private button: HTMLButtonElement;
    private mounted: Mounted | null = null;
    /** The in-flight phase 2, latched so every caller awaits the one send. */
    private starting: Promise<void> | null = null;
    private resizeObserver: ResizeObserver | null = null;
    private viewObserver: IntersectionObserver | null = null;
    /** Stops watching the display's scale (see `onScaleChange`). */
    private unwatchScale: (() => void) | null = null;
    /** Stops listening for this instance's `/gui_closed`. */
    private unwatchHost: (() => void) | null = null;
    /**
     * The mount and the unmount, one after another.
     *
     * Both are asynchronous and the DOM calls them synchronously — moving an
     * element is a disconnect immediately followed by a connect — so they queue
     * rather than race: an unmount that started must finish giving its ids back
     * before the mount that follows takes new ones.
     */
    private work: Promise<void> = Promise.resolve();
    /**
     * The bundle a generated tag mounts, when the markup names none — set by
     * `defineComponent`'s subclass and reflected into `src` on connect.
     *
     * It is a field rather than an attribute written in the constructor
     * because a custom element's constructor may not touch its attributes: a
     * tag that did threw on `document.createElement`, which is exactly how a
     * page adds a component from script.
     */
    protected defaultSrc: string | null = null;

    constructor() {
        super();
        const shadow = this.attachShadow({ mode: "open" });
        shadow.innerHTML = `
            <style>
                :host { display: block; position: relative; }
                /*
                  touch-action: none — a drag inside a widget is a *value*, not
                  a scroll. Without it a phone pans the page and the host never
                  sees the gesture. The page still scrolls everywhere else,
                  including the margins around this element.
                */
                canvas {
                    display: block; width: 100%; height: 100%;
                    touch-action: none; -webkit-user-select: none; user-select: none;
                }
                .overlay {
                    position: absolute; inset: 0; display: grid; place-items: center;
                    background: #16161caa;
                }
                .overlay[hidden] { display: none; }
                .error {
                    font: 13px system-ui, sans-serif; color: #e88; padding: 8px 12px;
                    text-align: center;
                }
                ${BUTTON_STYLE}
            </style>
            <canvas part="canvas"></canvas>
            <div class="overlay"><button part="power">&#9211; power</button></div>
        `;
        this.canvas = shadow.querySelector("canvas") as HTMLCanvasElement;
        this.overlay = shadow.querySelector(".overlay") as HTMLDivElement;
        this.button = shadow.querySelector("button") as HTMLButtonElement;
        this.button.onclick = () => void startPage();
    }

    /**
     * Phase 1, on connect: the GuiDef opens and draws, with no gesture and no
     * audio. An element inserted later works the same way.
     */
    connectedCallback(): void {
        if (this.defaultSrc !== null && !this.hasAttribute("src")) {
            this.setAttribute("src", this.defaultSrc);
        }
        this.work = this.work.then(() => this.open());
    }

    /**
     * Phase 1 undone, and phase 2 with it: an element out of the document
     * frees what it allocated. See `unmount`.
     */
    disconnectedCallback(): void {
        // Stop observing here, synchronously: the element is already out of the
        // document, and an `IntersectionObserver` firing between now and the
        // queued unmount would talk to the host about a def on its way out.
        this.unobserve();
        waiting.delete(this);
        this.work = this.work.then(() => this.unmount());
    }

    private async open(): Promise<void> {
        if (this.mounted) return;
        try {
            // A mount always starts from the power affordance: an element
            // connected again may be carrying the hidden overlay of its last
            // start, or the error message of a mount that failed.
            this.overlay.hidden = false;
            this.overlay.replaceChildren(this.button);
            this.sizeCanvas();
            this.mounted = await openBundle({
                base: this.getAttribute("src") ?? "bundle",
                canvas: this.canvas,
                name: this.getAttribute("name"),
                attributes: this.declaredAttributes(),
                preset: this.getAttribute("preset"),
            });
            this.observe();
            // Enrolled *before* the event: a `clausters-ready` handler that
            // calls `startPage` (a page driving its own gesture) must find this
            // component already waiting, or its engine half would be skipped.
            waiting.add(this);
            this.dispatchEvent(
                new CustomEvent("clausters-ready", {
                    detail: { id: this.mounted.defId },
                    bubbles: true,
                }),
            );
            // A gesture that already happened does not come again: a component
            // inserted after it starts straight away.
            if (gestured) await this.start();
        } catch (error) {
            this.fail(error);
        }
    }

    /**
     * Phase 2: this component's engine half — its defs, its samples, its boot
     * list.
     *
     * Two callers reach here for the same component — the page's gesture and
     * the component itself, once the gesture already happened — so the second
     * one **awaits the first** rather than returning early. Returning early
     * would resolve while the defs were still travelling, and whatever the
     * caller did next would find them missing.
     */
    start(): Promise<void> {
        waiting.delete(this);
        if (!this.mounted) return Promise.resolve();
        this.starting ??= this.sendEngineHalf();
        return this.starting;
    }

    private async sendEngineHalf(): Promise<void> {
        try {
            await startBundle(this.mounted!);
            this.overlay.hidden = true;
        } catch (error) {
            this.fail(error);
        }
    }

    /**
     * Gives this instance back: its window and its widgets (`/gui_free`), the
     * nodes its boot instantiated (`/node_free`), the canvas the host held for
     * it, and every id it drew from the page's pools. The page's own — the
     * engine, the host, the defs it sent, the samples it loaded — is untouched,
     * and so is every other component on the page.
     *
     * `hostClosed` is the `/gui_closed` path: the window is already gone on the
     * host's side, so only the rest is given back.
     */
    private async unmount(hostClosed = false): Promise<void> {
        this.unobserve();
        waiting.delete(this);
        const mounted = this.mounted;
        if (!mounted) return;
        this.mounted = null;
        // A start in flight finishes first. Its defs and its boot are already
        // travelling, and the engine serves in order: freeing across it would
        // free the nodes before the boot that instantiates them arrives.
        const starting = this.starting;
        this.starting = null;
        if (starting) await starting.catch(() => {});
        await freeBundle(mounted, { hostClosed });
        if (hostClosed) {
            this.dispatchEvent(
                new CustomEvent("clausters-closed", {
                    detail: { id: mounted.defId },
                    bubbles: true,
                }),
            );
        }
    }

    /**
     * The resolved parameter values this instance mounted with, or `null`
     * before it did.
     */
    get params(): Record<string, unknown> | null {
        return this.mounted?.params ?? null;
    }

    /** The id this instance's GuiDef opened under, or `null` before it did. */
    get defId(): number | null {
        return this.mounted?.defId ?? null;
    }

    /**
     * What this instance was allocated, by symbol name — its node ids, its
     * buses, its buffers. What a page needs to talk to *this* component.
     */
    get symbols(): Record<string, number> | null {
        return this.mounted?.symbols ?? null;
    }

    /**
     * Every attribute except the element's own — the tag's parameter values,
     * as strings. The resolver ignores what the manifest does not declare, so
     * `class` and `style` pass through harmlessly.
     */
    private declaredAttributes(): Record<string, string> {
        const out: Record<string, string> = {};
        for (const { name, value } of this.attributes) {
            if (name === "src" || name === "name" || name === "preset") continue;
            out[name] = value;
        }
        return out;
    }

    /**
     * The canvas' backing store, in device pixels, from the element's box —
     * the host never reads the DOM, so the element reports the pixels.
     */
    private sizeCanvas(): void {
        const { width, height } = canvasBox(this);
        this.canvas.width = width;
        this.canvas.height = height;
    }

    /**
     * What a component watches: its box and the display's scale both drive
     * `resize` (they move independently — browser zoom or a monitor of another
     * density changes the scale with the CSS box untouched), and its place in
     * the viewport drives `set_visible` — a canvas nobody is looking at is
     * skipped on the tick and drops its buses from the stream.
     */
    private observe(): void {
        const defId = this.mounted?.defId;
        if (defId === undefined) return;
        // The way back from the host: `/gui_closed <def>` is a window closed
        // there rather than here — the user closing it on a native `--ws` host,
        // which drives the same components over a socket. The element that
        // mounted the def is who must hear it.
        const closed = (packet: Uint8Array) => {
            for (const { addr, args } of decodePacket(packet)) {
                if (addr === "/gui_closed" && args[0] === defId) {
                    this.work = this.work.then(() => this.unmount(true));
                }
            }
        };
        void guiHost().then((gui) => {
            // The mount may already be gone by the time the host answers.
            if (this.mounted?.defId !== defId) return;
            gui.addEvent(closed);
            this.unwatchHost = () => gui.removeEvent(closed);
        });
        const report = () => {
            const { width, height, scale } = canvasBox(this);
            // The backing store follows the box here, not only at mount: an
            // element measured before the browser laid it out (a component
            // appended from script, mounting in the same task) opens with a
            // 1x1 canvas, and this first firing is what corrects it. Written
            // only on a change, since assigning to a canvas' size clears it.
            if (this.canvas.width !== width || this.canvas.height !== height) {
                this.canvas.width = width;
                this.canvas.height = height;
            }
            void guiHost().then((gui) => gui.bridge.resize(defId, width, height, scale));
        };
        this.resizeObserver = new ResizeObserver(report);
        this.resizeObserver.observe(this);
        this.unwatchScale = onScaleChange(report);
        this.viewObserver = new IntersectionObserver((entries) => {
            for (const entry of entries) {
                void guiHost().then((gui) => gui.bridge.set_visible(defId, entry.isIntersecting));
            }
        });
        this.viewObserver.observe(this);
    }

    /** Stops everything `observe` started. Safe to call twice. */
    private unobserve(): void {
        this.resizeObserver?.disconnect();
        this.viewObserver?.disconnect();
        this.unwatchScale?.();
        this.unwatchHost?.();
        this.resizeObserver = null;
        this.viewObserver = null;
        this.unwatchScale = null;
        this.unwatchHost = null;
    }

    /**
     * A failure shows on this component and nowhere else: the page comes up
     * around it.
     */
    private fail(error: unknown): void {
        waiting.delete(this);
        this.overlay.hidden = false;
        this.overlay.innerHTML = `<div class="error">${String(error)}</div>`;
        this.dispatchEvent(
            new CustomEvent("clausters-error", { detail: { error }, bubbles: true }),
        );
    }
}

export class ClaustersPower extends HTMLElement {
    private button: HTMLButtonElement;

    constructor() {
        super();
        const shadow = this.attachShadow({ mode: "open" });
        shadow.innerHTML = `<style>${BUTTON_STYLE}</style>
            <button part="power">&#9211; power</button>`;
        this.button = shadow.querySelector("button") as HTMLButtonElement;
        this.button.onclick = async () => {
            const engine = await server();
            if (engine.context.state === "running") {
                await engine.suspend();
                this.button.textContent = "⏻ power";
            } else {
                // The page-wide switch also starts whatever is waiting to.
                await startPage();
                this.button.textContent = "⏸ suspend";
            }
        };
    }
}

customElements.define("clausters-bundle", ClaustersBundle);
customElements.define("clausters-power", ClaustersPower);

/**
 * Registers `tag` as a component mounting the bundle at `base` — what a
 * bundle's generated `index.js` calls, so a page gets a named tag from one
 * import:
 *
 * ```js
 * import { defineComponent } from "/dist/runtime.js";
 * defineComponent("fm-voice", new URL(".", import.meta.url));
 * ```
 *
 * The tag is `<clausters-bundle>` with its `src` already filled in, so
 * everything the generic element does — the attributes, `preset`, the
 * two-phase mount — works on it unchanged. Registering the same tag twice is
 * a no-op, so two copies of a generated module on one page are harmless.
 */
export function defineComponent(tag: string, base: string | URL): void {
    if (customElements.get(tag)) return;
    const src = String(base);
    customElements.define(
        tag,
        class extends ClaustersBundle {
            constructor() {
                super();
                // Not `setAttribute`: a constructor that touches its element's
                // attributes throws on `document.createElement`, so the bundle
                // is carried as a field and reflected on connect.
                this.defaultSrc = src;
            }
        },
    );
}
