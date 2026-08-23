// Widget handles: operate a live widget as an object, never by its integer id
// (mirrors `clausters/gui/handle.py`).
//
// `GuiHost.open`/`define` hand back a `WindowHandle` — the window's own widget
// handle, which additionally resolves the tree's **named** widgets. A lookup
// returns a `WidgetHandle`, a thin façade whose `set`/`bind`/`free`/`query`/
// `onEvent` delegate to the host with the resolved id, the same way a `Node`
// delegates to its `Server`. So a script holds the widget and acts on it
// (`win.widget("cutoff").set({ value: 800.0 })`) instead of tracking integers
// and matching them in an event stream.
//
// A name is stable; the assigned id is not (it recycles across redraws — see
// `./ids.ts`), which is why the handle addresses by name and the host resolves
// the current id underneath it.

import type { GuiHost, WidgetInfo } from "./host.ts";

/**
 * What a widget's `/gui_event` carries after its id: a control's value, or a
 * view's tag followed by its flat values.
 */
export type EventArgs = (number | string | Uint8Array | boolean | null)[];

/**
 * A live widget, addressed by its id but operated as an object. The mutating
 * methods return `this`, so they chain.
 */
export class WidgetHandle {
    protected readonly host: GuiHost;
    /**
     * The widget's current id on the host (an implementation handle; prefer
     * the name you looked it up by, which survives a redraw).
     */
    readonly id: number;

    constructor(host: GuiHost, id: number) {
        this.host = host;
        this.id = id;
    }

    /** `/gui_set` this widget's properties. */
    set(props: Record<string, number | string | boolean>): this {
        this.host.set(this.id, props);
        return this;
    }

    /**
     * Forward this widget's value straight to the audio server, bypassing
     * this script (see `GuiHost.bind`).
     */
    bind(address: string, ...prefix: (number | string)[]): this {
        this.host.bind(this.id, address, ...prefix);
        return this;
    }

    /**
     * Apply this widget's value to another widget's property, with no round-trip
     * through this script (see `GuiHost.bindWidget`). `target` may be a handle
     * or an id.
     */
    bindWidget(target: WidgetHandle | number, prop: string): this {
        this.host.bindWidget(this.id, typeof target === "number" ? target : target.id, prop);
        return this;
    }

    /** Remove this widget's binding, so its value comes back as an event. */
    unbind(): this {
        this.host.unbind(this.id);
        return this;
    }

    /** Point the keyboard at this widget (see `GuiHost.focus`). */
    focus(on = true): this {
        this.host.focus(this.id, on);
        return this;
    }

    /** Free this widget and its subtree. */
    free(): void {
        this.host.free(this.id);
    }

    /** Round-trip this widget's state through `/gui_query`. */
    query(timeout?: number): Promise<WidgetInfo> {
        return this.host.query(this.id, timeout);
    }

    /**
     * Call `handler(...payload)` whenever this widget emits a `/gui_event` —
     * the host pushes them, so there is nothing to pump. The payload is the
     * event's arguments after the id (a control's value, or a view's tag
     * followed by its flat values). `null` clears the handler.
     */
    onEvent(handler: ((...args: EventArgs) => void) | null): this {
        this.host.setEventHandler(this.id, handler);
        return this;
    }
}

/**
 * An open window's handle: the window's own widget handle, which also
 * resolves the tree's **named** widgets and carries `close`/`onClosed`.
 */
export class WindowHandle extends WidgetHandle {
    private readonly names: Map<string, number>;
    /**
     * widget id → the def control it was built from, collected by the id walk
     * — what {@link WindowHandle.bind} wires in one verb.
     */
    private readonly controls: Map<number, string>;

    constructor(
        host: GuiHost,
        id: number,
        names: Map<string, number>,
        controls: Map<number, string> = new Map(),
    ) {
        super(host, id);
        this.names = names;
        this.controls = controls;
    }

    /** The `WidgetHandle` for the widget built with `name: …`. */
    widget(name: string): WidgetHandle {
        const id = this.names.get(name);
        if (id === undefined) {
            throw new Error(
                `no widget named '${name}' in this window ` +
                    `(names: ${this.widgetNames().join(", ")})`,
            );
        }
        return new WidgetHandle(this.host, id);
    }

