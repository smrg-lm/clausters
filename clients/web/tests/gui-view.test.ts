// The GUI node as an object: `gui/guidef.ts`'s `View`.
//
// A builder used to return a bare object; it now returns a `View` — whose own
// properties *are* the document, so the JSON is unchanged — carrying the
// client-side name index and knowing how to open itself.
//
// The mirror of the Python client's `tests/test_view.py`, case for case: the
// two clients are one client in two languages, so what one refuses the other
// refuses.
//
// Needs the core wasm staged (`./build.sh`); run with `npm test`.

import assert from "node:assert/strict";
import test from "node:test";

import type { Connection } from "../src/base/connection.ts";
import { loadCore } from "../src/base/core.ts";
import { decodePacket } from "../src/base/osc.ts";
import { control } from "../src/defs/ugens/index.ts";
import { GuiHost } from "../src/gui/host.ts";
import { scoreView } from "../src/gui/notation/view.ts";
import { setAmbientHost } from "../src/gui/ambient.ts";
import {
    bpf,
    button,
    clip,
    INLINE_MAX,
    knob,
    label,
    layout,
    pianoroll,
    score,
    slider,
    source,
    toggle,
    view,
    View,
    waveform,
    window as windowAlias,
} from "../src/gui/guidef.ts";
import type { GuiNode } from "../src/gui/guidef.ts";

const here = new URL(".", import.meta.url);
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

/** A host over a recorder, plus the messages it has sent, decoded. */
function fakeHost(): { host: GuiHost; sent: () => { addr: string; args: unknown[] }[] } {
    const carrier = recorder();
    const host = new GuiHost({ connection: carrier });
    return {
        host,
        sent: () =>
            carrier.packets.flatMap((p) =>
                decodePacket(p).map((m) => ({ addr: m.addr, args: m.args as unknown[] }))
            ),
    };
}

const sentDoc = (sent: { addr: string; args: unknown[] }[]): Record<string, never> =>
    JSON.parse(String(sent.find((m) => m.addr === "/gui_def")!.args[1]));

// ---- the object ------------------------------------------------------------

test("a builder returns a View whose own properties are the document", () => {
    const v = knob({ name: "freq", min: 110.0, max: 880.0 });
    assert.ok(v instanceof View);
    assert.deepEqual({ ...v }, { type: "knob", name: "freq", min: 110.0, max: 880.0 });
    assert.equal(v.type, "knob");
    assert.equal(v.name, "freq");
});

test("the document is what it always was", () => {
    const v = view({ title: "t" }, layout({ flow: "col" }, knob({ name: "freq" }),
        slider({ name: "amp" })));
    assert.equal(
        v.toJson(),
        JSON.stringify({
            type: "window",
            title: "t",
            children: [{
                type: "layout",
                flow: "col",
                children: [{ type: "knob" }, { type: "slider" }],
            }],
        }),
    );
});

test("a name is found anywhere in the view", () => {
    const v = view({}, layout({}, layout({}, knob({ name: "freq" })), slider({ name: "amp" })));
    assert.deepEqual(v.names(), ["freq", "amp"]);
    assert.equal(v.find("freq").type, "knob");
    assert.equal(v.find("amp").type, "slider");
});

test("an unknown name names what is there", () => {
    const v = view({}, knob({ name: "freq" }));
    assert.throws(() => v.find("cutoff"), /freq/);
});

test("a duplicate name is refused where the tree is built", () => {
    assert.throws(
        () => layout({}, knob({ name: "freq" }), slider({ name: "freq" })),
        /duplicate widget name "freq"/,
    );
});

test("a duplicate name is refused across subtrees too", () => {
    const left = layout({}, knob({ name: "freq" }));
    const right = layout({}, slider({ name: "freq" }));
    assert.throws(() => view({}, left, right), /duplicate widget name "freq"/);
});

test("a nested view keeps its names to itself", () => {
    const v = view({}, layout({},
        view({ name: "osc1" }, knob({ name: "freq" })),
        view({ name: "osc2" }, knob({ name: "freq" }))));
    assert.deepEqual(v.names(), ["osc1", "osc2"]); // not "freq", twice
    assert.equal((v.find("osc1") as View).find("freq").type, "knob");
    assert.throws(() => v.find("freq"));
});

test("the bracket is the document key, not the name", () => {
    const v = knob({ name: "freq", min: 1.0 });
    assert.equal(v["type"], "knob");
    assert.equal(v["min"], 1.0);
    assert.equal(v["freq"], undefined);
});

