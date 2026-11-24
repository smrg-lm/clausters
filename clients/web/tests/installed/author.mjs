// The bundle a project that ran `npm install clausters` writes: imported by
// the package's names, from its node_modules, and written with the writer's
// defaults -- so the generated module imports the run time as
// `clausters/runtime`, which the page maps (index.html).
//
// test.sh copies this directory into tests/out/installed/, installs the
// packed tarball there and runs this script, then the page.

import { loadCore } from "clausters/page";
import { Bundle } from "clausters/bundle";
import { SynthDef, control, out, outCtl, sine } from "clausters/defs";
import { knob, meter, view } from "clausters/gui";

// The writer validates through the core, so it is loaded first.
await loadCore();

// A tone whose amplitude follows an LFO, the LFO published on a control bus
// so the page's meter, and this test, can read it.
function tone() {
    const freq = control("freq", 220.0);
    const lfoBus = control("lfo_bus", 0.0);
    const lfo = sine(3.0).mul(0.4).add(0.5);
    return new SynthDef("tone", out(0.0, sine(freq).mul(lfo).mul(0.1)), outCtl(lfoBus, lfo));
}

const b = new Bundle("npm-voice");
const freq = b.param("freq", "float", { default: 220.0, min: 60.0, max: 700.0 });
const lfo = b.bus("lfo");
const node = b.node("tone");
b.synthdef(tone());
b.gui(
    view(
        { title: "installed", layout: "col", w: 480, h: 240 },
        knob({ label: "freq", value: freq, min: 60.0, max: 700.0, bind: ["/node_set", node, "freq"], id: 2 }),
        meter(lfo, { rate: "control", label: "lfo", id: 3 }),
    ),
);
b.boot(["/synth_new", "npm-voice.tone", node, 0, 0, "freq", freq, "lfo_bus", lfo]);
await b.write(new URL("./npm-voice", import.meta.url).pathname);
