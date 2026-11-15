// The worker of worker.html: a session on the page's engine, reached over the
// port the page linked to it. It imports the package as the page does; where
// there is no document the package registers no elements.
//
// It plays an arpeggio on its own clock, opens a slider for its level on
// the page's GUI host and, when the page asks, gets stuck
// in a loop that never yields -- the thing only terminating a worker stops.

import { Session, play } from "../../dist/index.js";
import { Event } from "../../dist/seq/index.js";
import * as gui from "../../dist/gui/index.js";

const log = (line) => self.postMessage({ type: "log", line });

self.addEventListener("message", async (event) => {
  if (event.data.type === "links") {
    // The address of the page's engine, in a worker, is this port: the
    // session attaches to it as to any server already running. Its GUI host
    // is the page's too, named the same way.
    const session = (await Session.live(event.data.engine)).activate();
    const host = await session.gui({ port: event.data.gui });
    session.clock.start();

    // A level for the arpeggio, on a slider the page places: its events come
    // back to this worker, so they stop when the worker is stuck.
    let level = 0.05;
    const panel = host.open(gui.view(
      { title: "the worker's panel", w: 360, h: 90, layout: "col", margin: 8.0, gap: 8.0 },
      gui.slider({ name: "level", min: 0.0, max: 0.1, value: level, h: 60 }),
    ));
    // The raw stream carries the hand's press and release too, as strings;
    // the value is the number.
    panel.widget("level").onEvent((value) => {
      if (typeof value === "number") level = value;
    });

    function* arpeggio() {
      while (true) {
        for (const freq of [880.0, 1100.0, 1320.0, 1760.0]) {
          new Event({ freq, legato: 0.4, amp: level }).play();
          yield 0.25;
        }
      }
    }
    play(arpeggio);
    log("the worker plays an arpeggio on its own clock; the slider sets its level");
  } else if (event.data.type === "hang") {
    log("the worker is stuck in a loop: nothing it scheduled wakes again");
    for (;;) {}
  }
});
