// The worker of worker.html: a session on the page's engine, reached over the
// port the page linked to it. It imports the session layer and the builders,
// never the package facade, which defines a page's elements and needs a
// document a worker does not have.
//
// It plays an arpeggio on its own clock and, when the page asks, gets stuck
// in a loop that never yields -- the thing only terminating a worker stops.

import { Session, play } from "../../dist/session-layer.js";
import { Event } from "../../dist/seq/index.js";

const log = (line) => self.postMessage({ type: "log", line });

self.addEventListener("message", async (event) => {
  if (event.data.type === "link") {
    // The address of the page's engine, in a worker, is this port: the
    // session attaches to it as to any server already running.
    const session = (await Session.live(event.data.port)).activate();
    session.clock.start();

    function* arpeggio() {
      while (true) {
        for (const freq of [880.0, 1100.0, 1320.0, 1760.0]) {
          new Event({ freq, legato: 0.4, amp: 0.05 }).play();
          yield 0.25;
        }
      }
    }
    play(arpeggio);
    log("the worker plays an arpeggio on its own clock");
  } else if (event.data.type === "hang") {
    log("the worker is stuck in a loop: nothing it scheduled wakes again");
    for (;;) {}
  }
});
