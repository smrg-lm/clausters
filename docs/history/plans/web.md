# Plan - Clausters web client (TypeScript, browser-first): the closed tracks

The tracks of [`clients/web/PLAN.md`](../../../clients/web/PLAN.md) whose every milestone shipped, moved here
whole on 2026-11-12 so that the plan holds what is open. They are the record
of what each track decided and built; a track is not reopened here, and
work that changes what one built is written in the plan.

## Milestones

Labels (`Wx`) live only here, never in published docs or docstrings - the same rule as the other plans.

**`play` is a checklist every milestone has to tick.** The free verb
(`src/play.ts`) enumerates the kinds it knows, so it is the one surface that
goes stale *silently*: nothing fails to compile when a milestone lands a new
playable and the verb has never heard of it, and a kind the verb refuses by
name goes on refusing long after the milestone that would open it has shipped.
A milestone that opens a playable closes by walking the whole enumeration, not
just the dispatch:

- `src/play.ts` — the `Playable` union, the dispatch chain, the header's list
  of kinds, the `TypeError` that names what it expected, and the paragraph
  naming what this client cannot play yet, which shrinks by exactly the kind
  that just landed (with its own refusal, if it had one).
- `docs/src/guide.md` — the one sentence in "Sessions and the ambient verbs"
  that lists the kinds.
- `examples/basics/verbs.html` — the tour that visits every kind, and its closing list
  of the ones it cannot visit yet.
- `tests/session.html` — the sweep that asserts every kind `play` dispatches is
  **audible**; its verdict names them, so a kind missing there is visible in
  the test output rather than only in the source.

W11 is what made this a written rule: the automation landed and four of those
five still said the client had no lane.

**What a milestone defers becomes a later milestone, which names the milestone
it was deferred from.** So a "not in scope" line is always a forward reference
to a numbered slot, never a loose note, and every slot past W5 carries its own
back-reference instead of being grouped under one. Those later slots have **no
fixed sequential order** - each is independent and gets tackled when it is
wanted, the same convention the client track uses past C10 - and each *widens a
layer that already exists* rather than opening a new one, which is why none of
them blocks W5: the client is shippable without them, just narrower than the
Python one.

### ✅ W0 - Toolchain + OSC over both carriers

The smallest round trip, and the toolchain. **Rewritten 2026-07-18**: the original W0 predated the server's B track — it assumed the package had to be scaffolded from scratch, a bundler was table stakes, and WebSocket was the browser's only carrier. B4 changed all three: the `clausters` package exists (singletons, bundle boot, elements), the no-bundler shape is settled (see Tooling above), and the in-page engine is a second, process-free carrier.

