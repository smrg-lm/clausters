// Measuring an element for a canvas: the device-pixel box and the display
// scale, kept apart.
//
// A leaf on purpose. This is the arithmetic every canvas host needs -- the
// page's own `guiHost`, and the notebook widget, which cannot import
// `./page.ts` because that module boots the in-page engine and a cell wants
// only the GUI host. So the shared half lives here, with no imports of its
// own, and `./page.ts` re-exports it to keep its public surface unchanged.

/**
 * One element box measured for a canvas: its size in **device pixels** (floored
 * at 1, so a hidden element never asks for a zero-sized surface) and the
 * `devicePixelRatio` those pixels were measured at, kept **separately**.
 *
 * The two are not interchangeable. A canvas' backing store is device pixels, so
 * the surface takes `width`/`height`; the sizes a GuiDef declares are logical,
 * so resolving them takes `scale` — and the product alone cannot be
 * un-multiplied. Reporting both is what lets the host draw a 28-pixel strip as
 * 28 *apparent* pixels on any display, while never reading the DOM itself.
 */
export interface CanvasBox {
    /** The backing-store width, in device pixels. */
    width: number;
    /** The backing-store height, in device pixels. */
    height: number;
    /** The device-pixel ratio the box was measured at (the host's UI scale). */
    scale: number;
}

/**
 * Calls `apply` whenever the page's `devicePixelRatio` changes, returning the
 * disposer that stops watching.
 *
 * A `ResizeObserver` is not enough, and that is the whole reason this exists: it
 * observes the **CSS** box, so browser zoom or a drag onto a monitor of another
 * density changes the ratio while the box stays exactly as it was — no callback,
 * and the host keeps resolving its sizes against a scale that is no longer true.
 * This is the browser's answer to the desktop's `ScaleFactorChanged`.
 *
 * The mechanism is a media query on the *current* ratio (`(resolution: 2dppx)`),
 * which stops matching the moment it moves; so each firing re-measures and then
 * re-arms on the new ratio.
 */
export function onScaleChange(apply: () => void): () => void {
    let query: MediaQueryList | null = null;
    let stopped = false;
    const fired = () => {
        apply();
        arm();
    };
    const arm = () => {
        query?.removeEventListener("change", fired);
        query = null;
        if (stopped) return;
        const ratio = globalThis.devicePixelRatio || 1;
        // Absent in a non-browser run time (the module-graph tests): nothing to
        // watch, and nothing to fail either.
        query = globalThis.matchMedia?.(`(resolution: ${ratio}dppx)`) ?? null;
        query?.addEventListener("change", fired);
    };
    arm();
    return () => {
        stopped = true;
        arm();
    };
}

/** Measures `element` for a canvas (see {@link CanvasBox}). */
export function canvasBox(element: Element): CanvasBox {
    const scale = globalThis.devicePixelRatio || 1;
    const box = element.getBoundingClientRect();
    return {
        width: Math.max(1, Math.round(box.width * scale)),
        height: Math.max(1, Math.round(box.height * scale)),
        scale,
    };
}
