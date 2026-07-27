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
// Failures stay local: a component that cannot fetch or resolve its bundle
// shows the error on itself and emits `clausters-error`, and the rest of the
// page comes up. `clausters-ready` (detail: `{ id }`) fires per component with
// its resolved def id.

import { server } from "./engine/server.ts";
import { guiHost } from "./gui/page.ts";
import { openBundle, startBundle } from "./bundle.ts";
import type { Mounted } from "./bundle.ts";

const BUTTON_STYLE = `
    button {
        font: 14px system-ui, sans-serif;
        background: #22222c; color: #ddd; border: 1px solid #444;
        padding: 8px 18px; cursor: pointer;
    }
    button:disabled { opacity: 0.6; cursor: default; }
`;

/// The components waiting for the page's first gesture, and whether it came.
/// The AudioContext is page-wide, so any component's power affordance — or a
/// `<clausters-power>` — starts every one of them.
const waiting = new Set<ClaustersBundle>();
let gestured = false;

/// Starts every mounted component's engine half. Call from a user gesture.
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
    /// The declared parameters, once the manifest is read — the attributes
    /// this element answers to. `preset` and `src` are always among them.
    private canvas: HTMLCanvasElement;
    private overlay: HTMLDivElement;
    private button: HTMLButtonElement;
    private mounted: Mounted | null = null;
    private resizeObserver: ResizeObserver | null = null;
    private viewObserver: IntersectionObserver | null = null;

    constructor() {
        super();
        const shadow = this.attachShadow({ mode: "open" });
        shadow.innerHTML = `
            <style>
                :host { display: block; position: relative; }
                canvas { display: block; width: 100%; height: 100%; }
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

    /// Phase 1, on connect: the GuiDef opens and draws, with no gesture and no
    /// audio. An element inserted later works the same way.
    connectedCallback(): void {
        if (this.mounted) return;
        void this.open();
    }

    disconnectedCallback(): void {
        this.resizeObserver?.disconnect();
        this.viewObserver?.disconnect();
        this.resizeObserver = null;
        this.viewObserver = null;
        waiting.delete(this);
    }

    private async open(): Promise<void> {
        try {
            this.sizeCanvas();
            this.mounted = await openBundle({
                base: this.getAttribute("src") ?? "bundle",
                canvas: this.canvas,
                name: this.getAttribute("name"),
                attributes: this.declaredAttributes(),
                preset: this.getAttribute("preset"),
            });
            this.observe();
            this.dispatchEvent(
                new CustomEvent("clausters-ready", {
                    detail: { id: this.mounted.defId },
                    bubbles: true,
                }),
            );
            // A gesture that already happened does not come again: a component
            // inserted after it starts straight away.
            if (gestured) await this.start();
            else waiting.add(this);
        } catch (error) {
            this.fail(error);
        }
    }

    /// Phase 2: this component's engine half — its defs, its samples, its boot
    /// list. Driven by `startPage`; safe to call twice.
    async start(): Promise<void> {
        if (!this.mounted || this.mounted.started) return;
        try {
            await startBundle(this.mounted);
            this.overlay.hidden = true;
        } catch (error) {
            this.fail(error);
        }
    }

    /// The resolved parameter values this instance mounted with, or `null`
    /// before it did.
    get params(): Record<string, unknown> | null {
        return this.mounted?.params ?? null;
    }

    /// The id this instance's GuiDef opened under, or `null` before it did.
    get defId(): number | null {
        return this.mounted?.defId ?? null;
    }

    /// Every attribute except the element's own — the tag's parameter values,
    /// as strings. The resolver ignores what the manifest does not declare, so
    /// `class` and `style` pass through harmlessly.
    private declaredAttributes(): Record<string, string> {
        const out: Record<string, string> = {};
        for (const { name, value } of this.attributes) {
            if (name === "src" || name === "name" || name === "preset") continue;
            out[name] = value;
        }
        return out;
    }

    /// The canvas' backing store, in device pixels, from the element's box —
    /// the host never reads the DOM, so the element reports the pixels.
    private sizeCanvas(): void {
        const ratio = globalThis.devicePixelRatio || 1;
        const box = this.getBoundingClientRect();
        const width = Math.max(1, Math.round((box.width || this.canvas.width) * ratio));
        const height = Math.max(1, Math.round((box.height || this.canvas.height) * ratio));
        this.canvas.width = width;
        this.canvas.height = height;
    }

    /// The two observers a component carries: its box drives `resize`, and its
    /// place in the viewport drives `set_visible` — a canvas nobody is looking
    /// at is skipped on the tick and drops its buses from the stream.
    private observe(): void {
        const defId = this.mounted?.defId;
        if (defId === undefined) return;
        this.resizeObserver = new ResizeObserver(() => {
            const ratio = globalThis.devicePixelRatio || 1;
            const box = this.getBoundingClientRect();
            const width = Math.max(1, Math.round(box.width * ratio));
            const height = Math.max(1, Math.round(box.height * ratio));
            void guiHost().then((gui) => gui.bridge.resize(defId, width, height));
        });
        this.resizeObserver.observe(this);
        this.viewObserver = new IntersectionObserver((entries) => {
            for (const entry of entries) {
                void guiHost().then((gui) => gui.bridge.set_visible(defId, entry.isIntersecting));
            }
        });
        this.viewObserver.observe(this);
    }

    /// A failure shows on this component and nowhere else: the page comes up
    /// around it.
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

/// Registers `tag` as a component mounting the bundle at `base` — what a
/// bundle's generated `index.js` calls, so a page gets a named tag from one
/// import:
///
/// ```js
/// import { defineComponent } from "/dist/runtime.js";
/// defineComponent("fm-voice", new URL(".", import.meta.url));
/// ```
///
/// The tag is `<clausters-bundle>` with its `src` already filled in, so
/// everything the generic element does — the attributes, `preset`, the
/// two-phase mount — works on it unchanged. Registering the same tag twice is
/// a no-op, so two copies of a generated module on one page are harmless.
export function defineComponent(tag: string, base: string | URL): void {
    if (customElements.get(tag)) return;
    const src = String(base);
    customElements.define(
        tag,
        class extends ClaustersBundle {
            constructor() {
                super();
                if (!this.hasAttribute("src")) this.setAttribute("src", src);
            }
        },
    );
}
