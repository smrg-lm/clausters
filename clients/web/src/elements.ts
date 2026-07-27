// The web components: standalone bundles as page elements.
//
// `<clausters-bundle src="<bundle url>" [name="<GuiDef name>"]>` boots a
// native-format standalone bundle against the page singletons and shows its
// GuiDef — the whole instrument as one HTML element. The element carries the
// **standard power affordance** the autoplay policy demands: it renders as a
// power button, and the user's click is the gesture that resumes the
// AudioContext and boots everything. When up, it adopts the GUI host's canvas
// into its shadow DOM.
//
// `<clausters-power>` is the affordance alone, for pages that drive the raw
// singletons (a REPL, the TS client) and only need the gesture: it toggles
// the engine's AudioContext between running and suspended.
//
// Elements share the singletons by construction: one engine, one GUI host per
// page — components meet in the same node/bus/buffer namespace. The host
// shows one window-rooted GuiDef at a time, so the canvas lives inside the
// element that most recently booted (a later `<clausters-bundle>` adopts it).
//
// Events: `clausters-ready` (detail: `{ id }`) after the bundle is up,
// `clausters-error` (detail: `{ error }`) on failure.

import { server } from "./engine/server.ts";
import { guiHost } from "./gui/page.ts";
import { bootBundle } from "./bundle.ts";

const BUTTON_STYLE = `
    button {
        font: 14px system-ui, sans-serif;
        background: #22222c; color: #ddd; border: 1px solid #444;
        padding: 8px 18px; cursor: pointer;
    }
    button:disabled { opacity: 0.6; cursor: default; }
`;

export class ClaustersBundle extends HTMLElement {
    private button: HTMLButtonElement;

    constructor() {
        super();
        const shadow = this.attachShadow({ mode: "open" });
        shadow.innerHTML = `
            <style>
                :host { display: inline-block; }
                canvas { display: block; }
                ${BUTTON_STYLE}
            </style>
            <button part="power">&#9211; power</button>
        `;
        this.button = shadow.querySelector("button") as HTMLButtonElement;
        this.button.onclick = () => this.boot();
    }

    /// Boots the bundle (also callable from script; needs a prior gesture for
    /// audible output if not called from one).
    async boot(): Promise<void> {
        this.button.disabled = true;
        this.button.textContent = "booting…";
        try {
            const engine = await server();
            await engine.resume();
            const gui = await guiHost();
            const { id } = await bootBundle({
                base: this.getAttribute("src") ?? "bundle",
                name: this.getAttribute("name"),
            });
            // The page-wide canvas moves into whichever element booted last.
            this.button.remove();
            this.shadowRoot!.append(gui.canvas);
            this.dispatchEvent(new CustomEvent("clausters-ready", {
                detail: { id },
                bubbles: true,
            }));
        } catch (error) {
            this.button.disabled = false;
            this.button.textContent = "⚠ retry";
            this.dispatchEvent(new CustomEvent("clausters-error", {
                detail: { error },
                bubbles: true,
            }));
        }
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
                await engine.resume();
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
