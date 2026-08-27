#!/usr/bin/env node
// Author, with the TypeScript client, a bundle whose GuiDef controls a
// GraphDef — then mount it as a web component, twice on one page if you like.
//
// The point of this example is the **whole chain with no intermediary client
// at run time**: the client only *authors* the files (it talks to nothing);
// what runs afterwards is the persisted bundle — the same one everywhere —
// with each control knob wired **straight to the synthesis** through the
// GraphDef's named surface:
//
//     knob turn -> /node_set <graph node> "<port>" <value>   (the widget's `bind`)
//               -> the graph's surface port fans out to its member controls
//
// The instrument is an FM voice feeding a tremolo through a private graph bus:
//
//     fm.voice --[voice bus]--> fm.trem --> OUT (stereo)
//                                      `--> the LFO bus (for the meter/scope)
//
// and the `GraphDef` exposes the musically meaningful **surface ports**
// (`freq`, `ratio`, `bright`, `rate`, `depth`, `amp`) rather than the raw
// member controls — `bright` shows a scaled port: the 0..1 knob maps to FM
// index 0..8.
//
// **What makes it a component.** Two things the mount allocates per instance,
// declared rather than picked:
//
//     const node = b.node("graph");   // -> "@graph": the node the boot
//                                     //    /graph_new creates, and every
//                                     //    knob's bind target
//     const lfo  = b.bus("lfo");      // -> "@lfo":  where the tremolo
//                                     //    publishes, reaching the def
//                                     //    through a surface port
//
// so two instances instantiate two graphs at two node ids, each watching its
// own LFO bus. `freq` and `amp` are declared parameters, so the markup can
// tune each instance:
//
//     <fm-trem></fm-trem>
//     <fm-trem freq="330" amp="0.15"></fm-trem>
//     <fm-trem preset="bright"></fm-trem>
//
// **It is a node script, not a page**, and that is the design rather than a
// convenience: writing a bundle is not something a page does, because a bundle
// is an *input* a static page boots with no interpreter at all. The Python
// client's `clausters.bundle` is the same writer in the other language, and
// the two emit the same directory byte for byte.
//
// Run it (from this directory, after `../../../build.sh`):
//
//     node make_bundle.mjs
//
// It writes into `examples/out/graph-controls/` — the ignored directory every
// generator in this tree writes to, so a run leaves nothing to clean up by
// hand. Then the **same** bundle runs on every leg, no script attached to any
// of them:
//
// - **Browser, as a web component** (the wasm engine in an AudioWorklet):
//   serve **from `clients/web` — the package root, never this folder** (the
//   page imports `../../../dist/...`, which must stay inside the served root;
//   serving `graph-controls/` itself turns those imports into 404s):
//
//       cd clients/web && python3 -m http.server
//
//   and open `http://localhost:8000/examples/panels/graph-controls/` —
//   `index.html` here is just `<clausters-bundle src="…">`; its power button
//   boots the whole instrument in the tab.
// - **Desktop, self-contained** (the embedded server; from `clients/gui`):
//
//       cargo run --features standalone --bin clausters-gui -- \
//           --standalone fm-trem --data-dir <clients/web>/examples/out/graph-controls
//
// - **Desktop, loopback** (a running `clausters` + `clausters-gui --server`
//   pointing at it, the bundle's dir as `--data-dir`): the same files again.
//
// The layout it writes:
//
//     defs/synthdefs/fm-trem.voice.json    the members (SynthDef specs,
//     defs/synthdefs/fm-trem.trem.json      the /def_send synth payloads)
//     defs/graphdefs/fm-trem.graph.json    the GraphDef (the /def_send graph
//                                           payload: buses, members, surface)
//     defs/guidefs/fm-trem.json            the GuiDef record — a template
//     presets/bright.json                  a named parameter bundle
//     bundle.json                          the manifest
//     index.js                             the generated ES module

import { fileURLToPath } from "node:url";

import { Bundle, loadCore } from "../../../dist/bundle-writer.js";
import { GraphDef, SynthDef, control, in_, out, outCtl, sine, sub } from "../../../dist/defs/index.js";
import { knob, label, meter, panel, scope, toggle, view } from "../../../dist/gui/index.js";

/**
 * The bundle's name — the tag `index.js` registers, and the prefix its def
 * names carry (`fm-trem.voice`, `fm-trem.graph`).
 */
const BUNDLE = "fm-trem";

/**
 * A two-operator FM voice. `out` is the control the graph wires to its private
 * bus; `fm` is the modulation index (driven scaled, see the `bright` port).
 */
function fmVoice() {
    const freq = control("freq", 220.0);
    const ratio = control("ratio", 2.0);
    const index = control("fm", 3.0);
    const modulator = sine(freq.mul(ratio)).mul(freq).mul(index);
    const voice = sine(freq.add(modulator)).mul(0.5);
    return new SynthDef("voice", out(control("out", 0.0), voice));
}

/**
 * A tremolo reading the voice bus (the `in` control the graph wires) to the
 * hardware outputs, and publishing its LFO on `lfo_bus` — **a control**, so
 * each mounted instance watches the bus it was allocated instead of every
 * instance writing the same one.
 */