// ---- the root decides -------------------------------------------------------

test("a root that is not a window is framed in one", () => {
    const { host, sent } = fakeHost();
    const tree = layout({ flow: "col" }, knob({ name: "freq" }));
    const win = host.open(tree);

    const doc = sentDoc(sent()) as unknown as { type: string; hug: number; children: [{ type: string }] };
    assert.equal(doc.type, "window");
    assert.equal(doc.hug, 1);
    assert.deepEqual(doc.children.map((c) => c.type), ["layout"]);
    assert.ok(win.widget("freq").id >= 1000);
    assert.equal(tree.type, "layout", "the caller's tree is not the frame");
});

test("a lone control opens as a window that is that control", () => {
    const { host, sent } = fakeHost();
    host.open(knob({ name: "freq", min: 110.0, max: 880.0 }));
    const doc = sentDoc(sent()) as unknown as { type: string; children: [{ type: string }] };
    assert.equal(doc.type, "window");
    assert.deepEqual(doc.children.map((c) => c.type), ["knob"]);
});

test("window is the older spelling of view", () => {
    assert.equal(windowAlias, view);
});

// ---- ids belong to the instance --------------------------------------------

test("one view opens twice with ids of its own each time", () => {
    const { host, sent } = fakeHost();
    const strip = view({}, knob({ name: "gain" }), toggle({ name: "mute" }));
    const a = host.open(strip);
    const b = host.open(strip);

    assert.notEqual(a.id, b.id);
    assert.notEqual(a.widget("gain").id, b.widget("gain").id);
    const docs = sent().filter((m) => m.addr === "/gui_def")
        .map((m) => JSON.parse(String(m.args[1])) as { children: { id: number }[] });
    const first = new Set(docs[0]!.children.map((c) => c.id));
    for (const child of docs[1]!.children) assert.ok(!first.has(child.id));
    assert.equal(strip.children?.[0]?.id, undefined, "the tree was not written into");
});

test("the same subtree nested twice gets two id runs", () => {
    const { host, sent } = fakeHost();
    const strip = layout({}, knob({}));
    host.open(view({}, strip, strip));

    const doc = sentDoc(sent()) as unknown as {
        children: { id: number; children: { id: number }[] }[];
    };
    const [left, right] = doc.children;
    assert.notEqual(left!.id, right!.id);
    assert.notEqual(left!.children[0]!.id, right!.children[0]!.id);
});

// ---- opening ----------------------------------------------------------------

test("a view opens itself on the host it is given", async () => {
    const { host, sent } = fakeHost();
    const win = await view({}, label("hi")).open(null, { id: 7, host });
    assert.equal(win.id, 7);
    assert.equal(sent().filter((m) => m.addr === "/gui_def").length, 1);
});

test("a view with no host opens on the ambient one", async () => {
    const { host, sent } = fakeHost();
    setAmbientHost(host);
    try {
        await view({}, label("hi")).open();
    } finally {
        setAmbientHost(null);
    }
    assert.equal(sent().filter((m) => m.addr === "/gui_def").length, 1);
});

test("a host adopts the ambient registration first-wins, and stop gives it up", () => {
    setAmbientHost(null);
    const first = new GuiHost({ connection: recorder() }).adoptAmbient();
    const second = new GuiHost({ connection: recorder() }).adoptAmbient();
    try {
        // The second call finds one registered and does not displace it.
        assert.equal(first.adoptAmbient(), first);
        second.stop();
        first.stop();
    } finally {
        setAmbientHost(null);
    }
});

// ---- the source -------------------------------------------------------------

test("a source expands into the carrier it picked", () => {
    const sig = source([0.1, 0.2, 0.3], { channels: 1, sampleRate: 48_000.0 });
    const w = waveform({ name: "wave", data: sig });
    assert.equal(sig.carrier, "data");
    assert.deepEqual(w["data"], [0.1, 0.2, 0.3]);
    assert.equal(w["sample_rate"], 48_000.0);
    assert.equal(w["blob"], undefined);
    assert.equal(w["path"], undefined);
});

