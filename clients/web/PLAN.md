# Plan - Clausters web client (TypeScript, browser-first)

The TypeScript client: a high-level client of `clausters-server` and of the browser GUI host, the web sibling of `clients/python`. It is its **own package, docs, examples and tests** under `clients/web`, exactly the way the Python client lives under `clients/python` - a parallel client track, not part of the GUI host (`clients/gui`) and not part of the server.

> **Read alongside `clients/python/PLAN.md`.** That plan already covers the *shared* client rationale (the native Rust core, the C-ABI/wasm seam, the "only flat data crosses" boundary, the coroutine-driver-stays-in-the-language rule) and was written from the start "to serve a future JavaScript client too". This file is the web-specific track; it does not restate the shared design, it points at it and records only what is different in the browser. As there, **sc3 is the reference model** for module structure, clock/routine behaviour, events, patterns and OSC/MIDI semantics; deviate only with an explicit reason.

## Why a separate track (and why now)

The browser GUI host (`clients/gui`, milestones G11-G16) runs the `/gui_*` widget system under WebGPU and is driven *through* a small wasm binding surface or over WebSocket. Those milestones deliberately use a **throwaway** in-page harness to test the host; the real, product driver - the thing a web app actually programs against - is this TypeScript client. Keeping it here, separate from the host, preserves the same boundary the rest of the system keeps: the host is a front, the client is a consumer of the protocol, and either can change without dragging the other.

So the web client is one more consumer of the exact same wires the Python client uses: OSC-in-JSON GuiDefs to the GUI host, OSC to the audio server, the same `decode_packet` framing - only the carrier (browser `WebSocket`), the binding technology (wasm, not ctypes) and the host language (TS, not Python) differ.

## Guiding principles

- **Maximum reuse; the browser only adds its I/O.** What is value or time transformation is shared, not re-implemented: OSC assembly/decode, TempoClock arithmetic, the numeric builtins and the analysis kernels (peaks/FFT) come from **`clausters-core` compiled to wasm** (via wasm-bindgen), so the client is **numerically equivalent to the Python client and to the server by construction**. The GuiDef/`/gui_*` protocol and the def specs are the same JSON the Python builders emit. New TS code is confined to: the language-side control flow (generators/async routines), the browser carriers (`WebSocket`, Web MIDI, Web Audio clock, `fetch`), and the ergonomic builder/typed API.
- **The seam is the same as `clients/python/PLAN.md`.** The Rust core owns builtins, TempoClock (queue + arithmetic) and OSC bundle/timetag assembly + sample-clock conversion; the coroutine driver (`function*`/async in TS, sc3-style routines) stays in the language; no Rust callbacks into JS - the loop asks the wasm queue "what's next and when?", sleeps on the browser clock, resumes the routine. "Only flat data crosses" the wasm boundary (typed arrays / numbers / strings in and out, no callbacks).
- **Client/server separation, as in the Python client.** Timing/sequencing/GuiDef authoring is transport-agnostic; only a `Server`/`GuiHost` object knows the connection. The `TempoClock` must not talk to the server (the same rule corrected in the Python client's C4).
- **Browser realities are first-class, not afterthoughts.** WebSocket is the only *network* transport (no UDP, no shared memory, no mmap); since the server's B track, the browser also has a second, process-free carrier — the **in-page engine** (the server compiled to wasm in an AudioWorklet, reached through the B4 package's `server()` singleton) — and the client stays carrier-agnostic above a small connection seam. Bulk data arrives by `fetch`/`/b_getn`; meters/scopes read control buses over the wire; the sample-clock timebase uses the Web Audio clock (`AudioContext.currentTime`). These are the same "async fallbacks" the server/gui plans reserved for the browser.

## Target architecture

