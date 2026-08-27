#!/usr/bin/env node
// Author a standalone bundle, then launch it from `clausters-gui` alone.
//
// A *bundle* is a data directory that holds a named GuiDef beside the
// SynthDefs/GraphDefs it needs. `clausters-gui --standalone <name>` boots such
// a bundle against an **embedded** audio server (loaded in-process), runs the
// GuiDef's `boot` messages to bring the instrument up, then opens its window —
// a self-contained instrument with **no separate audio server and no language
// client running**.
//
// This script does the *authoring* half. It writes a bundle to disk and prints
// the single command that launches it; it talks to nothing. A bundle is just
// files, so the layout is the whole story:
//
//     <out>/defs/synthdefs/standalone_drone.json   the instrument (a SynthDef
//                                                    spec, the /def_send synth
//                                                    payload)
//     <out>/defs/guidefs/drone.json                the GuiDef record
//
// Two GuiDef features make a saved tree self-driving, so it needs no live
// script:
//
// - a root `boot` list — OSC messages the standalone host sends right after
//   the defs load, to instantiate the instrument (here one `/synth_new`
//   creating node 1000 from the drone SynthDef);
// - a widget `bind` prop — the declarative form of `/gui_bind`, wiring the
//   knob's value **straight to the embedded server** (here `/node_set 1000
//   freq`), so turning it changes the pitch with no round-trip through any
//   script.
//
// **This is the node half of a pair**, and the page beside it is not the other
// half of *this* — `standalone.html` is the counterpart of **running**
// `clausters-gui --standalone`, in a tab. The counterpart of this script is
// `clients/python/examples/panels/standalone.py`: the same two files written
// by the same calls in the other language. A page never writes a bundle,
// because a bundle is an input a static page boots with no interpreter at all.
//
// It writes the files itself rather than through `Bundle`, and that is the
// point of this example: the persisted format is plain JSON, and a host reads
// a directory somebody could have typed. `Bundle` is the writer for the
// component format on top of it (the manifest, the holes, the parameters) —
// `examples/panels/piano/make_bundle.mjs` is that one. One consequence worth
// knowing before diffing the two halves of the pair: what they produce is the
// same tree and not the same bytes, because writing by hand skips the
// canonical form `Bundle` emits — the key order is each builder's, and
// JavaScript spells `160.0` as `160`.
//
// Run it (from this directory, after `../../build.sh`):
//
//     node standalone.mjs
//
// It writes into `examples/out/standalone/` — the ignored directory every
// generator in this tree writes to — and prints the two ways to boot it.

import { mkdir, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { loadCore } from "../../dist/bundle-writer.js";
import { SynthDef, control, out, sine } from "../../dist/defs/index.js";
import { knob, view } from "../../dist/gui/index.js";

/**
 * The instrument's def name; the GuiDef's `boot` /synth_new references it, and
 * it is the file stem under `defs/synthdefs`.
 */
const SYNTH_NAME = "standalone_drone";
/**
 * The GuiDef (bundle) name; the file stem under `defs/guidefs` and the name
 * passed to `--standalone`.
 */
const GUI_NAME = "drone";
/**
 * The node id the boot message creates and the knob binds to. A fixed,
 * script-allocated id, so the saved GuiDef can name it directly.
 */
const DRONE_NODE = 1000;

/**
 * A quiet stereo sine drone whose pitch is the `freq` control (default 160 Hz)
 * — the boot `/synth_new` instantiates it and the knob's binding drives its
 * `freq`.
 */
function drone() {
    const sig = sine(control("freq", 160.0)).mul(0.2);
    return new SynthDef(SYNTH_NAME, out(0.0, sig), out(1.0, sig));
}

/**
 * The GuiDef: one knob over a low range, made self-driving by `boot` and
 * `bind` so the standalone host needs no script.
 *
 * - `boot` runs once after the defs load: create node `DRONE_NODE` from the
 *   drone SynthDef in the root group.
 * - the knob's `bind` forwards its value as `/node_set <DRONE_NODE> freq
 *   <value>` straight to the embedded server on every turn.
 * - `name` lets a *live* `clausters-gui --data-dir` auto-persist this same
 *   tree on `/gui_def`; here we write the file ourselves, so it is only for
 *   symmetry with that path.
 */
function scene() {
    return view(
        // `hug` sizes the window to what it holds -- this one holds a knob, so
        // the window is the knob: no size to declare and none to keep in step
        // with it.
        {
            title: "Standalone drone", hug: true, layout: "col",
            name: GUI_NAME,
            boot: [["/synth_new", SYNTH_NAME, DRONE_NODE, 0, 0]],
        },
        knob({ id: 10, label: "freq", min: 80.0, max: 400.0, value: 160.0,
               bind: ["/node_set", DRONE_NODE, "freq"] }),
    );
}

/**
 * Writes the two bundle files under `dataDir` and returns their paths.
 *
 * A SynthDef file is exactly the `/def_send synth` spec JSON
 * (`SynthDef.dumpDef`); a GuiDef record wraps the tree with the id it is
 * defined under, `{"id": <int>, "gui": <tree>}` — the standalone host replays
 * it as `/gui_def <id> <tree>`.
 */
async function writeBundle(dataDir) {
    const synthPath = join(dataDir, "defs", "synthdefs", `${SYNTH_NAME}.json`);
    const guiPath = join(dataDir, "defs", "guidefs", `${GUI_NAME}.json`);
    for (const path of [synthPath, guiPath]) await mkdir(dirname(path), { recursive: true });
    await writeFile(synthPath, drone().dumpDef());
    await writeFile(guiPath, JSON.stringify({ id: 1, gui: scene() }));
    return [synthPath, guiPath];
}

// The core carries the def builders' arithmetic, so it goes in before one is
// built -- the one call that differs from the Python half, where the client
// loads its own native library on import.
await loadCore();
const dataDir = process.argv[2]
    ? join(process.cwd(), process.argv[2])
    : fileURLToPath(new URL("../out/standalone", import.meta.url));
const [synthPath, guiPath] = await writeBundle(dataDir);
console.log(`wrote ${synthPath}`);
console.log(`wrote ${guiPath}`);
console.log("\nlaunch the bundle as a self-contained instrument (the GUI host is " +
            "its own workspace, so point cargo at its manifest -- run this from the " +
            "repo root):\n");
console.log(`    cargo run --manifest-path clients/gui/Cargo.toml ` +
            `--features standalone --bin clausters-gui -- ` +
            `--standalone ${GUI_NAME} --data-dir ${dataDir}\n`);
console.log("a window opens; turning the knob drives the drone's freq on the " +
            "embedded server (no other process).");
console.log("\nthis script only WRITES the bundle once. Re-launching it needs no " +
            "interpreter: the line above (or, with [standalone].gui set in your " +
            "config, just `clausters-gui --standalone`) runs the app directly. The " +
            "embedded server loads the data-dir's defs and boot.json itself.");
console.log("\nor boot the same bundle in a browser tab (from clients/web, after " +
            "./build.sh):\n");
console.log(`    python3 tools/bundle-manifest.py ${dataDir}`);
console.log("    python3 -m http.server  # then open");
console.log("    http://localhost:8000/examples/panels/standalone.html" +
            "?bundle=/examples/out/standalone\n");
console.log("the engine runs in an AudioWorklet, the GUI on a canvas — no server " +
            "process anywhere.");