test("a long source rides a blob, and the index is assigned at open", () => {
    const { host, sent } = fakeHost();
    const sig = source(new Array(INLINE_MAX + 1).fill(0.0));
    assert.equal(sig.carrier, "blob");
    const w = waveform({ name: "wave", data: sig });
    assert.equal(w["data"], undefined);
    assert.equal(w["blob"], undefined, "the index is not knowable before the message");

    host.open(view({}, w));
    const def = sent().find((m) => m.addr === "/gui_def")!;
    const doc = JSON.parse(String(def.args[1])) as { children: { blob: number }[] };
    assert.equal(doc.children[0]!.blob, 0);
    assert.equal((def.args[2] as Uint8Array).byteLength, (INLINE_MAX + 1) * 4);
});

test("a spilled source rewrites its blob, and pushes the bytes to what draws it", () => {
    const { host, sent } = fakeHost();
    const sig = source(new Array(INLINE_MAX + 1).fill(0.0));
    const win = host.open(view({}, waveform({ name: "wave", data: sig })));
    const before = sent().length;

    // The reference client rewrites the **file** it spilled to and tells the
    // widget to reload; a page has no file, so the bytes ride the blob
    // `/gui_set` carries — same carrier, new samples, and the widget on screen
    // was built around neither shape but around the source.
    sig.set(new Array(INLINE_MAX + 1).fill(0.5));
    const set = sent().slice(before).find((m) => m.addr === "/gui_set")!;
    assert.equal(Number(set.args[0]), win.widget("wave").id);
    assert.equal(String(set.args[1]), "data");
    assert.ok(set.args[2] instanceof Uint8Array, "the samples ride as bytes");
    assert.equal((set.args[2] as Uint8Array).byteLength, (INLINE_MAX + 1) * 4);

    // And the carrier is still fixed, as it is there: an inline source handed
    // more than it can hold says so rather than promoting itself.
    const short = source([0.5]);
    assert.throws(() => short.set(new Array(INLINE_MAX + 1).fill(0.0)), RangeError);
});

test("one source in two views is one payload and two references", () => {
    const sig = source([0.5]);
    const a = waveform({ name: "a", data: sig });
    const b = waveform({ name: "b", data: sig });
    sig.set([0.25, 0.75]);
    assert.deepEqual(a["data"], [0.25, 0.75]);
    assert.deepEqual(b["data"], [0.25, 0.75]);
});

test("set reaches every widget already drawing it", () => {
    const { host, sent } = fakeHost();
    const sig = source([0.5]);
    const v = view({}, waveform({ name: "wave", data: sig }));
    const a = host.open(v);
    const b = host.open(v);

    const before = sent().length;
    sig.set([0.25]);
    const after = sent().slice(before).filter((m) => m.addr === "/gui_set");
    assert.deepEqual(
        after.map((m) => m.args[0]).sort(),
        [a.widget("wave").id, b.widget("wave").id].sort(),
    );
    assert.ok(after.every((m) => m.args[1] === "data"));
});

test("a freed widget stops being a live end", () => {
    const { host, sent } = fakeHost();
    const sig = source([0.5]);
    const win = host.open(view({}, waveform({ name: "wave", data: sig })));
    host.close(win.id);

    const before = sent().length;
    sig.set([0.25]);
    assert.equal(sent().slice(before).filter((m) => m.addr === "/gui_set").length, 0);
});

test("a source in a prop that names no samples is refused", () => {
    assert.throws(
        () => knob({ label: source([0.5]) as unknown as string }),
        /a source names a view's payload/,
    );
});

// ---- the same object for the structures: points, notes, boxes, a page -------

test("a structure source expands into the flat wire form", () => {
    const env = source(undefined, { points: [[0.0, 0.0], [1.0, 1.0, "exp"]] });
    const a = bpf({ points: env });
    const b = bpf({ points: [[0.0, 0.0], [1.0, 1.0, "exp"]] });
    assert.equal(env.carrier, "points");
    assert.deepEqual(a["points"], [0.0, 0.0, 1, 0.0, 1.0, 1.0, 2, 0.0]);
    assert.deepEqual(a["points"], b["points"]);
});

test("a structure set rewrites the definition and the live widget", () => {
    const { host, sent } = fakeHost();
    const roll = source(undefined, { notes: [[0.0, 1.0, 60]] });
    const v = view({}, pianoroll({ name: "roll", notes: roll }));
    const win = host.open(v);

    const before = sent().length;
    roll.set([[0.0, 2.0, 62, 90]]);
    assert.deepEqual(v.find("roll")!["notes"], [0.0, 2.0, 62, 90, 0]);
    const after = sent().slice(before).filter((m) => m.addr === "/gui_set");
    assert.deepEqual(after.map((m) => [m.args[0], m.args[1], m.args[2]]), [
        [win.widget("roll").id, "notes", "[0,2,62,90,0]"],
    ]);
});

