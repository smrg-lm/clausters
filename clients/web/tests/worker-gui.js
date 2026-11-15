// The worker half of tests/worker-gui.html: a session in a worker on the
// page's engine, with the page's GUI host reached over a second link. It opens
// the panel tests/gui.html opens from the page, binds its knob to a node,
// listens to its slider, and opens a scope on a tone; the page plays the hand
// on the canvas it placed the window in.

import { Session, scope } from "../dist/session-layer.js";
import { Synth, SynthDef, control, out, sine } from "../dist/defs/index.js";
import * as gui from "../dist/gui/index.js";

const say = (message) => self.postMessage(message);
const log = (line) => say({ type: "log", line });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

let played = null;
self.addEventListener("message", (event) => {
  if (event.data.type === "played") played?.();
});
/** Asks the page to play a gesture on the window, and waits until it has. */
const ask = (gesture) => new Promise((resolve) => {
  played = resolve;
  say({ type: "gesture", gesture });
});

async function run(enginePort, guiPort) {
  const session = (await Session.live(enginePort)).activate();
  const server = session.server;
  const host = await session.gui({ port: guiPort });
  log("a session in the worker, with the page's GUI host attached over a link");

  const freq = control("freq", 220.0);
  const amp = control("amp", 0.1);
  await new SynthDef("worker_gui_tone", out(0.0, sine(freq).mul(amp))).send(server);
  const synth = new Synth("worker_gui_tone", { freq: 220.0, amp: 0.1 }, { server });
  await server.sync();

  // The panel tests/gui.html opens from the page, opened from here.
  const tree = gui.view(
    { title: "driven from a worker", w: 480, h: 300, layout: "col", margin: 8.0, gap: 8.0 },
    gui.label("knob: bound to the engine -- slider: back to this worker", { h: 28 }),
    gui.knob({ name: "freq", min: 50.0, max: 2000.0, value: 220.0, h: 110 }),
    gui.slider({ name: "cutoff", min: 0.0, max: 1.0, value: 0.2, h: 110 }),
  );
  const win = host.open(tree);
  const events = [];
  win.widget("cutoff").onEvent((...args) => events.push(args));
  win.widget("freq").bind("/node_set", synth.id, "freq");
  const info = await win.widget("cutoff").query();
  const shown = info.type === "slider" && info.props.max === 1;
  log(`a window from the worker: ${info.type} over [${info.props.min}, ${info.props.max}]`);
  await sleep(600);

  // The slider reports back to this worker.
  await ask("slider");
  for (let i = 0; i < 40 && events.length === 0; i++) await sleep(50);
  const moved = (await win.widget("cutoff").query()).props.value;
  const heard = events.length > 0 && moved !== 0.2;
  log(`the slider's /gui_event reached the worker: ${heard} (value ${moved})`);

  // The knob is bound: its value goes to the engine, not to this worker.
  const before = (await synth.info()).controls.freq;
  const seen = events.length;
  await ask("knob");
  let after = before;
  for (let i = 0; i < 40 && after === before; i++) {
    await sleep(50);
    after = (await synth.info()).controls.freq;
  }
  const bound = after !== before && events.length === seen;
  log(`the bound knob drove the engine: freq ${before} -> ${after}`);

  // A scope on the tone, on the same host: fed from the engine it is wired to.
  const view = await scope(0, { channels: 1 });
  const traced = await view.host.query(view.widgetId);
  await sleep(500);
  const profile = await view.host.profile(view.id, 1.0);
  const live = Number(profile.live ?? 0) + Number(profile.whole ?? 0);
  const fed = traced.type === "signal" && live > 0;
  log(`a scope from the worker: ${traced.type}/${traced.props.view}, ${live} frames drawn in 1 s`);

  say({ type: "results", shown, heard, bound, fed, window: win.id, scope: view.id });
}

self.addEventListener("message", (event) => {
  if (event.data.type === "links") {
    run(event.data.engine, event.data.gui)
      .catch((error) => say({ type: "error", message: String(error) }));
  }
});
