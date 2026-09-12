// The application: what a window set owns, as against what one structure owns —
// `gui/editing/application.ts`, and the `GuiHost.redefine` a part of a window is
// published through.
//
// The mirror of the Python client's `tests/test_gui_editing.py` and
// `tests/test_gui_publish.py` for these facts: the two clients are one client in
// two languages, so what one holds the other holds.
//
// Needs the core wasm staged (`./build.sh`); run with `npm test`.

import assert from "node:assert/strict";
import test from "node:test";

import type { Connection } from "../src/base/connection.ts";
import { loadCore } from "../src/base/core.ts";
import { decodePacket } from "../src/base/osc.ts";
import { Application } from "../src/gui/editing/index.ts";
import { GuiHost } from "../src/gui/host.ts";
import { label, node, panel, window as guiWindow } from "../src/gui/guidef.ts";
import type { GuiNode } from "../src/gui/guidef.ts";

await loadCore();

/** A carrier that only records; nothing replies. */
function recorder(): Connection & { packets: Uint8Array[] } {
    const packets: Uint8Array[] = [];
    return {
        packets,
        send: (packet) => packets.push(packet),
        addReply: () => {},
        removeReply: () => {},
        close: () => {},
    };
}

function fakeHost(): { host: GuiHost; defs: () => number[] } {
    const carrier = recorder();
    const host = new GuiHost({ connection: carrier });
    return {
        host,
        defs: () =>
            carrier.packets
                .flatMap((p) => decodePacket(p))
                .filter((m) => m.addr === "/gui_def")
                .map((m) => Math.trunc(Number(m.args[0]))),
    };
}

/** A two-widget picture: the ids are the caller's, as a named draw's are. */
function aTree(left = 0.0, right = 0.0): GuiNode {
    return guiWindow(
        {},
        node("number", { id: 10, value: left }),
        node("number", { id: 11, value: right }),
    );
}

// ---- a redraw says what to look like, and the host decides what it costs ----

test("a publish sends the tree and nothing is remembered", () => {
    // The client holds no picture of the host's. It used to keep the last tree
    // per window and send the difference, which is only correct if that copy
    // equals what the host holds — and it cannot, because the host moves widgets
    // on its own and screen state is reported by nothing.
    const app = new Application();
    const { host, defs } = fakeHost();
    app.host = host;
    app.publish(1, aTree());
    app.publish(1, aTree());
    assert.deepEqual(defs(), [1, 1], "the same picture twice is the same message");
});

test("publishing a part names the widget and the window it is in", () => {
    // The granularity is the caller's, and this is the door for it: a `/gui_def`
    // names any widget, so an edit publishes the one it touched rather than the
    // window around it.
    const app = new Application();
    const { host, defs } = fakeHost();
    app.host = host;
    app.publish(1, aTree());
    app.publish(11, node("number", { id: 11, value: 0.5 }), [], 1);
    assert.deepEqual(defs(), [1, 11], "and the window was not redrawn for it");
});

test("a redefine keeps the window's other names and the handle a page holds", () => {
    // The bookkeeping a part needs, and the whole reason `redefine` is not
    // `define`: what the window knows about the rest of itself is not this
    // subtree's to state.
    const { host } = fakeHost();
    const handle = host.open(
        guiWindow(
            {},
            panel({ name: "left" }, label("a", { name: "readout" })),
            label("b", { name: "right" }),
        ),
    );
    const left = handle.widget("left").id;
    const right = handle.widget("right").id;
    const readout = handle.widget("readout").id;

    host.redefine(left, panel({ name: "left" }, label("c", { name: "fresh" })), [], handle.id);

    assert.equal(handle.widget("right").id, right, "the rest of the window is untouched");
    assert.notEqual(handle.widget("fresh").id, readout, "the subtree was rebuilt");
    assert.throws(
        () => handle.widget("readout"),
        "the name under the old subtree went, and the handle a page holds is the one it holds",
    );
});

// ---- the id space, and the name that survives a redraw ----

test("a named id is the same number on every redraw and a lease is not", () => {
    const app = new Application();
    const drawer = {};
    const first = app.idFor(7, "waveform", "", drawer);
    const leased = app.newId(drawer);
    assert.equal(app.idFor(7, "waveform", "", drawer), first, "a name is an identity");
    assert.notEqual(app.newId(drawer), leased, "a lease is not");
});

test("a draw retires only the names it stopped drawing", () => {
    // On a host, which is where it matters: the leases there belong to every
    // window the page has open, so a draw takes back what *it* stopped drawing
    // and nothing else. (With no host the table is the drawer's own and starts
    // over instead — nothing outside that draw holds one of its ids.)
    const app = new Application();
    app.host = fakeHost().host;
    const drawer = {};
    app.resetIds(drawer);
    const kept = app.idFor(7, "waveform", "", drawer);
    const dropped = app.idFor(7, "roll", "", drawer);
    assert.deepEqual(app.retireIds(drawer), [], "the first draw stopped drawing nothing");

    app.resetIds(drawer);
    assert.equal(app.idFor(7, "waveform", "", drawer), kept, "still in the picture");
    assert.deepEqual(app.retireIds(drawer), [dropped], "and the roll is not");
});

test("a host-less draw starts its numbering over, so drawing twice is one tree", () => {
    // An unopened draw's ids reach nothing — no window, no pending gesture, no
    // second drawer in that table — so it restarts, which is the property a test
    // that inspects a tree twice rests on. Two applications keep their own for
    // the same reason.
    const app = new Application();
    const drawer = {};
    app.resetIds(drawer);
    const first = app.idFor(1, "curve", "", drawer);
    app.resetIds(drawer);
    assert.equal(app.idFor(1, "curve", "", drawer), first, "the same picture, the same tree");
    const other = new Application();
    other.resetIds(drawer);
    assert.equal(other.idFor(1, "curve", "", drawer), first, "each counts from its own base");
});

// ---- what a window set is for ----

test("an application answers its own host and never a second one", () => {
    // The rule the multitrack learned the hard way: an application already open
    // answers *its* host, and overwriting that sends every acknowledgement to
    // the wrong place — silently, since in the ordinary case the two are the
    // same object.
    const app = new Application();
    const { host } = fakeHost();
    const second = fakeHost().host;
    app.host = host;
    assert.equal(app.host, host);
    app.host = second;
    assert.equal(app.host, second, "set outright is set");
});

test("an application with no host publishes nothing rather than failing", () => {
    const app = new Application();
    app.publish(1, aTree());
    assert.equal(app.host, null);
});
