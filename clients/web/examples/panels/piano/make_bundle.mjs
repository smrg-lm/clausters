#!/usr/bin/env node
// Author, with the TypeScript client, a bundle whose GuiDef is a playable
// piano — then mount it as a web component, twice on one page if you like.
//
// The point of this example is the `piano` widget's **host-voice mode with no
// script at run time**: the client only *authors* the files (it talks to
// nothing); what runs afterwards is the persisted bundle, and the *host*
// manages one server voice per held key:
//
//     key press   -> /synth_new piano.voice <id> 0 0 freq <hz> amp <vel/127> gate 1
//     key release -> /node_set <id> gate 0        (the envelope releases and
//                                                  the def frees the node itself)
//
// so the keyboard plays the wasm engine in the tab with zero page JS — the
// same posture as `examples/panels/graph-controls`, whose knobs bind
// `/node_set`s. The other mapping path (the widget unbound, the script
// programming voices from the `"note"` events) is the Python example
// `clients/python/examples/panels/piano.py`.
//
// **What makes it a component.** The voice publishes its envelope on a control
// bus, and the bus is *declared* rather than picked:
//
//     const env = b.bus("env");                  // -> "@env"
//     ...  outCtl(control("env_bus"), env)       // the def takes the bus it
//                                                //    is given
//
// The def payload holds no bus number, so mounting the bundle twice gives each
// instance its own bus and each meter reads its own keyboard. Written the old
// way — `outCtl(0.0, env)`, the number compiled in — both instances would
// write bus 0 and the page would show one signal twice. That is the authoring
// rule the whole format rests on: *a bus, a node or a buffer reaches a def as
// a control, never as a baked constant.*
//
// `title` is a declared parameter, so the markup can name each instance:
//
//     <piano-keys title="left hand"></piano-keys>
//     <piano-keys title="right hand"></piano-keys>
//
// The keyboard itself: real piano proportions (it resizes with the element),
// the overview strip above the keys pans/zooms the visible MIDI range, and the
// keys outside the 88-key piano range draw grayed (`activeMin`/`activeMax`).
//
// **It is a node script, not a page**: writing a bundle is not something a
// page does, because a bundle is an *input* a static page boots with no
// interpreter at all. The Python client's `clausters.bundle` is the same
// writer in the other language, and the two emit the same directory byte for
// byte.
//
// Run it (from this directory, after `../../../build.sh`):
//
//     node make_bundle.mjs
//
// It writes into `examples/out/piano/` — the ignored directory every generator
// in this tree writes to, so a run leaves nothing to clean up by hand. Then
// the **same** bundle runs on every leg, no script attached to any of them:
//
// - **Browser, as a web component** (the wasm engine in an AudioWorklet):
//   serve **from `clients/web` — the package root, never this folder** (the
//   page imports `../../../dist/...`, which must stay inside the served root):
//
//       cd clients/web && python3 -m http.server
//
//   and open `http://localhost:8000/examples/panels/piano/` — `index.html`
//   here imports the generated module, which registers the `<piano-keys>` tag;
//   its power button boots the whole instrument in the tab.
// - **Desktop, self-contained** (the embedded server; from `clients/gui`):
//
//       cargo run --features standalone --bin clausters-gui -- \
//           --standalone piano --data-dir <clients/web>/examples/out/piano
//
// - **Desktop, loopback** (a running `clausters` + `clausters-gui --server`
//   pointing at it, the bundle's dir as `--data-dir`): the same files again.
//
// The layout it writes (the native persisted formats plus the manifest, which
// both the browser and the desktop read):
//
//     defs/synthdefs/piano.voice.json    the voice (the /def_send synth payload)
//     defs/guidefs/piano.json            the GuiDef record — a template
//     bundle.json                        the manifest
//     index.js                           the generated ES module

import { fileURLToPath } from "node:url";

import { Bundle, loadCore } from "../../../dist/bundle-writer.js";
import {
    DoneAction, Env, SynthDef, control, envGen, out, outCtl, sine,
} from "../../../dist/defs/index.js";
import { label, meter, piano, view } from "../../../dist/gui/index.js";

/**
 * The bundle's name — the tag `index.js` registers, and the prefix its def
 * names carry (`piano.voice`), since a def name is a global namespace on the
 * server.
 */
const BUNDLE = "piano";
/**
 * The custom element the generated module registers. HTML wants a hyphen in a
 * custom element name, and "piano" — a perfectly good GuiDef name on the
 * desktop — has none.
 */
const TAG = "piano-keys";

/**
 * The gated voice a key plays: the conventional `freq`/`amp`/`gate` surface
 * the piano's host-voice mode drives — the note-on opens the gate, the
 * note-off closes it, and the release tail frees the synth (`FREE_SELF`).
 *
 * The envelope goes out on `env_bus`, **a control**: the mount passes each
 * instance the bus it allocated, so two keyboards on a page do not write over
 * each other.
 */
function voice() {
    const freq = control("freq", 440.0);
    const amp = control("amp", 0.2);
    const gate = control("gate", 1.0);
    const envBus = control("env_bus", 0.0);
    const env = envGen(
        Env.adsr(0.005, 0.1, 0.7, 0.4),
        { gate, doneAction: DoneAction.FREE_SELF },
    );
    const sig = sine(freq).mul(env).mul(amp);
    return new SynthDef("voice", out(0.0, sig), out(1.0, sig), outCtl(envBus, env));
}

/**
 * The bundle: the declared bus and title, the voice, and the GuiDef that plays
 * it.
 *
 * Widget ids are **local** — the root is 1, so the children start at 2 — and
 * the mount offsets the whole block per instance.
 */
function build() {
    const b = new Bundle(BUNDLE);
    const title = b.param("title", "string", { default: "Piano (host voices)" });
    const env = b.bus("env");
    const voiceName = b.synthdef(voice());

    b.gui(view(
        { title, w: 820, h: 300, layout: "col" },
        label("click/drag plays; drag the strip to pan, wheel to zoom", { id: 2 }),
        // `voice` names the def the host spawns per held key; `voiceArgs`
        // rides along with every /synth_new, which is how this instance's own
        // bus reaches its voices.
        piano({ min: 48, max: 84, activeMin: 21, activeMax: 108,
                voice: voiceName, voiceArgs: [["env_bus", env]], label: "keys",
                id: 3 }),
        meter(env, { rate: "control", min: 0.0, max: 1.0, label: "env", id: 4 }),
    ));
    return b;
}

await loadCore();
const dataDir = fileURLToPath(new URL("../../out/piano", import.meta.url));
await build().write(dataDir, { tag: TAG });
console.log(`bundle written to ${dataDir}`);
console.log("\nserve the PACKAGE ROOT (clients/web) — not this folder — and " +
            "open the component page:\n");
console.log("    cd ../../..   # clients/web");
console.log("    ./build.sh && python3 -m http.server");
console.log("    http://localhost:8000/examples/panels/piano/\n");
console.log("or run the same bundle self-contained on the desktop " +
            "(from clients/gui):\n");
console.log(`    cargo run --features standalone --bin clausters-gui -- ` +
            `--standalone ${BUNDLE} --data-dir ${dataDir}`);
