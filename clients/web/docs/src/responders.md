# Receiving: responders

Everything else here has the client *sending* — building OSC and pushing it to a server. A responder is the other direction: a callback that fires when a matching message **arrives**, and which may send onward, so the client becomes a hub rather than only a mouth.

The object is `OscFunc`, the same one the [Python client](https://clausters-python.readthedocs.io/) documents and the same one SuperCollider calls `OSCFunc`. Its model is shared and is not restated here; what follows is what the browser changes.

```js
import { OscFunc, Synth } from "clausters";

// Every node the server starts, as it starts.
const starts = new OscFunc((msg) => console.log("node", msg[1]), "/node_start");

// A def's SendReply, narrowed to one reply id.
new OscFunc(([, , , weight]) => new Synth("ping", { amp: weight }), "/onset", {
    argTemplate: [null, 1],
});

starts.free();
```

A responder is enabled the moment it is created. `disable()` suspends it and `enable()` puts it back; `free()` is permanent; `oneShot()` frees it after its first match. The callback is `func(msg, time, src)` — `msg` the message as `[addr, ...args]`, `time` the containing bundle's Unix seconds (`null` for an immediate or bare message), `src` the sender. `argTemplate` matches the arguments by position, an entry being a literal, a predicate or `null` for a hole; `oscfunc(path, options)(callback)` is the builder form.

## The receiver is the carrier, because a page has no port

A responder registers with a **receiver**, the thing that decodes packets and hands them around. In the reference client that is a UDP socket it binds — a port any application on the machine can target. A page can bind nothing and be targeted by nobody: what it has is the connection it already opened, to the in-page engine or to a `--ws` server. So `OscReceiver` wraps a `Connection`, and everything that arrives on it reaches the responders registered there.

Two consequences follow, both visible in the API:

- **`src` names a carrier**, not a `(host, port)` pair: a socket's URL, or `"page"` for the in-page engine. Narrowing a responder with `{ src }` still answers the question it answers there — *who sent this?* — with what a browser actually knows.
- **The default receiver is the ambient session's server.** A responder that names no `recv` resolves it the way [the ambient verbs](verbs.md) resolve a server, and fails the same way when there is none. Each server has one receiver (`server.receiver`), so a page holding two sessions gets each one's messages from each one's responders; `setDefaultOscReceiver` pins one anyway.

Every server reaches its own responders through `server.receiver`, which is also the door the client's own reply handling goes through — the node ids that recycle as `/node_end` arrives are an `OscFunc` like any other. Under it, `server.onReply` is still there as the raw seam: it sees every decoded message in arrival order with no matching of its own, which is what a stream decoder wants and what a responder is built on.

## Keep a callback quick

A callback runs on the page's one thread as the packet arrives, so a slow one delays everything else — the same golden rule the reference client states for its receiver thread, with the thread taken out. To *sequence* in response to a message, schedule a routine on a clock rather than looping inside the callback.

A receiver can also be given a clock (`new OscReceiver(connection, { clock })`), which dispatches its handlers through it: they then run as clock items, with the clock's logical time, instead of at the instant the bytes landed.

## What is not here

MIDI. `MidiFunc` and MIDI destinations are a separate milestone, since in a browser both directions are one API (Web MIDI) and neither exists in this package yet.

The repository's `examples/responders.html` is the worked example: a def reports its own onsets with `SendReply`, a responder answers each one with a synth, and two more keep the count of what is alive.
