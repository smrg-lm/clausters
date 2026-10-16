/**
 * A node's generic editor: the window of its def's controls, which any node
 * gets without anybody writing one.
 *
 * `edit(node)` reads the node's def (`/node_query` names it, `/def_query`
 * describes its controls, the same record in every family) and hands the
 * description to the shared core (`clausters_core::node_editor`), which decides
 * what each control becomes -- a slider, one with detents, a toggle, a list --
 * what range it is drawn over and how a large def is grouped and paged. What
 * comes back is an ordinary GuiDef, opened like any other.
 *
 * **The host keeps it, not this page.** Every widget is bound to its control
 * (`/node_set`), so a hand writes the node with no round trip through here, and
 * the host reads each binding back from the server, so the window follows the
 * node whoever changes it -- another client, the plugin's own window, a loaded
 * state. All this object does after opening is answer the search field and the
 * page chooser, with the core's next page.
 *
 * **It is not a document editor.** A running node's controls are the server's
 * state and not a structure the client holds, so nothing here enters an undo
 * order ({@link Editor}); the plugin's own window is a different thing again,
 * `Node.openPluginEditor`.
 *
 * @module
 */

import { node_editor } from "../../core/clausters_core_web.js";
import { requireCore } from "../../base/core.ts";
import type { ControlInfo } from "../../defs/info.ts";
import type { Node } from "../../defs/node.ts";
import { resolveServer } from "../../defs/wire.ts";
import { resolveHost } from "../../plot.ts";
import type { GuiNode } from "../guidef.ts";
import type { WindowHandle } from "../handle.ts";
import type { GuiHost, Stage } from "../host.ts";

/** What the core answers: the window, the page's rows and the pages there are. */
interface Generated {
    window: GuiNode;
    contents: GuiNode;
    pages: string[];
    page: number;
}

/**
 * **A running node's controls in a window**, generated from its def and bound
 * to the node by the host.
 *
 * The window has a search field over the controls' names, labels and groups,
 * and a page chooser; {@link NodeEditor.search} and {@link NodeEditor.showPage}
 * do the same from code.
 */
export class NodeEditor {
    /** The node it edits. */
    readonly node: Node;
    /** Its def's controls, as `ControlInfo` -- read when the editor opens. */
    controls: ControlInfo[] = [];
    /** What the search field holds. */
    query = "";
    /** The page shown, from zero. */
    page = 0;
    /** The open window's handle, or `null`. */
    window: WindowHandle | null = null;
    /** The host the window is open on. */
    host: GuiHost | null = null;
    readonly #title: string | undefined;
    #def = "";

    /**
     * @param node the node to edit: a synth of a SynthDef, a FaustDef or a
     *   plugin.
     * @param title the window's title; by default the def's name and the
     *   node's id.
     */
    constructor(node: Node, { title }: { title?: string } = {}) {
        this.node = node;
        this.#title = title;
    }

    #generate(): Generated {
        requireCore("a node's generic editor");
        const request = {
            node: this.node.id,
            name: this.#def,
            controls: this.controls,
            query: this.query,
            page: this.page,
        };
        const answer = JSON.parse(node_editor(JSON.stringify(request))) as Generated & { error?: string };
        if (typeof answer.error === "string") throw new Error(answer.error);
        return answer;
    }

    /**
     * Opens the window on `host` (the ambient one by default), reading the
     * node's def first. An open editor is left as it is.
     */
    async open(host?: GuiHost, { stage }: { stage?: Stage | null } = {}): Promise<this> {
        if (!this.closed) return this;
        const server = resolveServer(this.node.server);
        const info = await this.node.info();
        if (!info.exists || info.isGroup) {
            throw new Error(`node ${this.node.id} is not a synth: it has no controls to edit`);
        }
        this.#def = info.defname;
        this.controls = (await server.queryDefs([info.defname]))[0].controls;
        const tree = this.#generate().window;
        if (this.#title !== undefined) tree.title = this.#title;
        this.host = host ?? await resolveHost(server);
        this.window = this.host.open(tree, { element: stage });
        // A field reports its value, and also that it took the focus
        // (`"focus" 1`): only the value is a search.
        this.window.widget("search").onEvent((...payload) => {
            if (payload.length === 1) this.search(String(payload[0]));
        });
        this.window.widget("page").onEvent((index) => this.showPage(Number(index)));
        return this;
    }

    /**
     * Shows the controls whose name, label or group holds `query` (ignoring
     * case), from their first page.
     */
    search(query: string): this {
        this.query = String(query);
        this.page = 0;
        return this.#show();
    }

    /** Shows page `page` (from zero; past the last is the last). */
    showPage(page: number): this {
        this.page = Math.trunc(page);
        return this.#show();
    }

    #show(): this {
        if (this.closed || this.window === null || this.host === null) return this;
        const answer = this.#generate();
        this.page = answer.page;
        this.host.redefine(this.window.widget("controls").id, answer.contents, [], this.window.id);
        this.window.widget("page").set({ options: answer.pages, index: answer.page });
        return this;
    }

    /** Whether the window is closed, or was never opened. */
    get closed(): boolean {
        return this.window === null || this.window.closed;
    }

    /** Closes the window. The node is left as it is. */
    close(): void {
        this.window?.close();
    }

    /** Waits until the window closes (see `WindowHandle.wait`). */
    async wait(timeout?: number): Promise<boolean> {
        return this.window === null || this.window.wait(timeout);
    }

    /** Calls `func()` when the window closes. */
    onClosed(func: (() => void) | null): this {
        if (this.window === null) throw new Error("the node editor is not open");
        this.window.onClosed(func);
        return this;
    }

    /** A picture of the window, as a PNG at `path` (see `GuiHost.capture`). */
    async capture(path: string, timeout = 5.0): Promise<string> {
        if (this.window === null || this.host === null) throw new Error("the node editor is not open");
        return this.host.capture(this.window.id, path, timeout);
    }
}