A TS package mirroring `clients/python`'s shape — the `src/` module tree at the same relative paths as `clausters/`'s modules, `dist/` reproducing it 1:1, and `examples/`/`tests/`/`docs/` beside them — so a reader who knows one finds the other. This is the **only web directory in the repo**: every browser JS/HTML artifact (package modules, the engine's worklet/loader runtime, examples, test pages, tools) lives here, and the wasm crates stay Rust-only, their wasm-bindgen bundles staged in by `build.sh` (see `docs/decisions.md`, "The web front-end lives in one package"). The layout as it stands after W1 (parenthesized entries are where the later milestones grow):

```
clients/web/
  package.json  tsconfig.json  tsconfig.build.json   # the `clausters` npm package
  build.sh                # wasm builds + staging into dist/ + tsc emit — the one builder
  test.sh                 # type-check + node suites + the page-carrier smoke
  src/                    # ← mirrors clients/python/clausters/, module for module
    index.ts              #   the package facade (the __init__)
    base/                 #   the low-level seam (mirrors clausters/base)
      core.ts             #     the shared core's wasm: one load, the registry
      osc.ts              #     OSC encode/decode over the wasm core (mirrors _osclib)
      connection.ts       #     the carrier seam: WsConnection | pageConnection()
      (clock.ts  timebase.ts  builtins.ts — W3)
    errors.ts             #   the error hierarchy (mirrors errors.py)
    defs/                 #   the def model + server client (mirrors clausters/defs)
      server.ts  node.ts  bus.ts  buffer.ts
      signals.ts  ugens.ts  synthdef.ts  faustdef.ts  graphdef.ts
    gui/                  #   the GUI host driver (mirrors clausters/gui)
      host.ts             #     GuiHost + the per-page guiHost() singleton
      guidef.ts  handle.ts  ids.ts
    (seq/                 #   sequencing (mirrors clausters/seq) — W3
      event.ts  eventstream.ts  pattern.ts  timeline.ts)
    data/                 #   the data paths: what a view reads off the server
      buses.ts  taps.ts   #     the streamed sources (/c_stream, /tap_stream)
      samples.ts  peaks.ts analysis.ts
    (responders.ts        #   OscFunc/MidiFunc dispatch (mirrors responders.py) — W8/W9)
    (session.ts           #   the Session facade — W18)
    engine/               #   browser-only: the in-page engine runtime
      worklet.ts  loader.ts  worklet-shim.ts  server.ts (the server() singleton)
    bundle.ts elements.ts #   browser-only: bundle boot + the custom elements
  dist/                   # emitted src/ 1:1 (.js + .d.ts + maps) + the staged wasm
                          #   bundles engine/ gui-host/ core/ — the _bin/_libs analog
  examples/               # synth.html, sequencing.html, gui-host.html, demo.html,
                          #   engine.html, standalone.html, the bundle components
                          #   (piano/ graph-controls/ document/) and the ports of
                          #   the Python examples (the rest of them — W16)
  tests/                  # node --test suites + parity vectors (osc, def, gui)
                          #   + the browser acceptance pages (client/defs/gui/
                          #   smoke/parity)
  tools/                  # bundle-manifest.py, demo-bundle.sh
  docs/                   # an mdBook (mirrors clients/python/docs), the API
                          #   reference generated from the TSDoc by typedoc
  typedoc.json  .readthedocs.yaml   # that generator, and the RTD build (W17)
```

The wasm `clausters-core` build is **shared with the GUI host** (G11-G16 already needs core compiled to wasm); this client links the same artifact, it does not produce a second one.

## Tooling (decided 2026-07-18, at W0's start; the no-heavy-deps rule)

The repo-wide posture — minimal, user-space, reproducible — applied to the JS toolchain. B4 already established the package's shape (plain browser-native ES modules, wasm bundles as static assets, served as-is); the toolchain must preserve it, not fight it.

- **node LTS under `~/.local`, no sudo** — the same pattern as libfaust. The recipe (kept current in `clients/web/BUILD.md` once W0 lands): download the `linux-x64.tar.xz` of the newest LTS from nodejs.org/dist, verify against `SHASUMS256.txt`, extract to `~/.local/lib/`, symlink the versioned dir to `~/.local/lib/node`, and symlink `node`/`npm`/`npx`/`corepack` into `~/.local/bin` (already on `PATH`). Installed 2026-07-18: v24.18.0 (npm 11.16.0).
- **`typescript` is the only package dependency** (dev-only; v7, the native compiler — a single package, no transitive deps; `@types/node` rides along for the test files, type declarations only). `tsc` does both jobs: **type-checking** (`tsconfig.json`, src + tests, no emit) and **emitting** (`tsconfig.build.json`: `src/` → `dist/` module-per-module, with declarations and source/declaration maps — the browser interface is JS with a type map). Imports between our modules are written with `.ts` extensions and rewritten on emit (`rewriteRelativeImportExtensions`), which is what lets node run the sources directly; the output is the same plain servable ESM the B4 modules were. The dev loop is `tsc -p tsconfig.build.json --watch` + `python3 -m http.server`.
- **No bundler.** Nothing here needs one: the package ships unbundled, the wasm bundles and the worklet module must stay static assets anyway (`AudioWorklet.addModule` and bundlers are a known friction), and the browser loads bare ESM natively. Evaluated and not adopted: **vite** (a dev server with HMR plus rollup/esbuild underneath — tens of MB of dev machinery whose two roles are already covered by `http.server` and `tsc --watch`; revisit only if HMR-grade DX is genuinely missed), **esbuild** (only earns its place when bundling), **vitest** (pulls vite in as its platform).
- **Tests: `node:test`, built into node — zero dependencies.** Node runs `.ts` directly (native type stripping, default since 23.6), so pure-logic tests (codec parity, clock arithmetic, builders) run straight from source with `node --test`, no compile step, no runner package. Browser-only behavior (audio, canvas, the elements) keeps the B-track posture: headless-Chrome smoke scripts with the access-log beacon.
- `typedoc` (the W5 API-reference generator) gets evaluated under this same lens when W5 starts.
- The **Emscripten SDK** (`emcc`, user-space via `emsdk`) is the one heavy addition this lens admits, and it is **W7's**, not the toolchain's baseline: it builds `libfaust-wasm` so a Faust def compiles in the page (`third_party/BUILD-FAUST.md`, "WebAssembly parts" — documented, never built here). It stays out of the JS toolchain proper — nothing in `src/` or the test loop touches it, `build.sh` only stages its output as static assets, and the slim run-time entry never loads them. Evaluated under the same lens when W7 starts, decision recorded then.

## Milestones

Labels (`Wx`) live only here, never in published docs or docstrings - the same rule as the other plans.

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

**Acceptance:** dual — a `/status` round trip through the *same* connection interface over **both** carriers (in-page under headless Chrome with no server process; WebSocket against a native `--ws` server), the parity vectors green under `node --test`, and the package type-checking clean (`tsc`).

### ✅ W1 - Server client + the def model

*(The hold this milestone carried from 2026-07-18 — "waiting for the Python
client review", since W1 is the first milestone to mirror the Python **API
surface** rather than only the wire — was lifted on 2026-07-26: the reference
client's arc had settled, so the mirror could start without turning every
Python change into two.)*

Drive the audio server.

- `defs/server.ts`: the `Server` object - send `/d_recv`/`/d_graph`/`/d_faust` specs, `/s_new`, `/n_set`/`/n_free`, groups, the `/sync` barrier, buses and buffers; receive replies through `responders` (W8 hardens this).
- The def builders (`signals`/`ugens`/`synthdef`/`faustdef`/`graphdef`): start by sending the **same spec JSON the Python builders emit** (reused verbatim), then grow the typed TS builder API for parity, with the Python builders (both def families) as the reference.

**Acceptance:** from a browser page, define a def and play it (`/s_new` then `/n_set`), with `/sync` ordering and an audible/queryable result, **over either carrier** through the same `Server` (the W0 seam: nothing above it names a transport) — a synth def against the in-page engine with no server process, and both families against a `--ws` server (the Faust half is WS-only by nature: the wasm engine is the `synth,embed` build, no LLVM JIT).

**What shipped.** The whole `src/defs/` tree, mirroring `clausters/defs/`
module for module: `server.ts` (reply dispatch, the `/sync` barrier, the three
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
  sizes them from `/server_info`, so the client matches the server that is
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
`examples/synth.html`, the same def and the same code over either carrier, the
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
closing the window frees the subtree. Example: `examples/gui-host.html`, now
the product client rather than the B-track harness — the bound and the scripted
control paths side by side, a `/c_stream`-fed meter/scope loop, the linked
waveform + spectrogram, and one button that swaps the in-page host for a native
one.

Not in scope here, by the plan's own division, and now **W10**: the browser
data paths the heavy views feed on (`/c_stream` decoding client-side,
`fetch`/`/b_getn` bulk, the wasm peak pyramid) and the
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
  (exact - they are the same clock), over a socket it feeds `/clock` anchors
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
- **`/g_queryTree` is not an observation of a schedule.** The reply comes from
  the network-side mirror, which applies each message as it is translated, and
  a note's `/s_new` and its release are sent in the same instant (only their
  timetags differ) - so the mirror shows a scheduled note born and freed at
  once while the engine still has it sounding. The end-to-end suites read
  `/n_go`/`/n_end` instead.

**Verified:** `./test.sh` - 102 `node --test` cases (the clock/RNG/builtin
parity vectors frozen from the Python client, the driver on manual seams, the
emitted bundle bytes under both timebases, the patterns and the timeline, and
two WS cases against a real `clausters --ws`) and four headless-Chrome
acceptances, the new one being `tests/seq.html`: a pattern on the in-page
engine's own sample clock, its notes' starts and ends read back off the
server's notifications. Example: `examples/sequencing.html` - the generative
half and the seekable half side by side.

Not in scope, by the plan's own division, each now its own milestone:
`automation` (**W11** — a break-point control curve; it pulls in buffers, `Env`
and a control def), `MidiEvent` and MIDI destinations (**W9**), the shared
`/transport` grid (**W12**), and an NRT/score drive (**W13** — the client has
no score interface, and `Timeline.fromPattern` bounces by driving the ordinary
clock through its manual seams).

### ✅ W4 - Components: the host's canvases in the document

*(Design: `specs/2026-07-27-web-w4-components-design.md`; implementation plan
beside it. What this slot used to name — the responders, MIDI, and the browser
data paths — moved to the milestones after W5.)*

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

- **The host, from one canvas to N** (`clients/gui/src/host/web.rs`): `window`/`render`/`current_def` are singular today ("the browser shows one at a time"); they become a map keyed by def id — a wgpu surface, a size, a gesture state and a visibility flag each. The native front already keeps one surface per `window`-rooted GuiDef, so the model is ported, not invented. Two reversals ride along: the **element supplies the canvas** (winit's `with_canvas`, instead of `guiHost()` hunting for the one winit appended to `<body>`), and its size comes from the element (`ResizeObserver` + `devicePixelRatio`). A canvas out of the viewport is skipped on the tick and drops its buses from the `/c_stream`/`/tap_stream` sets — a document can hold fifty canvases with three in view, and the browser's own compositing skip does not stop *our* host from computing or the server from streaming.
- **The bundle grows a contract** (`clausters_core::bundle`, opened to the browser by `clausters-core-web` and to Python by `clausters-ffi`): `bundle.json` gains a symbol table, declared `params` and presets, and becomes a file **both** legs read (absent, the native host keeps listing the directory as today). The GuiDef record becomes a *template* with two kinds of hole — `@symbol`, an id the page allocates, and `$param`, a value the tag supplies — while widget ids stay local `1..N` and are offset by an allocated base. **Holes live only in the GuiDef record**, so def payloads are byte-identical between instances and are sent once; the authoring rule that follows is that a bus or a node reaches a def **as a control**, never as a baked constant (which is exactly why today's `piano_voice`, with its `out_ctl(0.0, env)`, cannot be mounted twice). Resolution is two pure functions — `requirements(manifest)` then `resolve(template, allocation, params)` — with the caller allocating in between, so nothing is added to the `/gui_*` protocol and no state to the host.
- **The component** (`src/elements.ts`, `src/runtime.ts`): a custom element owning its canvas, with the declared parameters as attributes and `preset` beside them, mounted in two phases — the GuiDef opens and draws on connect, and the engine half (defs, buffers, boot) goes out on the first page gesture, since the AudioContext is page-wide and N power buttons would be wrong. Failures stay per component. A new **slim run-time entry**, `dist/runtime.js`, carries the engine, the host, the codec and the mount and *not* the builders — today's `examples/piano/index.html` imports the whole package facade to use none of it.
- **The authoring API** (`clausters.bundle`, Python first, Node after): a writer over the existing builders that holds the symbol table, so the author names things instead of numbering them, declares `params` and presets, validates through the core before emitting — an unmountable bundle is unwritable — and generates the five-line ES module that registers the tag.

**Acceptance:** a page interleaving prose with three components — two instances of one bundle and one of another, all authored from Python and mounted with no client library loaded — draws three canvases; the two instances of the same bundle hold different buses and node ids while their def was sent once; a `freq` attribute makes one audibly different from its sibling (asserted on a control bus); a component scrolled out of view stops streaming; one component's failure leaves the rest of the page up; and the same bundles still run on the desktop, `--standalone` and loopback.

**What shipped.** The substitution, and the format that makes it repeatable.

The **browser host went from one canvas to N** (`clients/gui/src/host/web.rs`):
`window`/`render`/`current_def` — all singular, with the comment "the browser
shows one at a time" — became a map keyed by def id, each entry a wgpu surface,
a size, a pointer, a gesture state, the scope/tap/spectrum histories and a
visibility flag. The native front already keeps one surface per window-rooted
def, so the model was ported, down to the `by_winit` index every per-canvas
event routes through. With it, two reversals: the **element supplies the
canvas** (`attach(def_id, canvas)`, winit's `with_canvas`) instead of the page
grabbing whichever one winit appended to `<body>`, and the **size comes from the
element** (`ResizeObserver` × `devicePixelRatio`), so the host never reads the
DOM. `set_visible` is the one that pays: the `/c_stream`/`/tap_stream` sets and
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
Examples: `examples/piano/` and `examples/graph-controls/` ported to the writer,
and `examples/document/` — the interactive text the milestone is for.

Not in scope, deliberately, each now its own milestone: window *management*
(**W14** — an element removed from the DOM does not free its def; a
`/gui_closed` travelling back is a separate feature) and a TypeScript bundle
writer (**W15** — the reference client leads and the port is mechanical, the
repo's standing rule).

### ✅ W5 - Docs, examples, tests, packaging

Make it a real, shippable client.

- An mdBook in `clients/web/docs` (mirroring `clients/python/docs`), with the API reference **generated from TSDoc by typedoc** (the TS counterpart of the Python client's pydoc-markdown), and the GUIA-style manual-testing notes kept current. The client books cross-link by their RTD URLs.
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

### W6 - The full UGen catalogue

*Deferred out of W1.* W1 shipped the def model with representatives of each
family; this fills out the UGen-graph one, so a graph written against the
Python client ports by transcription rather than by lookup. The Faust
authoring surfaces are W7's, both of them.

- `defs/ugens.ts`: the rest of the server's UGen catalogue — sources, filters, delays, panning, envelopes, triggers, bus and buffer I/O, the demand pair, the spectral chain (`fft`/`ifft`/`pv_*`, the client side of S8), the output-less roots (`sendReply`/`sendTrig`/`poll`), and the complete unary/binary operator tables.
- The W1 composition rule is unchanged: TypeScript has no operator overloading, so operators stay methods and parity is asserted on the **emitted spec**, never on the source.

**Acceptance:** every UGen builder the Python client exposes has a TS counterpart emitting the same spec JSON, checked by extending the frozen vectors (`tests/gen-def-vectors.py`); a graph transcribed from a Python example plays over either carrier.

### W7 - The Faust surfaces: the box algebra, then the signal API

*Deferred out of W1.* The two Faust def-authoring surfaces, together because
they are one family — `clausters.defs.boxes` (C22) and `clausters.defs.signals`
— and in that order: the **box API first**, it being the richer and more
important of the two (Faust's own algebra, and the surface a whole `.dsp`
source folds into). And with them the piece the rest of the track never needed:
**a Faust compiler in the page**, so the Faust family stops being the one thing
a browser can author but not run.

- `defs/boxes.ts`: the point-free algebra (`seq`/`par`/`split`/`merge`/`rec`, `wire`/`cut`, controls, tables) emitting the same box-tree JSON the Python builders emit, plus `faust(src, ...)` to fold a Faust source expression — its libraries (`fi.`/`os.`/`re.`/`pm.`) included — into a composable `Box`.
- `defs/signals.ts`: the sample-level signal API filled out to the whole surface `clausters.defs.signals` exposes, the same emitted-spec parity rule W6 states.
- **The browser Faust toolchain** — the new build and packaging leg, below. Against a `--ws` server the two builders work without it (they emit JSON and the native server compiles it, the `faust(src, …)` escape hatch included — it ships its generated program as a `{"op": "faust", "src": …}` node rather than parsing Faust on the client); the in-page carrier is what needs it, and having *one* def family that only runs over WS is the asymmetry this milestone closes.

**The build and packaging step (new to this plan, and the reason W7 is not just
TypeScript).** Nothing in the track so far compiles anything but Rust and TS:
the toolchain is `tsc` with no bundler, and the wasm artifacts are the core and
host bundles `build.sh` stages. Faust in the page adds a second compiler
toolchain, in two halves — **only the first is packaging, and the second is the
one that decides whether the milestone lands**:

- **The compiler.** `libfaust-wasm` — the whole Faust compiler as a wasm library, what faustwasm and the Faust IDE use — built with the **Emscripten SDK** (`emcc` user-space via `emsdk`, then `make wasmlib` in `third_party/faust`), producing `libfaust-wasm.{js,wasm,data}`, the `.data` carrying the stdlib for Emscripten's virtual FS. The repo already documents the recipe and has **never built it** (`third_party/BUILD-FAUST.md`, "WebAssembly parts": excluded from `make most`, `emcc` not installed here), so this milestone is where it gets built, pinned the way `faust.pin`/`verovio.pin` pin the native ones, and staged by `build.sh` as static assets beside the core/host bundles — **off the slim `dist/runtime.js`**, since a page that mounts a *prebuilt* bundle must not download a compiler it never calls. CI grows the emsdk leg or the artifact is fetched, a decision to record.
- **The engine's side, which a compiler alone does not solve.** The in-page engine is the `synth,embed` build: no libfaust, **no LLVM JIT**, so it cannot instantiate the factory a native FaustDef becomes. A compiled-in-the-page def therefore needs a second instantiation path — Faust emitting a **wasm DSP module** run behind our AudioWorklet (the `faust -lang wasm` output plus glue), or the **Faust interpreter backend** the server's B track already names as future work. That half lives where the engine lives, so W7 **pairs with a B-track milestone** the way G25 pairs with M25 and P2 shipped as M30; the pairing is what makes this schedulable, and until it is numbered W7's in-page half is blocked on it while the WS half is not.
- **The decision to record** (`docs/decisions.md`): adopting a second compiler toolchain, its size and its licensing, in a repo whose stated posture is minimal, user-space and reproducible — plus which of the two instantiation paths the engine takes, and why the compiler stays out of the slim runtime.

**Acceptance:** the Python box-API and signal-API examples rebuilt in TS emit byte-identical spec JSON (new frozen vectors), and one of each compiles and plays **over either carrier** — against a `clausters --ws` server, and in the page with no server process, the source compiled by the staged `libfaust-wasm` and sounding through the in-page engine; a page that mounts a prebuilt bundle loads none of the compiler's assets.

### W8 - Responders: `OscFunc` over the reply stream

*Deferred out of W1*, which grew its reply handling ad hoc inside `Server`. The
client's input path and its role as a general OSC hub (sclang's `OSCFunc`),
mirroring the half of `responders.py` that does not involve MIDI.

- `responders.ts`: pattern-matched dispatch over the connection's reply stream — either carrier exposes it through the W0 seam (`addReply`), so nothing here names a transport — with handlers scheduled on a clock rather than run from the socket callback, the browser counterpart of the Python client's "never block the clock thread".
- The reply handling W1 grew ad hoc inside `Server` (the dispatch table and the `/sync` barrier) folds onto this one door, so everything arriving comes in the same way.

**Acceptance:** a TS app registers and unregisters `OscFunc` handlers that fire on server notifications (`/n_go`/`/n_end`, `/done`, `/tr`) over either carrier, and the W1/W3 end-to-end suites stay green through the new door.

### W9 - MIDI: `MidiFunc` in, `MidiEvent` and MIDI destinations out

*Deferred out of W3*, which left `MidiEvent` and MIDI destinations out of the
sequencing layer. Both directions of MIDI in one milestone, since in the
browser they are one API: Web MIDI is the only MIDI I/O a page has.

- `MidiFunc` over `navigator.requestMIDIAccess`, mirroring `responders.py`/`base/_midiinterface` — note/cc/program dispatch, port selection, handlers scheduled on a clock; convenience responders turn notes into `/s_new`, as C13 does.
- `MidiEvent` and MIDI as a **destination** of the sequencing layer: an event stream plays to a MIDI output exactly the way it plays to a `Server`, over the `play(destination)` seam W3 established, mapping `Event` → channel-voice messages the way `clausters-midi` already defines them for C11.
- Timing stays **best-effort by design**, as C18 settled for the Python client — but the browser gives it back cheaply: `MIDIOutput.send(data, timestamp)` takes a `performance.now()` deadline, so the driver hands over the deadline it has already computed instead of sleeping to it.

**Acceptance:** a pattern plays to a browser MIDI output on the same beat grid it plays to the audio server, and a `MidiFunc` on an input port drives defs on the server, over either carrier.

### ✅ W10 - The browser data paths: buses, bulk, and the analysis exports

*Deferred out of W2.* The paths the heavy views feed on, read by the **script**
this time. The host
already reads them itself (that is why a GuiDef naming a bus, a tap or a URL
works today); this is the client getting the same numbers.

- Control buses over the connection: `/c_stream periodMs bus...` subscribed from the client and its periodic `/c_set` snapshots decoded in a responder (W8's door) — the message-based counterpart of the native host's shared memory (G14). The server side exists on both carriers already: one subscription per client, replaced per call, `periodMs <= 0` cancels, 10 ms floor, ≤128 buses (`docs/schemas.md`), and B3 left `/c_stream`/`/tap_stream`/`/b_getn`/`/clock` streaming over the in-page leg too.
- Bulk buffers by `fetch`/`/b_getn` (G15), with the **peak pyramid built in wasm** from the fetched samples, so a waveform draws at screen resolution without a second implementation of the reduction; plus the fetch + `decodeAudioData` → `bLoad` sample path B3 left in `bundle.ts`, folded into the client's buffer API.
- The core's `correlation`/`lissajous` analysis exports surfaced to TS.

**Acceptance:** a TS app reads a control bus and a buffer over either carrier and draws them **itself** (a canvas the script feeds, not a host-fed widget), numerically matching what the GUI host draws from the same source.

**What shipped.** The three paths, and the move that makes "numerically
matching" true by construction rather than by care.

The **script reads what the host reads**. `Server` grew the commands
(`streamBuses`, `tap` with a `taps` registry beside the bus and buffer ones,
`streamTaps`, `getSamples` chunked by the frame ceiling the transport
advertises) and `src/data/` the sources over them: `BusStream` decoding the
periodic `/c_set` snapshots, `TapStream` placing each `/tap_data` window on its
tap's own sample axis by `endPosition`, `Peaks` over the wasm pyramid whose
`columns` reads a whole pixel row per crossing, and the measurements a view is
drawn with. The subscriptions ride `Server.onReply`, so **W8** folds them onto
`OscFunc` later without changing their surface.

Three things are worth carrying forward:

- **The signal logic moved into the core.** The oscilloscope's trigger
  alignment and the spectrum's decibel curve lived in the GUI host crate,
  correctly, while the host was the only thing computing them; a page drawing
  its own trace makes a second consumer, so they became
  `clausters_core::{oscil, spectrum}` and the host now consumes them from
  there. The alternative — a trigger re-implemented in TypeScript — fails
  silently, as two subtly different pictures of one signal, and exporting the
  host's internals would make a script that draws a canvas download 5.3 MB of
  GPU host to reach 40 lines of arithmetic. Rationale in `docs/decisions.md`.
- **The peak cache is byte-identical across clients.** `Peaks.toBytes()` writes
  what the Python client writes and what the GUI host maps — the mono layout
  for one channel, the multichannel one above it — which is what the parity
  vectors assert, one digest covering the whole format.
- **The bulk path is read-only, and not by choice.** The server has no
  buffer-write command: `/b_set`/`/b_setn` exist only as the *replies* to
  `/b_get`/`/b_getn`. So samples reach a buffer through `/b_gen`,
  `/b_allocRead`, or — in the page, where the carrier shares memory with the
  engine — `loadSample`, which fetches and decodes with the browser's own
  decoder and installs through the embed door. Writing from a client is noted
  as **M31** in the server's `PLAN.md`; the order will be the standing one,
  server command → the Python client → the port here.
- **On one page, the host and the script are one client.** Everything reaching
  the in-page engine goes through one shared-memory ring, which the server sees
  as a single `ClientId::Ring`, and `/c_stream`/`/tap_stream` are one
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
the script subscribes to both, reads a generated buffer, and draws a meter, a
scope and a waveform on its own canvas — with the columns asserted to be the
min/max of the very samples read, the trigger locked, the spectrum's peak on
the tone, and the canvas carrying ink. Example: `examples/scope.html`, the
three paths in one page; book chapter: "Reading the server".

### W11 - Automation: a break-point curve as a control vector

*Deferred out of W3*, because it is the one sequencing piece that is not pure
timing: the TS side of C23 pulls in buffers, `Env` and a control def.

- `seq/automation.ts`: a break-point curve discretized into a control buffer on the server (`/b_gen "env"`, the server's own `envshape`) and read back onto a control bus by a lane synth (`OutCtl`), prepared without blocking the driver, then played and freed like any other element.
- The `bpf` builder already in W2's GuiDef catalogue becomes its editor, so the curve is authored, heard and edited over the same loop the multitrack editor uses.

**Acceptance:** a curve authored in TS drives a synth's control over either carrier with the same values the Python client produces for the same break points, and dragging it in the browser GUI's `bpf` widget moves the sounding value.

### W12 - The shared `/transport` grid

*Deferred out of W3.* Phase alignment across clients — the TS counterpart of
C15 and of C16's `follow_transport`.

- `quant` honored when a routine starts (snap to a beat boundary), and joining the server's `/transport` grid so pages started at different moments share one bar line.
- Following the `/transport_play|stop|locate` broadcasts, so a page's playhead rolls in lockstep with every other client: the server broadcasts control, never audio.
- W3's rule is not bent: the clock still never talks to a server. The `Server` feeds the grid inward, the way `sampleTimebase()` already anchors the timebase.

**Acceptance:** two pages — or a page and a Python client — join the same transport and land on the same bar; a `/transport_locate` moves the page's playhead with it.

### W13 - An NRT / score drive

*Deferred out of W3*, whose `Timeline.fromPattern` bounces by driving the
ordinary clock through its manual seams. The third `Server` interface, beside
the two carriers: a destination that *writes* time instead of waiting for it.

- A score destination for the sequencing layer — the same pattern or timeline that plays live emitted as a timestamped score — rendered either by a native server's NRT mode over WS, or in-page by the wasm engine running faster than real time into a buffer the page can play or download.
- W3 already *bounces* without one (`Timeline.fromPattern` drives the ordinary clock through its manual seams); what is missing is the interface, not the arithmetic.
- Score parity is the check C5 keeps on the Python side: one piece, one score, compared byte for byte.

**Acceptance:** a piece written once emits a score byte-identical to the Python client's for the same input, and renders from the browser to a WAV that matches the native NRT render.

### W14 - Component lifecycle: freeing what a removed element owns

*Deferred out of W4*, which mounted components but never unmounted them: an
element removed from the DOM leaves its def standing, and a window the host
closes has no way back to the page. The missing half of the mount, in both
directions.

- **Down** (`src/elements.ts`, `src/base/pool.ts`): `disconnectedCallback` frees the component's GuiDef subtree (`/gui_free`), returns its allocated block — widget ids, buses, node ids — to the core-backed pools, and drops its canvas entry from the host (`web.rs`'s map, the `set_visible` sibling), so a long document that adds and removes components does not leak ids, streams or surfaces. The engine half is page-wide and stays up; only what the instance allocated goes.
- **Up**: `/gui_closed` travelling back from the host to the element that mounted the def — the event exists on the wire and in the `GuiHost` driver (W2), but a component has no handler for it; a host-closed window must reach its element rather than leave a live tag over a freed def.
- The reverse of W4's two-phase mount is deliberately **not** symmetric: a re-connected element mounts again from the same bundle (defs already sent, a fresh allocation), so the resolver is re-run, not cached.

**Acceptance:** a page that mounts and removes the same component a hundred times holds a flat id/bus/node occupancy (read from the pools and from `/g_queryTree`); a removed component stops sounding and stops streaming; a `/gui_closed` from a native `--ws` host reaches its element; and the surviving components on the page are untouched throughout.

### W15 - The TypeScript bundle writer

*Deferred out of W4*, which shipped the writer in Python only — the reference
client leads and the port is mechanical, the repo's standing rule. This is that
port, so a bundle can be authored in the same language the page is written in.

- `src/bundle-writer.ts` (authoring, **not** part of the slim `dist/runtime.js`): the TS counterpart of `clausters.bundle.Bundle` — the symbol table, the bundle-prefixed def names, the declared `params` and presets, validation through the core wasm door (`check_def_payload` and the `requirements`/`resolve` pair) before emitting, and the generated five-line ES module that registers the tag. An unmountable bundle stays unwritable, on this leg too.
- Runs in Node (a file writer) and in the page (an in-memory bundle mounted without a round trip through disk), the two being the same code over a small output seam.

**Acceptance:** the W4 bundle parity vector runs both ways — what the TS writer emits and what the Python writer emits are byte-identical for the same input, and each resolves through the browser's wasm door to the same mount; `examples/document/` rebuilt from the TS writer draws the same page.

### W16 - Example parity with the Python client

*Deferred out of W5*, which ported the examples that run in a page as they
stand and left the rest. This closes the gap, so the two example sets read as
one catalogue: same name, same instrument, same point of interest, one written
as a script and one as a page.

- The remaining `clients/python/examples/` ported to `clients/web/examples/`, each keeping the name of the example it mirrors and the catalog row that says so.
- Most of what is left is **not** blocked on porting effort but on a surface this client does not have yet — the responders, MIDI, automation, the transport grid, an offline render, the box algebra, the UGens outside the shipped set. Each such example lands with (or after) the milestone that opens its surface, which is why this slot is a destination rather than a queue.
- The examples that are Python-process shaped by nature (a launcher, a live UDP peer, a native GUI shell) have no page counterpart and stay unported; the catalog says so rather than leaving a hole.

**Acceptance:** every Python example either has a web page of the same name or a stated reason in the catalog for having none, and each ported page runs on the in-page engine with the carrier line marked.

### ✅ W17 - Publishing: the npm registry and a third Read the Docs project

*Deferred out of W5*, which built the package and the book and left them on the
machine. The distribution step, deliberately separate: the artifacts are held
publishable by a checker long before anyone runs `npm publish`.

- **The package**: `npm publish` for `clausters`, on the repository's own SemVer (package, crate and wheel are one release), through the gate `prepublishOnly` runs. The procedure, including what has to be built first, is `clients/web/BUILD.md`, "Publishing".
- **The book**: a third Read the Docs project pointing at `clients/web/.readthedocs.yaml` (the file exists and drives the whole build: TypeDoc, then mdBook).
- **The inbound links**: the other books and READMEs do not link the web book, because the URL does not resolve yet. Once it does, the badge/link rows (root `README.md`, `clients/python/README.md`, `clients/gui/README.md`) and the Python book's introduction gain it — the cross-linking the three books otherwise already do.
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
`clients/gui/Cargo.lock` was ignored while `build.sh` reads it (the
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

### W18 - The `Session` facade

*Deferred out of W5*, whose layout sketched a `session.ts` into the slot while
the milestone itself was docs, examples and packaging. It is an API layer, and
it leans on verbs this client does not have yet.

- `session.ts`: the browser counterpart of `clausters.Session` — one handle bundling a `Server`, a `TempoClock` and its timebase, so a page stops wiring the three by hand. On this page the singletons already give the *shared* half (one engine, one host, one namespace); what a `Session` adds is the ergonomics and the ability to hold more than one at a time (a page against the in-page engine beside one against a remote `--ws` server).
- The Python client's ambient verbs are the reason not to do it early: `play` is here already as `Event.play`/`Pattern.play`, but `plot` wants the script-side data paths (**W10**) and `render` an offline drive (**W13**). A facade shipped before them would name verbs it cannot keep.

**Acceptance:** the getting-started example rewritten through a `Session` is shorter and does the same thing; two sessions over different carriers coexist in one page, each with its own clock.

## Future directions

- **Node target.** Already true in the harness, not yet a supported target: the `node --test` suites drive a real `clausters --ws` server and a real `clausters-gui --ws` host, so `WsConnection` runs under node's global `WebSocket` (`src/base/connection.ts` says so) and the wasm core loads there (`loadCore(bytes)`, node's `fetch` not reading `file://`). What remains is making it a *product*: a load path that finds the core's `.wasm` without the test's manual read, a documented entry point for headless scripting/CI the way `clients/python` runs without a display, and the boundary written down — the def, sequencing and GUI-driver layers port, the in-page engine (AudioWorklet) and the page host (canvas) do not.
- **Type-safe GuiDef/def schemas.** Generate TS types for the widget/def vocabularies from a single source shared with the server, so an invalid GuiDef is a compile error, not a runtime warning. Two things have since appeared that change the shape of the answer rather than the want: the frozen parity vectors (`tests/gen-*-vectors.py`) already catch a drifted *builder* at test time, and M30's `/d_query`/`/u_query` make the server's own catalogue readable at run time — so the open question is narrower, which source generates the types and when, not whether one exists.
- **A remote-server standalone page.** The in-tab standalone (a bundle booting against the embedded wasm engine) **shipped with the B track** and grew up in W4 (the bundle contract, the resolver, the pools, the components); what remains is the same mount against a **remote `--ws` server** — a one-file instrument front for a server running elsewhere. The old note called this cheap "once W1/W2 exist"; they exist, and W4 is what actually decides the work: `openBundle`/`startBundle` reach the page's `guiHost()` and `engine` singletons directly, so the step is giving the mount a **destination seam** (a `Server` + `GuiHost` pair, both already carrier-agnostic since W1/W2) in place of those singletons. The boot replay itself stays carrier-agnostic above the W0 seam, as it always was.