function tremolo() {
    const lfo = sine(control("rate", 4.0)).mul(0.5).add(0.5); // 0..1
    const gain = sub(1.0, lfo.mul(control("depth", 0.5))).mul(control("amp", 0.25));
    const sig = in_(control("in", 0.0)).mul(gain);
    return new SynthDef("trem", out(0.0, sig), out(1.0, sig),
                        outCtl(control("lfo_bus", 0.0), lfo));
}

/**
 * The composition: voice -> bus -> tremolo, and the **surface** — the named
 * ports the outside world sets. `bright` maps a 0..1 knob to FM index 0..8
 * (`.scaled`); the rest pass through 1:1.
 *
 * `lfo_bus` is a port like any other: that is how a per-instance bus reaches a
 * member's control without being baked into either def.
 */
function graph(voiceName, tremName) {
    const g = new GraphDef("graph");
    const voiceBus = g.bus("voice", { rate: "audio" });
    const v = g.add(voiceName, { out: voiceBus });
    const t = g.add(tremName, { in: voiceBus });
    g.port("freq", [v.control("freq")], 220.0);
    g.port("ratio", [v.control("ratio")], 2.0);
    g.port("bright", [v.control("fm").scaled(8.0)], 0.4);
    g.port("rate", [t.control("rate")], 4.0);
    g.port("depth", [t.control("depth")], 0.5);
    g.port("amp", [t.control("amp")], 0.25);
    g.port("lfo_bus", [t.control("lfo_bus")], 0.0);
    return g;
}

/**
 * The bundle: two declared symbols (the graph's node, the LFO bus), two
 * declared parameters, the three defs, and the GuiDef that drives them.
 *
 * Widget ids are **local** — the root is 1, so the children start at 2 — and
 * the mount offsets the whole block per instance.
 */
function build() {
    const b = new Bundle(BUNDLE);
    const freq = b.param("freq", "float", { default: 220.0, min: 60.0, max: 700.0 });
    const amp = b.param("amp", "float", { default: 0.25, min: 0.0, max: 0.5 });
    const node = b.node("graph");
    const lfo = b.bus("lfo");

    const voiceName = b.synthdef(fmVoice());
    const tremName = b.synthdef(tremolo());
    const graphName = b.graphdef(graph(voiceName, tremName));

    const portKnob = (widgetId, port, lo, hi, value) =>
        knob({ label: port, min: lo, max: hi, value,
               bind: ["/node_set", node, port], id: widgetId });

    b.gui(view(
        { title: "FM + tremolo (a GraphDef's surface)", w: 680, h: 400, layout: "col" },
        // The header row: the note, and this instance's own play/stop. A page
        // holding several instruments has them all sounding at once otherwise,
        // and each needs to be silenced on its own — which is what the toggle
        // is for, bound to `/node_run` on *this* instance's graph node. Pausing
        // a group skips its whole subtree on the audio thread, so a stopped
        // instrument costs nothing rather than merely going quiet. `weight`
        // splits the row 3:1.
        panel(
            { layout: "row", h: 30, id: 2 },
            label("every knob sets a surface port of the running GraphDef",
                  { weight: 3, id: 3 }),
            toggle({ label: "play", value: true, bind: ["/node_run", node], weight: 1,
                     id: 4 }),
        ),
        panel(
            { layout: "row", id: 5 },
            portKnob(6, "freq", 60.0, 700.0, freq),
            portKnob(7, "ratio", 0.5, 8.0, 2.0),
            portKnob(8, "bright", 0.0, 1.0, 0.4),
            portKnob(9, "rate", 0.2, 12.0, 4.0),
            portKnob(10, "depth", 0.0, 1.0, 0.5),
            portKnob(11, "amp", 0.0, 0.5, amp),
        ),
        panel(
            { layout: "row", id: 12 },
            meter(lfo, { rate: "control", min: 0.0, max: 1.0, label: "lfo", id: 13 }),
            scope(lfo, { rate: "control", min: 0.0, max: 1.0, label: "lfo", id: 14 }),
        ),
    ));
    // One message brings the instance up: its own node id, its own LFO bus and
    // its tag's parameters, as initial port values.
    //
    // The bus rides *in* the `/graph_new` rather than in an `/node_set` after
    // it, because a def latches its output bus when the synth starts — a later
    // value would arrive after the member had already chosen where to write.
    b.boot([
        "/graph_new", graphName, node, 0, 0,
        "lfo_bus", lfo, "freq", freq, "amp", amp,
    ]);
    b.preset("bright", { freq: 110.0, amp: 0.3 });
    return b;
}

await loadCore();
const dataDir = fileURLToPath(new URL("../../out/graph-controls", import.meta.url));
await build().write(dataDir);
console.log(`bundle written to ${dataDir}`);
console.log("\nserve the PACKAGE ROOT (clients/web) — not this folder — and " +
            "open the component page:\n");
console.log("    cd ../../..   # clients/web");
console.log("    ./build.sh && python3 -m http.server");
console.log("    http://localhost:8000/examples/panels/graph-controls/\n");
console.log("or run the same bundle self-contained on the desktop " +
            "(from clients/gui):\n");
console.log(`    cargo run --features standalone --bin clausters-gui -- ` +
            `--standalone ${BUNDLE} --data-dir ${dataDir}`);