test("an engraved page is five props in a definition and one live", () => {
    const { host, sent } = fakeHost();
    const page = source(undefined, { displayList: { vb: [10, 20], glyphs: {}, prims: [] } });
    const v = view({}, score({ name: "page", displayList: page }));
    const win = host.open(v);
    assert.deepEqual(v.find("page")!["vb"], [10, 20]);
    assert.equal(v.find("page")!["display_list"], undefined);

    const before = sent().length;
    page.set({ vb: [30, 40], glyphs: {}, prims: [7] });
    assert.deepEqual(v.find("page")!["vb"], [30, 40]);
    assert.deepEqual(v.find("page")!["prims"], [7]);
    const msg = sent().slice(before).find((m) => m.addr === "/gui_set")!;
    assert.deepEqual([msg.args[0], msg.args[1]], [win.widget("page").id, "display_list"]);
    assert.deepEqual(JSON.parse(String(msg.args[2])).vb, [30, 40]);
});

test("a page rewrite drops the part the new one does not have", () => {
    const page = source(undefined, {
        displayList: { vb: [1, 2], glyphs: {}, prims: [], cursors: [3] },
    });
    const n = score({ displayList: page });
    assert.deepEqual(n["cursors"], [3]);
    page.set({ vb: [1, 2], glyphs: {}, prims: [] });
    assert.equal(n["cursors"], undefined, "the last expansion's keys are what a rewrite clears");
});

test("two sources on one widget do not clear each other", () => {
    const take = source([0.1, 0.2]);
    const roll = source(undefined, { notes: [[0.0, 1.0, 60]] });
    const c = clip({ dur: 2.0, data: take, notes: roll });
    roll.set([[0.0, 1.0, 64]]);
    assert.deepEqual(c["data"], [0.1, 0.2]);
    take.set([0.3]);
    assert.deepEqual(c["notes"], [0.0, 1.0, 64, 100, 0]);
});

test("a structure source names exactly one prop", () => {
    assert.throws(
        () => source(undefined, { points: [], notes: [] }),
        /exactly one way/,
    );
    assert.throws(
        () => source(undefined, { notes: [], channels: 2 }),
        /describe samples/,
    );
});

test("a score view sizes itself from a page held as a source", () => {
    const page = source(undefined, { displayList: { vb: [1000, 500], glyphs: {}, prims: [] } });
    const v = scoreView(page, { scrollId: 10, scoreId: 11, width: 800.0 });
    assert.equal(v["content_h"], 400.0);
    const inner = (v["children"] as GuiNode[])[0]!;
    assert.deepEqual(inner["vb"], [1000, 500]);
    assert.equal(inner["display_list"], undefined);

    page.set({ vb: [1000, 500], glyphs: {}, prims: [{ kind: "line" }] });
    assert.deepEqual(inner["prims"], [{ kind: "line" }]);
});

test("a structure source has nowhere to reload from", () => {
    assert.throws(() => source(undefined, { points: [] }).reload(), /set\(\)/);
});

// ---- a control widget is built from the control it drives -------------------

test("a knob built from a control reads its name and default; the range is the widget's", () => {
    const freq = control("freq", 440.0);
    const k = knob(freq, { min: 110.0, max: 880.0 });
    assert.deepEqual({ ...k }, {
        type: "knob",
        name: "freq",
        label: "freq",
        value: 440.0,
        min: 110.0,
        max: 880.0,
    });
});

test("an option wins over the control", () => {
    const freq = control("freq", 440.0);
    const k = knob(freq, { min: 110.0, max: 880.0, name: "pitch" });
    assert.equal(k.name, "pitch");
    assert.equal(k["label"], "freq", "the label is still the control's");
});

test("a control with no range says so instead of being guessed at", () => {
    assert.throws(() => knob(control("freq", 440.0)), /no range to be drawn over/);
    assert.equal(knob(control("freq", 440.0), { min: 1.0, max: 2.0 })["min"], 1.0);
});

test("a control is still a signal, where min and max are the operators", () => {
    const freq = control("freq", 440.0);
    assert.equal(freq.min(2.0).kind, "BinaryOpUGen");
    assert.equal(freq.max(2.0).kind, "BinaryOpUGen");
});