    /**
     * Adopts a redrawn tree's name → id map, in place. Called by
     * `GuiHost.define` when a window is redefined: one window is one handle,
     * so every reference the caller kept goes on resolving names correctly
     * instead of pointing at ids the redraw returned to the pool.
     *
     * @internal
     */
    refreshNames(names: Map<string, number>, controls?: Map<number, string>): void {
        this.names.clear();
        for (const [name, id] of names) this.names.set(name, id);
        if (controls === undefined) return;
        this.controls.clear();
        for (const [id, control] of controls) this.controls.set(id, control);
    }

    /**
     * Wire every widget built from a def control straight to `node`.
     *
     * The counterpart of {@link knob} taking a control: the widget already
     * knows which control it draws, so the whole surface is one verb instead of
     * one hand-typed string per widget:
     *
     * ```ts
     * const w = await view({}, knob(freq), slider(amp)).open();
     * w.bind(synth);
     * ```
     *
     * Each becomes a `/gui_bind` forwarding `address <node> <control> <value>`
     * — the host talks to the audio server itself, with no round trip through
     * this script (see `GuiHost.bind`, which is still there for anything that
     * is not a def control: a bus, an arbitrary address, another widget).
     *
     * **Two widgets on one control both bind**, both set the node, and neither
     * is told when the other moves; the host fires an apply rather than a
     * second binding, so they settle rather than cascade. That drift is yours
     * to make and is not detected.
     *
     * `node` is a `Node` (a `Synth`, a `Group`, a GraphDef instance) or a bare
     * node id. Throws when no widget in this window was built from a control,
     * which can only be a mistake.
     */
    bind(
        node: { id: number } | number | string,
        ...rest: (string | number | { address?: string })[]
    ): this {
        if (typeof node === "string") {
            throw new TypeError(
                "a window binds a node, not an address — the one-at-a-time form " +
                    'is on the widget: win.widget("freq").bind("/node_set", node, "freq")',
            );
        }
        const options = typeof rest[0] === "object" ? rest[0] : {};
        const address = options.address ?? "/node_set";
        if (this.controls.size === 0) {
            throw new Error(
                "no widget in this window was built from a def control, so " +
                    "there is nothing to bind — build them from controls " +
                    "(knob(freq), slider(sd.control(\"amp\"))), or bind one at a time " +
                    "with win.widget(\"freq\").bind(\"/node_set\", node, \"freq\")",
            );
        }
        const target = typeof node === "number" ? node : node.id;
        for (const [id, control] of this.controls) {
            this.host.bind(id, address, target, control);
        }
        return this;
    }

    /**
     * Drop every binding {@link WindowHandle.bind} made, so the widgets' values
     * come back as `/gui_event`s to this script.
     */
    unbind(): this {
        for (const id of this.controls.keys()) this.host.unbind(id);
        return this;
    }

    /**
     * `widget name -> def control name` for every widget in this window built
     * from a control — what {@link WindowHandle.bind} wires.
     */
    controlMap(): Map<string, string> {
        const byId = new Map<number, string>();
        for (const [name, id] of this.names) byId.set(id, name);
        const out = new Map<string, string>();
        for (const [id, control] of this.controls) out.set(byId.get(id) ?? control, control);
        return out;
    }

    /** Whether this window binds `name`. */
    has(name: string): boolean {
        return this.names.has(name);
    }

    /** The names bound in this window, sorted. */
    widgetNames(): string[] {
        return [...this.names.keys()].sort();
    }

    /**
     * Close this window: `/gui_free` frees the subtree and its window, and
     * the ids return to the pool.
     */
    close(): void {
        this.host.close(this.id);
    }

    /**
     * Call `handler()` when the user closes this window (a `/gui_closed`).
     * `null` clears it.
     */
    onClosed(handler: (() => void) | null): this {
        this.host.setClosedHandler(this.id, handler);
        return this;
    }
}
