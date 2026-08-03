# The client, layer by layer

The package mirrors the [Python client](https://clausters-python.readthedocs.io/) module for module: `base/` is the server-agnostic seam (the codec, the clock, the values), `defs/` the definitions and the `Server` that drives them, `gui/` the GUI-host driver, `seq/` the events and patterns, and `session.ts` the handle that bundles them. A reader who knows one client finds the other at the same relative path.

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

`pageConnection()` wraps the page's engine — the server compiled to wasm in this tab's AudioWorklet — and `WsConnection` a browser (or node) `WebSocket`. Both carry raw OSC in both directions and nothing else, so **no layer above them names a transport**. Swapping carriers is a one-line edit in a program of any size, which is exactly the property the examples demonstrate by offering a radio button.

That engine is reachable directly when a page needs the browser-specific parts: `server()` gives `send`/`addReply`, its `clock()`, `bLoad(...)` (the browser's `/buffer_allocRead`, over `fetch` and `decodeAudioData`) and `resume()`/`suspend()`. Every component and script in the tab gets that same engine, so they meet in one node, bus and buffer namespace.

### More than one of either

One engine and one GUI host per page is the **default, not a limit**: it is what a page wants, since its components belong to one mix. A document embedding several *independent* clients — notebooks open in one tab, isolated demos side by side — needs each to keep its own node, bus, buffer and widget ids, and asks for its own of each:

```ts
import { engine, pageConnection, Server } from "clausters";

const audio = await engine({ channels: 2 });      // not the page's
const server = await Server.open(await pageConnection(audio));
```

The GUI host has the same pair. `newGuiHost()` boots an instance — its own engine unless you hand it one — where `guiHost()` returns the page's:

```ts
import { newGuiHost, newPools } from "clausters";

const gui = await newGuiHost();                   // not the page's
const pools = newPools();                         // nor its ids
gui.attach(windowId, myCanvas);                   // and it appends no canvas
```

Instances share the browser tab and nothing else, so two of them may use the very same window, widget and node ids without colliding — which is the point, since clients that allocate ids independently have no way to agree on a range. Take `newPools()` along with the host: the page's pools are shared by everything that shares the page's engine, and an independent client wants an id space of its own. A second host costs neither a download nor a GPU device; a second engine is a second `AudioContext`, and browsers cap those (Chrome at six). `gui.bridge.close()` releases one.

What each pair differs in is only what a page wants by default. `guiHost()` and `server()` are memoized and come with the page's default canvas; `newGuiHost()` and `engine()` are neither, since a host that is not the page's has no business appending a canvas to `<body>`. `examples/two-hosts.html` shows the whole arrangement end to end.

## Defs and the `Server`

`Server` is the only object that knows a connection.

```js
const server = await Server.open(connection);
```

Opening it queries `/server_query` and sizes the allocators from the answer, so the client's ids match the server that is actually running. It registers for the server's pushes, which is what lets a node id be recycled once its `/node_end` arrives, and it carries what is the server's own: the message paths (`sendMsg`, `sendBundle`, `request`, `sync()`), the id pools, `freeDef`, the bus and tap subscriptions, the [shared transport](transport.md) (`setTransport`, `transportGroup`, `transportPlay`/`transportStop`/`transportLocate`, `schedAtTransport`), and the introspection queries about what it holds (`queryInfo`, `queryDefs`, `queryBuffers`, `queryUgens`, `queryTree`, `dumpGraph`). A command addressed to a resource is that resource's method — `def.send()`, `node.set`, `bus.watch`, `buffer.getSamples` — so the receiver is never an argument, and that holds for a question about one resource too: `node.info()` and `buffer.info()` ask about themselves, where `queryTree` and `queryBuffers` ask about all of them.

Two def families are peers, as everywhere in Clausters:

```js
const def = new SynthDef("voice", out(0.0, rlpf(saw(freq), cutoff).mul(amp)));
const fdef = FaustDef.fromSignals("blown", signals.hslider("freq", 440, 50, 2000, 1).sin());
```

`SynthDef` takes a UGen graph built from the lowercase callables (`sine`, `saw`, `rlpf`, `envGen`, `playBuf`, `out`, …) — the whole catalogue the server has, family for family with the Python client's, so a graph written against one transcribes into the other. Three of its families are worth naming because they are not read like the rest:

- **The frequency-domain chain** — `fft` opens a spectral *frame*, each `pv*` transforms it in place and `ifft` closes it back to samples. The frame is synth-private scratch, so nothing is allocated and only `fft` names a size; the server propagates it down the chain. `pvKernel` is the general case: a per-bin program written with the terms in `defs/pv_expr` (`mag`, `phase`, `binIndex`, `nbins`, `binfreq`, `param(i)`), which the server validates when the def is sent.
- **The demand streams** — `dseq`, `dxrand`, `dbrown` and the rest have no samples, only a next value, and they yield one each time a driver (`demand`, `duty`, `tduty`) asks. A stream's items may be streams themselves, which is what makes a sequence of phrases expressible; `repeats` of `0` means endlessly. Because a stream is *pulled*, it cannot be shared: two drivers reading one `dseq` take alternate items from it, so give each its own.
- **The stereo field** — `pan2`, `panAz`, `rotate2`, `midSide` and `stereoWidth` produce several channels, and a UGen has one output, so each returns a `ChannelList` the same way `dup` does.

`diskIn`/`diskOut` are the one pair that only means something against a **native** server: they stream the server's own filesystem, which a tab does not have.

A `FaustDef` is built from a signal expression (`fromSignals`), a box tree or Faust source (`fromSource`) — the last only against a native server, the in-page engine having no Faust compiler in it. `GraphDef` wires several of either into one named, instantiable configuration with a port surface.

A def is a plain value until it is sent, and the definitions themselves mean exactly what the [server book](https://clausters.readthedocs.io/) says they mean: this client only builds the JSON.

Handles are `Synth`, `Group`, `Bus` and `Buffer` — thin objects over an id from the core's allocator, freed by `node.free()`, `bus.free()`, `buf.free()`. **The constructor creates the thing**: `new Synth("beep", { freq: 440 })` allocates an id and sends the `/synth_new`, so the synth is sounding by the time it returns. The other door is `fromId`, a handle on a node that already exists — an id a responder, a tree query or the GUI reported — which sends nothing.

Neither names a server, because the [ambient session](#sessions-and-the-ambient-verbs) resolves one; `{ server }` in the options bag names another.

A `Group` is **born named** — `new Group({ name: "mixer" })`, and `group.rename(...)` afterwards — a referenceable label on top of the node id: the id stays the identity every command uses, and the name is how you *refer* to the group instead of to a number, comes back in every node record, and makes the tree navigable by path (`server.groupAt("/mixer/drums")`). That is what lets a mixer's channels, its sends and its master be built out of groups and still be sayable.

## Sessions and the ambient verbs

A **`Session`** is a server, a clock and (if you ask for one) a GUI host, bundled into the handle a piece is written against:

```js
const s = await Session.page({ tempo: 2.0 });       // this tab's engine
const s = await Session.connect(url);               // a `clausters --ws` server
```

That one call opens the connection, opens the `Server`, builds a clock at that tempo and anchors it to the server's own sample counter — the four lines a page used to write by hand. It is also the unit of **isolation**: its own random root (`s.seed(1)` reproduces this session's material and no other's), and, with `Session.page({ own: true })`, its own engine, so its nodes, buses and buffers share nothing with the rest of the document. Several coexist, which is why this exists at all: since the engines and hosts became instances, an environment is a thing a page has more than one of.

`s.close()` releases what the session owns — its GUI host, its server client, its clock, and an engine it opened for itself. The page's shared engine is not a session's to stop.

### The default session

Beside the named ones there is the **default session**, `defaultSession`: the ambient environment everything falls back to when no session was named. Lend a server to it and the ambient verbs work with nothing wired:

```js
import { Session, Synth, play, Event, seq } from "clausters";

const s = (await Session.page()).adoptDefault();

play(new Event({ degree: 0, dur: 0.5 }));   // a note, now
play(new seq.Pbind({ degree: new seq.Pseq([0, 2, 4]), dur: 0.5 }));
new Synth("beep", { freq: 440 });           // and the bare constructor too
```

`play` dispatches by kind — an `Event` or a plain object of event keys, an event pattern, a `Routine` or a bare generator, a def or a bare expression, a `Timeline`, a `Buffer`, an `Automation` — and returns something that knows how to end what just started. `examples/verbs.html` visits every kind. Its two siblings take the same material and do something else with it: `plot` draws it and `render` evaluates it offline, both covered in [The ambient verbs](verbs.md).

Resolution is one ladder, and it is worth knowing because it is what makes several sessions safe on one page:

- **the server**: the one you passed, else the session of the routine running right now (a clock names its session), else the session active in a `use()` block, else the default session's;
- **the clock**: the one you passed, else the clock driving the running routine, else the active session's, else the default session's — created at tempo 1.0 and started on first use, never at import.

`adoptDefault()` lends the session's **server** and not its clock, which is deliberate: a stopped clock lent to `play()` would never fire. To put a piece on a *named* session's clock, use `s.play(pattern)` or write inside `s.use(...)`:

```js
b.use(() => new Synth("beep"));   // reaches b, whatever the page's default is
```

`use` is synchronous by design: an `await` inside it would let another task run while this session is ambient, and the page's one thread has no way to scope that. Await outside, create inside.

### The GUI leg

`s.gui()` opens the session's host once and wires it to **this session's** engine, so a bound widget reaches this server and not the page's — the browser parallel of the Python client's `session.gui()`, which boots a `clausters-gui` process pointed at its session's server. `s.connectGui(url)` drives a native `--ws` host instead.

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
  gui.meter(level.index, { rate: "control", name: "level", label: "level" }),
));
win.widget("freq").bind("/node_set", note.id, "freq");
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
- **The timebase** decides what the clock measures sleeps against and how emissions are stamped. `MonotonicTimebase` (the default) paces on `performance.now()` and sends NTP-timetagged bundles; `SampleTimebase` paces on the server's own sample counter and emits `/sched_at <absolute sample>`, which removes the drift between two clocks entirely. `server.sampleTimebase()` builds one — the `Server` anchors it because the `Server` is what knows the carrier, and **the clock itself never talks to a server**.

[Routines and clocks](routines-and-clocks.md) is this layer written out, from a melody by hand to the patterns above it.

Above that sit `Event` and `rest`, the value patterns (`Pseq`, `Pser`, `Prand`, `Pwhite`, `Pseries`, `Pgeom`, `Pfunc`, `Pn`, `Pconst`), `Pbind`, and the seekable counterpart: `Timeline`, a static beat-sorted list that `Timeline.fromPattern` can bounce a pattern into, played by a `Playhead` with play/stop/locate/loop. Random values come from the stream a routine derives when it is created, so `seed(n)` replays a whole piece. The model itself — what an event's keys mean, how `dur` and `sustain` differ, what `Pbind` does with a pattern of patterns — is the Python book's [routines and clocks](https://clausters-python.readthedocs.io/) chapters; it is the same model.

## Three things the browser changes

**Everything that waits is a promise.** Where the reference client blocks a thread on a reply, this one `await`s: `await server.sync()`, `await bus.get()`, `await server.queryTree()`. The page has a single thread and has to keep running, so "never block the clock thread" — a discipline in the Python client — is here simply the language.

**The graph composes by method.** `sine(freq).mul(amp).add(bias)` where Python writes `sine(freq) * amp + bias`, TypeScript having no operator overloading. Because of that, parity between the clients is asserted on the **emitted spec**, never on the source text.

**Nothing pumps.** There is no drain call and no event loop of ours: a page subscribes once (`onEvent`, `onClosed`, an [`OscFunc`](responders.md) or the raw `onReply` under it) and the host's or server's messages arrive as calls, while a query resolves a promise.

## Errors

`ClaustersError` is the root: `CommandError` for a `/fail` from the server, `ReplyTimeout` for a reply that never came, `AllocationError` for an exhausted or double-released id pool. They are ordinary exceptions from ordinary async calls.