- **Adopt the B4 package — and consolidate the web front-end into it**: migrate the B4 modules to typed sources under `src/` (the Python-client mirror; the runtime surface — `server()`, `guiHost()`, `bootBundle`, the elements — stays identical), emitted to `dist/`; absorb every browser JS/HTML artifact the B track left beside the crates (the worklet/loader runtime as `src/engine/`, the harness/standalone/parity pages as `examples/`/`tests/`, the bundle tooling as `tools/`), leaving the crates Rust-only with `build.sh` the one builder/stager; and document the result: an extended `clients/web/README.md` and a new `clients/web/BUILD.md` (the node recipe, stage/build/type-check/test/serve — the tooling made explicit, the way the root `BUILD.md` documents the server's). Rationale in `docs/decisions.md` ("The web front-end lives in one package").
- **The core wasm bundle**: a thin wasm-bindgen shell over `clausters-core` (a sibling of `crates/clausters-web`, staged as `dist/core/` by `build.sh`) exposing OSC encode/decode first — numerically identical to the server and the Python client by construction. It replaces the interim hand-written page codec (declared temporary since B2, deleted).
- **`base/osc.ts`** over that core, with encode/decode **parity vectors** shared with the Python client, in `node --test`.
- **`base/connection.ts`** — the carrier seam, two implementations behind one interface: `WsConnection` (a browser `WebSocket` to a `--ws` server — the remote/native-server carrier, the TS sibling of `examples/ws_ping`) and `pageConnection()` (the in-page engine, wrapping the B4 `server()` singleton's `send`/`addReply`). Everything W1+ builds sits above this seam and never names a carrier.

**Acceptance:** dual — a `/server_status` round trip through the *same* connection interface over **both** carriers (in-page under headless Chrome with no server process; WebSocket against a native `--ws` server), the parity vectors green under `node --test`, and the package type-checking clean (`tsc`).

### ✅ W1 - Server client + the def model

*(The hold this milestone carried from 2026-07-18 — "waiting for the Python
client review", since W1 is the first milestone to mirror the Python **API
surface** rather than only the wire — was lifted on 2026-07-26: the reference
client's arc had settled, so the mirror could start without turning every
Python change into two.)*

Drive the audio server.

- `defs/server.ts`: the `Server` object - send `/def_send`/`/def_send faust` specs, `/synth_new`, `/node_set`/`/node_free`, groups, the `/server_sync` barrier, buses and buffers; receive replies through `responders` (W8 hardened this).
- The def builders (`signals`/`ugens`/`synthdef`/`faustdef`/`graphdef`): start by sending the **same spec JSON the Python builders emit** (reused verbatim), then grow the typed TS builder API for parity, with the Python builders (both def families) as the reference.

**Acceptance:** from a browser page, define a def and play it (`/synth_new` then `/node_set`), with `/server_sync` ordering and an audible/queryable result, **over either carrier** through the same `Server` (the W0 seam: nothing above it names a transport) — a synth def against the in-page engine with no server process, and both families against a `--ws` server (the Faust half is WS-only by nature: the wasm engine is the `synth,embed` build, no LLVM JIT).

**What shipped.** The whole `src/defs/` tree, mirroring `clausters/defs/`
module for module: `server.ts` (reply dispatch, the `/server_sync` barrier, the three
def commands, nodes/groups/graph instances, buses, buffers, the introspection
queries), `node.ts`/`bus.ts`/`buffer.ts` (the handles and their allocators),
`ugens.ts` + `synthdef.ts` (the UGen graph and its spec walk), `signals.ts` +
`faustdef.ts` (the Faust signal API and the three payload forms) and
`graphdef.ts`. Three things are worth carrying forward:

- **Everything that waits is a promise.** Where the Python client blocks a
  thread on a reply, this one `await`s — the browser has one thread, and the
  page has to keep running. The "never block in a routine" discipline of the
  reference client is here simply the language.
- **The allocators come from the core.** `crates/clausters-core-web` grew the
  registry surface (`Registry`, `node_id_partition`, `graph_bus_reserved`) —
  the wasm sibling of the C door `clausters-ffi` opens for Python — so node
  ids, buses and buffers are allocated by the same occupancy map the server
  and the Python client use, not by a second implementation. `Server.open`
  sizes them from `/server_query`, so the client matches the server that is
  actually running.
- **The graph composes by method** (`sine(freq).mul(amp)`), TypeScript having
  no operator overloading, and parity is therefore asserted on the **emitted
  spec** rather than on the source: `tests/def-parity.test.ts` rebuilds each
  reference graph independently and compares the JSON against vectors frozen
  from the Python builders (`tests/gen-def-vectors.py`). Rationale in
  `docs/decisions.md` ("The TypeScript graph composes by method").

Not in scope here, by the plan's own division, each now its own milestone: the
exhaustive UGen catalogue (**W6** — the set the acceptance and the examples
exercise is in: sources, filters, delays, panning, envelopes, triggers, bus and
buffer I/O, the demand pair, the full operator tables), the two Faust surfaces,
the box algebra and the rest of the signal API (**W7**), the reply handling
this milestone grew ad hoc inside `Server` (**W8**), and the bulk/streaming
data paths (**W10**).

**Verified:** `./test.sh` — 29 `node --test` cases (16 def-parity vectors, the
9 end-to-end against a real `clausters --ws` server covering both def families,
plus the W0 codec and carrier suites) and the two headless-Chrome acceptances,
`tests/client.html` (the carrier seam) and `tests/defs.html` (a def built,
sent, played — asserted **audible** on an analyser — read back out of the node
tree and freed, over the in-page engine with no server process). Example:
`examples/basics/synth.html`, the same def and the same code over either carrier, the
choice being the one line that names one.

### ✅ W2 - GUI host driver (`GuiHost` + GuiDef builders)

The product driver the GUI track (G13) deferred here - this closes the loop with G11-G16.

- `gui/guidef.ts`: the GuiDef builders (`window`/`panel`/`knob`/`slider`/`number`/`toggle`/`menu`/`waveform`/`meter`/`scope`/`canvas`/...), mirroring `clausters.gui.guidef`, emitting the same JSON.
- `gui/host.ts`: a `GuiHost` mirroring `clausters.gui.host`, grown over the pieces that already exist — in-page it wraps the `guiHost()` singleton (B4), whose `GuiBridge` (`def`/`feed`/`poll`, B3's `connect_page`/`server_reply` leg already wired to the engine singleton) is the binding surface; the remote leg drives a `--ws` host: `/gui_def`/`/gui_set`/`/gui_free`/`/gui_bind` out, `/gui_event`/`/gui_closed` in.

**Acceptance:** a TS app builds a panel GuiDef and drives the browser GUI host with it; interactions return as `/gui_event`; a `bind`-ed widget drives the audio server with no round-trip through the script, over either carrier (in-page, the bind→engine leg B3/B4 already exercise; and against a `--ws` server) - the same examples the Python client runs against the native host, now in the browser.

**What shipped.** The whole `src/gui/` tree, mirroring `clausters/gui/` module
for module: `guidef.ts` (the full widget catalogue — containers, the light
controls, the heavy `waveform`/`spectrogram`/`plot`, the bus- and tap-fed
`meter`/`scope`/`phasescope`/`spectrum`, `nodetree`, the `bpf`/`pianoroll`/
`piano`/`track`/`clip`/`score`/`patch`/`canvas` editors, plus `toJson`,
`samplesToBlob` and the `Env`↔break-point pair), `host.ts` (`GuiHost` over the
W0 carrier seam, beside the `guiHost()` page singleton and the
`pageGuiConnection()` that wraps its bridge), `ids.ts` (the core-backed widget-id
allocator) and `handle.ts` (the name-addressed widget/window handles). Three
things are worth carrying forward:

- **The GUI reuses the audio client's seam whole.** A `GuiHost` is a
  `Connection` and a name; `GuiHost.page()` drives the wasm host on this page's
  canvas (through `GuiBridge.feed`/`poll`) and `GuiHost.connect(url)` a native
  `clausters-gui --ws` — the two carriers the browser has, behind the interface
  W0 already defined. Nothing above it names one.
- **Nothing pumps.** Where the Python client drains the host from the script's
  loop, this client subscribes once and the host's events arrive as calls:
  `win.widget("cutoff").onEvent(fn)`, `win.onClosed(fn)`, and `query` a promise.
- **The options are TypeScript's, the props are the wire's** (`textSize` →
  `text_size`), and parity is asserted on the **emitted document** —
  `tests/gui-parity.test.ts` rebuilds each reference tree independently and
  compares it against vectors frozen from the Python builders
  (`tests/gen-gui-vectors.py`). Rationale in `docs/decisions.md` ("A GuiDef
  from TypeScript").

**Verified:** `./test.sh` — 45 `node --test` cases (13 new GuiDef-parity and
allocator cases, 3 end-to-end against a real `clausters-gui --ws` host covering
define/query/set/bind/redefine/free, plus the W0/W1 suites) and three
headless-Chrome acceptances, the new one being `tests/gui.html`: a panel built
with the builders, opened on the in-page host, then **played with** — the
gestures are synthesized as pointer events on the host's own canvas, so an
unbound slider's move comes back as a `/gui_event` while the bound knob drives
the engine in the same tab (asserted by reading the node's control back), and
closing the window frees the subtree. Example: `examples/panels/host.html`, now
the product client rather than the B-track harness — the bound and the scripted
control paths side by side, a `/bus_stream`-fed meter/scope loop, the linked
waveform + spectrogram, and one button that swaps the in-page host for a native
one.

Not in scope here, by the plan's own division, and now **W10**: the browser
data paths the heavy views feed on (`/bus_stream` decoding client-side,
`fetch`/`/buffer_getRange` bulk, the wasm peak pyramid) and the
`correlation`/`lissajous` analysis exports — the host already reads those paths
itself, so a GuiDef that names a bus, a tap or a URL works today.

### ✅ W3 - Sequencing: clock, routines, events, patterns

The timing layer, transport-agnostic, sc3-modelled.

- `base/clock.ts` + `base/timebase.ts` + `seq/*`: the routine driver over the **wasm TempoClock** queue (arithmetic and timetag/sample-clock conversion in the core; the `function*`/async driver in TS), with both timebases - the monotonic clock and the **Web Audio sample-clock** (`AudioContext.currentTime`) - and events/patterns mirroring `clausters.seq`.
- Keep the C5 lesson: the clock advances by yielding; the monotonic clock only computes sleeps, so relative timing is exact; the clock never talks to the server.

**Acceptance:** a routine schedules events that play with exact relational timing under both timebases, over either carrier (the sample-clock timebase pairs naturally with the in-page engine's `clock()`), matching the Python client's behaviour on the shared vectors.

**What shipped.** Mostly a *Rust* milestone with a thin TypeScript driver on top.
`crates/clausters-core-web` grew the ten doors the layer needs - the beat-ordered
`Scheduler`, the beat/second/sample arithmetic, the bar grid, bundle assembly
with a timetag, `unix_to_ntp`/`unix_to_sample`, the seeded `Rng`, the builtins
and `degree_to_midinote`, and `SampleClockModel` - each a mechanical shell over
`clausters-core`, the same doors `clausters-ffi` already opens for Python. What
is new in TS is only the coroutine driver, the pacing seam, the queue's
id→routine bookkeeping and the composition of events/patterns/timelines: no
time formula and no random value is computed in TypeScript.

Then `base/{stream,clock,timebase,rand,builtins,context}.ts`, `seq/{event,
pattern,eventstream,timeline}.ts`, and the `Server`'s timed-send path
(`sendBundle`, `sendBundleAfter`, `playEvent`, `sampleTimebase`). Four things
are worth carrying forward:

- **The driver stays on the page; only the wake-up moves to a worker.** A
  routine is a closure over the script's own objects and cannot cross to a
  Worker, so the Python client's background *thread* has no direct port. What
  does port is the property that thread buys: a wake-up the page cannot starve.
  Hence the `Ticker` seam - a shared tick worker in the browser, `setTimeout`
  elsewhere - and, with `Timebase`, the pair of seams that let `node --test`
  drive the real driver by hand, deterministically.
- **The Server anchors the timebase; the clock never talks to a server.**
  `server.sampleTimebase()` resolves by carrier: in-page it pairs the engine's
  counter with the AudioContext's frame counter in one worklet round trip
  (exact - they are the same clock), over a socket it feeds `/clock_query` anchors
  into the core's model. This is the inversion of the Python client's
  `clock.lock_to(server)`, which contradicts that client's own C5 rule.
- **Two Python-client bugs surfaced while porting**, and were fixed in *both*
  clients: `set_tempo` read the pinned instant *after* moving the base beat, so
  a tempo change jumped the timeline (beat 8 went from 4.0 s to 0.0 s); and
  `stop`/`start` restarted the beat axis at zero while the queue kept absolute
  beats, stranding whatever was queued. Both clocks now pin the instant and
  hold the beat across a stop — and a stop keeps *both* origins, the pacing one
  and the wall-clock one, moving them together on resume, which is what keeps
  the first timetag after a restart honest (that third one was only in the TS
  clock, and the port is what exposed it).
- **`/group_queryTree` is not an observation of a schedule.** The reply comes from
  the network-side mirror, which applies each message as it is translated, and
  a note's `/synth_new` and its release are sent in the same instant (only their
  timetags differ) - so the mirror shows a scheduled note born and freed at
  once while the engine still has it sounding. The end-to-end suites read
  `/node_start`/`/node_end` instead.

**Verified:** `./test.sh` - 102 `node --test` cases (the clock/RNG/builtin
parity vectors frozen from the Python client, the driver on manual seams, the
emitted bundle bytes under both timebases, the patterns and the timeline, and
two WS cases against a real `clausters --ws`) and four headless-Chrome
acceptances, the new one being `tests/seq.html`: a pattern on the in-page
engine's own sample clock, its notes' starts and ends read back off the
server's notifications. Example: `examples/transport/sequencing.html` - the generative
half and the seekable half side by side.

Not in scope, by the plan's own division, each now its own milestone:
`automation` (**W11** — a break-point control curve; it pulls in buffers, `Env`
and a control def), `MidiEvent` and MIDI destinations (**W9**, done), the shared
`/transport_set` grid (**W12**), and an NRT/score drive (**W13** — the client has
no score interface, and `Timeline.fromPattern` bounces by driving the ordinary
clock through its manual seams).

### ✅ W4 - Components: the host's canvases in the document

*(What this slot used to name — the responders, MIDI, and the browser data
paths — moved to the milestones after W5.)*

On the desktop, `clausters-gui` opens one window per `window`-rooted GuiDef and
the system's window manager places them. In a tab the drawing surface is a
`<canvas>` in an HTML document, and **the document does the placing** — CSS,
the order of the markup, the flow of the page. That substitution is the
milestone: the desktop working arrangement transposed onto a document, so
canvases interleave with prose and images and one page can be an interactive
text with the instrument sounding beside the paragraph that explains it, or an
editing program whose panels are laid out like everything else on the page.

Running one is the browser equivalent of `clausters-gui --standalone`: the host
is the server's client and **no TypeScript client is loaded at run time**. The
builders run earlier, in the authoring script, and what the page fetches is
data.

- **The host, from one canvas to N** (`crates/clausters-gui/src/host/web.rs`): `window`/`render`/`current_def` are singular today ("the browser shows one at a time"); they become a map keyed by def id — a wgpu surface, a size, a gesture state and a visibility flag each. The native front already keeps one surface per `window`-rooted GuiDef, so the model is ported, not invented. Two reversals ride along: the **element supplies the canvas** (winit's `with_canvas`, instead of `guiHost()` hunting for the one winit appended to `<body>`), and its size comes from the element (`ResizeObserver` + `devicePixelRatio`). A canvas out of the viewport is skipped on the tick and drops its buses from the `/bus_stream`/`/bus_tapStream` sets — a document can hold fifty canvases with three in view, and the browser's own compositing skip does not stop *our* host from computing or the server from streaming.
- **The bundle grows a contract** (`clausters_core::bundle`, opened to the browser by `clausters-core-web` and to Python by `clausters-ffi`): `bundle.json` gains a symbol table, declared `params` and presets, and becomes a file **both** legs read (absent, the native host keeps listing the directory as today). The GuiDef record becomes a *template* with two kinds of hole — `@symbol`, an id the page allocates, and `$param`, a value the tag supplies — while widget ids stay local `1..N` and are offset by an allocated base. **Holes live only in the GuiDef record**, so def payloads are byte-identical between instances and are sent once; the authoring rule that follows is that a bus or a node reaches a def **as a control**, never as a baked constant (which is exactly why today's `piano_voice`, with its `out_ctl(0.0, env)`, cannot be mounted twice). Resolution is two pure functions — `requirements(manifest)` then `resolve(template, allocation, params)` — with the caller allocating in between, so nothing is added to the `/gui_*` protocol and no state to the host.
- **The component** (`src/elements.ts`, `src/runtime.ts`): a custom element owning its canvas, with the declared parameters as attributes and `preset` beside them, mounted in two phases — the GuiDef opens and draws on connect, and the engine half (defs, buffers, boot) goes out on the first page gesture, since the AudioContext is page-wide and N power buttons would be wrong. Failures stay per component. A new **slim run-time entry**, `dist/runtime.js`, carries the engine, the host, the codec and the mount and *not* the builders — today's `examples/panels/piano/index.html` imports the whole package facade to use none of it.
- **The authoring API** (`clausters.bundle`, Python first, Node after): a writer over the existing builders that holds the symbol table, so the author names things instead of numbering them, declares `params` and presets, validates through the core before emitting — an unmountable bundle is unwritable — and generates the five-line ES module that registers the tag.

**Acceptance:** a page interleaving prose with three components — two instances of one bundle and one of another, all authored from Python and mounted with no client library loaded — draws three canvases; the two instances of the same bundle hold different buses and node ids while their def was sent once; a `freq` attribute makes one audibly different from its sibling (asserted on a control bus); a component scrolled out of view stops streaming; one component's failure leaves the rest of the page up; and the same bundles still run on the desktop, `--standalone` and loopback.

**What shipped.** The substitution, and the format that makes it repeatable.

The **browser host went from one canvas to N** (`crates/clausters-gui/src/host/web.rs`):
`window`/`render`/`current_def` — all singular, with the comment "the browser
shows one at a time" — became a map keyed by def id, each entry a wgpu surface,
a size, a pointer, a gesture state, the scope/tap/spectrum histories and a
visibility flag. The native front already keeps one surface per window-rooted
def, so the model was ported, down to the `by_winit` index every per-canvas
event routes through. With it, two reversals: the **element supplies the
canvas** (`attach(def_id, canvas)`, winit's `with_canvas`) instead of the page
grabbing whichever one winit appended to `<body>`, and the **size comes from the
element** (`ResizeObserver` × `devicePixelRatio`), so the host never reads the
DOM. `set_visible` is the one that pays: the `/bus_stream`/`/bus_tapStream` sets and
the animation tick are all derived from the *visible* canvases
(`host::live::demand`, natively tested), so a component scrolled out of the
viewport stops costing a computed frame, wire traffic and server CPU.

The **bundle grew a contract** (`clausters_core::bundle`, opened to the browser
by `clausters-core-web` and to Python by `clausters-ffi`, `CORE_ABI_VERSION` 13):
the GuiDef record is a template with two kinds of hole, resolved in two pure
steps with the caller's allocation in between. Holes live only in that record,
so def payloads stay byte-identical between instances and are sent once — and
`check_def_payload` enforces the authoring rule that follows. Both legs read it:
`--standalone` mounts through the same resolver, and a bundle written before the
contract mounts verbatim.

The **component** (`src/elements.ts`, `src/bundle.ts`, `src/base/pool.ts`) owns
its canvas, takes the declared parameters as attributes, and mounts in two
phases — the GuiDef on connect, the engine half on the page's first gesture. A
new **slim run-time entry** (`dist/runtime.js`) carries the engine, the host, the
codec and the mount and *not* the builders, which needed the page's own host
split out of `gui/host.ts` into `gui/page.ts`.

The **writer** is `clausters.bundle.Bundle`: it holds the symbol table, prefixes
def names with the bundle's, and validates through the core before emitting — an
unmountable bundle is unwritable.

Four things the acceptance found, each a real defect rather than a test
adjustment: the id pools are built on the core's `Registry`, so the core has to
be loaded before them; a pre-contract bundle's id block must be measured from
its template, or two instances overlap; a component's phase 2 has two callers,
so the second must *await* the first rather than return early; and the def
dedup has to be a promise, not a flag, or a sibling boots before the payload is
on its way. Plus one browser behaviour worth knowing: winit focuses a canvas it
creates and the browser scrolls to it, so a document's last component yanked the
reader to the bottom (`with_active(false)`).

**Verified:** `./test.sh` — 111 `node --test` cases (including the module-graph
test that holds the slim entry, and the bundle parity vector: what the Python
writer emits, resolved through the browser's wasm door) and five headless-Chrome
acceptances, the new one being `tests/components.html` — two instances of one
bundle plus one of another interleaved with prose: three canvases drawn, distinct
nodes and buses with one def sent, `freq="110"` resolved on the second, the
off-screen component streaming nothing, and the broken one failing alone.
Examples: `examples/panels/piano/` and `examples/panels/graph-controls/` ported to the writer,
and `examples/editors/document/` — the interactive text the milestone is for.

Not in scope, deliberately, each now its own milestone: window *management*
(**W14** — an element removed from the DOM does not free its def; a
`/gui_closed` travelling back is a separate feature) and a TypeScript bundle
writer (**W15** — the reference client leads and the port is mechanical, the
repo's standing rule).

### ✅ W5 - Docs, examples, tests, packaging

Make it a real, shippable client.

- An mdBook in `clients/web/docs` (mirroring `clients/python/docs`), with the API reference **generated from TSDoc by typedoc** (the TS counterpart of the Python client's pydoc-markdown), and the manual-testing notes kept current. The client books cross-link by their RTD URLs.
- A **selection** of the Python examples ported to TS, the `node --test` suite, and the npm package build; a parity pass against the Python client on the shared vectors (OSC, clock arithmetic, GuiDef JSON).

**Acceptance:** the workspace build yields a usable, installable client; the ported examples run in a browser over the in-page engine, and the carrier is the one line that says so; the docs build like the Python client's.

**What shipped.** The book, five ports, and the packaging held by a checker.

The **book** is the third mdBook (`clients/web/docs/`, `.readthedocs.yaml`
beside it): an orientation, a getting-started that ends with a tab making a
sound, the client layer by layer, the components chapter, the examples catalog,
and an API reference **generated by TypeDoc** from the sources' TSDoc — which
first meant converting the whole tree from Rust-style `///` to `/** */`, a form
TypeScript tooling actually reads. TypeDoc is a user-space *tool*, not a
dependency, parsing with its own TypeScript 5.9 while the package keeps
compiling with the v7 in `node_modules`; it runs with warnings as errors, which
is what widened the exported type surface. Rationale in `docs/decisions.md`
("The web client's API reference").

The **examples** are five ports, each named after the Python example it mirrors
so the two can be read side by side: `multichannel`, `typed-controls`,
`graph-maths`, `wavetables`, `pause-resume`. They were chosen for running in a
page as they stand — live on the in-page engine, interactively or as a scripted
phrase; the offline-render half of the Python set has no browser counterpart
until there is a score drive.

The **package** is publishable but unpublished, and the gap is covered by a
check rather than by care: `tools/check-package.mjs` (what `prepublishOnly`
runs, and `tests/package.test.ts` too) refuses a `dist/` missing the wasm
bundles `build.sh` stages, a version out of step with the crate's, or an
`exports` entry the `files` list leaves out — plus a read of what `npm pack`
would ship. The procedure itself is `clients/web/BUILD.md`, "Publishing".

**Verified:** `./test.sh` — 114 `node --test` cases (the three new packaging
ones on top of W0-W4's) and the five headless-Chrome acceptances; the book
builds clean (`docs/build.sh`, TypeDoc with zero warnings); each ported example
was driven in a browser and asserted **audible** on an analyser, `pause-resume`
across its three states (0.20 sounding → 0.0 paused → 0.20 resumed).

Not in scope, by the plan's own division, each now its own milestone: the rest
of the Python examples (**W16**), the publication itself — the npm registry and
a Read the Docs project — (**W17**), and the `Session` facade this layout
sketched into this slot (**W18**), which is an API layer rather than a
packaging one and leans on verbs the client does not have yet.

### ✅ W6 - The full UGen catalogue *(done 2026-08-03)*

*Deferred out of W1.* W1 shipped the def model with representatives of each
family; this fills out the UGen-graph one, so a graph written against the
Python client ports by transcription rather than by lookup. The Faust
authoring surfaces are W7's, both of them.

- `defs/ugens/`: the rest of the server's UGen catalogue — sources, filters, delays, panning, envelopes, triggers, bus and buffer I/O, the demand pair, the spectral chain (`fft`/`ifft`/`pv_*`, the client side of S8), the output-less roots (`sendReply`/`sendTrig`/`poll`), and the complete unary/binary operator tables.
- The W1 composition rule is unchanged: TypeScript has no operator overloading, so operators stay methods and parity is asserted on the **emitted spec**, never on the source.

**The gap, measured 2026-08-02** (the list above predates the U track, which
grew the demand family from the "pair" it names to thirteen builders). Python
exposes 151 builders in `defs/ugens.py`, TypeScript 116 in `defs/ugens/`; **40
are missing**, and they are not scattered — they are five whole families, which
is why the difference reads as "the same catalogue minus some entries" and is
not:

- **Demand (13)** — `dseries`, `dgeom`, `dwhite`, `diwhite`, `dbrown`,
  `dibrown`, `dxrand`, `dshuf`, `dbufrd`, `dstutter`, `dswitch1`, `duty`,
  `tduty`. The `dr` rate itself has no TS presence, so this is the family that
  costs most: the rate is part of the def model, not just a list of builders.
- **Spectral (17)** — `fft`, `ifft` and the fifteen `pv_*`
  (`pv_add`/`pv_mul`/`pv_max`/`pv_min`, `pv_mag_above`/`_below`/`_clip`/
  `_mul`/`_shift`/`_smear`/`_freeze`, `pv_bin_shift`, `pv_brick_wall`,
  `pv_copy_phase`, `pv_kernel`). The `fr` rate is in the same position as `dr`.
- **Panning and stereo field (4)** — `pan_az`, `rotate2`, `mid_side`,
  `stereo_width`.
- **Convolution (2)** — `conv`, `partconv_frames`.
- **Disk I/O (2)** — `disk_in`, `disk_out`. These are the one family worth
  questioning before porting: they stream from the *server's* filesystem, so
  the builders port fine but only mean something against a native server, never
  against the in-page wasm engine. Port them with that written next to them.
- **Filters (2)** — `svf`, `svf_morph`.

Since W21 the two catalogues are split by family into the same nine module
names, so **each row above already knows the file it lands in**: `ugens/demand.ts`,
`ugens/pan.ts`, `ugens/io.ts` and `ugens/filter.ts` each gain their missing
entries beside the ones they hold, and `ugens/spectral.ts` — the one module the
split left unwritten, because nothing in it is ported — is created by this
milestone. Convolution lands in `spectral.ts`, where the Python client keeps
`conv`/`partconv_frames`.

Naming note: Python's `oscn` is TS's `oscN` — a spelling difference, not a
missing builder. Both directions also carry helpers with no counterpart
(`ugen_input_names` here; `add`/`sub`/`mul`/`div`/`resolveCurve` there, which
are the operator methods the composition rule calls for); neither is a gap.

**Acceptance:** every UGen builder the Python client exposes has a TS counterpart emitting the same spec JSON, checked by extending the frozen vectors (`tests/gen-def-vectors.py`); a graph transcribed from a Python example plays over either carrier.

**What shipped.** The forty builders, in the five modules the gap named, plus
the module the gap did not: `defs/pv_expr.ts`, the per-bin expression language
`pvKernel` takes, which is a *surface* rather than a list of entries. The
catalogues now match name for name — a sweep of both trees reports nothing
missing in either direction beyond the differences this plan already records
(the free `add`/`sub`/`mul`/`div` and `resolveCurve` here, `ugen_input_names`
there). Three things are worth carrying forward:

- **`dr` was already in the def model; `fr` never needed to be.** The gap read
  the two rates as the family's real cost, and only half of that was true. The
  demand rate was there since W1 (`dseq` set it), so the thirteen new sources
  are ordinary builders. The spectral chain sets **no** rate at all: `fft` and
  the `pv*` filters carry the frame at control rate and `ifft` produces audio,
  which the server's own registry decides — so `fr` has no client presence in
  *either* client, and the plan's symmetry between the two rates was wrong.
- **The expression language reuses the graph's math surface** instead of
  copying it. `SynthExpr` gained an operand type parameter, so `PvExpr` extends
  the same class and inherits the whole vocabulary — seventy-odd methods
  carrying the wire's own operator names — while composing its own tree. That
  is what keeps a per-bin `mag.ge(param(0))` and a graph's `sig.ge(x)` the same
  operator by construction, and it is the first step of the `base/absobject.ts`
  the parity section has wanted since W21.
- **A demand stream cannot be shared, and that is a fact about the model, not
  a client rule.** A stream is *pulled*, so two drivers reading one `dseq` node
  take alternate items from it and each hears half the pattern. The builders
  cannot prevent it (the node is a perfectly good input twice over), so it is
  written where it is met: the book's def chapter, and `examples/basics/demand.html`,
  whose duration stream is a factory for exactly this reason.

`conv` and `partconvFrames` land in `spectral.ts` beside the chain, where the
Python client keeps them, and `partconvFrames` — the one plain-number helper in
the catalogue — is frozen as **values** in the vectors rather than as a spec.
`diskIn`/`diskOut` ported with the warning the gap asked for written next to
them: they stream the *server's* filesystem, so they only mean something
against a native server, never against the in-page engine.

**Verified:** `./build.sh && ./test.sh` — 232 `node --test` cases (10 new
SynthDef-parity vectors covering every new family, and the scalar one) and
sixteen headless-Chrome acceptances, the new one being `tests/catalogue.html`:
what a spec vector cannot show, which is that the graph it describes does what
its family says. The brick wall cuts 30 dB above 8 kHz while the passband
survives, a `duty` pulling two `dseq`s visits both its pitches with nothing
sent after the synth starts, the `midSide` round trip returns each tone to its
own channel while `stereoWidth(…, 0)` folds both onto both, and `svfMorph`
sweeps the response from lowpass to highpass on one control. Examples:
`examples/spectral/chain.html`, the port of `chain.py` with the wipe live under
the hand instead of rendered offline, and `examples/basics/demand.html`, the
sequence that lives in the def. Book: the def chapter now names the three
families that are not read like the rest.

### ✅ W7 - The Faust surfaces: the box algebra, then the signal API *(done 2026-08-27)*

*Deferred out of W1.* The two Faust def-authoring surfaces, together because
they are one family — `clausters.defs.boxes` (C22) and `clausters.defs.signals`
— and in that order: the **box API first**, it being the richer and more
important of the two (Faust's own algebra, and the surface a whole `.dsp`
source folds into). And with them the piece the rest of the track never needed:
**a Faust compiler in the page**, so the Faust family stops being the one thing
a browser can author but not run.

- ✅ `defs/boxes.ts`: the point-free algebra (`seq`/`par`/`split`/`merge`/`rec`, `wire`/`cut`, controls, tables) emitting the same box-tree JSON the Python builders emit, plus `faust(src, ...)` to fold a Faust source expression — its libraries (`fi.`/`os.`/`re.`/`pm.`) included — into a composable `Box`. **Done 2026-08-27**, with `defs/expr.ts` (`FaustExpr`) under it and the `asDef`/`fromBox` coercions closed; see "What shipped" below.
- ~~`defs/signals.ts`: the sample-level signal API filled out to the whole surface `clausters.defs.signals` exposes, the same emitted-spec parity rule W6 states.~~ **Already done** — it landed with W1 and has been at parity since: the same ops, the same `select2`/`select3`, `delay`, `fconst`/`fvar`/`sr`, the fifteen unary and six binary functions, `PI`/`TAU`, the five controls and the three tables, with frozen vectors in `tests/def-vectors.json` (`gen-def-vectors.py`, `faust_cases`). What is left of the TypeScript half is `boxes.ts` alone.
- **The browser Faust toolchain** — the new build and packaging leg, below. Against a `--ws` server the two builders work without it (they emit JSON and the native server compiles it, the `faust(src, …)` escape hatch included — it ships its generated program as a `{"op": "faust", "src": …}` node rather than parsing Faust on the client); the in-page carrier is what needs it, and having *one* def family that only runs over WS is the asymmetry this milestone closes.

**The build and packaging step (new to this plan, and the reason W7 is not just
TypeScript).** Nothing in the track so far compiles anything but Rust and TS:
the toolchain is `tsc` with no bundler, and the wasm artifacts are the core and
host bundles `build.sh` stages. Faust in the page adds a second compiler
toolchain, in two halves — **only the first is packaging, and the second is the
one that decides whether the milestone lands**:

- ✅ **The compiler**, built on 2026-08-25. `libfaust-wasm` — the whole Faust compiler as a wasm library — built from `third_party/faust` at the same pin the native library uses, by `third_party/build-faust-wasm.sh`, producing `libfaust-wasm.{js,wasm,data}`, the `.data` carrying the stdlib for Emscripten's virtual FS, and staged by `build.sh` as static assets **off the slim `dist/runtime.js`**, since a page that mounts a *prebuilt* bundle must not download a compiler it never calls. The script documents five departures from `make wasmlib`, the largest being a patch that binds the Box and Signal APIs: upstream binds them for the native library and not for the wasm one, so without it a page could compile Faust source alone. The **packaging half** outlived this milestone and was closed on 2026-08-27: the release builds it (`.github/actions/wasm-vendor`), the SDK is pinned, and `check-package` refuses a package without it — the record is in "Future directions" below ("Who builds `libfaust-wasm`") and in `docs/decisions.md`.
- ✅ **The engine's side, which a compiler alone does not solve, is `B5`** (root `PLAN.md`, B track), and it landed on 2026-08-25. The in-page engine had no libfaust and **no LLVM JIT**, so it could not instantiate what a compiler in the page produced; now the Faust module is compiled in the **NRT worker** (not the main thread, as this bullet first said — B6 priced that at the GUI host's frames) and **linked into the engine's own memory and function table**, so the node stays a node and nothing on the wire changes (`docs/decisions.md`). All three def forms work: the box and signal trees are read by `faust::boxes` and `faust::signals` compiled to wasm, so there is one reading of the schema and not two. W7 and B5 ship together, the way G25 pairs with M25; **W7's WS half was never blocked on it**.
- **The decision is recorded** (`docs/decisions.md`, "The page's Faust is a second wasm module linked into the engine's own memory"): which instantiation path the engine takes and why the two it rejects are not near misses, why the protocol learns nothing, and the one hazard the wasm backend carries (its JSON data segment at absolute offset 0). What it left open was the packaging half alone — whether CI builds `libfaust-wasm` or fetches it — closed on 2026-08-27 in favour of the release building it, since there is no fetchable build that binds the Box and Signal APIs.

**Acceptance:** the Python box-API and signal-API examples rebuilt in TS emit byte-identical spec JSON (new frozen vectors), and one of each compiles and plays **over either carrier** — against a `clausters --ws` server, and in the page with no server process, the source compiled by the staged `libfaust-wasm` and sounding through the in-page engine; a page that mounts a prebuilt bundle loads none of the compiler's assets.

**W16's `faust/boxes-library` page rides here**, and is part of this milestone's
acceptance rather than of the example pass': it is the natural proof that a
Faust def compiles in a page.
**And it is B5 it waits on, not the compiler** — the script is an offline NRT
render (`Session.nrt` + `session.render`), and a page's `Session.nrt` renders
through the in-page engine, so writing it against `--ws` instead would make it
the same picture by different calls, which is the one thing a pair of examples
may not be. B5's compiler landing did not unblock it and B5's **offline render**
did, on 2026-08-26: a page's render now compiles the score's Faust defs before
it starts and links them into the renderer's own instance, so `Session.nrt` +
`render()` carry a FaustDef in a tab. The page is writable.
The script is `clients/python/examples/faust/boxes_library.py`; the page is
`clients/web/examples/faust/boxes-library.html`, same material, same names, the
same calls in the same order.

**What shipped.** `src/defs/boxes.ts` at its sibling's path and with its
sibling's surface — the algebra (`seq`/`par`/`split`/`merge`/`rec`,
`wire`/`cut`), `faust(src, …)` with its evaluation-stage arguments, the
controls and the groups, the three tables, the structure primitives, the
seventeen unary and five binary functions, `PI`/`TAU` and `sr()` — plus the two
pieces around it the milestone turned out to need.

- **`src/defs/expr.ts`, the roof the two Faust surfaces share.** `FaustExpr` is
  the last name the 2026-08-25 parity sweep listed as this milestone's
  (`clausters/defs/expr.py`), and porting it is what makes `Box`'s operators
  *the same operators*: it sits under `AbstractObject`, so both `Signal` and
  `Box` answer the whole vocabulary and each maps a selector through its own
  table (the box schema has no `lsh`/`rsh`, and `mod` is `rem` for a signal and
  `fmod` for a box) — an op a family has no node for throws where it is
  written, exactly as it does in Python. `Signal` moved onto it in the same
  pass, which *added* methods rather than removing any. What the branch adds
  over `AbstractObject` is `rsub`/`rdiv`: Python writes `1 - sig` and gets
  `__rsub__`, and a language with no operator overloading needs the case
  spelled out.
- **The coercion, which was the milestone's own loose end.** `asDef` now takes
  a `Box` (W13 left the leg empty saying so), `exprChannels` answers the box's
  own arity, and `FaustDef.fromBox` accepts a `Box` and runs `checkWires` on
  it — the one silent mistake the algebra allows, a `wire()`/`cut()` *object*
  reused in two positions. The raw-JSON form stays for a generated tree.

**Two spellings differ from the reference client, and nothing else does.**
Applying a box is `f.call(a, b)` where Python calls the box itself
(`__call__`), and selecting one output is `st.get(0)` where Python writes
`st[0]`: a class instance in TypeScript is neither callable nor indexable. Two
more are the options bag this package already uses everywhere — `faust`'s
keyword arguments (`{ ins, outs, defs }`) travel as a trailing object, and
`outs()` answers an array rather than a tuple. The emitted tree is the same
tree.

**Verified:** `./build.sh && ./test.sh` — 469 `node --test` cases (5 new in
`def-parity.test.ts`: three frozen box vectors regenerated from the Python
builders by `gen-def-vectors.py`'s new `box_cases` — the library instrument
with its channel selection, the algebra over bare wires, and the
table/control/`sr` corner with a reversed operand — plus the wire-reuse
rejection and the arity rules) and the headless-Chrome acceptances, where
`tests/faust.html` now **builds its box tree with the box API** instead of
spelling the JSON: source, boxes and signals all compiled in the page and
sounded (boxes 0.40 at ~305 Hz), and an offline render carried all three.
Example: `examples/faust/boxes-library.html`, the port of
`clients/python/examples/faust/boxes_library.py` — read side by side, verb by
verb, and run: both render 144000 frames (3.00 s) at peak 0.686. Book: the def
section of "The client, layer by layer" now carries the box algebra, the
`api/` reference gained the `boxes` namespace (and its `signals` entry stopped
pointing at a page mdBook was inventing), and the two claims that B5 had
already made false — Faust needing a native server — are gone from the guide
and the introduction.

### ✅ W8 - Responders: `OscFunc` over the reply stream *(done 2026-08-03)*

*Deferred out of W1*, which grew its reply handling ad hoc inside `Server`. The
client's input path and its role as a general OSC hub (sclang's `OSCFunc`),
mirroring the half of `responders.py` that does not involve MIDI.

- `responders.ts`: pattern-matched dispatch over the connection's reply stream — either carrier exposes it through the W0 seam (`addReply`), so nothing here names a transport — with handlers scheduled on a clock rather than run from the socket callback, the browser counterpart of the Python client's "never block the clock thread".
- The reply handling W1 grew ad hoc inside `Server` (the dispatch table and the `/server_sync` barrier) folds onto this one door, so everything arriving comes in the same way.

**Acceptance:** a TS app registers and unregisters `OscFunc` handlers that fire on server notifications (`/node_start`/`/node_end`, `/done`, `/node_trigger`) over either carrier, and the W1/W3 end-to-end suites stay green through the new door.

**W16's two MIDI pages ride here**, for the same reason and on the same terms:
`io/midi-responder` and `editors/pianoroll-midi` cannot be written until this
milestone gives a page MIDI at all, and writing them is how it is shown to
work.

**What shipped.** `src/responders.ts` at its sibling's path and with its
sibling's surface — `OscFunc(func, path, { src, argTemplate, recv })`, the
`(msg, time, src)` callback with `msg` the reference's `[addr, ...args]` list,
`enable`/`disable`/`free`/`oneShot`, the `oscfunc` builder and the module
default — and `src/base/receiver.ts` under it, the port's one real question.
Three things are worth carrying forward:

- **A receiver wraps a `Connection`, because a page can bind no port.** There,
  a responder registers with a receiver that owns a UDP socket any application
  can target; a tab can be addressed by nobody, so the door is the carrier the
  client already opened. `src` therefore names a carrier (a socket's URL, or
  `page`) rather than a `(host, port)` pair, and the default receiver is the
  **ambient session's server**, resolved per call rather than cached — a page
  can hold two sessions, and each server owns one receiver. Rationale in
  `docs/decisions.md`.
- **`time` needed a new door in the core.** The callback is defined to receive
  the containing bundle's time, and `osc_decode_packet` flattens bundles and
  drops their timetags — right for a reply reader, wrong here. So
  `clausters-core-web` grew `osc_decode_packet_timed`, carrying each message's
  bundle time as Unix seconds (`null` for an immediate bundle or a bare
  message), the rule the reference client's own decoder applies; it is declared
  in `docs/bindings.md` and asserted natively.
- **The client's own reply handling folded onto it**, which is what this slot
  was for: the node ids recycling off `/node_end`, `BusStream` and `TapStream`
  decoding their snapshots, and `Playhead.followTransport` — each had grown its
  own address test inside a raw subscription — are `OscFunc`s on the server's
  receiver now. `Server.onReply` stays under them as the unmatched seam (a
  decoder that wants everything in arrival order is a real caller), and the
  packet is decoded once, at the door.

**Verified:** `./build.sh && ./test.sh` — 221 `node --test` cases (14 new over a
fake carrier: the matching, the template's literal/predicate/hole, the sender
filter, the bundle time, the lifecycle, the one-shot, the builder, a clock-bound
receiver, a handler freeing itself mid-dispatch, and the undecodable packet; 4
new against a real `clausters --ws` server: the node notifications and their
silence once freed, `/done` narrowed by a template, a def's `SendTrig` and
`SendReply` on their own addresses, and the id recycling through the new door)
and fifteen headless-Chrome acceptances, the new one being
`tests/responders.html` — the same matching on the in-page engine, plus a
responder that names no receiver resolving the ambient session's server.
Example: `examples/io/responders.html`, the port of `osc_responder.py` — a def
reports its onsets with `SendReply` and a responder answers each with a synth,
so what arrives is what plays. Book: "Receiving: responders".

Not in scope, by the plan's own division: `MidiFunc` and the MIDI destinations
(**W9**), both directions being one browser API.

### ✅ W9 - MIDI: `MidiFunc` in, `MidiEvent` and MIDI destinations out *(done 2026-08-27)*

*Deferred out of W3*, which left `MidiEvent` and MIDI destinations out of the
sequencing layer. Both directions of MIDI in one milestone, since in the
browser they are one API: Web MIDI is the only MIDI I/O a page has.

- `MidiFunc` over `navigator.requestMIDIAccess`, mirroring `responders.py`/`base/_midiinterface` — note/cc/program dispatch, port selection, handlers scheduled on a clock; convenience responders turn notes into `/synth_new`, as C13 does.
- `MidiEvent` and MIDI as a **destination** of the sequencing layer: an event stream plays to a MIDI output exactly the way it plays to a `Server`, over the `play(destination)` seam W3 established, mapping `Event` → channel-voice messages the way `clausters-midi` already defines them for C11.
- Timing stays **best-effort by design**, as C18 settled for the Python client — but the browser gives it back cheaply: `MIDIOutput.send(data, timestamp)` takes a `performance.now()` deadline, so the driver hands over the deadline it has already computed instead of sleeping to it.

**Acceptance:** a pattern plays to a browser MIDI output on the same beat grid it plays to the audio server, and a `MidiFunc` on an input port drives defs on the server, over either carrier.

**What shipped.** The reference client's MIDI layer, name for name: `parseMidi`,
`MidiScore`, `MidiNrtInterface`, `MidiRtInterface`, `MidiServer` and
`MidiReceiver` in `src/base/midi.ts`; `MidiFunc`, `midifunc`,
`defaultMidiReceiver` and `setDefaultMidiReceiver` beside `OscFunc` in
`responders.ts`; `MidiEvent` beside `OscEvent` on the timeline. A `MidiServer`
is an `EventDestination` and a `PlayDestination`, so the *same* `Pbind` plays to
a MIDI port or to the engine by which object it is handed — which is the
milestone, the rest being spelling.

**The file writers are the core's, not a second implementation.**
`clausters-core-web` grew `midi_write_smf`/`midi_write_clip` over
`clausters-midi` (default features: `live` is midir, which is exactly what a page
cannot have; the writers are pure Rust over midly and compile to wasm unchanged).
So `MidiScore.toSmf` and Python's `to_smf` are one writer reached two ways, and
`tests/midi-vectors.json` freezes the bytes to prove the wasm door is wired to
*that* writer — the failure the shared core exists to prevent. The rows are in
`docs/bindings.md`, in a section of its own: it is the one place whose C
counterpart lives in `clausters-midi` rather than `clausters-ffi`, so the table's
left column is empty and the note names the symbols.

**Web MIDI's one shape difference, and where it is written.** A page cannot
*create* a port — `navigator.requestMIDIAccess()` asks the user once and hands
back what exists — so `port` **selects** here where the reference client's
**opens** a virtual one. Three consequences, each recorded in
`docs/src/platform.md` and in the book's responder page rather than left to be
rediscovered:

- **`requestMidiPorts()` is TS-only**: the grant itself, which the Python client
  has nothing to ask for. Passing its result as `access` reuses one prompt across
  every receiver and output.
- **`MidiRtInterface.open()` is a static async constructor**, because resolving a
  port awaits; the reference client's `__init__` opens on the spot.
- **`MidiServer.write(path)` is Python-only** — a page has no filesystem. Both
  clients have `MidiScore.to_smf`/`toSmf`, which is the byte-level verb the file
  is written from, and a page offers the bytes as a download exactly as
  `wavBytes` does for a take.

Nothing else differs: the ports are named as *shapes* (`MidiOutputPort`,
`MidiInputPort`), which a real `MIDIOutput` satisfies and a test satisfies in
three lines — the same move `Connection` makes, and what lets the acceptance run
with no device.

**Timing.** Best-effort by design, as `C18` settled — but the browser gives some
of it back: `MIDIOutput.send(data, timestamp)` takes a `performance.now()`
deadline, so a note-off two beats out is handed the deadline the driver already
computed instead of being slept to. `performance.timeOrigin` makes the conversion
from the moment's Unix seconds exact rather than sampled.

**Verified:** `./build.sh && ./test.sh` — 17 new `node --test` cases (the parse
against the reference client's answers, the score's stable sort, the SMF and clip
bytes against the frozen vectors, the `Event`→note mapping, the deadline vs the
immediate send, the panic on close, the receiver's port selection and decode, the
responder's three filters, `disable`/`enable`/`free`/`oneShot`, the pinned
default, the `MidiEvent`, and the milestone's own acceptance: one `Pbind` on two
destinations lands its note-ons on one grid and releases each note at its own
sustain) and a new headless acceptance, `tests/midi.html`, which is that
acceptance in a page over the in-page engine — onsets 1/1.5/2/3 on both legs,
four note-offs handed a deadline, a `MidiFunc` playing and freeing two synths on
the engine, and 63 bytes of Standard MIDI File written in the tab.

Examples: `examples/io/midi-responder.html` (the port of `midi_responder.py` —
a keyboard playing the engine) and `examples/editors/pianoroll-midi.html` (the
port of `pianoroll_midi.py` — the keys painted into a roll). Both need a real
device, so they are hand tests; what a machine without one can check was checked,
and both run to the port and report the browser's refusal cleanly. Book:
"Receiving: responders" grew its MIDI half and its playing-to-MIDI half, and
`platform.md` the row above.

### ✅ W10 - The browser data paths: buses, bulk, and the analysis exports

*Deferred out of W2.* The paths the heavy views feed on, read by the **script**
this time. The host
already reads them itself (that is why a GuiDef naming a bus, a tap or a URL
works today); this is the client getting the same numbers.

- Control buses over the connection: `/bus_stream periodMs bus...` subscribed from the client and its periodic `/bus_set` snapshots decoded in a responder (W8's door) — the message-based counterpart of the native host's shared memory (G14). The server side exists on both carriers already: one subscription per client, replaced per call, `periodMs <= 0` cancels, 10 ms floor, ≤128 buses (`docs/schemas.md`), and B3 left `/bus_stream`/`/bus_tapStream`/`/buffer_getRange`/`/clock_query` streaming over the in-page leg too.
- Bulk buffers by `fetch`/`/buffer_getRange` (G15), with the **peak pyramid built in wasm** from the fetched samples, so a waveform draws at screen resolution without a second implementation of the reduction; plus the fetch + `decodeAudioData` → `bLoad` sample path B3 left in `bundle.ts`, folded into the client's buffer API.
- The core's `correlation`/`lissajous` analysis exports surfaced to TS.

**Acceptance:** a TS app reads a control bus and a buffer over either carrier and the figures it reads are, number for number, the ones the GUI host draws from the same source.

> **Amended in place by W26** (2026-08-20). This acceptance read "and draws them
> **itself** (a canvas the script feeds, not a host-fed widget)", and that
> clause was the divergence: it was written when a hand-drawn canvas was the
> only way to prove the numbers arrived — `scope`/`plot` and the host's own
> widgets landed six days later — and it stayed as a rule long after it had
> stopped being one. Everything drawn is drawn by the host. What the milestone
> actually established, and what is still true, is the sentence above: one
> implementation of every figure, reached by whoever needs it.

**What shipped.** The three paths, and the move that makes "numerically
matching" true by construction rather than by care.

The **script reads what the host reads**. `Server` grew the commands
(`streamBuses`, `tap` with a `taps` registry beside the bus and buffer ones,
`streamTaps`, `getSamples` chunked by the frame ceiling the transport
advertises) and `src/data/` the sources over them: `BusStream` decoding the
periodic `/bus_set` snapshots, `TapStream` placing each `/bus_tapStream.reply` window on its
tap's own sample axis by `endPosition`, `Peaks` over the wasm pyramid, and the
measurements a view is drawn with. (The pyramid's pixel-row read went with
**W26**; the cache and its own readers stayed.) The subscriptions rode
`Server.onReply`, and **W8** folded them onto `OscFunc` without changing their
surface, as this slot said it would.

Three things are worth carrying forward:

- **The signal logic moved into the core.** The oscilloscope's trigger
  alignment and the spectrum's decibel curve lived in the GUI host crate,
  correctly, while the host was the only thing computing them; a page drawing
  its own trace made a second consumer, so they became
  `clausters_core::{oscil, spectrum}` and the host now consumes them from
  there. That move was right and stands — the code is in the core, natively
  tested, and the host reads it. What did not stand is the second consumer:
  **W26 removed the JS door onto `oscil` and `spectrum`** (and onto a pixel
  row), because a figure of the *screen* computed in a client is a drawing the
  other client does not have. Rationale in `docs/decisions.md`.
- **The peak cache is byte-identical across clients.** `Peaks.toBytes()` writes
  what the Python client writes and what the GUI host maps — the mono layout
  for one channel, the multichannel one above it — which is what the parity
  vectors assert, one digest covering the whole format.
- **The bulk path is read-only, and not by choice.** The server has no
  buffer-write command: reading a buffer has `/buffer_get`/`/buffer_getRange`,
  and nothing writes one back. So samples reach a buffer through `/buffer_gen`,
  `/buffer_allocRead`, or — in the page, where the carrier shares memory with the
  engine — `Buffer.load`, which fetches and decodes with the browser's own
  decoder and installs through the embed door. Writing from a client is noted
  as **M31** in the server's `PLAN.md`; the order will be the standing one,
  server command → the Python client → the port here.
- **On one page, the host and the script are one client.** Everything reaching
  the in-page engine goes through one shared-memory ring, which the server sees
  as a single `ClientId::Ring`, and `/bus_stream`/`/bus_tapStream` are one
  subscription per client — so a host `meter` and a script `BusStream` take the
  stream from each other, and the host (which only re-subscribes when its own
  widget set changes) stays frozen afterwards. Found by probing after the
  milestone was written, reproduced in `tests/ring-clash-probe.html`, and fixed
  where it belongs: server **M31**, ring clients get identities. Until then the
  book says plainly that a page picks one live reader; over a socket the two
  are ordinary separate clients and nothing collides.

**Verified:** `./test.sh` — 137 `node --test` cases (23 new: the peak cache and
the stereo field against `data-vectors.json` frozen from the Python client, the
trigger and the spectrum's behaviour, the snapshot decoding and the bulk
chunking over a fake carrier, plus four against a real `clausters --ws` server
— a streamed bus, an LFO through it, a tap carrying a synth's samples with its
trace locked, and a buffer read back in chunks) and six headless-Chrome
acceptances, the new one being `tests/data.html`: a def feeds a bus and a tap,
the script subscribes to both and reads a generated buffer, and every figure is
checked as a number (**since W26** — it drew them on a canvas until then).
Examples: `examples/views/meters.html`, the three paths in one page, each under the
host view that reads it (**rebuilt by W26**; it drew its own canvas before);
and `examples/views/editor.html`, the
port of the Python client's `editor.py` — a decoded file in a server
buffer, drawn by the **host** as a linked waveform and spectrogram, with a
transport whose playhead is anchored to the engine clock and whose pause is
`/node_run`. Book chapter: "Reading the server".

### ✅ W11 - Automation: a break-point curve as a control vector *(done 2026-08-03)*

*Deferred out of W3*, because it is the one sequencing piece that is not pure
timing: the TS side of C23 pulls in buffers, `Env` and a control def.

- `seq/automation.ts`: a break-point curve discretized into a control buffer on the server (`/buffer_gen "env"`, the server's own `envshape`) and read back onto a control bus by a lane synth (`OutCtl`), prepared without blocking the driver, then played and freed like any other element.
- The `bpf` builder already in W2's GuiDef catalogue becomes its editor, so the curve is authored, heard and edited over the same loop the multitrack editor uses.
- **`plot(automation)` comes with it** (*named here from W23*, which ports the visual verbs without this kind): an automation's curve *is* an `Env`, so the leg is the `Env` one already written, labelled with the automation's control name — a few lines once the class exists.

**Acceptance:** a curve authored in TS drives a synth's control over either carrier with the same values the Python client produces for the same break points, and dragging it in the browser GUI's `bpf` widget moves the sounding value.

**Removed 2026-09-20.** `seq/automation.ts` is gone with its Python sibling, and the curve it carried is `Bpf` in `defs/ugens` -- see "`seq/automation.ts` goes, and `Bpf` comes" in Found by use, and the entry it references in the Python plan for why. What follows is the record of what this milestone shipped, kept as written.

**What shipped.** `seq/automation.ts`, at its sibling's path and with its
sibling's shape: `LANE_DEF`, `autoLaneDef()`, `addAutomationDef()` and the
`Automation` class (`fromPoints`/`toPoints`/`duration`/`prepare`/`play`/
`stop`/`free`), exported from `seq/` and dispatched by the free `play` — the
same placement the reference module has, down to staying out of the package
facade, which is where the Python client keeps it too. Three things are worth
carrying forward:

- **The two phases are the same two, for a different reason.** There they keep
  a *thread* unblocked; here `prepare` is the half that `await`s and `play`
  waits for nothing, so a routine can start a lane without holding up the
  page's one thread. Same split, same words, the language doing the arguing.
- **`play` refuses an unprepared curve rather than preparing it.** The
  reference verb prepares on the spot, blocking off the clock thread; a
  synchronous verb in a page cannot, and returning a promise from `play` for
  one kind of playable would break the verb. So the refusal names
  `await auto.prepare(server)`. The other half of that asymmetry — the NRT
  self-prepare — has nothing to port to: there is no score destination here
  (**W13**).
- **The `/buffer_gen "env"` payload is tagged, not inferred.** A whole-numbered
  level would otherwise ride as an int where the reference client sends a
  float, so `envGenArgs` tags every value the way that client types it (the
  shape an int, everything else a float) and the parity vector asserts the
  tags as well as the numbers.

The one piece of this slot that did **not** ship is the one it borrowed:
`plot(automation)` needs `plot`, which was not written yet. *(It landed with
**W13**, which brought the whole verb forward.)*

**Verified:** `./build.sh && ./test.sh` — 188 `node --test` cases (7 new: the
lane def's spec and three curves against `seq-vectors.json`, frozen from the
reference lane by the new `gen-seq-vectors.py`; the target normalization; the
refusal; and one against a real `clausters --ws` server where the curve sweeps
a `/node_map`-ed control read back off a bus) and twelve headless-Chrome
acceptances, the new one being `tests/automation.html`: the lane sweeping a
mapped control on the in-page engine (200 → 1153 → 2107 read off the bus by
the script), `stop` holding it, then the same curve seeded into a `bpf` widget,
**bent by a synthesized drag**, coming back as shape 5 with a curvature, and
the rebuilt lane reading 544 where the straight one read 2107 — the drawn
curve and the played one being one object. Example:
`examples/transport/automation-lane.html`, the port of `automation_lane.py` with the
editor beside it (the Python one renders offline, which a page has no drive
for). Book: "Automation: a curve driving a control", in the routines chapter.

**The frozen vectors were re-checked while porting**: every generator in
`tests/` (`osc`, `def`, `gui`, `clock`, `data`, `bundle`) was re-run against
today's Python client and none of the committed files moved — the parity
surface is exactly what it claims to be, and `seq-vectors.json` joins it.

### ✅ W12 - The shared `/transport_set` grid *(done 2026-08-03)*

*Deferred out of W3.* Phase alignment across clients — the TS counterpart of
C15 and of C16's `follow_transport`.

- `quant` honored when a routine starts (snap to a beat boundary), and joining the server's `/transport_set` grid so pages started at different moments share one bar line.
- Following the `/transport_play|stop|locate` broadcasts, so a page's playhead rolls in lockstep with every other client: the server broadcasts control, never audio.
- W3's rule is not bent: the clock still never talks to a server. The `Server` feeds the grid inward, the way `sampleTimebase()` already anchors the timebase.
- **`Session.joinTransport()`** (*owed here since W18*, which shipped the facade without it): the third of the Python `Session`'s chaining verbs, beside `lockToServer`. It is one line over whatever this milestone gives the clock, and the facade is incomplete against its reference until it exists.

**Acceptance:** two pages — or a page and a Python client — join the same transport and land on the same bar; a `/transport_locate` moves the page's playhead with it.

**What shipped.** The joining half, in the two places the reference client
keeps it, plus the facade verb it was owed.

- **`TempoClock.joinTransport(server)` / `leaveTransport()` / `gridBeat()`**
  (`src/base/clock.ts`): the join reads `/transport_query` once and keeps three
  numbers — tempo, origin, and which axis the origin lives on — so W3's rule
  holds exactly as written: the clock reads a `Server` handed to it and never
  talks to one again. Nothing about a joined clock is asynchronous afterwards.
  `play(item, quant)` now snaps against `gridBeat()` rather than `beats()`,
  which is the whole behavioral change: on an unjoined clock the two are the
  same number, so no existing page moved.
- **Two axes, as in Python.** A clock on a `SampleTimebase` keeps the origin in
  samples and reads its own counter, which is sample-exact by construction; a
  wall-clock one maps the sample origin to Unix time through the
  `/clock_query` anchor and the core's `samples_to_secs`. A timebase swapped
  out from under a joined sample grid falls back to the clock's own beats
  rather than reading a meaningless origin — re-join after a `lockToServer`.
- **`Playhead.followTransport(server, { quant })` / `unfollowTransport()`**
  (`src/seq/timeline.ts`): the `/transport_query.reply` broadcasts drive the
  local transport — play rolls it from the broadcast position, stop halts and
  locates it. It shipped as a raw `server.onReply` subscription, W8 not being a
  prerequisite: a page has one connection per server and every reply already
  arrives on it. **W8 folded it onto `OscFunc`**, where the reference client had
  it all along — the receiver being the connection the page already has rather
  than a socket opened for the purpose.
- **`Session.joinTransport()`** — the third chaining verb, one line over the
  clock's, closing what W18 shipped the facade without.
- **The reference client moved with it.** `grid_beat()` was private in Python
  (`_grid_beat`) and a test was already reaching through the underscore, while
  the example and two book pages recomputed it by hand from `transport()` and
  the timebase. It is public there now, with `joined` beside it, so the two
  clients name the same thing — and `sync.py`'s `next_bar_sample`
  is a line shorter for it.
- **A decode divergence found by the port** (`crates/clausters-core-web`): an
  OSC **timetag argument** crossed to JS as raw NTP seconds where the Python
  decoder yields Unix seconds. Nothing read one until the wall-clock join
  needed `/clock_query.reply`'s third field, so it had never surfaced; the
  wasm door now converts through the core's own `ntp_to_unix`, and the two
  clients read one wire the same way. The committed vectors could not have
  caught it — they are encode vectors, and a decode divergence is invisible to
  byte parity — so `gen-osc-vectors.py` grew a second output,
  `osc-decode-vectors.json`: packets the Python encoder cannot build, paired
  with what the Python decoder reads out of them.

**Verified:** `./build.sh && ./test.sh` — 196 `node --test` cases (8 new) and
the twelve headless-Chrome acceptances, unchanged. Five of the new cases are
offline: the grid as the conductor's axis and not the clock's, two clocks
landing on the same bar from different beats of their own, the wall-clock
mapping through the anchor, an undefined transport left alone, and a playhead
driven by simulated broadcasts. Two run against a real `clausters --ws`
server — a wall-clock and a sample-locked client join one grid, agree on it
and land their notes under 50 ms apart, and a playhead follows a conductor's
play / locate / stop over the wire and stops following on
`unfollowTransport` — and the eighth is the decode-parity vector. Example:
`examples/transport/sync.html`, the port of `sync.py` with the
following half Python's does not have — driven headlessly, a client joining
2.6 beats late still sounded at grid beat 4.011 alongside one that had been
waiting. Book: the joining and following sections of "The transport".

### ✅ W13 - An NRT / score drive *(done 2026-08-03)*

*Deferred out of W3*, whose `Timeline.fromPattern` bounces by driving the
ordinary clock through its manual seams. The third `Server` interface, beside
the two carriers: a destination that *writes* time instead of waiting for it.

- A score destination for the sequencing layer — the same pattern or timeline that plays live emitted as a timestamped score — rendered either by a native server's NRT mode over WS, or in-page by the wasm engine running faster than real time into a buffer the page can play or download.
- W3 already *bounces* without one (`Timeline.fromPattern` drives the ordinary clock through its manual seams); what is missing is the interface, not the arithmetic.
- Score parity is the check C5 keeps on the Python side: one piece, one score, compared byte for byte.
- **`Session.nrt()` and `session.render(...)`** (*owed here since W18*): the third factory and the verb that drains the clock into a render. W18 shipped the facade with the two live carriers only, and said so — a facade must not name a verb it cannot keep, which is the whole reason this pairs with the drive rather than preceding it.
- **`defs/asdef.ts`** — the ephemeral-def coercion (`asDef`, `exprChannels`), so a bare **expression** becomes playable: `play(sine(440).mul(0.5))` sends a def it wrapped for you. W18's `play` dispatches every other kind and refuses this one by name.
- **The three rendered legs of `plot`** (*deferred out of W23*, which ports the visual verbs with only what runs live): a def, a bare expression and — through them — the offline look at what a def actually produces without a server or an audio device. That is the Python verb's headline use, and it is blocked on nothing but this drive.

**Acceptance:** a piece written once emits a score byte-identical to the Python client's for the same input, and renders from the browser to a WAV that matches the native NRT render.

**What shipped.** The drive, its two verbs, and — since the verb it was
deferred *from* had not landed either — the whole of `plot` with it.

- **The score carrier** (`base/connection.ts`): `Score` and `ScoreConnection`,
  the Python client's `OscScore`/`OscNrtInterface` at this client's seam. A
  `Connection` grew a `timeMode` and a structured `addBundle(secs, messages)`,
  and `Server` branches on the first exactly where the reference branches on
  `interface.time_mode` — a score's bundle is stamped in seconds from the
  render's start, its `latency` is 0 (there is no deadline to lead), its node
  ids are unbounded (no `/node_end` stream to recycle from), and every command
  that would await a `/done` resolves at once, since nothing answers a score.
  Rationale in `docs/decisions.md`.
- **`TempoClock.render(until?)`** — the same driver the browser runs with the
  waiting taken out, synchronous on purpose: nothing is being waited *for*.
- **`Session.nrt()` and `session.render(...)`**, closing what W18 shipped the
  facade without, and **`Server.render`** under them.
- **`render.ts`**: the verb, `RenderStats`, `renderScore`, `bounceDef`,
  `wavBytes`. No `path` — a page has no writer and no filesystem, so the take
  is a `Float32Array` and `Buffer.fromSamples` (new) puts it straight back into
  the engine, which is the browser's render-then-load.
- **`defs/asdef.ts`** — `asDef`/`exprChannels`/`isExpr`, so `play`, `plot` and
  `render` all take a bare expression. `play` dispatches it now; the Faust
  `Box` leg had nothing to coerce until the box algebra landed (**W7**, which
  filled it on 2026-08-27).
- **`plot.ts`, all six legs** (*brought forward from* **W23**, which never
  shipped): the three rendered ones this milestone unblocked (a def, a bare
  expression, an `Env`/`Automation`) and the three live ones W23 had scoped (a
  `Buffer`, an iterable/`Pattern`), plus `PlotWindow` and
  `gui/ambient.ts`'s `setAmbientHost`/`ambientHost`. **W23's planned decision
  entry is obsolete**: it argued the `Env` leg would reach the same math by
  another door (`/buffer_gen "env"`), because a page had no NRT — with one, the
  leg is the reference client's verbatim, an `envGen` rendered offline, and the
  parity vectors compare the drawn curves across clients.
- **A seedless render is a fresh take again.** The engine's entropy source is
  `SystemTime`, absent on wasm, so a render given no seed took a *fixed* one
  and every take of a noisy piece was the same take. The client forwards a word
  from `crypto.getRandomValues`, which is what the wasm shell's own comment
  asks the caller to do.
- **Both clients gained `defs` on the bounce paths.** `render(pattern)` sent
  the extra defs only on the def path — in Python too — so a bounced pattern
  naming its own instrument rendered silence against an ephemeral session that
  had never been sent it. Fixed in `clausters/render.py` and here.

**Verified**, in pieces rather than by one `./test.sh` run (the browser pages
are ~460 MB each and were run individually): the node suites, `plot.html`,
`gui.html` and `data.html` under headless Chrome, and the example driven end to
end. **The full page suite has not been run against this commit** — do that
before the next release. The acceptance's first half is
`tests/score-parity.test.ts` against `score-vectors.json`, frozen from the
Python client's own NRT sessions by the new `gen-score-vectors.py`: three
pieces — a synth, a routine, a pattern — written twice by hand and compared
**byte for byte**, which pins the score epoch's timetag packing, the ordering
rule, the framing and the beat-to-second mapping in one assertion. (The pieces
send no def: a def's payload is JSON *text* whose formatting differs between
the two serializers, which `def-parity.test.ts` already pins by comparing the
parsed spec.) Beside it the render half — a score this client wrote rendered by
the engine's wasm, the seed drawn fresh and then handed back for a
sample-identical replay, an expression refused for being wider than the
render's outputs, `until` bounding an endless pattern, and the three rendered
envelopes matching the Python client's drawn curves. The page half is
`tests/plot.html`: the six kinds each opening a window the host reports back.
Example: `examples/buffers/offline-render.html` — a phrase rendered at 60x real time, looked
at, downloaded and played back through a buffer, with the seed demonstrated on
a noisy def. Book: "The ambient verbs: play, plot, render".

### ✅ W14 - Component lifecycle: freeing what a removed element owns *(done 2026-08-03)*

*Deferred out of W4*, which mounted components but never unmounted them: an
element removed from the DOM leaves its def standing, and a window the host
closes has no way back to the page. The missing half of the mount, in both
directions.

- **Down** (`src/elements.ts`, `src/base/pool.ts`): `disconnectedCallback` frees the component's GuiDef subtree (`/gui_free`), returns its allocated block — widget ids, buses, node ids — to the core-backed pools, and drops its canvas entry from the host (`web.rs`'s map, the `set_visible` sibling), so a long document that adds and removes components does not leak ids, streams or surfaces. The engine half is page-wide and stays up; only what the instance allocated goes.
- **Up**: `/gui_closed` travelling back from the host to the element that mounted the def — the event exists on the wire and in the `GuiHost` driver (W2), but a component has no handler for it; a host-closed window must reach its element rather than leave a live tag over a freed def.
- The reverse of W4's two-phase mount is deliberately **not** symmetric: a re-connected element mounts again from the same bundle (defs already sent, a fresh allocation), so the resolver is re-run, not cached.

**Acceptance:** a page that mounts and removes the same component a hundred times holds a flat id/bus/node occupancy (read from the pools and from `/group_queryTree`); a removed component stops sounding and stops streaming; a `/gui_closed` from a native `--ws` host reaches its element; and the surviving components on the page are untouched throughout.

**What shipped:** `freeBundle` (the mount's way out: `/gui_free`, `/node_free`,
`detach`, and the allocation back to the pools — the page-shared def payloads and
sample buffers deliberately kept), `disconnectedCallback` and the `/gui_closed`
handler queued on one chain per element so a DOM *move* cannot race its own
teardown, `Pool.inUse` (the occupancy a leak is read from), and
`ClaustersGui.deliver`, where a foreign host's event stream joins the page's.
Two defects the acceptance found: `defineComponent` set `src` in the
constructor, which throws on `document.createElement` — the whole
script-mounting path — and a component's canvas only followed its element's box
at mount, so one appended and mounted inside a single task kept the 1x1 backing
store it was measured at.

**Verified:** `./test.sh`, the new page being `tests/lifecycle.html` — a hundred
mount/remove cycles leaving the pools and `/group_queryTree` exactly where they
were, the removed instance going from peak 0.125 to silence on the engine's own
output (the survivor paused, so it is the only thing sounding) and off the
`/bus_stream` set, and a `/gui_closed` reaching its element. The one thing not
exercised end to end is the *socket*: the packet is the one a native `--ws` host
sends, delivered through `deliver` rather than by a host process, since a
component mounts into the in-page host and nothing there closes a canvas.
Example: `examples/components/lifecycle.html` — instruments added and removed by hand, with
the pools' occupancy on screen.

### ✅ W15 - The TypeScript bundle writer *(done 2026-08-27)*

*Deferred out of W4*, which shipped the writer in Python only — the reference
client leads and the port is mechanical, the repo's standing rule. This is that
port, so a bundle can be authored in the same language the page is written in.

**It is a node milestone, and that was decided rather than assumed** *(with the
user, 2026-08-26, closing the `panels/standalone` entry in the root plan)*: a
bundle is an input a static page boots with no interpreter, so **writing** one
is not something a page does. The file writer below is the milestone's centre
and it runs in node; the page's half is the in-memory mount, which writes
nothing. **That made "Node target" (then a Future direction) this milestone's
dependency rather than a someday**, and it shipped here rather than separately
— the entry left that list, as an entry that converges into a milestone does.

- `src/bundle-writer.ts` (authoring, **not** part of the slim `dist/runtime.js`): the TS counterpart of `clausters.bundle.Bundle` — the symbol table, the bundle-prefixed def names, the declared `params` and presets, validation through the core wasm door (`check_def_payload` and the `requirements`/`resolve` pair) before emitting, and the generated five-line ES module that registers the tag. An unmountable bundle stays unwritable, on this leg too.
- Runs in Node (a file writer) and in the page (an in-memory bundle mounted without a round trip through disk), the two being the same code over a small output seam.

**Acceptance:** the W4 bundle parity vector runs both ways — what the TS writer emits and what the Python writer emits are byte-identical for the same input, and each resolves through the browser's wasm door to the same mount; `examples/editors/document/` rebuilt from the TS writer draws the same page.

The first half is met and is the suite's own assertion. **The second is the
half that waits on the open decision below**: the bundles that page mounts are
written by `examples/panels/*/make_bundle.py`, so rebuilding them from the TS
writer means adding a node script to a directory that holds pages — which is
exactly the question this milestone did not answer. What stands in for it until
then is stronger than a redrawn page and weaker than a ported script: the two
writers are compared on bytes, so a bundle written by either is the same
directory by construction.

**The output seam turned out to be `files()`, and it went into both clients.**
`write(directory)` is `files()` plus the disk, and `files()` — the whole bundle
as `path -> text` — is what a page mounts (`openBundle({ files })`, a `Source`
inside the mount that reads a path from memory or fetches it under `base`, the
one place that had to learn the difference). A verb only one client had would
have been the divergence this rule exists to stop, so `Bundle.files()` is in
the Python client too, and `write` there is now the same two lines.

**Byte-identity is what forced the format to become canonical.** Two writers
emit one directory only if they agree on the bytes, and JavaScript has a single
number type: `220.0` is unsayable there. So both writers emit **canonical
JSON** — keys sorted (which also keeps the two GuiDef builders' key order out
of the comparison), no space between tokens, two spaces of indent for the two
files a person reads, and an integral float spelled `220`. Python's writer
moved to meet it; nothing downstream reads a type out of the spelling, and the
core already coerces an integer where a `"type": "float"` is declared. The
sorted keys also mean the *files* are sorted where the builders' output was
not, which is why this had to be decided rather than discovered.

**The node target, which was the dependency.** `loadCore()` with no argument
now finds the core's `.wasm` in either environment — the glue's URL-relative
fetch in a browser, a read off disk under node (the emitted `dist/base/` layout
and the source `src/base/` one, in that order) — so the 31 suites that each
carried their own `readFile` of the same file simply call `loadCore()`, which
is the load path being *exercised* rather than described. The boundary is
written down in the book (`docs/src/platform.md`, "Without a tab: the client
under node"): the def, sequencing, arrangement, data and writer layers port and
the WS carriers work; the in-page engine and the page host do not; and the
facade `import "clausters"` does not either, because importing it registers
custom elements a document has to exist for — so a node script imports the
subpaths, and `clausters/bundle-writer` re-exports `loadCore` to be
self-sufficient.

**Verified:** `./test.sh`. The parity suite is the acceptance and it holds both
halves — the TypeScript writer's `files()` compared **byte for byte** with the
frozen output of the Python writer for the same authoring calls, `write()`
compared the same way against what lands on disk, and what the TS writer emits
resolved through the wasm door into the frozen mounts. The new page is
`tests/authored.html`: a bundle authored in the tab, mounted from memory,
drawing and sounding, with the assertion that the page's own resource timeline
holds **no** bundle request. Example:
`examples/components/authored.html` — the same thing by hand, with the
emitted directory listed on screen.

**What it leaves open, and it is the decision this milestone did not take:**
where a **node script** lives in `clients/web/examples/`. The directory holds
pages, the `examples` skill says a web example *is* a page, and the counterpart
of `standalone.py` is a script — so the entry in the root plan ("Two pairs read
side by side turn out not to be pairs") stays open with exactly that question,
and the writer shipped without its node-side example. What it did not lack is a
manual test: `write()` is covered by the suite, and the page half has both a
smoke and an example.

### ✅ W16 - Example parity with the Python client *(done 2026-08-25)*

*Deferred out of W5*, which ported the examples that run in a page as they
stand and left the rest. This closes the gap, so a Python example and its page
read as **one example in two languages**: same material, same names, the same
calls to the same API in the same order, one written as a script and one as a
page. Not one *catalogue* — no book enumerates them, and the page keeps its own
spelling rather than the script's name (below).

**It is now blocked, and by something that is not porting effort.** The Python
client's GUI surface was reformed on 2026-08-23 (its plan's "The client API
reform" track, C39–C43 and C46): a builder returns a `View` that opens itself,
`window()` is `view()` and any root opens, ids belong to the instance, the
samples a view draws are a `source`, a control declares its range and a widget
reads it, and a window binds against the controls it was built from. Every one
of those changes what an example *says*, so a page ported against the surface
this client has today is a page that has to be written twice. The port of that
reform — its shape is in W24, "The GUI node becomes a `View`" — goes first, and
this milestone's acceptance is exactly what makes that an order rather than a
preference: two programs that produce a similar picture by different calls are a
divergence, not a port.

- The remaining `clients/python/examples/` ported to `clients/web/examples/`, each named after the example it mirrors — **the page's own spelling**: a hyphen for the underscore, and without the `gui_` prefix, which says which *process* runs it and means nothing to a page (`composer.py` is `composer.html`, `offline_render.py` is `offline.html`). Where a page's name cannot be read back to a script's, the page's header comment says which example it is.
- Most of what is left is **not** blocked on porting effort but on a surface this client does not have yet — MIDI, automation, the transport grid, an offline render, the box algebra, the UGens outside the shipped set. Each such example lands with (or after) the milestone that opens its surface, which is why this slot is a destination rather than a queue.
- The examples that are Python-process shaped by nature (a launcher, a live UDP peer, a native GUI shell) have no page counterpart and stay unported. **This is stated where a reader is, not in a catalog** — the project's example rule keeps the books out of enumerating them, so the absence is recorded in this plan, and the *nearest* page says in its header what its Python sibling does that a tab cannot.

**Acceptance:** every Python example either has a web page named after it or a reason stated in this plan for having none, and each ported page runs on the in-page engine with the carrier line marked. And, for a pair that exists twice, the stronger check the `examples` skill now states: the two are **one example in two languages** — same material, same names, the same calls to the same API in the same order — so a port is finished when the two screens can be put side by side and read as the same piece.

The largest single item behind this, named here because it is a **track and not
an example**: `composer.py` needs the **arrangement layer** — `clausters.form`
(elements placed recursively, and the rendering that flattens them) plus the
multitrack `Editor` and its transport, roughly two thousand lines of Python with
no TypeScript counterpart — and it pulls W13 (the offline bounce of its take)
with it; the automation lane it also needs is here since W11. No design is staged for that port, and the
cross-client rule says the reference client is finished and polished first, then
ported, with whatever is language-agnostic pushed into the shared core as it is
written; deciding *what* goes down there is the first question of that milestone,
not a detail of it. Until then `clients/web/examples/editors/composer.html` covers the
**host** half — the lanes, clips, shared axis, drag grid and gestures, built as
widgets with no model behind them — which is what the host's own bugs need to be
reproducible in a browser, and it is where G32b was found.

**Done, and the count is the milestone's own measure.**
On 2026-08-25 the pairing stood at **19 pairs, 47 scripts with no page, 12 pages
with no script**. Two batches the same day took it to **33 pairs, 33 scripts
alone, 11 pages alone**: `basics/hello-note`, `basics/envelope`, and the whole
of `panels/` — `skeleton`, `panel`, `bind`, `stack`, `text`, `workspace`,
`attach`, `canvas`, `oscsend`, `shell`, `gestures`, `style`. Each was
written against the script line by line and then **run** under headless Chrome,
which is the half that matters: four defects came out of them, none in the
pages.

- **Opening a server adopted nothing**, so the very first example — `Server`
  then a bare `play` — failed with advice about two verbs the reader had not
  asked for. In "Found by use", fixed.
- **A window handle's `controls` was `controlMap()` here**, a name nothing
  outside a test used and the reference client spells `controls`. It is
  `controls` now, and a **property** answering a plain object, which is the
  reference's shape down to the last detail.
- **`query()` answered a record here and a `(kind, props)` pair there.** The
  record won and **the reference client moved**: `clausters.gui.WidgetInfo` is
  a dataclass beside the server's own records, printing one readable line like
  every one of them, and the timeout it used to swallow with `None` now raises
  `ReplyTimeout` — which is what every other query in that client already did,
  so the odd one out was the GUI's. The parity vectors caught the first thing
  that drifted: a prop's number printed `20.0` there and `20` here, because a
  float is a float in one language and a number in the other. Both run the
  `%g` every other record line already ran.

- **An attached `GuiHost` did not know its surface.** `panels/attach` is *about*
  the two verbs, and a second handle over the page carrier could not open a
  window into an element: only `boot` set `page`. What `boot` marks is
  ownership, never what the host is, so `attach` reads the carrier's surface
  too.

The lesson is the one the milestone was written for: the pages are not the
product, the **exercise** is. Nothing had opened a bare server on a page before,
so nothing had noticed that a page could not — and nothing had attached a second
GUI handle in a tab, so nothing had noticed that one could not draw.

**`panels/style` stopped the batch, and reported a missing feature** rather
than a porting gap — which is what an example that cannot be written is for. Its subject is the three levels a theme
overlays at — the host's own look, a `theme` group on a container, a widget's
`color` — and the page has the last two and no door to the first. The host-wide
theme is a **launch flag** (`--theme file.toml`) and a **wasm export**
(`GuiBridge.theme(json)`) with **no protocol verb**, which is exactly the shape
the typeface had before `/gui_font`: a property of the host that every client
should be able to hand over at run time. The fix was that verb, and it landed the same day
(`crates/clausters-gui/PLAN.md`, Found by use): `/gui_theme <json>` and — since `metrics`
sat beside it in exactly the same shape — `/gui_metrics <json>`, with both wasm
exports removed and `GuiHost.theme(table)`/`.metrics(table)` in both clients.
`panels/style` is written now, on both sides, and the folder is complete.

**The track is in.** `src/form/` (the arrangement), `gui/transport.ts` (the
playhead and its four buttons) and `gui/editor.ts` (the multitrack editor) are
all ported, with `defs/patch.ts` under the last of them — level 1 of the
patcher, `GraphPatch`, which is what a logical aggregate draws through.
`clausters_core_patch_compile` reached wasm with it, closing the one **gap**
`docs/bindings.md` still declared.

Two idioms are the page's and are written down here rather than in a diff: the
editor **subscribes** where the Python one is polled (`open` listens, `detach`
stops), and `Document`/`Log` grew synchronous constructors beside their
awaiting `open`, because an edit answers the host in the same turn and an
`await` in the middle of that is a window for a second gesture.

**`DefPatch` — level 2 of the patcher — landed on 2026-08-22**, with the window
that opens it: a def's internal UGen graph as boxes, `PatchWindow`, and
`plotDef()` on `SynthDef`, `FaustDef` and `GraphDef` (level 1 had the model and
no opener). `tests/patch-parity.test.ts` freezes what the *Python* models decode
five defs into — the boxes, the cords and the widget schema — and asserts this
client reads the same, and `examples/editors/patch2.html` is `patch2.py`'s page.

Two things it turned up, neither a decision:

- **The inlet labels come from each client's own builder signatures**, which is
  the one place the two must read their own source: Python uses `inspect`, this
  one `Function.prototype.toString()` (the parity suite's own trick, now in the
  package as `ugenInputNames`). Same fact, each language's way to it. Where the
  two deliberately spell a parameter differently — the resonant filters' `rq`
  against `res` — the caption differs with it, and the parity test declares that
  table rather than hiding it.
- **`PlotWindow` was missing `onClosed`**, which the Python one has and the
  patch example needs; it is there now on both window handles.
- **The ambient verbs needed a `Session` to have run first**, and said so
  nowhere: `plot`, `scope` and now `plotDef` resolve a host of their own, and a
  `GuiHost`'s id allocators are core registries — so on a page that had opened
  nothing else, the first call died inside `new Registry` on a wasm module
  nobody had loaded. In Python the same call needs nothing, which is what makes
  this a divergence rather than a platform's price: `resolveHost` awaits
  `loadOsc()` itself now, and the verbs resolve what they need on every page.

The parity suite is the point of the whole thing:
`tests/editor-parity.test.ts` freezes the tree the *Python* editor draws for one
composition per branch of the mapping rule and asserts this one draws the same,
and it earned its keep on the first run — it found a curve drawn with a shape it
did not have (the Python client resolved an already-resolved break-point quad a
second time; fixed there, recorded in that plan's "Found by use").

**The first leg of that track was `src/form/` — the arrangement itself.**
The four modules sit at their siblings' paths (`element`, `aggregate`,
`render`, `document`), and what holds them to the Python client is not the
source but what leaves the layer: `tests/gen-form-vectors.py` builds five
compositions with the reference surface and freezes both the **document** each
is written as and the **flattened timeline** it renders to, and
`form-parity.test.ts` rebuilds the same five here and asserts the same two
results. What is still absent from this leg, and is the rest of the track: the
multitrack `Editor`, its transport, and `gui/notation` (whose own blocker is
below).

Four differences the port has, each an idiom and none a surface:

- **The node id is stamped in a `WeakMap`, not as an attribute.** Python sets
  `_doc_id` on the object; a `Track`'s items are `Event`s the sequencing layer
  owns, and this bridge has no business growing a field on one, so the
  association lives beside them and is read through `docIdOf`/`setDocId`.
- **`renderLogical` is async**, because sending a def is a round trip and a
  page has one thread to wait on. `render` over a logical aggregate therefore
  answers with a promise of the instance group where the Python one answers
  with the group.
- **`element.render()` reaches the dispatch through a registry.** The two
  modules name each other (rendering knows every element kind; every element
  renders itself), which Python spells as a function-level import and a page
  cannot: a static cycle whose far end declares `class Aggregate extends
  Element` fails at *load*. `render.ts` registers itself as it loads.
- **An event's two renamed keys are written the file's way.** `addAction`/
  `hasGate` here are `add_action`/`has_gate` in the document, as on the wire
  and in the Python client, so one composition reads the same in both clients;
  every other key is a def's control name and crosses untouched.

**The engraver's blocker moved**: verovio 6.3.0 is released and on npm as an
Emscripten build, so "libverovio is not built for wasm" has stopped being true.
What that changes for the port — including the part of it that is *not* a
packaging question — is written with the rest of the notation shape below.

### ✅ W17 - Publishing: the npm registry and a third Read the Docs project

*Deferred out of W5*, which built the package and the book and left them on the
machine. The distribution step, deliberately separate: the artifacts are held
publishable by a checker long before anyone runs `npm publish`.

- **The package**: `npm publish` for `clausters`, on the repository's own SemVer (package, crate and wheel are one release), through the gate `prepublishOnly` runs. The procedure, including what has to be built first, is `clients/web/BUILD.md`, "Publishing".
- **The book**: a third Read the Docs project pointing at `clients/web/.readthedocs.yaml` (the file exists and drives the whole build: TypeDoc, then mdBook).
- **The inbound links**: the other books and READMEs do not link the web book, because the URL does not resolve yet. Once it does, the badge/link rows (root `README.md`, `clients/python/README.md`, `crates/clausters-gui/README.md`) and the Python book's introduction gain it — the cross-linking the three books otherwise already do.
- The npm publication also decides what W5 could leave open: whether the wasm bundles ship inside the tarball (they do today, ~2 MB) or are fetched, and how a consumer's bundler is expected to treat the worklet module.

**Acceptance:** `npm install clausters` in an empty project yields a working client (the getting-started page's example runs against it unchanged), the web book is live and cross-linked in both directions, and the release's three version numbers agree.

**What shipped.** The publication is **automated rather than manual**:
`release.yml` grew a `publish-npm` job beside the PyPI one, so the
`v*` tag that cuts the wheel cuts the package too — one tag, one version,
which is the only way the "package, crate and wheel are one release" rule can
hold by construction rather than by memory. CI never builds `clients/web`, so
the job carries the whole recipe itself (the wasm32 target, the
lockfile-pinned `wasm-bindgen` CLI taken as a prebuilt binary, `build.sh`, the
package checker, then a publish with provenance). The two open questions W5
left are settled and recorded in `BUILD.md`: the wasm bundles **ship inside
the tarball** (an install has to work offline, with no CDN), and the worklet
is reached as `new URL("./worklet.js", import.meta.url)` — the form a bundler
copies as an asset — with `workletUrl` as the escape hatch for one that does
not.

**Three things only a clean checkout could find**, each fixed where it broke
rather than worked around in the workflow — a release runner and a docs
builder are the first machines that are not somebody's working copy:
`crates/clausters-gui/Cargo.lock` was ignored while `build.sh` reads it (the
`wasm-bindgen` pin is an agreement between two lockfiles, so one resolved
fresh per machine is not a pin); the Read the Docs build installed no
dependencies for the package whose tsconfig asks for the `node` type library;
and TypeDoc could not resolve the wasm boundary, whose three declaration files
are now versioned rather than putting a Rust toolchain on Read the Docs to
recompile wgpu per doc build. Publishing is one-way, so the workflow gained
two gates on the way: PyPI publishes only if npm did, and the release page
waits for both — a half-published release is the one failure that cannot be
retried.

**Verified:** `clausters@0.4.1` installed from the registry into an empty
project boots the engine in headless Chrome and plays a note (peak 0.100
read off an analyser, the node freed afterwards); the wheel is on PyPI at the
same version; the book is live, with the API pages generated on Read the Docs
itself; the doc build was reproduced from `git archive` before the push, on a
tree with no `node_modules` and no Rust.

### ✅ W18 - The `Session` facade *(done 2026-08-03)*

*Deferred out of W5*, whose layout sketched a `session.ts` into the slot while
the milestone itself was docs, examples and packaging. It is an API layer, and
it leans on verbs this client does not have yet.

- `session.ts`: the browser counterpart of `clausters.Session` — one handle bundling a `Server`, a `TempoClock` and its timebase, so a page stops wiring the three by hand. On this page the singletons already give the *shared* half (one engine, one host, one namespace); what a `Session` adds is the ergonomics and the ability to hold more than one at a time (a page against the in-page engine beside one against a remote `--ws` server).
- The Python client's ambient verbs are the reason not to do it early: `play` is here already as `Event.play`/`Pattern.play`, but `plot` wants the script-side data paths (**W10**) and `render` an offline drive (**W13**). A facade shipped before them would name verbs it cannot keep.

- The **default session** comes with it, and with it the ambient clock. `Routine.play()` here (`src/base/stream.ts`) resolves only the routine currently being resumed and throws outside one, while the Python client resolves *running routine -> active session -> default session's clock, created and started on first use*, so `Routine(f).play()` needs no session, no clock and no server. Port the whole ladder, not the last rung: the default session is what the other two rungs resolve against, and it is also where a default `Server` would be adopted first-wins (`Server.boot()`'s role there). Lazily, never at import — a page that only renders or only draws must not start a clock by loading the module.

- **`Synth.new` / `Group.new` become the constructor here too, and this is the milestone that can do it.** The Python client made the move already: `new` was an sclang transliteration — that language has no distinguished initializer, so every constructor is a class method called `new` — and Python has `__init__`, so `Synth("blip", {"freq": 440}, target=group)` and `Group()` now create and send, with `Synth.from_id` / `Group.from_id` naming a node that already exists (a responder's id, a tree query) and sending nothing. The rule is general: **`new` is the constructor, and alternates get names** (`Group.graph`, `Bus.audio`, `Buffer.alloc`, `FaustDef.fromSource`). The port waits for this milestone because the two signatures only converge once there is an ambient session: today `Synth.new(server, defname, …)` takes the server as its first positional, which as a constructor would read `new Synth(server, "blip", …)` — the server first here, the def name first there. With a session resolving it, both become `Synth("blip", controls, {target, action, server})`. Do the two together; `Synth.fromId(id, defname, server)` and `Group.fromId(id, server)` come with it, and `nodeId()` already lets `target` be a node or an id, which Python has now copied.

**Acceptance:** the getting-started example rewritten through a `Session` is shorter and does the same thing; two sessions over different carriers coexist in one page, each with its own clock; a bare `Routine(f).play()` runs with nothing else set up; `new Synth("blip", …)` creates against the session's server, and `Synth.fromId` is the only way to wrap a reported id.

**What shipped.** The whole ambient layer, at the Python client's paths:
`base/environment.ts` (`RandomContext` + `Environment`), `base/main.ts`
(`Main`, `main`/`defaultSession`, `resolveServer`/`resolveClock`/
`getDefaultClock`), `session.ts`, `play.ts` and `defs/wire.ts` — the five
modules W21 listed as this milestone's. Four things are worth carrying
forward:

- **The page has one thread, so the registry is not thread-local.** Python
  keeps `current_tt` and `current_session` in a `threading.local`; the running
  routine already lived in `base/context.ts` (a module slot is exactly as
  sound when a wake runs to its next `yield` with nothing interleaving) and
  the active session is one more slot beside it. `Session.use(body)` is the
  port of `with session:` and is **synchronous by design** — an `await` inside
  would let another task run while this session is ambient, and there is no
  way to scope that on one thread.
- **The factories were named for the carriers, and are not any more.**
  They were `Session.page()` and `Session.connect(url)`, matching
  `GuiHost.page()`/`GuiHost.connect()` and the `Server` carriers this package
  had, rather than `embed`/`live(host, port)` — "whose parameters (a host, a
  port, a process to boot) a page has none of". `nrt`/`render` and
  `join_transport` waited on **W13** and **W12**, and are in.

  **Reversed on 2026-08-25**: they are `Session.embed()` and `Session.live(url)`.
  Half the reasoning was the match with `GuiHost.page()`/`connect()`, and those
  are gone — the same divergence one layer up, made `boot`/`attach` the same
  day. The other half confused the constructor with the verb: `live` takes an
  address, which a page has, and only the *boot* inside it is impossible here.
  What the two names mean is what a tab has both of — a server **inside this
  program** (the wasm engine in the worklet, sharing memory, which is what
  `embed` names there too) and one **running as its own process**, reached over
  a socket. So `embed` boots and `live` attaches, and the one missing half —
  `live` starting a server when none answers — is named in its doc comment
  rather than left to be found.
- **`adoptDefault()` lends the server and not the clock.** In Python the
  default server is adopted by a free-standing `Server.boot()`; a page has no
  process to boot, so the verb is the session's. It stops at the server on
  purpose: the default session's clock is created *and started* by the first
  ambient play, so lending a stopped one would hand `play()` a clock nothing
  ever starts. A named session's clock is reached through `session.play` or
  inside `session.use`.
- **`new` became the constructor**, as the plan called for: `new Synth("blip",
  controls, { target, action, server })`, `new Group({ name, … })`,
  `Group.graph(defname, ports, …)`, with `Synth.fromId`/`Group.fromId` the
  door for an id something else reported. The server moved into the options
  bag on every resource constructor (`Bus.audio`, `Buffer.alloc`/`read`/
  `load`, `def.send`) and resolves ambiently when absent — 93 call sites
  across the package, the tests and the examples moved with it.

Two smaller things the milestone needed and grew: `ClaustersServer.close()`
(the engine sibling of W20's `GuiBridge.close()`, so a session that opened its
own engine releases the `AudioContext` with it) and `pageGuiConnection(host)` /
`GuiHost.page(host)` taking an instance, so `session.gui()` can wire a GUI leg
to *this session's* engine rather than the page's.

**Verified:** `./build.sh && ./test.sh` — 176 `node --test` cases (9 new in
`tests/session.test.ts`: the resolution ladder, `use` scoping and unwinding,
`fromId` sending nothing, the default clock created and started by a bare
`Routine.play()`, two independent random contexts, the `play` dispatch) and
ten headless-Chrome acceptances, the new one being `tests/session.html`: two
sessions on two engines, an ambient note audible on the default session's
analyser (0.200) and silent on the other's (0.000), the same call inside
`b.use(…)` the other way round, a bare `play()` driving the default session's
clock, and every kind `play` dispatches asserted audible. Example:
`examples/basics/verbs.html`, the port of the Python client's `verbs.py` — a session
opened, then every playable kind visited in turn.

Not in scope, and each now owned: the `plot`/`scope` visual verbs and the
`set_ambient_host` registry that serves them (**W23**, opened by this
milestone), a bare **signal expression** as a playable (it needs the
ephemeral-def wrapper, **W13**'s `asdef`), an `Automation` (**W11**), and the
two chaining verbs the Python `Session` has and this one does not —
`joinTransport` (**W12**) and `nrt`/`render` (**W13**).

### W19 - The notebook front end - moved to the `jupyter` branch

Shipped 2026-08-03 and taken off `main` on 2026-08-05, with the
`clausters-jupyter` package it belonged to. `src/notebook/` was never a
directory beside this package: it was built by a dedicated esbuild step,
asserted into the npm tarball, exercised by its own test page, and fronted by a
re-export entry that constrained what the rest of `src/` could be renamed to —
so every consumer of this package shipped a notebook front end. The audit and
the plan for bringing it back are in `clients/jupyter/ISOLATION.md` on that
branch.

Three seams here left with it, each having had exactly one caller:
`newGuiHost`'s `engine: null` (which is what widened `ClaustersGui.engine` to
`| null`), its `wasm` option, `setTickWorkerUrl` and `ClaustersServer.onQuit`.
What stayed, on its own merits: `newGuiHost`/`newPools` themselves, `IdShare`,
per-instance hosts, and `ClaustersGui.close`/`detach`.

### ✅ W20 - Host and engine instances: the page is not the unit *(done 2026-08-03)*

A page held one GUI host because winit's one `EventLoop` was mistaken for one
host — a second `start()` was `RecreationAttempt`, a panic inside the wasm
rather than an error a caller could catch. The loop drives any number of
windows, which is already how one host serves a document's canvases, so the
instances share the loop and nothing else.

- `web.rs`: `WebApp` became the instance and a new `WebHosts` is the one `ApplicationHandler` winit takes, owning the set. Events carry a `HostId`; `window_event` finds the owner by asking who holds the `WindowId`, so no second index has to stay in step with every attach and detach. The loop is built once and memoized in the proxy the instances already shared; `start()` builds it on the first call and adds an instance on every later one.
- Instances share **nothing** else: each has its own `Host` (and therefore its own widget-id space), its own audio-server leg, canvases, buses, taps, tick and fetches. No id range is partitioned between them — which is the point, since the two allocating clients are separate processes with no channel to agree over. The GPU was already per canvas, so an instance adds no device.
- `GuiBridge.close()`, new: an instance that outlives its purpose otherwise keeps its WebSocket open, its `setInterval` running and its GPU surfaces alive. A page that holds its host until it unloads never needs it.
- The same rule on the audio side: `engine()` beside `server()`, and `pageConnection(target?)` to carry a client over one. Nothing there was ever page-global except the memo — `bootClausters` already built its own `AudioContext` and worklet per call.
- `guiHost()` in `page.ts` keeps its memo: its contract is *the page's host with the page's default canvas*, and a second one of those means nothing. Its instance counterpart is `newGuiHost()`, added once it was clear that leaving callers to the raw wasm binding was the gap, not the design — the notebook front end was reaching under the client to call `start()` itself, and so would anyone else. `newPools()` travels with it: page-global pools were right while the page held one client.

**Verified:** `tests/hosts.html` under the headless-Chrome harness — two hosts in one page both draw; widget `1003` holds `0.2` in one and `0.8` in the other at once; a gesture on one leaves the other's outbox empty and its widget unmoved; each bound knob reaches its own engine and not its sibling's (`220 → 984.7` while the other stays `220`); closing one leaves the other drawing, answering and driving its engine. Example: `examples/panels/two-hosts.html`.

### ✅ W28 - The generic editor: `edit(x)`, and `Editor` renamed to `FormEditor`

The port of the Python client's phased track of the same name
(`clients/python/PLAN.md`, "The generic editor: one editor per domain, and
`edit(x)`"), which is where the analysis, the map of what each view edits, the
decisions and the acceptance are read. **Nothing here is decided separately** —
same class names, same calls in the same order, `Editing` split the same way,
and the same answer to where the editing registry hangs (`C49` took it: on the
**data**, through `Editing.of` over a `WeakMap`, plus the `context` option — no
ambient holder, because a second way to answer one question is how two
histories over one curve happen) —
and the surface must not appear in one client before the other, which is the
whole of what this entry is here to prevent.

The four phases land here as they land there, in the same order — `C48`'s
session-file resolver first, since `edit(x)` over samples is only half a verb
until a reopened take is a buffer again — and `C50` in particular (`edit(x)` and
the three structure editors) closes with its TypeScript half in the **same
commit**, because that phase is where the new surface appears. `C49` carries the
**rename** (`Editor` becomes the generic class, the arrangement's becomes
`FormEditor`) and the same four collaborators — `View`, `Domain`, `Echo` and the
editing context — under the same names, in the same new `gui/editing/`
subpackage; it is not an internal split this client may spell differently, since
a class name and a module path are both surface. `C48` and `C51` move with
their Python halves, books included.

**`C49` landed 2026-09-02**, here in the same commit: `src/gui/editing/` with
`context.ts`, `domain.ts`, `echo.ts`, `editor.ts`, `formeditor.ts`, `view.ts` and
`index.ts`; `Editor` is the generic class and `FormEditor extends Editor`;
`Editing`/`FormEditing` split the same way. Two spellings are this client's own
and both are `idiom` rather than divergence: `Editor.open` is **async** (the
ambient host may have to boot, which `View.open` already had), and the
collaborators are re-exported as the `gui.editing` **namespace** rather than
flattened into `gui`, because `View` would collide with the guidef `View` the
module already exports — the Python client reaches them at the same path
(`clausters.gui.editing`), so the module tree is still one tree.

**`C50` landed 2026-09-02**, here in the same commit and case for case:
`gui.edit` over a `Buffer`, an `Automation` and a `Timeline`, the three domains
and views beside it in `src/gui/editing/`, `domainEdit` bound in `document.ts`,
`tests/gui-edit.test.ts` mirroring `test_gui_edit.py`, and three example pages
mirroring the three scripts. `"osc"` still reaches no branch, in both clients
alike. The one spelling of this client's own, `idiom` and written in both books:
a page's buffer calls are asynchronous, so a stroke's write is **queued in
order** rather than awaited — two strokes over one span that raced would leave
the buffer holding the first.

**`C51` landed 2026-09-02**, here in the same commit and case for case:
`openPianoroll` and `openSignal` compose a `NotesEditor` and a `SamplesEditor`
joined to the piece's editing context (`FormEditor.composed`), `drawPianoroll`,
`drawSignal` and the `mode` field are gone, a history step is handed round the
context (`Editor.projectLegs`/`reflectStep`), `MEASURES`/`measures` moved to
`samples.ts` with the measure stack as the `SamplesView`'s, and
`examples/editors/composed.html` mirrors `composed.py`. Two spellings are this
client's own and both are `idiom`: the composed editors are fed from the
`FormEditor`'s **one** `onMessage` subscription (a page has no poll loop to
share, so a second subscription would hand each of them every message twice),
and `Editing.views()` was added as the public accessor the Python context
already had, since the walk now asks the context who else holds a structure.

The gaps this client carries are identical, which at least makes them shared
holes rather than divergences: `"osc"` reaches no branch of `Editor.route`
(a composed roll now **draws** the lane, in both clients — moving a marker by
hand is still nobody's).
The other two closed with `C50` on 2026-09-02 — the inversion is the crate's
now (`domainEdit`), and `"sample"` and `"draw"` route through `SamplesDomain` in
both clients. `mute`/`solo`/`level`/`height` **closed with `C48`**
(2026-09-02), in this client in the same commit: `Element` carries the mixing,
`sessionResolver`/`sourcesOf`/`take` are here under those names, `flatten` takes
a fifth `mixed` argument (the positional shape this client's `flatten` already
had), and `Editor` draws the header from the composition and routes the three
tags through the log — `"height"` is answered as the screen state it is. The one
`idiom` (in `docs/bindings.md`'s sense of the word), written in both books'
composition chapters rather than left to be re-derived: reading a file is
asynchronous here, so the takes are read **while the resolver is built** —
`sessionResolver` is `await`ed and the resolver it returns is the same
synchronous function as the Python one, so the two programs make the same calls
in the same order. The parity vectors
grew a `a_mixed_piece` case and a `sources` section
(`gen-form-vectors.py`/`form-parity.test.ts`), so a mixing rule that drifts into
one client fails a test rather than reaching a piece.

**What is genuinely this client's**, and it is the one thing the port decides on
its own: `open` subscribes here where a script feeds one poll loop, so a
composed view over several editable structures **wires itself** rather than
being pumped. That is `idiom` in `docs/bindings.md`'s sense and is written there
rather than being allowed to become a second shape.


### ✅ W29 - `edit(x)` opens, and the `AppClock` the page already half has *(done 2026-09-04)*

The port of `clients/python/PLAN.md`'s `C52`/`C53`, where the analysis, the
three measurements and the decisions are read. **Nothing here is decided
separately.**

The unusual half is that this client is the **reference** for once. `gui/host.ts`
already states the shape the Python client is being brought to - *"Nothing
pumps... the browser pushes"* - and routes `/gui_event`/`/gui_closed` to the
handles as they arrive, which is why no example here drains anything. So the
phase contract `C52` writes down is read off this client rather than invented
there, and what the page owes back is only the surface:

- **`edit(x)` opens** on the ambient host and returns the `Editor` of the
  structure's kind, instead of an editor a caller must `open`. Same rule
  `plot`, `scope` and `View.open` follow here already.
- **`Editor` gains the handle surface**: `id`, `closed`, `close()`,
  `onClosed()`. No per-kind window class in either client.
- **`AppClock`** - the same class with the same calls, over the page's own loop
  (`setTimeout`, and `requestAnimationFrame` where a redraw is what is being
  paced) instead of a thread and a wait. `defer` exists here too and is not a
  no-op: it is what puts a callback after the current task rather than inside
  it. The **`EventLoop`** of the Python side has no counterpart and needs none -
  the page is one - which is `idiom` in `docs/bindings.md`'s sense and is
  written there.
- **`Timeline.replace`** - the verb `C52` added for the loop's sake (one step
  instead of a clear and a rebuild) and this client took in the same shape:
  nothing here can observe a half-rebuilt timeline, since a page has one thread,
  but the two surfaces are one surface and the verb is where an edit's
  projection belongs in both. **Landed 2026-09-04 with the curve-axis fix**,
  ahead of the rest of `W29`.
- **`wait` on everything that opens a window**, and this is where the two
  clients genuinely differ in *shape* rather than in capability. Python grew
  `GuiHost.wait` / `WindowHandle.wait` / `Editor.wait` / `FormEditor.wait` /
  `PlotWindow.wait` / `PatchWindow.wait` / `ScopeWindow.wait` (plus a `closed`
  beside each) because a **script** must not exit while a window is on screen. A
  page never exits, so a blocking wait is meaningless and must not be
  transliterated; what the page owes is the *other* half — `closed`, and a
  promise-shaped `closed()`/`onClosed` so a caller can sequence on the close.
  That is `idiom` in `docs/bindings.md`'s sense and is written there when the
  port lands. `closed` itself is plain surface and is owed as-is.
- **The transport ticks itself.** `Transport.update` stopped being "call it once
  per pass of the script's loop" and is scheduled on the host's `AppClock` while
  a pass is sounding. Same change here, on the page's own loop — it is the same
  reason (a view's cursor parking at the end of the piece is not the caller's
  errand), and until it lands the two clients disagree about who ticks.
- **`clausters-core::tempoclock::Scheduler`** is the timer queue on both sides,
  bound here through wasm as `TempoClock` already binds it, keyed in seconds.
  It is the only part of this that is shared code rather than a shared contract.

The two problems the Python prototype measured - a reader seeing a half-rebuilt
timeline, and a reply stolen from `query` - **do not exist here**: one thread,
and a subscription that already separates a reply from an event. Recording that
is the point: they are the language's, not the design's, so the port must not
grow a lock or a reply slot to look like the other client.

One more thing the Python side settled and this client should be read against:
**opening a window is what starts the loop** there, for every window and not
only for an editor. That is the page's behaviour already and always was, which
is the point - the asymmetry is gone, and a plain panel now delivers its
callbacks the same way in both.

*Acceptance:* the same program in both languages, verb by verb - build a curve,
`edit` it, read it back after drawing, close the window - with no loop written
in either, and both example directories showing it.

**Done**, and the reading of the two examples side by side is what the milestone
was for: it found that a plain `Editor` here **was never fed at all**. Only
`FormEditor.open` subscribed, so `edit-curve.html`, `edit-notes.html` and
`edit-samples.html` opened windows whose gestures reached nothing and whose
read-back answered the curve as the page had built it. `Editor.open` now listens,
as the reference client's always did. That is the kind of thing only a verb-by-
verb read finds, and it is why the rule says to do one.

What landed, beyond the list above:

- **`Editor` is a handle**: `id`, `closed`, `close()`, `onClosed()`, `wait()`,
  and `open` is idempotent (one editor, one window). The protected `closed()`
  hook it collided with is `onWindowGone()` now, and the base class does the
  unsubscribing `FormEditor` used to do alone.
- **`closed` and `wait` on everything that opens a window** —
  `WindowHandle`, `PlotWindow`, `PatchWindow`, `ScopeWindow` (which also grew
  the `onClosed` it was missing), `Editor`, `FormEditor` (over its composed
  views). `GuiHost.wait()` is every window of the host, and `waitWhile` is the
  shared body: it is woken by a close, never a poll.
- **The subscribers are handed a message before the widget handles.** The
  reference client's order, and this client had it the other way round: a
  script's `onEvent` on an edited widget saw the structure as it was a moment
  *before* the gesture it was being told about.
- **One `resume`, as in the reference client.** `TempoClock`'s private `wake`
  and the app clock's driving were two copies of the same eight lines; they are
  `stream.resume` now. Writing it once surfaced a hole **both** clients had: the
  app clock was remembered in a routine's `clock`, which everything else reads
  for a beat, a tempo or a session. It is `appClock` now, apart from the musical
  one, and `pause` reaches whichever is holding the routine — fixed on both
  sides in the same commit.
- **The examples lost their ticks, and their stopwatches**:
  `setInterval(() => transport.update())` in five pages, and every page that
  did *timed* work by hand. `multitrack.html`'s counter and
  `two-windows.html`'s history read-out are `appClock().sched` returning a
  number now; `text.html` and `style.html` schedule their timed changes on the
  clock instead of a `setTimeout` per change; `linked.html`'s second-and-a-half
  pause is a `sched` rather than a promise helper of its own. Read against
  their scripts, those five were the same material by different calls — which
  is the divergence the rule names, not a port — and the four beyond the
  transport were found by reading the two directories side by side after the
  milestone's own list was done.

### ✅ W30 - The editing seam comes across, and the two clients are read against each other *(done 2026-09-09)*

The last structural divergence of the `application-scope` branch, and the one
the standing rule is written about: **a class that exists in one client and not
in the other**. It is a milestone rather than a parity-gap entry because it now
has an acceptance and an order, and a pending item with both is a milestone
whatever list it started in.

**What is missing: an application, and a widget id that names what it draws.**
Both were grown by the Python client on the `application-scope` branch, and the
shape the port must follow is fixed already so the two do not re-derive it
differently.

**The application** (`clausters.gui.editing.Application`) owns what is true of
a *window set* rather than of one structure: the host and its adoption rule,
the widget-id space, the `Echo`, the socket drain and the walk of the undo
order. `Editor` keeps the structure, the `Domain`, the `View` and the unit
bridge, and reads the rest through its application; an editor handed none
makes one of its own, so nothing a script writes changes. Several editors can
then share one host, one id space, one drain and one order — the shape a
bundle of subviews over unrelated structures needs.

**The named widget id** is the core's (`WidgetIds`, bound here already as
`clausters_core_web`'s `WidgetIds` and used by `GuiIdAllocator`). What is
*not* ported is the door a view takes: `View.widget(editor, role, showing,
key)`, which asks the application for the id that draws
`(structure identity, role, key)` and gives back the same number on every
redraw, plus the bracket `Editor.draw` puts around a draw (`reset_ids` before,
`retire_ids` after) that releases only what the picture genuinely stopped
drawing. The three built-in views name their widget `"curve"`, `"waveform"`
and `"roll"` respectively, and those spellings are part of the name: two
clients drawing one structure must ask for the same id.

**The publish, which is now much smaller than it was** *(rewritten
2026-09-07)*. `Application.publish(widget, tree, window=…)` sends a
`/gui_def` of the widget it names and nothing else: the host **reconciles**
the tree it is handed against the tree it draws, so a def says what to look
like rather than what to destroy, and deciding how much of the window a
redraw costs is the host's. There is no difference to compute and no record
of what the host is drawing to keep — `clausters_core::guidiff` and its two
bindings are **gone** (core ABI v42), because a difference is only correct
against a copy of the host's picture and no client can hold one.

So what the port owes is a `publish` that forwards to `define` or
`redefine`, and the granularity that goes with it: publish the widget the
edit named, not the window, because the window is megabytes a second of JSON
at drag rates on a large piece. It also owes `GuiHost.redefine` itself, which
is the door for a part and which this client does not have — see the AP track's "Found by use" in
`crates/clausters-gui/PLAN.md`.

It is a **recorded** divergence rather than an accidental one, and it is now
a much cheaper one to close.

**And then the two clients are read against each other, verb by verb.** Not as a
courtesy at the end but as the milestone's second half, because the branch that
produced this gap produced it *invisibly*: the class went missing on 2026-09-06
and nothing failed, no test went red, and it surfaced a day later only because
somebody was writing a wire word and needed the door to send it through. A gap
that costs nothing to have is a gap nothing will find.

What that reading covers, and it is the standing rule's own list:

- **`gui/editing/` module for module** — `context`, `domain`, `echo`, `edit`,
  `editor`, `events`, `points`, `samples`, `view`, and the `application` this
  milestone adds. Measured on 2026-09-06 the surfaces differed by `Application`,
  `ATTR`, `BASE_ID` and `watch` in Python's favour, with `contexts` (behind
  `Editing.of`) and `resolveEditorHost` running the other way as idiom. Every
  difference left after the port is either `idiom` in `docs/bindings.md`'s sense
  or a gap with an entry.
- **`GuiHost` verb for verb** — this is where `redefine` was found missing, and
  finding one that way says nothing about the next one.
- **The examples in pairs.** `clients/python/examples/` against
  `clients/web/examples/`, the same material by the same calls in the same
  order — which is the reading that found four divergences the last time it was
  done and would have found this one.

**Order: after the milestones that are still moving what is being ported.**
Porting a seam that is about to change means porting it twice, which is the
argument `AP0` made when it wrote the seam in Python only, and it held: the
difference this client bound and never called was deleted on 2026-09-07 before
the port could copy it. So this waits on

- **`crates/clausters-gui/PLAN.md`'s AP track** — `AP5`'s remainder, `AP7` and `AP8`. `AP5` is what
  decides what an application still owns; `AP8` is the pass over the packages,
  and a parity reading before the docs and the bindings tables are settled is a
  reading against a moving target.
- **`crates/clausters-document/PLAN.md`'s `O24`** — the three applications. They
  are the first real consumers of this seam, and `AP7` already defers to them;
  an abstraction ported before its consumers exist is ported against a guess.
- **`crates/clausters-gui/PLAN.md`'s open tracks**, where they touch the wire this seam
  speaks.

**Acceptance:** `clients/web/src/gui/editing/application.ts` exists and holds
what the Python one holds; `GuiHost.redefine` exists and does the bookkeeping a
part needs (the names under the old subtree go, the new ones merge into what the
window already had, the handle a script holds stays the one it holds); a page
publishes the widget its edit named through it; the two `gui/editing/` surfaces
and the two `GuiHost` surfaces differ only where `docs/bindings.md` says
`idiom`; the example pairs have been read side by side and whatever that reading
turned up is fixed or written down.

**What landed, 2026-09-09.** `Application` holds what a window set owns — the
host and its resolution, the widget-id space with both its doors, the `Echo`,
the publish and the walk of the undo order — and an `Editor` reads the rest
through it, making one of its own when it is handed none. The named id came with
it: `View.widget(editor, role, showing, key)`, `Editor.namedId`, and the bracket
`Editor.draw` puts around a draw, so a widget still in the picture keeps its
number and only one that is genuinely gone releases it. The three built-in views
name theirs `"curve"`, `"waveform"` and `"roll"`, which are the Python client's
spellings because two clients drawing one structure must ask for the same id.
`GuiHost.redefine` is the door for a part, with the bookkeeping the acceptance
names.

**Three things the branch had grown since this milestone was written came across
with it**, because a port of a seam that has moved is a port of the old seam:
`Domain.payloads` and `Editor.editAll` (a report that means several edits lands
as **one** entry), the whole `MultitrackEditor`/`MultitrackDomain`/
`MultitrackView`/`Sources` module with `multitrackPicture`/`multitrackRead`
wrapped beside the model, and `edit()` opening a piece.

**What the reading turned up, which is why it is the milestone's second half.**

- **A defect, in the *Python* client, found from the TypeScript side.**
  `MultitrackView.build` handed the flat `lanes`/`clips` props to the
  `multitrack` **builder**, whose own arguments are the tuples a script types —
  so the tree it published had one row per number: six lanes named `10`, `one`,
  `96.0`, `False`, `False`, `1.0`. Every test read `view.props` and none read
  the tree that is actually published, which is exactly the shape of gap this
  reading exists to find. Both clients now build the node from the props, since
  the flat form is what `props` has to answer in anyway (a correction rides as a
  `/gui_set`), and both have the regression test.
- **A gap with an entry**: the `clausters.log` **area system** is Python-only,
  and `editing.watch` is one door of it. It is not an editing gap and it is
  written up under "Parity gaps carried from the Python client" below.
- **Idiom, recorded rather than closed**: `contexts` (the page's exposure of what
  the Python client stashes on the structure under `ATTR`), `resolveEditorHost`,
  `quads`/`quintuples` (the tuple helpers the Python client keeps private in
  `guidef`), and — one family rather than twelve rows — `GuiHost`'s socket loop
  (`poll`, `pump`, `read`, `fileno`, `loop`, `looping`, `start`, `listen`,
  `deliver`, `dispatch`, `subscribe`, `unsubscribe`) against the page's
  `onMessage`: a Python client owns a socket and must drain it, a page has an
  event loop.
- **The example pairs** were read verb by verb: the eight that exist in both
  (`bpf`, `edit-curve`, `edit-notes`, `edit-samples`, `patch1`, `patch2`,
  `pianoroll`, `pianoroll-midi`) make the same calls in the same order and none
  touches what moved here. Two are unpaired and neither is this milestone's:
  `multitrack_audio.py` has no page (`G34` owns writing one) and `session.py`
  launches a standalone host, which a page has no counterpart for. *(The first
  of those closed on 2026-09-10: the example is `edit_multitrack.py` /
  `edit-multitrack.html`, one program in two languages.)*


### ✅ W31 - The timeline plays what is under the cursor (port of `C54`) *(closed 2026-10-03)*

The port half of `C54` (`clients/python/PLAN.md`, "A timeline plays what is
under the cursor, and an edit reaches the pass that is running"), which carries
the problem, the shape and the acceptance -- read it there, and do not re-derive
it here.

**`C54`'s first question is answered, and it shrank this milestone**
*(2026-09-08)*. Neither branch it offered was taken: the multitrack editor plays
through the **server's transport** -- a region is a resident reader following
`TransportPos`, and the position is the engine's -- so the audio half needs no
player in any client, and `Timeline` is left exactly as it is for what it was
written for (a bounced pattern, an offline render, a followed transport), here as
there. `clients/web/src/seq/timeline.ts` therefore does not change on that
account.

What is left to port is the **events** half: a region of notes fires voices, so
it keeps a queue -- on `/sched_atTransport` -- and keeps needing a re-cue on a
**locate**, over what is live at that position. That is one structure serving one
case, and if it is written in Rust in `crates/clausters-document` (the standing
rule, and where the arrangement already lives) this milestone is a **binding**
rather than a port. The transport half needs no decision at all: it is OSC, and
`clients/web/src/defs/server/transport.ts` already has the builders.

`clients/web/src/seq/timeline.ts` is the Python module ported **verbatim**: the
same sorted `(beat, item)` list, the same `indexAt`, the same `feed` generator
walking an **index** and sleeping to each onset. So it has the same three faces
-- a queue that holds onsets and no extent, a position stored in terms of the
content, and no way for an edit to reach a running pass -- and one rule in two
spellings is what the standing rule asks for. It lands **in the same commit** as
`C54`: the two files are read against each other, call by call, before either is
believed.

**Acceptance:** the same by-ear pass over `examples/editors/multitrack.html`
that `C54` asks of the Python example -- a clip dragged onto the line enters
from the frame the line is at, one dragged away goes quiet as the hand lifts it
-- plus the same three unit tests, and `./build.sh && ./test.sh` green.

**Where it stands, 2026-09-27:** the port landed with `C54`'s `Timeline` half,
in the same commit and with the same eleven tests; what both still wait on is
written in `C54` ("Where it stands"). The transport half -- an edit heard on a
server transport, and a re-cue that keeps the releases of what is sounding --
landed in both on the same day, with the same four tests.

**Closed 2026-10-03 with `C54`**, and its edit half taken out of
`src/seq/timeline.ts` in the same commit as the Python one: `C54` says what
went, what stayed and why no case was left for it.

## Parity gaps carried from the Python client

- ✅ **The editing seam: an application, and a widget id that names what it
  draws.** Grew acceptance and an order of its own on 2026-09-07 and became
  **`W30`** above, which is where its content lives; closed there 2026-09-09.
  Named here so a reader going through the parity gaps is not left thinking it
  is unrecorded.

- ✅ **MIDI is missing here entirely, not only MPE** *(found 2026-09-19, sizing
  the server's MPE milestone and looking for the web end of the wire)*. The
  Python client has a whole MIDI leg: `base/_midiinterface.py` (the RT/NRT seam
  for MIDI — a `MidiServer` a `Pbind` plays through, a `MidiScore` that writes a
  `.mid` or a MIDI 2.0 clip file offline, a virtual output port live), a
  `MidiReceiver` draining a virtual input port, and `responders.MidiFunc`
  demuxing the decoded messages. This client has **none of it** — not a
  destination, not a receiver, not a responder — and `clients/web/src` mentions
  MIDI only where `midicps` happens to be a builtin.

  So it is not a gap MPE opens; MPE only makes it visible, because the shared
  decoder the server's milestone puts in `clausters-midi` is the piece a page
  would bind too. Naming it here keeps the standing rule honest: a surface one
  client has and the other does not is a defect, and "the port has not reached
  it yet" is a reason for this entry, not for the silence.

  **The order is decided and it is not this milestone's**: the Python client is
  the reference and this one ports it, *after* MPE lands there — so the port
  crosses once, against a leg that already carries per-note expression, instead
  of porting MIDI 1.0's channel model and then porting it again. What the port
  will have to decide, and what makes this an entry rather than a translation:
  the browser has **Web MIDI** of its own, so a page's transport is that API and
  not a virtual ALSA port, and whether the shared decoder reaches it through the
  wasm core or the page decodes with what the browser hands it is the design
  question to answer first. The file writers (`.mid`, the clip file) have no such
  question — they are the same bytes from the same crate.

  **Closed 2026-10-07, and the premise was wrong the day it was written.** The
  MIDI leg had been here for three weeks: **W9** ported it name for name on
  2026-08-27 — `parseMidi`, `MidiScore`, `MidiNrtInterface`, `MidiRtInterface`,
  `MidiServer`, `MidiReceiver` in `src/base/midi.ts`, `MidiFunc` in
  `responders.ts` — over Web MIDI, with the file writers through the core's
  door. The search that found "none of it" did not look in `src/base`, and the
  gap row it left in `docs/bindings.md` repeated the claim. What the entry got
  right is only what it mentions in passing: MPE reached the Python
  `MidiServer` after that port, and the port did not follow.

  So what was actually missing was two things, and both are in now:

  - **The zone.** `MidiServer({ zone, upper, bendRange })` is Python's
    `MidiServer(zone=, upper=, bend_range=)`: the RPN first, each note on a
    member channel with its `bend`, `press` and `slide` ahead of its note-on.
    The design question the entry asked has the answer the file writers already
    gave — through the wasm core. `clausters-core-web` grew `mpeZoneMessages`,
    `mpeExpressionMessages` and `MpeAssigner` over `clausters-midi`, so the
    channel a note takes and the bytes around it are one rule reached two ways;
    the *decoder* is not bound, because reading an MPE stream is the server's
    and the host's work and neither client does it.
  - **`MidiServer.write(path)`.** W9 left it out because "a page has no
    filesystem", which stopped being a reason when a path in a page came to
    name its own storage. It writes through `base/files.ts` like every other
    file verb, awaited where Python's returns.

  Held by `tests/midi-vectors.json`, which gained two zones played by the
  Python client (lower and upper, a bend range of its own, the timbre stated
  and unstated) and is matched event for event by "a zone puts each note on the
  member the reference client puts it on"; by "a score is written at a path,
  as a file or as a clip" on the disk under node; and by `tests/midi.html`,
  which writes its score at a path of the page's storage and reads it back.

- ✅ **The logging area system is Python-only** *(found 2026-09-09, reading the
  two `gui/editing/` surfaces against each other for `W30`)*. `clausters.log`
  gives every area of the Python client a logger a script can arm by name —
  `CLAUSTERS_LOG=gui.editing`, or `watch()` from a script — and the editing path
  says what it did at five joints: an event routed, an entry recorded, a step of
  the pile, a publish, an acknowledgement. The web client has none of it: no
  area registry, no `watch`, and no equivalent of the five lines.

  It surfaced as `editing.watch` missing from one surface, and that is the wrong
  size to fix it at: a `watch` in `gui/editing` with nothing behind it would be a
  second logging mechanism rather than a port. What it needs first is the
  decision a page makes differently — the browser has a console with levels and
  a filter of its own, so whether the areas become `console` categories, a
  `debug`-style namespace, or a registry like the Python one's is a design
  question and not a translation.

  **Why it is worth an entry rather than a shrug:** the trace exists because an
  editing session fails in ways nothing else can see — a gesture becomes a
  payload, becomes an entry, comes back as a correction — and "it did the wrong
  thing after a few edits" is not a bug report anybody can act on. A page has
  exactly the same failure mode and no way to answer it.

- **`boot`/`attach`: ported as far as the browser has the concepts** (closed
  2026-08-05, recorded so a name-by-name diff of the two clients does not
  re-raise it). The audio server takes `--port` now, so a machine runs several
  servers, and the Python client grew the pair of verbs that reach one: `boot`
  (starts a process and owns it, refusing a port that already answers) and
  `attach` (verifies a server is there, reconciles the handle's allocators from
  `/server_query`, takes no ownership). Here: `freeAll` came across whole, and
  `attach` came across as **`Server.open`'s `verify`** — threaded through
  `SessionOptions`, so `Session.connect(url, { verify: true })` — because the
  reconciliation half already existed, `open` having always sized its allocators
  from the server's own answer, and what was missing was the refusal: a carrier
  open with nothing behind it warned and carried on. `boot` has **no counterpart
  and needs none** (a page has no process to spawn), and neither do the terminal
  verbs `clausters stop|panic|status`, a page not outliving its client.

- **The introspection records print as data, not as lines.** `Tree` here has a
  `toString()` that draws the tree, and the Python client now gives the same
  treatment to every other record: `NodeInfo`, `BufferInfo`, `DefInfo`,
  `ControlInfo`, `UgenInfo`, `UgenInput`, `NodeMap` and `ServerInfo` each print
  one readable line (`buffer 0: 1024 frames x 2 ch @ 48000 Hz`,
  `beep (synth): freq=440 kr`, `1001 beep  freq=440 amp<-c3`), and `Tree` draws
  a synth by printing its own `NodeInfo` so the two cannot disagree. Here the
  records are **interfaces, not classes**, so they cannot carry a method: the
  port is a set of free formatters in `defs/info.ts` — `formatNodeInfo(info)`
  and friends, or one `describe(record)` — with `Tree.lines` calling the node
  one, keeping the same single-source property. Do it when the def layer is
  next opened; the strings above are the reference output.

  ✅ **Done 2026-08-25**, as eight named formatters rather than one `describe`
  (an interface has no tag to dispatch on, so a single entry point would have
  had to guess a record's kind from its fields). `Tree.lines` calls the node
  one. What holds the *wording* together is not a shared core — there is none
  under a printed line — so it is frozen the way everything else across the two
  clients is: `gen-info-vectors.py` writes what the Python records print for 24
  cases and `info-parity.test.ts` asserts these produce the same text.

- **The four `guidef` helpers that look missing and are not** (checked
  2026-08-02, recorded so the next diff of the two modules does not re-raise
  them). A name-by-name comparison of `gui/guidef.py` against `gui/guidef.ts`
  reports `correlation`, `lissajous`, `peaks_cache_file` and `samples_to_file`
  as Python-only. The first two are **reachable here already**, through the
  wasm core rather than the GUI module (`correlation` and `lissajous` in
  `core/clausters_core_web.d.ts`) — the capability is present, only the door
  differs, and re-exporting them from `gui/guidef.ts` for symmetry is a taste
  question, not a port. The other two write a local file for the host to map
  (`waveform(path=…)`, `waveform(cache=…)`), which a page has no equivalent of
  and should not: the browser's bulk path is the blob/`ArrayBuffer` one, which
  W10 shipped. So: nothing to port, by design in both halves.

- **`gui/__init__.py`'s `set_ambient_host`/`ambient_host` have no counterpart
  here, and W18 did not change that.** They exist so the Python client's
  ambient visual verbs (`clausters.plot`, `clausters.scope`) can resolve a host
  without being told one, ahead of their boot-a-process fallback. This client
  has neither verb nor fallback, and W18 gave the GUI leg a better owner than a
  process-wide registry: `session.gui()`, which wires the host to *its*
  session's engine. So the registry ports with the visual verbs, which is
  **W23** — it carries `set_ambient_host` along with `scope.py` and as much of
  `plot.py` as runs without an offline drive.

- **The server abstractions are complete, checked symbol by symbol**
  (2026-08-03, over `Server`, `Node`/`Synth`/`Group`, `Bus`, `Buffer`,
  `SynthDef`, `FaustDef`, `GraphDef`). W22's port closed the last two names
  (`Buffer.readInto`/`Buffer.write`). What a fresh comparison still reports, and
  why each is not a gap: `Server.boot`, `Server.args` and `Server.shm` are
  process- and segment-shaped, so a page has no counterpart; `Server.render` is
  **W13**'s; `Server.sample_clock` is `sampleTimebase()` plus `defs/clocksync.ts`,
  the rename W21 recorded; and `plotDef()` on the three def classes waits on the
  patch model, which is **W24**'s. Everything else matches name for name, with
  the TypeScript-only additions being option bags and type aliases the language
  needs (`Placement`, `BufferLike`, `ServerSizing`).

- **The widget props are their own manifest, and it is `docs/gui-props.md`.**
  That table compares all three surfaces — the host, the Python builders, this
  client's option types — and `clients/python/tests/test_gui_props.py` fails on
  a difference it does not name. A prop added here or there is checked against
  it, so this plan does not carry a second list of them.

  **Closed 2026-09-12** (`e1372cb2`, "clausters.log, ported"): `base/log.ts`
  carries the area registry and `watch`, the environment list arms areas by
  name, and `gui/editing/trace.ts` is the `gui.editing` area.

- ✅ **Patterns compose by operators, and the library is sc3's** *(opened
  2026-10-10, ported with each milestone and closed with `C74` the same day; the arc is `clients/python/PLAN.md`, "Patterns: SuperCollider's
  operators and library", `C70`-`C74`)*. Neither client has it today, so it is
  ported milestone by milestone rather than after the arc. The shape this port
  follows: a pattern is a lowercase function written as `pattern(function*)`
  over one generic `Pattern` (`pseq([1, 2, 3]).add(60)`), the generator
  receiving the incoming event as `const inval = yield value`; `Pattern` and
  `Stream` extend `AbstractObject` through the same
  `unop`/`binop` hooks a graph uses, so composition is by method
  (`pseq([74, 60, 46]).add(2)`, `.linlin(...)`, `.blend(b, frac)`), each step
  calls the `base/builtins` function of that selector, and a list function
  (`normalize` and its table) refuses a pattern exactly as Python's does. The
  classes, their names and their argument order are Python's; sc3's
  `@pattern` decorator is `pattern(function*)` here. The modules are the same
  families, each class carries the same brief description and kind (value or
  event pattern) in TSDoc, and every pattern stays iterable through
  `[Symbol.iterator]` (`for (const x of pat)`), as Python's `for x in pat`.

### ✅ W21 - The module tree mirrors the Python client's *(done 2026-08-03)*

Not a milestone the plan foresaw, and it earns a number for the same reason
the server's R track does: what shipped between W1 and W20 was placed
correctly *within* a module, and two modules had grown past the point where
their Python siblings had already been split. The reference client leads, so
this is the port of a shape, not a new one — no symbol moved out of its public
path, no behaviour changed, and the whole point is that a reader who knows one
tree finds the other.

- **`defs/server.ts` becomes `defs/server/`** (the port of server R6): the 896
  lines that spanned the connection, the raw OSC, the requests, the
  configuration, the queries and the subscriptions become the handle itself in
  `index.ts`, with `options` (the sizes it is built against and the
  `ServerInfo` it reports), `queries` (`queryDefs`/`queryBuffers`/`queryUgens`/
  `queryInfo`/`queryTree`/`groupAt`/`dumpGraph`) and `streams`
  (`streamBuses`/`streamTaps`) beside it. **Mixins rather than collaborators**,
  exactly as in the Python package and for the same reason: `server.queryTree(…)`
  is the same call it was. TypeScript has no multiple inheritance, so the
  composition is the language's own mixin recipe — the mixin methods declare
  `this: Server` and their prototypes are copied onto `Server` — which is what
  keeps both the runtime path and the type.
- **The timeout is the handle's**, `server.timeout`, the second half of R6 that
  the plan already recorded as owed here: 26 copies of `timeout = 5.0`
  (plus the `10.0` and `30.0` in `Buffer`) became `timeout?: number`, and an
  absent one now resolves in the two places that consume it, `awaitReply` and
  `requestBatch`. `server.timeout = 30` is one assignment instead of an
  argument at every call site. One behaviour change rides along and is
  deliberate: `Server.open`'s sizing probe waited 2 s and now waits the
  handle's default (5 s, the Python client's), because one number naming one
  thing is worth more than the shorter wait on a server that is not answering
  — the probe already degrades to the compiled defaults, and it is awaited, not
  blocking.
- **`defs/ugens.ts` becomes `defs/ugens/`** (the port of server R8): 1811 lines
  that were one long list with the families marked by a comment become
  `graph` (the node, control and channel-list types, plus the fused
  arithmetic), `osc`, `filter` (filters, delays, smoothers), `pan`, `io`,
  `buf`, `trig`, `demand` and `env` — **the same names the Python package
  uses, with each callable in the module its Python sibling is in**, so the
  two catalogues diff family by family. Everything is re-exported from
  `index.ts`. The one helper crossing modules, `isList`, is exported
  `@internal` the way the Python package shares its underscored helpers.
  (**W6** completed the set with the tenth module, `spectral`, and widened that
  last point: `sources`, `channelBinop`/`channelUnop` and the two operator
  tables cross modules the same way now, each for the same reason its Python
  underscored sibling does.)
- **`defs/tap.ts` folds into `defs/server/options.ts`**, where the Python
  client keeps `DEFAULT_TAPS`: how many rings exist is the server's property,
  and a one-constant module was a file the Python tree does not have.
- **`defs/clocksync.ts` is extracted from `Server`.** The one placement gap a
  file-name comparison does not show, since it is about where methods sit: the
  sample-clock tracking — the anchor round trip, the warmup, the tracking
  interval, the model, the teardown — was inlined in `Server` where the Python
  client has a module of its own, with one class per carrier and a common
  surface (`anchor`/`warmup`/`track`/`untrack`/`now`/`rate`/`timebase`/`close`).
  It is now `WsSampleClock` and `EmbedSampleClock`, named for the carriers the
  way `UdpSampleClock` and `EmbedSampleClock` are there — the in-page engine
  *is* an embedded server (the `synth,embed` build, reached through the embed
  door), and what makes its counter readable with no round trip is that it
  runs in this process, not that a page holds one of it. `Server.sampleTimebase()` is
  what it reads as in Python: resolve the carrier, warm it up, keep it. **One
  difference is the carrier's and is written next to it**: the Python tracker
  opens its *own* UDP socket so `/clock_query` never contends with the command
  socket, and a page has one WebSocket to a given server, so this tracker rides
  the `Server`'s connection. The model is untouched — the anchor is still the
  midpoint of a measured round trip.

The two families the split left empty are both written now: `ugens/spectral.ts`
was **W6**'s and `server/transport.ts` **W22**'s. The
Python modules with no counterpart at all are unported *features*, not
misplaced code, and each is already owned: `defs/boxes.py` came with **W7**
(`defs/pv_expr.py` with **W6**, which is what `pvKernel` takes),
the MIDI half of `responders.py` (**W9**, ported 2026-08-27),
`session.py`/`play.py`/`base/main.py`/`base/environment.py`/`defs/_wire.py`
(**W18**), `render.py`/`defs/asdef.py` (**W13**), `gui/editor.py`/
`gui/transport.py`/`gui/notation.py` (**W16**'s named track, whose first leg —
`form/` — is ported), `defs/patch.py`
(unclaimed), and the launcher/IPC/CLI/config set (`launch.py`, `ipc.py`,
`_cli.py`, `config.py`, `_midi.py`, `_libpath.py`), which is a process-shaped
surface a page has no counterpart for.

**The names the arrangement's port must use, decided on the Python side
2026-08-18 rather than re-derived here.** The three primitives that collided
with something else were renamed at every end at once, so a TypeScript port
starts from the new vocabulary and never has to migrate: `Clang` (was `Event`,
which the arrangement's element and `clausters.seq.Event` both answered to),
`Vector` (was `Buffer`, the name of the server resource it wraps) and
`Aggregate` (was `Group`, which is scsynth's node-tree group). The `kind`
strings of the saved format moved with them - `"clang"`, `"vector"`,
`"aggregate"` - so `document-vectors.json` already carries them and a port that
spells one the old way fails the parity suite rather than drifting. The rationale
is in `docs/decisions.md`; `grouping`, `Sequence`, `Segments`, `Track` and
`Generator` are unchanged.

**The shape the arrangement's port must follow on the clip, decided on the
Python side 2026-08-18 rather than re-derived here.** The `clip` builder already
carries the props (`start`, `loop`, `fit`, `layer`, `hidden` — the props test
would fail otherwise); what has no TypeScript counterpart is the **editor** that
answers them, and these are the three payloads it will have to route:

- `"clip" offset dur start` — the third argument is the window's head, and a
  **trim** moves all three. It is one edit, not two: where a clip sits is its
  placement's and what it reads is its element's, so the Python editor states
  the result of the whole gesture as one `setmembers` over the parent's members
  (a member carries both) rather than a `place` plus a `configure`, or one undo
  would leave a clip showing frames it does not play.
- `"split" t` — the second half is **built by the client** (same material, a
  window that begins where the first stops) rather than left for a projection to
  invent from the document node, and it is stamped with an id before any
  conversion sees it.
- `"join" id…` — only the run of windows onto **one** buffer that continue each
  other; anything else is an element reading several segments, which the
  arrangement has no element for and which the Python editor refuses by name
  (`clients/python/PLAN.md`, Found by use).

**The shape `gui/notation.py`'s port must follow, decided on the Python side
2026-08-09 rather than re-derived here.** The reference is a *package* now
(`clausters/gui/notation/`), split by what each part knows, and the port takes
the same four files because the seams are the layer's, not Python's:
`engraver` (the loaded document, the one-shot engrave, the SVG-to-display-list
adapter and the page-replacement payload), `mei` (the client's own sequencing
data reduced to a *voice* and handed to the shared encoder), `view` (wrapping a
page in a scroll, and the transport that plays it) and a private `_abi` for the
two shapes both native callers share. Two things are not negotiable in the
port. **The engraver lives on the client and the walk lives in the core**: a
page reaches the host as the same display list Python sends, so the host's
`score` renderer is reused rather than re-implemented — which is the whole
reason this is portable at all. And **`mei` is the seam**, not an
implementation detail: the client half reduces to the voice, the shared half in
`clausters_core::notation` lays it out into barred, tied measures, and richer
encoding (tuplets, voices, spelling, articulations) extends the *shared* half,
so both clients gain it at once. This lands with **W16** and not before.

**What that seam grows into is decided, and this port follows it rather than
re-deriving it** (`crates/clausters-gui/PLAN.md`, "N track — notation: the score model").
Richer encoding turned out not to be four refinements of `mei` but a **model**
in `clausters_core::notation`: two durational structures that do not contain
each other — the metric layout, which does not sound and is addressable, and
flat voices of content — with durations as exact rationals, ticks demoted to one
edge conversion among ms and MEI values, and a vocabulary of operations over both.
Two consequences reach this package directly. **The algebra's client surface is
settled once, in `N1`, for both clients** — value or handle semantics, a symbol
per operation or the operation as data, who owns intermediates, how much sugar a
language may add — so nothing here chooses it separately; if the operation
crosses as data, `tests/bindings.rs` sees one symbol and no verbs, and the
**operation catalog contrasted against both clients** is what keeps the two from
drifting, exactly where the props manifest could not see a divergence. And the
line that keeps one algebra holds here too: *what computes is Rust, what names
is the shell* — `a + b` and a slice syntax are idiom while they assemble an
operation, but resolving a measure range to a span of rational time is
arithmetic against the grid and belongs to the core, or the two clients round
differently across a meter change.

**The blocker moved on 2026-08-21, and it moved the port's shape with it.**
verovio 6.3.0 is released and published on npm as an Emscripten build, and its
exports are the *same C wrapper* `clausters-notation` binds
(`emscripten/exports.txt`: `_vrvToolkit_edit`, `_editInfo`, `_getMEI`,
`_renderToSVG`, `_redoLayout`, …) — so the page can have an engraver without
this repository building one, and `third_party/verovio.pin` can take the 6.3.0
tag its own note reserved for it. What that does **not** license is a
TypeScript re-implementation of `clausters_notation::Score`: its edit cycle
(edit → commit → reload) and its undo stack of MEI snapshots are *logic*, and
there is one of it. So the port's shape is that **`Score` moves down into
`clausters-core`**, generic over a small toolkit port (load, render, `getMEI`,
edit, `redoLayout`, timemap); `clausters-notation` implements that port over
libverovio for a native caller, and a page implements it over the Emscripten
module's exports. The SVG walk and the MEI encoder are in the core already,
which is what makes this a move rather than a rewrite.
`docs/bindings.md`'s notation table recorded "not built for wasm" on every row,
and that table is now rewritten.

**Landed whole on 2026-08-21.** The pin is the 6.3.0 release; `Score` is
`clausters_core::notation::Score` over an `Engraver` port (`clausters-notation`
implements it over libverovio, `clausters-core-web` over a JS object); the
engraver is built for wasm by `third_party/build-verovio-wasm.sh` **from the
same pin and with the same importer options as the native library**, which is
the decision `docs/decisions.md` records — one *build*, not one version, because
a build is what decides which formats a client reads. The shell is
`src/gui/notation/` at the reference's four paths, and
`tests/notation-parity.test.ts` engraves the Python client's fixtures through
the browser stack and compares the page primitive by primitive, before an edit
and after one.

Its `transport` half landed the same day, with `gui/transport.ts`: the page
plays, `examples/editors/score.html` has the bar `score.py` has, and the layer is
whole.

One rule the `gui/transport.py` port must carry, since the reference learned it
after the list above was written: **a drained scan is not the end of the piece.**
A `Playhead` runs out when it renders its *last item*, and that item is still
sounding — parking the cursor there jumps the line to the end while the sound
goes on. The reference keeps a *tail* (the clock beat and timeline beat at the
drain), reports a position across it, counts it as playing, and parks only when
it reaches the `extent`. The primitive it reads is already here:
`Playhead.scannedAt`, the clock beat the scan last woke on.

**What is left, checked module by module and symbol by symbol against the
Python tree, so the next comparison does not re-derive it.** Every ported
module now sits at its sibling's path; these four differences remain, and each
is a decision rather than an oversight:

- **`SynthExpr` stays in `ugens/graph.ts`** rather than moving to a
  `defs/expr.ts`. In the Python client that module holds three empty markers
  (`Expr`, `SynthExpr`, `FaustExpr`) and the operator surface comes from
  `base/absobject.py`, which the value side shares; here `SynthExpr` *is* the
  math surface, `Signal` extends nothing, and there is no `base/absobject.ts`.
  Mirroring the file without porting what makes it a marker would leave a
  familiar name somewhere meaning something else, which is worse than the
  honest difference. It becomes real work when the abstract-object base is
  ported — and that is also what would let the value and graph sides share one
  written expression, as they do there. **W6 took the first step** rather than
  the whole one: `SynthExpr` gained an operand type parameter, so `PvExpr`
  composes the same vocabulary over its own operands without a second copy of
  it, and the class it hangs off is still the graph's.
- **Two modules keep a different name for the same role**, both declared in
  this plan's architecture sketch since W0: `base/osc.ts` is `base/_osclib.py`
  (the byte layer) and `base/connection.ts` is `base/_oscinterface.py` (the
  carrier seam). The Python names carry a leading underscore, which is that
  language's privacy marker and means nothing in a package whose surface is
  its `exports` map; renaming to match would import a convention rather than a
  structure.
- **There is no `_native.ts`.** Python's `_native.py` is a hand-written ctypes
  binding — 1065 lines of signature declarations — and the TypeScript door is
  generated by wasm-bindgen (`core/clausters_core_web.d.ts`), so there is no
  binding module to mirror. What the Python client reaches as
  `_native.beats_to_secs(...)` this one re-exports from the module that uses
  it (`base/timebase.ts`, `base/builtins.ts`, `base/core.ts`).
- **`errors.ts` is a browser-shaped subset with one addition.** Python's
  hierarchy carries `LibraryError`/`LibraryNotFoundError`/`LibraryFeatureError`/
  `AbiMismatchError` (loading a native library), `SegmentError`/
  `CommandRingFull` (the shared-memory transport) and `RenderError` (the
  offline drive) — none of which a page can reach, the last of them until
  **W13**. Missing and portable: `ServerError`. Added here and absent there:
  `AllocationError`, because Python raises a bare `RuntimeError` when an
  allocator is exhausted and JavaScript has no such class, so a named one is
  the idiomatic equivalent rather than a new concept.

`data/` has no Python counterpart at all, by design and not by omission: it is
the script's own reading path, which exists because a page draws its own
canvas (**W10** records the whole rationale).

**Verified:** `./build.sh && ./test.sh` — the 166 `node --test` cases and the
nine headless-Chrome acceptances, unchanged and green, which is the whole
claim: nothing moved that a caller can see.

### ✅ W22 - The governing transport *(server T1, 2026-08-02; ported 2026-08-03)*

The server gained a transport that freezes a governed subtree sample-exactly,
and the Python client is the reference. None of it exists in TypeScript yet;
it lands as `defs/server/transport.ts`, the `ServerTransport` mixin the Python
package already has and W21 left a slot for. The shape to follow:

- `transportGroup(group: number | null)` — `/transport_group`, `null` unbinds.
- `schedAtTransport(target: number, ...messages)` — `/sched_atTransport`, the
  transport-axis counterpart of `schedAt`. The server checks the declared axis
  and fails when it disagrees with its own classification, so surface that
  failure rather than swallowing it.
- `transportState()` grows two trailing fields, `group` (or `null` when `-1`)
  and `transportSample`. Both are always there — every server reports them —
  so read them straight, the way the Python client does.
- `Transport.resume()` **distinct from** `play()` — MIDI's continue against
  start. Play re-renders from a position; resume continues the frozen sound and
  must not call the source again. A governed `pause()` freezes the clock and
  sends `/transport_stop` instead of stopping the playhead.
- The shm reader follows ABI v6: the transport clock sits at header offset 48,
  in what was reserved space, so no existing offset moved.

The mixin this milestone adds is `ServerTransport` — `transport`,
`setTransport`, `transportState`, `transportGroup`, `schedAtTransport`,
`transportPlay`, `transportStop`, `transportLocate` — the third of the three
the Python package composes, and the only one W21 could not port because the
surface underneath it does not exist here yet.

**What shipped.** The mixin as sketched, at its sibling's path
(`defs/server/transport.ts`), plus the half of the feature that is not on the
`Server` at all.

- **`ServerTransport`**, the eight methods above, composed onto `Server` beside
  `ServerQueries` and `ServerStreams`, so `server.transportStop()` is the same
  kind of call `server.queryTree()` is. The two records it reports are
  interfaces — `TransportGrid` and `TransportState` — where the Python client
  returns a tuple and a dict; `group` is `null` rather than `-1` and
  `transportSample` is read straight, both of them always present. Everything
  that waits is a promise, as everywhere else in this client.
- **`TempoClock.freeze` / `thaw` / `frozen`**, which the sketch did not name
  and the feature does not work without: a page whose server froze would keep
  advancing beats and scheduling into a piece that is not moving. Only
  `beats()` consults the freeze, exactly as in the reference client — what was
  already scheduled stays scheduled and lands in the server's frozen queue, so
  the exactness is the engine's rather than the page's.
- **`Buffer.readInto` / `Buffer.write`**, found by the completeness pass this
  port ran over the server abstractions rather than by the transport itself:
  the two `Buffer` commands that address the *server's* filesystem, the last
  names missing from `Server`/`Node`/`Bus`/`Buffer`/`SynthDef`/`FaustDef`/
  `GraphDef` once `plotDef` (**W24**) is set aside. Like `Buffer.read` they
  mean something only against a native server; the in-page engine has no
  filesystem, and a page saving what it read downloads a blob instead.

What the sketch names and this milestone does **not** port: `Transport.resume()`
is `clausters.gui.transport`, part of the editor track (**W16**), and the shm
reader has no browser counterpart at all. The *joining* half of the advisory
transport — `clock.joinTransport`, a `Playhead` following the broadcasts,
`session.joinTransport()` — stays **W12**'s, and the book's transport chapter
says so in prose rather than leaving the reader to find out.

**Verified:** `./build.sh && ./test.sh` — 181 `node --test` cases (four new: the
grid defined, read, rolled and located; a governed group freezing the transport
clock; `/sched_atTransport` accepted for a governed packet and refused for one
that is not; and the buffer written to a file and read back into another — the
first three against a real `clausters --ws` server) and eleven headless-Chrome
acceptances, the new one being `tests/transport.html`: a drone inside a governed
group, **asserted audible** while rolling (0.200), silent when the transport
stops (0.000) with the transport clock held at 30720 samples and the node still
in the tree, audible again on the resume, and thawed by unbinding — with the
page's clock frozen and thawed alongside, its beat held and the pause not
charged to the piece. Example: `examples/transport/freeze.html`, the port of the
Python client's `freeze.py` — a generative texture frozen mid-gesture
and continued, driven end to end in a browser. Book chapter: "The transport: a
shared grid, and a piece that freezes".

### ✅ W23 - `scope`: the live views *(done 2026-08-03)*

*Deferred out of W18*, which ported the ambient environment and the `play`
verb and left its visual siblings — `clausters.plot` and `clausters.scope`, the
two Python modules W21's inventory found with no milestone at all.

**Half of this slot shipped with W13, and the reason is worth keeping.** This
milestone was written to port `plot` with only the three legs that run live,
because the other three needed an offline drive that did not exist; W13 built
that drive and the split stopped being real — a `plot` missing its headline use
(looking at what a def produces, with no server and no audio device) would have
been a verb shipped to be finished twice. So `plot.ts`, all six legs, and the
`setAmbientHost`/`ambientHost` registry both verbs resolve through went out
with the drive. What is left here is `scope`, which is what the title now says.

The sketch below is kept as written, minus what shipped; one of its
observations is **obsolete** and marked so.

- ~~**`gui/index.ts`: `setAmbientHost` / `ambientHost`.**~~ *Shipped with
  **W13*** (`gui/ambient.ts`), since `plot` is what consumed it. The ladder is
  the reference client's: *registered host → the current (else default)
  session's host when one is up → one the module opens and owns*, the third
  rung being `GuiHost.page()` because a page has no process to boot.
- **`scope.ts`, whole.** It is pure GuiDef assembly over a resolved host, so
  nothing blocks it: the three views (`signal` the triggered oscilloscope,
  `phase` the goniometer, `spectrum` the live FFT), the per-view defaults and
  labels, and a `ScopeWindow` handle with `set`/`close`. The one requirement
  that does **not** port is Python's shared-memory check — it refuses a server
  with no `shm` because the native host reads the taps out of that segment, and
  the browser host has no segment to map and streams them over its own server
  leg instead. Python's own comment beside that check already says so.
- ~~**`plot.ts`, three legs of six.**~~ *Shipped with **W13**, all six.*
- ~~**The `Env` leg reaches the same math by another door**~~ — **obsolete, and
  the reason is the interesting part.** The argument was that Python renders an
  `Env` through an NRT `EnvGen` so that what you plot is what the engine plays,
  that a page had no NRT, and that `/buffer_gen "env"` would reach the same
  core function by another door — so the property would survive and the door
  would need writing down. With W13 the premise is gone: the page renders an
  `envGen` offline exactly as the reference client does, so there is no second
  door and nothing to record. What replaced it is stronger than the note would
  have been — the parity vectors compare the *drawn curves* of both clients.

**Acceptance:** `scope()` opens each of its three views on a live in-page
engine and follows a signal (asserted on the host's own drawing, as `data.html`
does), and finds the ambient host with none named — a session's when one is up,
the page's otherwise. (`plot`'s half of this acceptance is met: `tests/plot.html`
draws all six kinds and `score-parity.test.ts` matches the Python client's
rendered envelopes.)

**What shipped.** `src/scope.ts` at its sibling's path and with its sibling's
surface — the `scope(bus, {...})` verb, its three views and a `ScopeWindow`
with `set`/`close` — and, exactly as the sketch said, nothing blocked it: it is
GuiDef assembly over a resolved host, sharing `plot`'s own ambient ladder
(`resolveHost` is `@internal`-exported the way the reference client shares
`plot._ambient_host`). Three things are worth carrying forward:

- **The shared-memory requirement did not port, and the sketch was right about
  why.** The reference verb refuses a server with no `shm` because the native
  host reads the taps out of that segment; the browser host has no segment to
  map and streams them over its own server leg. So this module has no check
  where its sibling has one, and that absence is written into the module's own
  header rather than left to be rediscovered.
- **A live `set` never reached the host, and had not since W13.** The window
  handles document their props the way the builders take them (`freqScale`,
  `windowMs`) while `GuiHost.set` sent each key verbatim, so every camelCase
  prop was a `/gui_set` the host ignored — silently, since an unknown prop is
  not an error. `examples/buffers/offline-render.html` had been calling
  `win.set({ freqScale: "log" })` into the void. The conversion now happens at
  the door, which is where the package's standing rule already lives: the
  options are TypeScript's, the props are the wire's. Fixed in its own commit,
  ahead of this one.
- **What the acceptance does *not* assert, and why it cannot.** The plan asked
  for the trace to be checked "on the host's own drawing, as `data.html`
  does", and that is not available here: `data.html` counts ink on a 2D canvas
  the *script* fills, while the host draws on a wgpu surface whose buffer is
  not readable after a frame — and the numbers behind it cannot be read from
  the script either, because on one page the host and the script are a single
  client and a script-side tap subscription would take the host's own away (the
  ring clash W10 records, fixed in server **M31**). So the page asserts the
  widget the host built, that its surface took a size, and the live `set` and
  `close`; the arithmetic behind the pixels is asserted natively, in
  `clausters_core::{oscil, spectrum}`, which is the one place both clients read
  it from. Worth revisiting once M31 lands and a page can hold two readers.

**Verified:** `./build.sh && ./test.sh` — 232 `node --test` cases unchanged and
seventeen headless-Chrome acceptances, the new one being `tests/scope.html`:
the three views on a live stereo tone, each reported back by the host as the
widget its view means, the two arguments that are refused rather than coerced
(a third channel on the phase view, an unknown view), `set` retuning the open
window and `close` freeing it, and the ambient ladder resolving with none
named and yielding to a registered host. Example: `examples/views/scoping.html`, the
port of `scoping.py` — where the Python one is a timed tour that opens each
window alone, the page puts the three side by side and leaves the knobs under
the reader's hand. Book: the verbs chapter is `play, plot, scope, render` now.

### W25 - The notebook front end is a client of the package - moved with W19

Landed 2026-08-04 and left with the rest of the notebook track on 2026-08-05.
Its lasting result stayed: a front end embedding this package boots through
`newGuiHost`, starts the engine through `engine()`, holds a `Session` that owns
both, and lets the client own the canvas policy — rather than hand-wiring the
pair and tearing them down in an order of its own. Two leaks it closed stayed
too: a session with its own engine used to leave its wasm host and GPU device
behind on `close()`, and `guiHost()`'s drain interval had no disposer.

What went with it: `src/notebook/client.ts` and the esbuild bundle behind it,
`newGuiHost`'s `wasm` and `engine: null` options, `setTickWorkerUrl`,
`ClaustersServer.onQuit`, `Session.adoptGui` and `tests/notebook.html`. The id
share stayed, being a property of the server's id model rather than of this
carrier — `tests/share.test.ts` and `clients/python/tests/test_id_share.py`
still pin it. See `clients/jupyter/ISOLATION.md` on the `jupyter` branch.

### ✅ W26 - The client stops drawing: the screen-shaped exports go *(done 2026-08-20)*

**The rule this restores**, and it is the project's own: *everything that is
drawn is drawn by the host.* A client names what to look at and the host draws
it. Somebody who does not want the host is free to do their own arithmetic over
their own canvas, and that is **outside this project** — not a surface this
package supports, documents or keeps in step.

**What went wrong is not a slip, it is this plan's own acceptance.** W10 reads
"a TS app reads a control bus and a buffer over either carrier and **draws them
itself** (a canvas the script feeds, not a host-fed widget)", and it was written
when that was the only way to prove the numbers arrived: the host driver landed
2026-07-26 (W2), W10 closed 2026-07-28, and `scope`/`plot` — the verbs that open
a host window, the exact mirror of `clausters/scope.py` and `plot.py` — arrived
2026-08-03, six days later. From that day the hand-drawn path was redundant and
nobody went back for it. What was a scaffold became a norm.

**The cost of leaving it is the one this track exists to avoid**: the two
clients stop being the same client. Everything below exists in TypeScript and in
no other client, so a figure a page can compute is one a Python script cannot,
and each of them is a second implementation of something the host already does.

| Export | What it is | Python |
|---|---|---|
| `Peaks.columns`, `Peaks.column`, `Peaks.levelFor`, `Peaks.levelBucket`, `Columns` | a row of pixel columns, a cell of it, and the level to read it at | absent |
| `joinColumns` | a **drawing rule**: what to ink, not what was measured | absent |
| `scopeFrames`, `scopeWindow`, `ScopeTrace` | the oscilloscope's display framing and trigger alignment | absent |
| `spectrumDb` | the decibel curve a spectrum is drawn as | absent |

Three of the four are already recorded as unresolved by the binding manifest
itself — `docs/bindings.md` carries `spectrum_db`, `oscil_raw_frames`,
`oscil_display_frames` and `oscil_align` as **`gap`** rows, which is that file
saying nobody decided. They close here by **deletion** rather than by a decision.

**What stays, and why it is not the same thing**: building and folding the peak
cache (`Peaks.build`, `toBytes`/`fromBytes`, `writeBuckets`, `updateRange`) is
*data* — the client's own recording path uses it and the Python client has the
same door; `correlation` and `lissajous` are measurements *of the signal*, bound
in the C ABI too; `interleave`/`deinterleave`/`fetchAudio` are plumbing. None of
them is of the screen.

**The core keeps everything.** `peaks::join`, `oscil` and `spectrum` are the
host's arithmetic and the host goes on consuming them — what goes is the
*client door* onto them (`clausters-core-web`'s `columns`, `join_columns`,
`spectrum_db`, `oscil_*`), which exists only for the JS client.

- ✅ Remove the four groups from `src/data/` and their re-exports, and the
  doors under them in `crates/clausters-core-web`.
- ✅ `docs/bindings.md`: the four `gap` rows go with the symbols.
- ✅ **W10's record is amended in place** — its acceptance sentence and the
  bullet that says a page draws its own trace, since a plan that still states
  the old rule is how this comes back.
- ✅ `examples/views/meters.html` is rebuilt on the host's own views, which is what its
  name has been promising: it drew its own oscilloscope while `scope()` and the
  `scope`/`phasescope` widgets sat unused beside it, and its own text claimed
  "this canvas and a host widget on the same bus show the same picture" — a
  claim the page never demonstrated, and one that was *false* for the waveform
  (the wasm door's `columns` widened across boundaries where the host tiles).
- ✅ `tests/data.html` — W10's acceptance — asserts **figures** rather than
  pixels: the same buses, taps and bulk reads, checked as numbers, with the
  drawing left to the host page beside it.
- ✅ `tests/recording.html` keeps its assertions about measurements and loses
  everything of a drawing — stated the way the reference client states it (see
  below), since the reading it used is gone on both sides.

**What shipped.** The table's four groups are gone from `src/data/` and their
doors with them, and the pyramid's reading surface came down to **what the cache
is** — `frames`, `channels`, `baseBucket`, `numLevels`, `toBytes` — and never
what it says. `column` and `levelBucket` went with the rest: they are a
*drawing's* questions asked one cell at a time, and no other client has them.

The bullet about `tests/recording.html` looked, at first, as though it needed
one of them back — its central claim is that the summary the wire told is the
summary the samples build, and with no reader there is nothing to compare. The
answer was to ask how the **reference client** states it, which is the question
this whole track exists to ask: the Python client holds a cache as `bytes` and
compares **bytes** (`peaks_cache_empty` + `peaks_cache_write_buckets` against
`peaks_cache`, one digest — the recipe behind `data-vectors.json`'s
`peaksStream` vector). So the page now reads the whole take back and compares
the two caches byte for byte, which is stronger than the cell walk it replaced
and is the same assertion in both languages. **Verified, not assumed**: the
caches are identical, so the streamed and the measured reduction agree to the
last bit of the mean square, not merely to a tolerance.

What is left of that page's live half is about the **wire**: the reports' own
blobs, read where they arrive, carry the tone at the amplitude the synth writes
and never run past the frontier `written` reports. `tests/data.html` does the
same — the bulk read is checked as the samples the server generated, and the
reduction as a cache over them, with the cache's own bytes pinned against the
Python client in `data.test.ts` where that comparison belongs.

**Not touched**: the parity vectors. `data-vectors.json` covers `peaks`,
`peaksStream` and `stereoField`, all of which stay — so nothing in this
milestone can move a number the two clients compare.

**Acceptance:** no export of `clients/web/src` computes something whose only
use is to put ink on a canvas; `./build.sh && ./test.sh` green; the scope
example opens host widgets; and a name-by-name diff of the two clients' data
surfaces turns up no drawing on either side.

### ✅ W27 - It is written down who draws *(done 2026-08-20)*

W26 removes the surface; this is what stops it growing back. The rule is stated
where each audience reads, in one sentence and with its consequence:

- ✅ **The three books** — the server's, the Python client's, the web client's:
  everything drawn is drawn by the GUI host; a client names what to look at
  (`plot`, `scope`, a `waveform`/`plot`/`scope` widget in a GuiDef). A page or
  a script that would rather draw its own is free to, and that is its own
  program, not a surface this project provides. It lands where each audience
  is already reading about the host: `docs/clients.md`'s "The GUI host: a
  scriptable peer" (a third design choice beside the declarative protocol and
  the GPU substrate), the head of the Python book's GUI chapter, and the head
  of the web book's — where it is said hardest, because a page has a
  `<canvas>` of its own within reach.
- ✅ **`docs/architecture.md`** — the same, as a placement rule beside the
  existing one for shared numeric logic, so the next milestone that needs a
  figure knows where it goes: into the host, or into the core for the host to
  read, never into a client. The bullet above it lost its old test ("signal
  logic belongs here only while the host is the only one computing it"), which
  was written when a client was expected to become a second drawer.
- ✅ **`clients/web/docs/src/data.md`** loses its waveform-drawing recipe and
  points at the host instead *(with W26, since the recipe named symbols that
  left with it)*; its closing section now states the rule rather than only the
  "one implementation of every figure" half of it.
- ✅ The `examples` skill gains the line, since an example is where the
  temptation appears: a page that wants to show a signal opens a host view —
  with the record of what happened when one did not.

**Acceptance:** the rule is findable from either client's book without knowing
it already, and `docs/architecture.md` states it as a rule rather than as a
description of what happens to be true.

### ✅ W24 - The completeness pass *(done 2026-08-25)*

The slot for what the milestone-by-milestone port leaves behind: differences
that are nobody's feature. It is deliberately last and deliberately open — it
gathers loose ends rather than opening a layer, and an entry leaves it as soon
as some other milestone has a better claim on it.

- ✅ **`defs/patch.ts`** — both patch models and the `PatchWindow` handle are
  in (level 1 with the editor, level 2 on 2026-08-22), so this entry is closed;
  what is left of it is an *example*, not a surface: `patch1.py` has no page
  yet, which is W16's queue.
- ✅ **The GUI node becomes a `View` (ports the Python client's C39)** *(done 2026-08-23)*. In
  Python a `guidef` builder no longer returns a bare object: it returns a `View`
  — the GUI's counterpart of a `SynthDef`, a tree a program composes and then
  sends — and the tree is the subject of the sentence, `window(...).open()`
  rather than `host.open(window(...))`. The shape this port must follow, so the
  two do not re-derive it differently:

  - `View` **is the plain JSON object** the builders already produce (a `dict`
    subclass there; here the natural spelling is a class whose instances
    serialize to the same document, or the object itself with the index kept
    beside it — whichever keeps `JSON.stringify` byte-identical to today's).
    Nothing on the wire, in the host or in the serializer changes.
  - The **document is addressed by key and the tree by name**: `v.type`,
    `v.props` / index access for the document; `v.find(name)` and `v.names()`
    for the index. The single bracket cannot mean both, which is the one place
    the Python plan corrected itself while landing.
  - A **duplicate name in one scope is an error** where the tree is built, and
    again where the host client walks a hand-written tree — never a silent
    last-wins, which leaves the shadowed widget drawing and unreachable.
  - A **nested view scopes its names**: a `window`-typed node inside another
    tree keeps its names to itself and is found by its own name.
  - **The root decides.** `window()` is `view()`, one node type rather than
    two: a view with a parent is a component, a view with no parent is the
    window, and any node opens. The wire type stays `"window"` (only a
    window-rooted def becomes an OS window), so a non-window root is **framed**
    client-side in a hugging window — and in a page, where a def is mounted into
    an element rather than opened as a window, the frame is the same decision
    made against the element's box.
  - **The samples are a `source`, not a carrier.** `waveform({data: sig})`,
    where `sig` picks inline-versus-spilled by size and stays addressable
    (`sig.set(...)` rewrites the definitions it feeds and pushes to every live
    widget). The page has no temp files, so the spill is whatever this platform's
    bulk path is — a Blob URL, an OPFS file, whatever `gui_bulk`'s page already
    uses — and that difference is `idiom`, to be written into `docs/bindings.md`
    rather than left as two policies. The threshold and the choice itself are the
    same in both clients.
  - **A control widget is built from the control it drives.** `knob(freq)`,
    `slider(sd["amp"])`, `number` and `toggle` read the control's **name** and
    **default** off it. All three def families answer with one `ControlInfo`
    shape (`sd["freq"]`, `fd["cutoff"]`, `gd["mix"]`, and `.controls`), which is
    the part that must not be re-derived differently. **The range is the
    widget's** — a control is a signal and a port is a name the server takes any
    float for, so neither says how a knob is drawn; only a **Faust** parameter
    arrives with one, from the `hslider` that could not have been written
    without it, and that is the only range on any wire. The props-parity
    manifest treats `control` as *not a prop* (it is a source of `name`/`value`),
    so the parameter is each client's own spelling.
  - **The binding is made against the control.** A control widget keeps the
    control it was built from, the id walk collects `widget id -> control name`
    beside the names, and the handle binds the whole surface in one verb:
    `win.bind(synth)` (a node or a bare id), `win.unbind()`, `win.controls`.
    The widget's *name* is the handle's index and the *control* name is what
    the server is told — usually the same string, and an explicit `name=` wins.
    Still `/gui_bind` underneath, with the one-at-a-time form kept for a bus,
    another widget or an arbitrary address.
  - `view.open({host})` resolves the **ambient** host exactly as `plot`/`scope`
    do. Python made that symmetric with the ambient server: `GuiHost.boot`
    grew `adopt_ambient` (first-wins, the mirror of `Server.boot`'s
    `adopt_default`) and `stop()` gives the registration up. This client's
    equivalent is `guiHost()` / `newGuiHost()`, whose naming is the entry two
    below — the two decisions land together or the page ends up with two ambient
    rules.

  **What landed**, and the four places the port had to decide rather than
  transliterate:

  - `View` is a class whose **own properties are the document**, with the state
    that is not the document in `#private` fields — so `JSON.stringify` writes
    what it always wrote and nothing on the wire moved. `find`/`names` are the
    index, the bracket stays the document key, a duplicate name is refused where
    a tree is built *and* where the host client walks a hand-written one, and a
    nested view scopes its names.
  - **Ids are stamped into a copy**, as in Python: `GuiHost.stamp` returns the
    document, the caller's tree is never written into, and the same subtree
    nested twice gets two id runs for free.
  - `view()` is the root builder, `window` its older spelling, and a root that is
    not a `window` is framed client-side in a hugging one.
  - **The spill is the platform's, and it is the one real difference.** Python
    spills past 2048 floats to a temp raw-f32 file the host maps; a page has no
    temp file, so a source rides a **blob** beside the JSON — and the index is
    assigned by `open` from where the bytes land, which is `blob: 0` ceasing to
    be a correspondence kept by hand. The threshold and the decision are the
    same in both. What is *not* the same: the host has no live door for a blob
    (`/gui_set` takes `data` inline or `reload` for a path), so `Source.set`
    refuses on a spilled source here where Python rewrites the file and
    re-reads it. That gap is the host's and is written in `crates/clausters-gui/PLAN.md`.
  - **A control is told from an option bag by its `default`.** Python separates
    them by position — one positional, the rest keywords — and TypeScript has
    one positional slot, so `knob(freq)` vs `knob({name: "freq"})` is decided on
    the shape: both carry a `name`, only a control carries a `default`.
  - **`sd.control("freq")` where Python writes `sd["freq"]`**: a class here
    cannot take a bracket without an index signature over everything else on it.
    `controls()` is a method for the same reason Python has a property — each
    language's own spelling of one surface.
  - **The port found a defect in the reference client, and the fix turned into a
    design correction**: a `Control` *is* a signal, and `min`/`max` on a signal
    are the binary operators, so a range shipping as attributes of those names
    shadowed them — for every control, since the attribute was `None` when no
    range was declared. TypeScript's compiler refused the override Python had
    accepted in silence. The answer was not to rename the read but to take the
    range off the def entirely: **it belongs to the GUI widget** (C42's own
    entry has it). That is the argument for porting before the examples, twice
    over — the port found the defect, and the defect found the design.

  **The page decision landed whole, and the note that said otherwise was
  wrong.** It first shipped with `open()`-without-an-element still drawing on
  the page's shared canvas, on the claim that a canvas per def waits on the
  host's web front — which **W4 already delivered on 2026-08-08**: the browser
  front has kept one `CanvasSlot` per `window`-rooted def, with its own surface,
  gesture state and visibility flag, ever since. The stale sentence was read out
  of W4's own *specification* text and repeated as if it were open work. Nothing
  in Rust was missing; the client was simply not asking for the canvases the host
  already had.

  So `open()` with no element now appends **a canvas of its own**
  (`page.newCanvas`), the page's canvas becomes the **fallback** for a def fed
  straight through the binding surface — appended the first time that happens,
  so a document whose views all name their place carries no empty one — and
  `WindowHandle.canvas` is where a page points at what it opened, which is what
  three pages had been using `guiHost().canvas` for. `scope.html` reports it:
  "3 canvases sized" where it used to say 1.

- ✅ **`SynthDef`'s roots are `outputs` in Python and `roots` here**, and both
  constructors take `*roots` — a public attribute spelled two ways, found while
  porting the Def view (which reads it). One of the two names is wrong and the
  reference client picks which. **Decided 2026-08-25: `roots`**, and the
  reference client is the one that moved. Its own docstring already said why —
  *"the arguments are the graph's roots, not its output"* — and its own test
  asserts `as_def(send_trig(...)).roots[0].kind == "SendTrig"`, which is not an
  output by any reading. `SynthDef.outputs` is gone rather than aliased (nothing
  ships that name yet), and `patch.py` reads the new one.
- ✅ **What `open()` means where there is no window: the page, the canvas and
  the document** *(named 2026-08-23, with the Python client's API reform; closed
  2026-08-25)*. The
  reform settled the native half — *a view with no parent is a window*, and a
  root that is not a `view` is framed in one client-side, because the wire opens
  an OS window for a `window`-rooted def and nothing else. A page has no OS
  window, so the same sentence has to be finished here, and it is the question
  that has been sitting under `guiHost()` all along rather than a consequence of
  the reform:

  - **A page's `ClaustersGui` bridge owns a canvas**, and a def mounted into an
    element is drawn against that element's box. So the page's counterpart of
    "with no parent" is "with no element": what does `view(...).open()` do in a
    tab — take the whole default canvas, append a canvas of its own, or refuse
    until told where? Every page today says it in two doors at once
    (`GuiHost.page()` for the driver, `guiHost()` for the surface, then
    `page.fit(win.id, el("stage"))`), which is legitimate platform idiom with
    the wrong shape: **the element belongs in `open`**, the way the host does.
  - **`guiHost()` / `newGuiHost()` / `Session.connectGui()` are three names for
    where a host comes from**, and the reform gave the reference client one rule
    for that — `GuiHost.boot(adopt_ambient=…)`, first-wins, the mirror of
    `Server.boot`'s `adopt_default`, with `stop()` giving the registration up.
    The page's three doors are answered by porting that rule, not by renaming
    them one at a time.
  - **Several canvases in one document** is the user's own framing of it: each
    canvas the page's equivalent of one opened view. That is what makes this a
    decision about the *document* the page is, and not only about a default.

  It is one decision with three surfaces, and it is taken **before** the
  example port (W16), because a page written against the current three doors is
  a page written twice.

  **Decided 2026-08-23**, all three:

  - **The view opens itself, and the element goes in `open`.** `view(...).open(el)`
    — the element positional, `host=` by keyword, exactly the shape the reference
    client settled in C41 (`open(*blobs, id=None, host=None)`) with one platform
    argument added. `guiHost().open(tree)` stays as the low-level door it is in
    Python, and `page.fit(id, el)` stops being something an author writes: `open`
    calls it. The two-doors-at-once shape every page uses today
    (`GuiHost.page()` for the driver, `guiHost()` for the surface, then `fit`) is
    what this replaces.
  - **With no element, `open()` makes a canvas and appends it.** The literal
    continuation of the reference client's sentence — *a view with no parent is a
    window* becomes *a view with no element is a canvas* — and it is what makes
    several canvases in one document fall out rather than being a feature. The
    machinery is already there: `newGuiHost` appends nothing, `attach(defId,
    canvas)` takes one per def, and the host's own web front is the one piece
    still singular (`window`/`render`/`current_def` in `host/web.rs`), which is
    the *other* W24 entry and this one's only dependency. Taking the page's
    default canvas instead was the alternative, and it breaks at the second
    view — which is the case this whole entry exists for.
  - **The three names collapse onto the reference's rule** (C47, done
    2026-08-23): `GuiHost.boot(adopt_ambient=…)` / `GuiHost.attach()`,
    first-wins, `stop()` giving the registration up. `guiHost()` keeps its memo
    as *the page's host with the page's default canvas* and becomes the ambient
    one; `newGuiHost()` is its instance counterpart. **`Session.connectGui(url)`
    goes**: it did *connect* and *adopt* in one call, and the reference now has
    both halves separately — the constructor takes a host (`new Session(server,
    clock, gui)`) the way it takes a server, and `attach()` is the connect. A
    session installing a host it did not open is the constructor, not a verb.

  **What was actually left, found by opening every page against it.** The
  *surface* had landed with the `View` port: `newCanvas` was already written,
  with the sentence in its doc comment, and `GuiHost.open(tree, {element})`
  already made a canvas per view and fitted it. What had not moved was
  everything that calls it — so the decision existed and nothing used it.

  - **Every page still opened through four doors**: `GuiHost.page()`, then
    `guiHost()` for the surface, then `el("stage").append(page.canvas)`, then
    `page.fit(win.id, el("stage"))` — with the disposer held in a page-level
    `let unfit`. That is one call in the reference client (`view(...).open()`),
    and it is now one call here: `view(...).open(el("stage"))`, the element
    being the browser's own argument. Nine pages and two acceptance pages
    moved; `panels/two-hosts.html` deliberately keeps the raw bridge, since
    two host instances *is* its subject.
  - **The verbs that open something had no element at all.** `plot`, `scope`,
    `plotDef` and `Editor.open` reached `host.open(tree)` with nothing, which
    is why a page that wanted one of them in its layout had to go around them
    to `guiHost().fit`. They take `element` now (`stage` in the editor's option
    bag, where `element` already means the arrangement's), forwarded and
    nothing more — **no DOM logic left `gui/page.ts`**, which is where the
    canvas is made, measured and followed.
  - **`Editor.open` demanded a host where the reference resolves the ambient
    one**, and so did `openSignal` and `openPianoroll` — a divergence, not a
    platform difference: Python's `_resolve_host` has been there since the
    editor landed. All three take an optional host now and are `async` for it,
    the same reason `View.open` is (resolving the ambient host may have to boot
    it, and a page boots asynchronously).
  - **The element's type is named in the module that owns the DOM**:
    `Stage`, exported from `gui/page.ts`, so the one browser-only argument
    reads as browser-only at every door instead of being spelled `Element` —
    which in the editor is the arrangement's `Element` and collided outright.
  - **The pages said `window(` where every Python example says `view(`.** The
    alias is kept in both clients, so this was not a rename: it was 12 pages
    and three bundle authors written before the root rule, which is exactly the
    drift the "same calls in the same order" rule exists to catch. The books'
    examples moved with them.

- ✅ **`Session.connectGui(url)` is a verb this client invented** — *connect*
  and *adopt* in one call — **dropped 2026-08-23** with the `View` port. The
  reference answered it (C47): a session installs a host it did not open
  **through the constructor**, the way it takes a `Server`, and connecting is
  `GuiHost.attach()` — `GuiHost.connect(url)` here, which is the same verb over
  the carrier this client has. Both halves landed instead of the one that did
  two things: `new Session(server, clock, await GuiHost.connect(url))`, and
  `GuiHost.page`/`connect` adopting the ambient registration first-wins with
  `stop()` giving it up.
- ✅ **Two names the sweep of 2026-08-03 left over**, each too small to own a
  milestone and neither a difference with a reason: `Routine.run(func, clock,
  quant)`, the classmethod that constructs and starts in one call (the instance
  `play` is here, the shortcut is not), and `ServerError`, which W21 already
  listed as missing and portable and which nothing has since claimed.
  **Closed 2026-08-25**: `Routine.run` is here, and `ServerError` turned out to
  have landed already (it is thrown by `Server.open`'s `verify`) — the entry was
  stale, which is what a slot for loose ends is for. Writing the shortcut turned
  up something bigger than either name, now in "Found by use": on this side a
  running clock resumes what it is handed **inside** `play`, so the Python idiom
  the shortcut exists for cannot be written here.
- ✅ **`Buffer.fromSamples` exists here and nowhere else** *(found 2026-08-22
  putting the two composer examples side by side)*. It is how a page installs a
  bounce it just rendered — the in-page engine shares memory, so the samples go
  straight in — and the Python client has no counterpart at all. Its own answer
  is the other half of the same gap: `Buffer.read(path)` (and the `read_*`
  family) reads a file into a buffer, which a tab cannot do because it has no
  filesystem. So the two clients get a take into a buffer by two different
  verbs, and neither verb exists in the other. Two things to decide on their
  merits, not by whichever example needed one first: whether `from_samples`
  belongs in the Python client (`Session.embed` shares memory too, so the
  capability is not the page's alone), and whether `Buffer.read` has a web
  spelling over a **URL** rather than a path. Until then it is the one line the
  composer examples cannot share, and both say so where they diverge.

  **Decided and closed 2026-08-25, and neither half was a platform's.**
  `from_samples` belongs in **both**: the capability is "samples this program
  holds become a buffer", and every carrier can do it — a shared-memory one in a
  single copy, a socket in `set_samples`' blob runs, which is the path the
  Python client has always had. So `Buffer.from_samples` is in the Python client
  (`alloc` + `set_samples` in one call), and `Buffer.fromSamples` stopped
  **refusing** a socket carrier and falls back to exactly that. Same verb, same
  meaning, same result on any leg; only the speed differs, which is what idiom
  means.

  `Buffer.read(path)` is in both already and stays: it is the server opening a
  *file*, which a remote server can do and a page's engine cannot. What a page
  lacks is the file, not a verb — so `Buffer.load(url)` stays web-only and is
  recorded here as idiom rather than as a gap. The composer examples turned this
  up and are what it settles: the page's comment no longer says "in-page only",
  the script's now names its sibling, and both say the same true thing.

- **The general rule this slot enforces**: a name that exists in one client and
  not the other is either a *feature* some milestone owns, or a *difference*
  `docs/gui-props.md` and this plan's parity section record with a reason.
  Anything that is neither belongs here, and the way to find them is the
  module-by-module, symbol-by-symbol comparison W21 did — worth repeating
  whenever a run of milestones has landed.

**A sweep is owed, and half of it is already done.** W6 closed the UGen
catalogue by *doing* that comparison over `defs/ugens/` — the two trees now
report nothing missing in either direction there, beyond the free
`add`/`sub`/`mul`/`div` and `resolveCurve` here and `ugen_input_names` there.
So when this slot is taken up, the catalogue is settled and what is left to
sweep is the rest of the tree; the entries above were found by the sweep of
2026-08-03 and are what that pass left standing, not a fresh reading. Re-read
**W21**'s parity section at the same time — it is the record the sweep writes
into, and it is the one section a later milestone can silently falsify (W6
already did, twice).

**Acceptance:** a fresh comparison of the two module trees reports only
differences that are written down somewhere — a milestone that owns them, a
row in `docs/gui-props.md`, or a paragraph in this plan's parity section.

✅ **The sweep was run on 2026-08-25**, over the two packages' public surfaces
(every `__all__` against every barrel, names normalized across the two
spellings). It reports **36 Python-only names and no unexplained
TypeScript-only one**, and all but two of the 36 are already written down:

- the **carrier objects** (`OscUdpInterface`, `OscTcpInterface`, `OscWsInterface`,
  `OscNrtInterface`, `OscEmbedInterface`, `NetAddr`, `OscScore`) — this client's
  seam is `Connection` and its openers, the shape difference W21 records;
- the **MIDI family** (`MidiRtInterface`, `MidiNrtInterface`, `MidiServer`,
  `MidiReceiver`, `MidiEvent`, `MidiScore`, `midifunc`, `parse_midi`) — ported by
  **W9**, 2026-08-27, name for name; what the port does *not* carry over is
  `MidiServer.write`, and what it adds is `requestMidiPorts` and
  `MidiRtInterface.open`, all three recorded in W9's own entry;
- the **Faust expression** (`FaustExpr`) — ported by **W7**, 2026-08-27,
  together with the `Box` under it;
- the **process** (`ServerOptions`, `launch`, `ipc`) and the **filesystem**
  (`read_soundfile`, `samples_to_file`, the four `peaks_cache_*` file doors) —
  a page has neither, recorded above;
- the **sample-clock spellings** (`SampleClockModel`, `SampleClockTimebase`,
  `UdpSampleClock`), which are `SampleClock`/`SampleTimebase`/`WsSampleClock`
  here — the rename W21 records;
- the **module namespaces** (`base`, `ugens`, `boxes`), which this package
  re-exports from other paths.

The two that were neither: **`YieldAndReset`** (a routine's way back to its own
beginning — a generator cannot restart itself, so the value leaves through an
exception) and **`DEFAULT_TAP_FRAMES`** (`DEFAULT_TAPS` was here without it).
Both are ported, both with a test. The TypeScript-only side is 293 names and
reads as one thing: option-bag interfaces and type aliases the language needs,
which is the difference already recorded — every value export in it either has a
Python counterpart under another spelling or is a page's own (`ANY_PEER`,
`startPage`, the bundle and component doors).
