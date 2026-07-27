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
import { spawn } from "node:child_process";
import { access, readFile } from "node:fs/promises";
import test from "node:test";
import { setTimeout as sleep } from "node:timers/promises";

import { WsConnection } from "../src/base/connection.ts";
import { loadOsc } from "../src/base/osc.ts";
import { GuiHost } from "../src/gui/host.ts";
import { knob, label, panel, slider, waveform, window } from "../src/gui/guidef.ts";
import { BASE_ID } from "../src/gui/ids.ts";

const here = new URL(".", import.meta.url);
const hostBin = new URL("../../gui/target/debug/clausters-gui", here).pathname;
const wsPort = 57988; // out of the default range; parallel-test friendly
const udpPort = 57989; // the host front's UDP port, moved off 57210 likewise

const hasHost = await access(hostBin).then(() => true, () => false);

await loadOsc(
    await readFile(new URL("../dist/core/clausters_core_web_bg.wasm", here)),
);

/**
 * Starts a headless host and drives it through a connected `GuiHost`,
 * stopping both afterwards.
 */
async function withHost(body: (gui: GuiHost) => Promise<void>): Promise<void> {
    const process = spawn(
        hostBin,
        ["--headless", "--no-tcp", "--port", String(udpPort), "--ws", String(wsPort)],
        { stdio: "ignore" },
    );
    try {
        // The host binds the WS listener during boot; retry until it is up.
        let connection: WsConnection | null = null;
        for (let i = 0; i < 50 && !connection; i++) {
            connection = await WsConnection.open(`ws://127.0.0.1:${wsPort}`)
                .catch(() => null);
            if (!connection) await sleep(100);
        }
        assert.ok(connection, "host WS endpoint never came up");
        const gui = new GuiHost(connection);
        try {
            await body(gui);
        } finally {
            gui.stop();
            connection.close();
        }
    } finally {
        process.kill();
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

        // Every id-less widget was assigned in place, out of the client's
        // recycling window, and the names resolve to them.
        assert.ok(win.id >= BASE_ID, `window id ${win.id} is below the base`);
        assert.deepEqual(win.widgetNames(), ["cutoff", "freq", "view"]);
        const freq = win.widget("freq");
        assert.equal(freq.id, tree.children?.[1]?.children?.[0]?.id);

        // The host holds what was sent — a `/gui_query` round trip.
        const info = await freq.query();
        assert.equal(info.type, "knob");
        assert.equal(info.props.label, "freq");
        assert.equal(info.props.value, 220.0);
        const view = await win.widget("view").query();
        assert.equal(view.type, "waveform");

        // A live edit, read back.
        freq.set({ value: 440.0, label: "pitch" });
        const moved = await freq.query();
        assert.equal(moved.props.value, 440.0);
        assert.equal(moved.props.label, "pitch");

        // Binding and unbinding a widget leaves it addressable (the value
        // path itself needs a gesture — that is `tests/gui.html`).
        freq.bind("/n_set", 1000, "freq");
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
        gui.closeAll();
    });
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