test("a toggle needs no range", () => {
    const gate = control("gate", 1.0);
    assert.equal(toggle(gate)["value"], 1.0);
});

// ---- what kind of thing a button is ----------------------------------------

test("a button is built from the control it drives", () => {
    // A gate is a control like any other: the button takes it first and reads
    // the name `/node_set` addresses.
    const gate = control("gate", 0.0);
    const b = button(gate, { label: "hold" });
    assert.equal(b.name, "gate");
    assert.equal(b["label"], "hold");
    assert.equal(b["value"], undefined, "a button holds nothing between presses");
    assert.equal(b["mode"], undefined, "the gate is the default and rides as an absence");
});

test("the bang is a press against a trigger", () => {
    const trig = control("fire", 0.0, { rate: "tr" });
    const b = button(trig, { mode: "press" });
    assert.equal(b["mode"], "press");
    assert.equal(b.name, "fire");
});

test("a press button refuses a control that holds what it is sent", () => {
    // The press would leave `on` standing forever: a category error, not a
    // preference.
    assert.throws(
        () => button(control("gate", 0.0), { mode: "press" }),
        /would leave it at `on` forever/,
    );
    // A button driving no control has no such trouble: one `/gui_event`
    // message *is* an event, so nothing holds anything.
    assert.equal(button({ mode: "press", label: "fire" })["mode"], "press");
});

test("an unknown button mode says which two there are", () => {
    assert.throws(
        () => button({ label: "x", mode: "click" as "gate" }),
        /unknown button mode/,
    );
});

test("the two values a switch sends are a pair and not a range", () => {
    // A bypass lives at 0.0/0.7 and a mode at 1/2, and neither is a span a
    // widget could be drawn over.
    assert.equal(toggle(control("byp", 0.0), { on: 0.7, off: 0.0 })["on"], 0.7);
    const b = button(control("duck", 0.0), { on: 0.7, off: 0.0 });
    assert.equal(b["on"], 0.7);
    assert.equal(b["off"], 0.0);
    assert.equal(b["min"], undefined);
});

test("an option bag is not mistaken for a control", () => {
    // Both carry a `name`; `default` is the tell.
    const k = knob({ name: "freq", min: 1.0, max: 2.0 });
    assert.equal(k.name, "freq");
    assert.equal(k["label"], undefined);
});

// ---- the binding is made against the control --------------------------------

test("the whole surface binds in one verb", () => {
    const { host, sent } = fakeHost();
    const freq = control("freq", 440.0);
    const amp = control("amp", 0.2);
    const win = host.open(view({}, knob(freq, { min: 110.0, max: 880.0 }),
        slider(amp, { min: 0.0, max: 1.0 })));

    const before = sent().length;
    win.bind({ id: 1000 });
    const binds = sent().slice(before).filter((m) => m.addr === "/gui_bind");
    assert.equal(binds.length, 2);
    assert.deepEqual(
        binds.map((m) => m.args.slice(1)),
        [["server", "/node_set", 1000, "freq"], ["server", "/node_set", 1000, "amp"]],
    );
    assert.deepEqual(win.controls, { freq: "freq", amp: "amp" });
});

test("a widget named apart from its control still binds the control", () => {
    const { host, sent } = fakeHost();
    const freq = control("freq", 440.0);
    const win = host.open(view({}, knob(freq, { name: "pitch", min: 110.0, max: 880.0 })));

    const before = sent().length;
    win.bind(1000);
    const bind = sent().slice(before).find((m) => m.addr === "/gui_bind")!;
    assert.deepEqual(bind.args.slice(1), ["server", "/node_set", 1000, "freq"]);
    assert.deepEqual(win.controls, { pitch: "freq" });
});

test("a window with no control widget says there is nothing to bind", () => {
    const { host } = fakeHost();
    const win = host.open(view({}, label("hi")));
    assert.throws(() => win.bind(1000), /nothing to bind/);
});

test("unbind gives every control widget back to the script", () => {
    const { host, sent } = fakeHost();
    const freq = control("freq", 440.0);
    const win = host.open(view({}, knob(freq, { min: 110.0, max: 880.0 })));
    win.bind(1000);
    const before = sent().length;
    win.unbind();
    const after = sent().slice(before).filter((m) => m.addr === "/gui_bind");
    assert.equal(after.length, 1);
    assert.equal(after[0]!.args.length, 1, "a bind with no target is the unbind");
});
