// `GuiHost` against a **native** GUI host: a real `clausters-gui --ws`
// (headless), one node process, the whole client surface over `WsConnection` —
// the browser's path into a desktop host, the same seam the audio-server test
// drives (`server.test.ts`). The in-page half of the acceptance, where the
// gestures and the bound path live, is `tests/gui.html`.
//
// Needs the host binary built (`cargo build` in clients/gui — its own
// workspace); skips (does not fail) when it is missing, so `npm test` stays
// runnable from a source tree without that build.

import assert from "node:assert/strict";
import { access } from "node:fs/promises";
import test from "node:test";
import { setTimeout as sleep } from "node:timers/promises";
import { spawnChild } from "./child.ts";

import { WsConnection } from "../src/base/connection.ts";
import type { Connection } from "../src/base/connection.ts";
import { encodeMessage, loadOsc } from "../src/base/osc.ts";
import { GuiHost } from "../src/gui/host.ts";
import { button, knob, label, panel, slider, waveform, window } from "../src/gui/guidef.ts";
import { BASE_ID } from "../src/gui/ids.ts";

const here = new URL(".", import.meta.url);
const hostBin = new URL("../../gui/target/debug/clausters-gui", here).pathname;
const wsPort = 57988; // out of the default range; parallel-test friendly
const udpPort = 57989; // the host front's UDP port, moved off 57210 likewise

const hasHost = await access(hostBin).then(() => true, () => false);

await loadOsc();

/**
 * Starts a headless host and drives it through a connected `GuiHost`,
 * stopping both afterwards.
 */
async function withHost(body: (gui: GuiHost) => Promise<void>): Promise<void> {
    const host = spawnChild(hostBin, [
        "--headless", "--no-tcp", "--port", String(udpPort), "--ws", String(wsPort),
    ]);
    try {
        // The host binds the WS listener during boot; retry until it is up.
        let connection: WsConnection | null = null;
        for (let i = 0; i < 50 && !connection; i++) {
            connection = await WsConnection.open(`ws://127.0.0.1:${wsPort}`)
                .catch(() => null);
            if (!connection) await sleep(100);
        }
        assert.ok(connection, "host WS endpoint never came up");
        const gui = new GuiHost({ connection });
        try {
            await body(gui);
        } finally {
            gui.stop();
            connection.close();
        }
    } finally {
        host.stop();
    }
}

/** The reference panel: a named control per kind, ids left to the client. */
const controlPanel = () =>
    window(
        { title: "ws front", w: 480, h: 320, layout: "col" },
        label("driven from TypeScript"),
        panel(
            { layout: "row" },
            knob({ name: "freq", label: "freq", min: 50.0, max: 2000.0, value: 220.0 }),
            slider({ name: "cutoff", label: "cutoff", min: 20.0, max: 20000.0, value: 800.0 }),
        ),
        waveform({ name: "view", data: [0.0, 0.5, -0.5], baseBucket: 64 }),
    );

test("GuiHost: a built panel defines, queries and frees on a native host", {
    skip: !hasHost,
}, async () => {
    await withHost(async (gui) => {
        const tree = controlPanel();
        const win = gui.open(tree);

        // Every id-less widget was assigned out of the client's recycling
        // window, and the names resolve to them. The ids ride in the document
        // that went out; the tree that was written is left as it was written,
        // which is what lets one view open twice.
        assert.ok(win.id >= BASE_ID, `window id ${win.id} is below the base`);
        assert.deepEqual(win.widgetNames(), ["cutoff", "freq", "view"]);
        const freq = win.widget("freq");
        assert.ok(freq.id >= BASE_ID, `widget id ${freq.id} is below the base`);
        assert.equal(tree.children?.[1]?.children?.[0]?.id, undefined);

        // The host holds what was sent — a `/gui_query` round trip.
        const info = await freq.query();
        assert.equal(info.type, "knob");
        assert.equal(info.props.label, "freq");
        assert.equal(info.props.value, 220.0);
        const view = await win.widget("view").query();
        assert.equal(view.type, "signal");
        assert.equal(view.props.view, "trace");

        // A live edit, read back.
        freq.set({ value: 440.0, label: "pitch" });
        const moved = await freq.query();
        assert.equal(moved.props.value, 440.0);
        assert.equal(moved.props.label, "pitch");

        // Binding and unbinding a widget leaves it addressable (the value
        // path itself needs a gesture — that is `tests/gui.html`).
        freq.bind("/node_set", 1000, "freq");
        freq.unbind();
        assert.equal((await freq.query()).type, "knob");

        // Closing the window frees the whole subtree on the host: a query
        // still answers, with the empty type that means "no such widget".
        win.close();
        assert.equal((await freq.query()).type, "");
        assert.equal((await win.query()).type, "");
    });
});

