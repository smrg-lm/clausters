// The worker half of tests/worker.html: a session in a worker, on the engine
// of the page that started it, over the port the page linked to it. It
// imports the package as the page does.
//
// The page answers `measure` with the peak its analyser reads on the engine's
// output, so what this asserts is heard there, not bookkeeping here. It ends
// in an infinite loop on purpose: the page terminates it from there.

import { Session, play } from "../dist/index.js";
import { Buffer, DoneAction, Env, Synth, SynthDef, control, envGen, out, sine } from "../dist/defs/index.js";
import { Event } from "../dist/seq/index.js";
import { Routine } from "../dist/base/stream.js";

const say = (message) => self.postMessage(message);
const log = (line) => say({ type: "log", line });

let answer = null;
self.addEventListener("message", (event) => {
  if (event.data.type === "peak") answer?.(event.data.value);
});
const measure = () => new Promise((resolve) => {
  answer = resolve;
  say({ type: "measure" });
});

const voice = () => {
  const amp = control("amp", 0.2);
  const gate = control("gate", 1.0);
  const sig = sine(control("freq", 440.0)).mul(amp).mul(
    envGen(Env.asr(0.01, 1.0, 0.05), { gate, doneAction: DoneAction.FREE_SELF }));
  return new SynthDef("worker_voice", out(0.0, sig), out(1.0, sig));
};

async function run(port) {
  const session = await Session.live(port);
  log("a session in the worker, attached over the page's link");
  const server = session.server;
  await voice().send(server);

  // A synth: a def sent and a node played from here, heard on the page.
  const note = session.use(() => new Synth("worker_voice", { freq: 440.0 }));
  const synthPeak = await measure();
  note.set({ gate: 0.0 });
  log(`a synth from the worker: peak ${synthPeak.toFixed(3)}`);

  // A buffer: samples installed over the port's bulk path, read back.
  const samples = new Float32Array(4800).map((_, i) => Math.sin(i / 10));
  const buffer = await Buffer.fromSamples(samples, 1, 48_000, { server });
  const back = await buffer.getSamples({ count: 16 });
  const loaded = back.every((v, i) => Math.abs(v - samples[i]) < 1e-6);
  log(`a buffer loaded and read back: ${loaded ? "the same samples" : "different samples"}`);

  // Offline in the worker: a score rendered with no engine at all.
  const offline = await Session.nrt();
  await voice().send(offline.server);
  const held = new Synth("worker_voice", { freq: 330.0 }, { server: offline.server });
  Routine.run(function* () {
    yield 1.0;
    offline.server.sendBundle([["/node_free", held.id]]);
  }, offline.clock);
  const stats = await offline.render({ sampleRate: 48_000 });
  const renderPeak = stats.samples.reduce((p, v) => Math.max(p, Math.abs(v)), 0);
  await offline.close();
  log(`an offline render: ${stats.frames} frames, peak ${renderPeak.toFixed(3)}`);

  // A routine on the session's clock, then a loop that never yields: the
  // page has to terminate this worker to stop it.
  session.activate().start();
  play(function* () {
    for (;;) {
      play(new Event({ instrument: "worker_voice", degree: 0, dur: 0.2, amp: 0.2 }));
      yield 0.25;
    }
  });
  const routinePeak = await measure();
  log(`a routine on the clock: peak ${routinePeak.toFixed(3)}`);
  say({ type: "results", synthPeak, loaded, renderPeak, routinePeak });
  say({ type: "looping" });
  for (;;) {}
}

self.addEventListener("message", (event) => {
  if (event.data.type === "link") {
    run(event.data.port).catch((error) => say({ type: "error", message: String(error) }));
  }
});
