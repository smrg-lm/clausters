# The client, layer by layer

The package mirrors the [Python client](https://clausters-python.readthedocs.io/) module for module: `base/` is the server-agnostic seam (the codec, the clock, the values), `defs/` the definitions and the `Server` that drives them, `gui/` the GUI-host driver, `seq/` the events and patterns. A reader who knows one client finds the other at the same relative path.

What follows is the layers from the bottom up, and — at the end — the three places where the browser makes this client shaped differently from its reference.

## The core: what is not written in TypeScript

Everything that is a *value* or a *time* comes from `clausters-core` compiled to wasm, the same crate the server itself runs and the same one the Python client reaches through its C ABI. The OSC bytes, the beat/second/sample conversions, the bar grid, the NTP timetag, the beat-ordered scheduler queue, the seeded random stream, the builtins, the node/bus/buffer allocators: all of them are calls into that bundle.

This is not an optimization. It is what makes a page numerically **equal** to the server and to the other client rather than merely close: a seeded pattern replays identically in three languages, and a beat resolves to the same instant on both sides of a socket, because there is one implementation.

Loading it is explicit and happens once:

```js
await loadOsc();      // the codec; loadCore() is the same bundle, either name
```

Everything below assumes it has been awaited.

## The connection seam

`Connection` is one duplex-OSC interface with two implementations:

```js
const connection = await pageConnection();               // this tab's engine
const connection = await WsConnection.open(url);         // a `--ws` server
```

`pageConnection()` wraps the per-page engine singleton — the server compiled to wasm in this tab's AudioWorklet — and `WsConnection` a browser (or node) `WebSocket`. Both carry raw OSC in both directions and nothing else, so **no layer above them names a transport**. Swapping carriers is a one-line edit in a program of any size, which is exactly the property the examples demonstrate by offering a radio button.

The engine singleton is reachable directly when a page needs the browser-specific parts: `server()` gives `send`/`addReply`, its `clock()`, `bLoad(...)` (the browser's `/b_allocRead`, over `fetch` and `decodeAudioData`) and `resume()`/`suspend()`. Every component and script in the tab gets that same engine, so they meet in one node, bus and buffer namespace.

## Defs and the `Server`

`Server` is the only object that knows a connection.

```js
const server = await Server.open(connection);
```

Opening it queries `/server_info` and sizes the allocators from the answer, so the client's ids match the server that is actually running. It registers for the server's pushes, which is what lets a node id be recycled once its `/n_end` arrives, and it carries what is the server's own: the transport (`sendMsg`, `sendBundle`, `request`, `sync()`), the id pools, `freeDef`, the bus and tap subscriptions, and the introspection queries about what it holds (`queryInfo`, `queryDefs`, `queryBuffers`, `queryUgens`, `queryTree`, `dumpGraph`). A command addressed to a resource is that resource's method — `def.send(server)`, `Synth.new(server, …)`, `node.set`, `bus.watch`, `buffer.getSamples` — so the receiver is never an argument, and that holds for a question about one resource too: `node.info()` and `buffer.info()` ask about themselves, where `queryTree` and `queryBuffers` ask about all of them.

Two def families are peers, as everywhere in Clausters:

```js
const def = new SynthDef("voice", out(0.0, rlpf(saw(freq), cutoff).mul(amp)));
const fdef = FaustDef.fromSignals("blown", signals.hslider("freq", 440, 50, 2000, 1).sin());
```

`SynthDef` takes a UGen graph built from the lowercase callables (`sine`, `saw`, `rlpf`, `envGen`, `playBuf`, `out`, …); a `FaustDef` is built from a signal expression (`fromSignals`), a box tree or Faust source (`fromSource`) — the last only against a native server, the in-page engine having no Faust compiler in it. `GraphDef` wires several of either into one named, instantiable configuration with a port surface.

A def is a plain value until it is sent, and the definitions themselves mean exactly what the [server book](https://clausters.readthedocs.io/) says they mean: this client only builds the JSON.

Handles are `Synth`, `Group`, `Bus` and `Buffer` — thin objects over an id from the core's allocator, freed by `node.free()`, `bus.free()`, `buf.free()`.

## Driving the GUI host

`GuiHost` is the same seam again — a connection and a name:

```js
const host = await GuiHost.page();            // the wasm host on this page
const host = await GuiHost.connect(url);      // a native `clausters-gui --ws`
```

The widget catalogue is a set of builders in the `gui` namespace, and a whole tree goes out in one `/gui_def`:

```js
const win = host.open(gui.window(
  { title: "a tone", w: 480, h: 240, layout: "col" },
  gui.knob({ name: "freq", label: "freq", min: 50.0, max: 2000.0, value: 220.0 }),
  gui.meter(level.index, { name: "level", label: "level" }),
));
win.widget("freq").bind("/n_set", note.id, "freq");
win.widget("freq").onEvent((value) => console.log(value));
```

Widgets are addressed by **name**, not by integer id. A **bound** widget's value travels from the host straight to the audio server with no round trip through the page's script — the gesture stays on the host's own frame — while an unbound one comes back as an event. The builders' options are camelCase where the wire's props are snake_case (`textSize` → `text_size`); the emitted document is byte-comparable with the Python builders', which is what the parity vectors hold.

On this page, the host draws **one canvas per `window`-rooted def**, and the page supplies the canvas: `attach(defId, canvas)`, with the size and the visibility told to the host rather than read from the DOM. A canvas out of the viewport stops drawing and drops its buses from the streams it was subscribed to.

The size is the **document's**, not the host's. A canvas starts at the host's default, so bind it to the element that should govern it:

```js
const win = host.open(tree);
const stop = (await guiHost()).fit(win.id, container);   // and follows it
```

`fit` sets the canvas' backing store from the element's box in device pixels and tells the host — the pixels **and** the `devicePixelRatio` they were measured at, since the sizes a GuiDef declares are logical and the host resolves them against that ratio — then keeps doing it as the box changes. So the drawing is as wide as the layout allows on a desktop and as wide as the screen on a phone, with no fixed size anywhere, and a 28-pixel strip looks the same on a retina display as on an ordinary one. A `<clausters-bundle>` component does exactly this for its own element; a script that opens a window calls it once.

A page that sizes a canvas itself does the same two things: `canvasBox(element)` measures both halves, and `bridge.resize(defId, width, height, scale)` reports them together (the product alone cannot be un-multiplied, which is why the ratio travels beside it). It also watches **two** triggers, because the box and the density move independently: a `ResizeObserver` for the layout, and `onScaleChange` for the scale — browser zoom or a drag onto a monitor of another density changes `devicePixelRatio` with the CSS box untouched, which no resize observer reports. `fit` and the component do both already.

## The clock and the patterns

`TempoClock` is musical time and the driver that resumes routines on it.

```js
const clock = new TempoClock(2.0, { timebase: await server.sampleTimebase() });
clock.start();

new seq.Pbind({
  instrument: "voice",
  degree: new seq.Pseq([0, 2, 4, 7], seq.INF),
  dur: new seq.Pseq([0.5, 0.25, 0.25]),
}).play(server, { clock });
```

A **routine** is a generator function; it yields a delay in beats and is resumed at that beat. The logical beat advances **only by those yields**, so a late wake-up shifts nothing: the lateness is absorbed by the timetag, not by the music. Never `await` inside a routine — the page has one thread, and a suspended routine would hold the clock.

Two things around it are the browser's:

- **The wake-up** sits behind a `Ticker`: a shared worker in a tab (a page's own timers are throttled to about a second in the background), `setTimeout` elsewhere. Tests fill the same seam by hand and so drive the real driver deterministically.
- **The timebase** decides what the clock measures sleeps against and how emissions are stamped. `MonotonicTimebase` (the default) paces on `performance.now()` and sends NTP-timetagged bundles; `SampleTimebase` paces on the server's own sample counter and emits `/sched <absolute sample>`, which removes the drift between two clocks entirely. `server.sampleTimebase()` builds one — the `Server` anchors it because the `Server` is what knows the carrier, and **the clock itself never talks to a server**.

[Routines and clocks](routines-and-clocks.md) is this layer written out, from a melody by hand to the patterns above it.

Above that sit `Event` and `rest`, the value patterns (`Pseq`, `Pser`, `Prand`, `Pwhite`, `Pseries`, `Pgeom`, `Pfunc`, `Pn`, `Pconst`), `Pbind`, and the seekable counterpart: `Timeline`, a static beat-sorted list that `Timeline.fromPattern` can bounce a pattern into, played by a `Playhead` with play/stop/locate/loop. Random values come from the stream a routine derives when it is created, so `seed(n)` replays a whole piece. The model itself — what an event's keys mean, how `dur` and `sustain` differ, what `Pbind` does with a pattern of patterns — is the Python book's [routines and clocks](https://clausters-python.readthedocs.io/) chapters; it is the same model.

## Three things the browser changes

**Everything that waits is a promise.** Where the reference client blocks a thread on a reply, this one `await`s: `await server.sync()`, `await bus.get()`, `await server.queryTree()`. The page has a single thread and has to keep running, so "never block the clock thread" — a discipline in the Python client — is here simply the language.

**The graph composes by method.** `sine(freq).mul(amp).add(bias)` where Python writes `sine(freq) * amp + bias`, TypeScript having no operator overloading. Because of that, parity between the clients is asserted on the **emitted spec**, never on the source text.

**Nothing pumps.** There is no drain call and no event loop of ours: a page subscribes once (`onEvent`, `onClosed`, `onReply`) and the host's or server's messages arrive as calls, while a query resolves a promise.

## Errors

`ClaustersError` is the root: `CommandError` for a `/fail` from the server, `ReplyTimeout` for a reply that never came, `AllocationError` for an exhausted or double-released id pool. They are ordinary exceptions from ordinary async calls.