test("GuiHost: a redefine replaces the tree under the same root", {
    skip: !hasHost,
}, async () => {
    await withHost(async (gui) => {
        const first = gui.open(controlPanel());
        const oldCutoff = first.widget("cutoff").id;

        // Re-defining the same root replaces its subtree on the host (and
        // returns the old ids to this client's pool): the old widgets are
        // gone, the new ones are the live ones.
        const second = gui.define(first.id, controlPanel());
        assert.equal(second.id, first.id);
        assert.notEqual(second.widget("cutoff").id, oldCutoff);
        assert.equal((await gui.query(oldCutoff)).type, "");
        second.widget("cutoff").set({ value: 1200.0 });
        assert.equal((await second.widget("cutoff").query()).props.value, 1200.0);

        // One window is one handle: the redraw refreshes the map in place, so
        // the reference taken before it still resolves names to live ids. A
        // second handle would leave the caller addressing widgets the redraw
        // returned to the pool -- which is how an editor redrawing its window
        // silently killed the transport bar beside it (`gui_daw.py`).
        assert.equal(second, first);
        assert.equal(first.widget("cutoff").id, second.widget("cutoff").id);
        gui.closeAll();
    });
});

test("GuiHost: a redraw keeps a named widget's handler", () => {
    // No live host: what is under test is this client's own bookkeeping. A
    // redefine takes fresh ids from the pool, so a callback kept under the old
    // id would be orphaned -- or fire for whatever widget inherited that
    // number. A callback belongs to the widget the *name* points at.
    // The carrier is a stub that only remembers the reply listener, so an
    // event can be delivered exactly as the host would deliver one.
    let deliver: ((packet: Uint8Array) => void) | undefined;
    const gui = new GuiHost({
        connection: {
            send: () => {},
            addReply: (fn: (packet: Uint8Array) => void) => {
                deliver = fn;
            },
            removeReply: () => {},
        } as unknown as Connection,
    });
    const event = (id: number, value: number) =>
        deliver!(encodeMessage("/gui_event", [["i", id], ["i", 1], ["i", 0], ["i", value]]));

    const fired: unknown[] = [];
    const win = gui.open(window({}, button({ name: "play" }), button({ name: "stop" })));
    win.widget("play").onEvent((...args) => fired.push(args[0]));
    const oldPlay = win.widget("play").id;

    gui.define(win.id, window({}, button({ name: "play" }), button({ name: "stop" })));
    assert.notEqual(win.widget("play").id, oldPlay);

    event(win.widget("play").id, 1);
    assert.deepEqual(fired, [1], "the handler followed the name onto the new id");

    fired.length = 0;
    event(oldPlay, 1);
    assert.deepEqual(fired, [], "the recycled id answers for nobody");
});

// ---- what the hand did, as against what the widget is worth ----------------

/** A client with no live host, and the door an event arrives through. */
function stubbed(): { gui: GuiHost; tag: (id: number, tag: string) => void;
    value: (id: number, v: number) => void } {
    let deliver: ((packet: Uint8Array) => void) | undefined;
    const gui = new GuiHost({
        connection: {
            send: () => {},
            addReply: (fn: (packet: Uint8Array) => void) => {
                deliver = fn;
            },
            removeReply: () => {},
        } as unknown as Connection,
    });
    return {
        gui,
        tag: (id, t) =>
            deliver!(encodeMessage("/gui_event", [["i", id], ["i", 1], ["i", 0], ["s", t]])),
        value: (id, v) =>
            deliver!(encodeMessage("/gui_event", [["i", id], ["i", 1], ["i", 0], ["i", v]])),
    };
}

test("GuiHost: the three interface verbs route by tag", () => {
    const { gui, tag } = stubbed();
    const win = gui.open(window({}, button({ name: "go", label: "go" })));
    const seen: string[] = [];
    win.widget("go").onPress(() => seen.push("press"));
    win.widget("go").onRelease(() => seen.push("release"));
    win.widget("go").onClick(() => seen.push("click"));
    const id = win.widget("go").id;
    for (const t of ["press", "release", "click"]) tag(id, t);
    assert.deepEqual(seen, ["press", "release", "click"]);
});

test("GuiHost: a press the hand slid off never reaches onClick", () => {
    // The cancellation: the host reports the release and not the click, so the
    // command simply does not run.
    const { gui, tag } = stubbed();
    const win = gui.open(window({}, button({ name: "go", label: "go" })));
    const ran: string[] = [];
    win.widget("go").onClick(() => ran.push("clicked"));
    win.widget("go").onRelease(() => ran.push("released"));
    const id = win.widget("go").id;
    tag(id, "press");
    tag(id, "release");
    assert.deepEqual(ran, ["released"], "the press was abandoned, so nothing was commanded");
});

test("GuiHost: onEvent is the raw stream and still sees everything", () => {
    // The two vocabularies are additive: a script may read the value, the
    // hand's events, or both on one widget.
    const { gui, tag, value } = stubbed();
    const win = gui.open(window({}, button({ name: "go", label: "go" })));
    const raw: unknown[] = [];
    const clicks: number[] = [];
    win.widget("go").onEvent((...args) => raw.push(args[0]));
    win.widget("go").onClick(() => clicks.push(1));
    const id = win.widget("go").id;
    value(id, 1);
    tag(id, "click");
    assert.deepEqual(raw, [1, "click"]);
    assert.deepEqual(clicks, [1]);
});

test("GuiHost: a redraw keeps the interface handlers", () => {
    // A callback belongs to the widget the *name* points at, so a redrawn
    // window is not a button that stopped working.
    const { gui, tag } = stubbed();
    const win = gui.open(window({}, button({ name: "go", label: "go" })));
    const ran: number[] = [];
    win.widget("go").onClick(() => ran.push(1));
    const before = win.widget("go").id;
    gui.define(win.id, window({}, button({ name: "go", label: "go again" })));
    assert.notEqual(win.widget("go").id, before, "a redraw takes fresh ids, which is the point");
    tag(win.widget("go").id, "click");
    assert.deepEqual(ran, [1]);
});

test("GuiHost: the three verbs clear with null", () => {
    const { gui, tag } = stubbed();
    const win = gui.open(window({}, button({ name: "go", label: "go" })));
    const ran: number[] = [];
    win.widget("go").onClick(() => ran.push(1));
    win.widget("go").onClick(null);
    tag(win.widget("go").id, "click");
    assert.deepEqual(ran, []);
});

test("GuiHost: a hand-picked id is kept, and an unknown widget answers empty", {
    skip: !hasHost,
}, async () => {
    await withHost(async (gui) => {
        gui.open(window({}, knob({ id: 7, label: "manual", value: 0.5 })), { id: 1 });
        const info = await gui.query(7);
        assert.equal(info.type, "knob");
        assert.equal(info.props.label, "manual");
        // Nothing was ever defined under this id.
        assert.equal((await gui.query(4242)).type, "");
    });
});

test("GuiHost: a typeface a host cannot read leaves it drawing", {
    skip: !hasHost,
}, async () => {
    await withHost(async (gui) => {
        gui.open(window({}, label("hello", { id: 7 })), { id: 1 });
        // `/gui_font` carries the bytes and no id: a face is the host's, not a
        // window's. Junk is refused the way a build with no rasterizer refuses
        // a real face -- a log, not a failure -- so the window is still there
        // and still answering afterwards.
        gui.font(new Uint8Array([0, 1, 0, 0, 110, 111]));
        assert.equal((await gui.query(7)).type, "label");
    });
});
