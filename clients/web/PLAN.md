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
- **Browser realities are first-class, not afterthoughts.** WebSocket is the only *network* transport (no UDP, no shared memory, no mmap); since the server's B track, the browser also has a second, process-free carrier — the **in-page engine** (the server compiled to wasm in an AudioWorklet, reached through the B4 package's `server()` singleton) — and the client stays carrier-agnostic above a small connection seam. Bulk data arrives by `fetch`/`/buffer_getRange`; meters/scopes read control buses over the wire; the sample-clock timebase uses the Web Audio clock (`AudioContext.currentTime`). These are the same "async fallbacks" the server/gui plans reserved for the browser.

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
      clock.ts timebase.ts builtins.ts
      environment.ts main.ts  #   the ambient environment + the default session
    errors.ts             #   the error hierarchy (mirrors errors.py)
    defs/                 #   the def model + server client (mirrors clausters/defs)
      server/             #     the handle, and beside it options/queries/streams
      ugens/              #     the UGen catalogue, one module per family
      node.ts  bus.ts  buffer.ts  info.ts  clocksync.ts  wire.ts
      signals.ts  synthdef.ts  faustdef.ts  graphdef.ts
    gui/                  #   the GUI host driver (mirrors clausters/gui)
      host.ts             #     GuiHost + the per-page guiHost() singleton
      guidef.ts  handle.ts  ids.ts
    seq/                  #   sequencing (mirrors clausters/seq)
      event.ts  eventstream.ts  pattern.ts  timeline.ts
    data/                 #   the data paths: what a view reads off the server
      buses.ts  taps.ts   #     the streamed sources (/bus_stream, /bus_tapStream)
      samples.ts  peaks.ts analysis.ts
    responders.ts         #   OscFunc dispatch (mirrors responders.py; MidiFunc — W9)
    session.ts            #   the Session facade + the default session
    play.ts               #   the free `play` verb
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
- **No bundler for the package.** Nothing a page loads needs one: the package ships unbundled, the wasm bundles and the worklet module must stay static assets anyway (`AudioWorklet.addModule` and bundlers are a known friction), and the browser loads bare ESM natively. Evaluated and not adopted: **vite** (a dev server with HMR plus rollup/esbuild underneath — tens of MB of dev machinery whose two roles are already covered by `http.server` and `tsc --watch`; revisit only if HMR-grade DX is genuinely missed), **vitest** (pulls vite in as its platform). **`esbuild` was adopted 2026-08-04 for one artifact and dropped again on 2026-08-05** with the notebook front end that needed it: a client handed over a carrier and imported from `blob:` URLs cannot load a module graph with cycles (this one has three), which is the one condition this bullet always named. Nothing in `dist/` is bundled now — it is the `src/` tree emitted 1:1 — and the carrier that wanted it lives on the `jupyter` branch.
- **Tests: `node:test`, built into node — zero dependencies.** Node runs `.ts` directly (native type stripping, default since 23.6), so pure-logic tests (codec parity, clock arithmetic, builders) run straight from source with `node --test`, no compile step, no runner package. Browser-only behavior (audio, canvas, the elements) keeps the B-track posture: headless-Chrome smoke scripts with the access-log beacon.
- `typedoc` (the W5 API-reference generator) gets evaluated under this same lens when W5 starts.
- The **Emscripten SDK** (`emcc`, user-space via `emsdk`) is the one heavy addition this lens admits, and it is **W7's**, not the toolchain's baseline: it builds `libfaust-wasm` so a Faust def compiles in the page (`third_party/BUILD-FAUST.md`, "WebAssembly parts" — documented, never built here). It stays out of the JS toolchain proper — nothing in `src/` or the test loop touches it, `build.sh` only stages its output as static assets, and the slim run-time entry never loads them. Evaluated under the same lens when W7 starts, decision recorded then.

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
- `examples/verbs.html` — the tour that visits every kind, and its closing list
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
server's notifications. Example: `examples/sequencing.html` - the generative
half and the seekable half side by side.

Not in scope, by the plan's own division, each now its own milestone:
`automation` (**W11** — a break-point control curve; it pulls in buffers, `Env`
and a control def), `MidiEvent` and MIDI destinations (**W9**), the shared
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

- **The host, from one canvas to N** (`clients/gui/src/host/web.rs`): `window`/`render`/`current_def` are singular today ("the browser shows one at a time"); they become a map keyed by def id — a wgpu surface, a size, a gesture state and a visibility flag each. The native front already keeps one surface per `window`-rooted GuiDef, so the model is ported, not invented. Two reversals ride along: the **element supplies the canvas** (winit's `with_canvas`, instead of `guiHost()` hunting for the one winit appended to `<body>`), and its size comes from the element (`ResizeObserver` + `devicePixelRatio`). A canvas out of the viewport is skipped on the tick and drops its buses from the `/bus_stream`/`/bus_tapStream` sets — a document can hold fifty canvases with three in view, and the browser's own compositing skip does not stop *our* host from computing or the server from streaming.
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
Examples: `examples/piano/` and `examples/graph-controls/` ported to the writer,
and `examples/document/` — the interactive text the milestone is for.

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
  written where it is met: the book's def chapter, and `examples/demand.html`,
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
`examples/spectral.html`, the port of `spectral.py` with the wipe live under
the hand instead of rendered offline, and `examples/demand.html`, the
sequence that lives in the def. Book: the def chapter now names the three
families that are not read like the rest.

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

### ✅ W8 - Responders: `OscFunc` over the reply stream *(done 2026-08-03)*

*Deferred out of W1*, which grew its reply handling ad hoc inside `Server`. The
client's input path and its role as a general OSC hub (sclang's `OSCFunc`),
mirroring the half of `responders.py` that does not involve MIDI.

- `responders.ts`: pattern-matched dispatch over the connection's reply stream — either carrier exposes it through the W0 seam (`addReply`), so nothing here names a transport — with handlers scheduled on a clock rather than run from the socket callback, the browser counterpart of the Python client's "never block the clock thread".
- The reply handling W1 grew ad hoc inside `Server` (the dispatch table and the `/server_sync` barrier) folds onto this one door, so everything arriving comes in the same way.

**Acceptance:** a TS app registers and unregisters `OscFunc` handlers that fire on server notifications (`/node_start`/`/node_end`, `/done`, `/node_trigger`) over either carrier, and the W1/W3 end-to-end suites stay green through the new door.

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
Example: `examples/responders.html`, the port of `osc_responder.py` — a def
reports its onsets with `SendReply` and a responder answers each with a synth,
so what arrives is what plays. Book: "Receiving: responders".

Not in scope, by the plan's own division: `MidiFunc` and the MIDI destinations
(**W9**), both directions being one browser API.

### W9 - MIDI: `MidiFunc` in, `MidiEvent` and MIDI destinations out

*Deferred out of W3*, which left `MidiEvent` and MIDI destinations out of the
sequencing layer. Both directions of MIDI in one milestone, since in the
browser they are one API: Web MIDI is the only MIDI I/O a page has.

- `MidiFunc` over `navigator.requestMIDIAccess`, mirroring `responders.py`/`base/_midiinterface` — note/cc/program dispatch, port selection, handlers scheduled on a clock; convenience responders turn notes into `/synth_new`, as C13 does.
- `MidiEvent` and MIDI as a **destination** of the sequencing layer: an event stream plays to a MIDI output exactly the way it plays to a `Server`, over the `play(destination)` seam W3 established, mapping `Event` → channel-voice messages the way `clausters-midi` already defines them for C11.
- Timing stays **best-effort by design**, as C18 settled for the Python client — but the browser gives it back cheaply: `MIDIOutput.send(data, timestamp)` takes a `performance.now()` deadline, so the driver hands over the deadline it has already computed instead of sleeping to it.

**Acceptance:** a pattern plays to a browser MIDI output on the same beat grid it plays to the audio server, and a `MidiFunc` on an input port drives defs on the server, over either carrier.

### ✅ W10 - The browser data paths: buses, bulk, and the analysis exports

*Deferred out of W2.* The paths the heavy views feed on, read by the **script**
this time. The host
already reads them itself (that is why a GuiDef naming a bus, a tap or a URL
works today); this is the client getting the same numbers.

- Control buses over the connection: `/bus_stream periodMs bus...` subscribed from the client and its periodic `/bus_set` snapshots decoded in a responder (W8's door) — the message-based counterpart of the native host's shared memory (G14). The server side exists on both carriers already: one subscription per client, replaced per call, `periodMs <= 0` cancels, 10 ms floor, ≤128 buses (`docs/schemas.md`), and B3 left `/bus_stream`/`/bus_tapStream`/`/buffer_getRange`/`/clock_query` streaming over the in-page leg too.
- Bulk buffers by `fetch`/`/buffer_getRange` (G15), with the **peak pyramid built in wasm** from the fetched samples, so a waveform draws at screen resolution without a second implementation of the reduction; plus the fetch + `decodeAudioData` → `bLoad` sample path B3 left in `bundle.ts`, folded into the client's buffer API.
- The core's `correlation`/`lissajous` analysis exports surfaced to TS.

**Acceptance:** a TS app reads a control bus and a buffer over either carrier and draws them **itself** (a canvas the script feeds, not a host-fed widget), numerically matching what the GUI host draws from the same source.

**What shipped.** The three paths, and the move that makes "numerically
matching" true by construction rather than by care.

The **script reads what the host reads**. `Server` grew the commands
(`streamBuses`, `tap` with a `taps` registry beside the bus and buffer ones,
`streamTaps`, `getSamples` chunked by the frame ceiling the transport
advertises) and `src/data/` the sources over them: `BusStream` decoding the
periodic `/bus_set` snapshots, `TapStream` placing each `/bus_tapStream.reply` window on its
tap's own sample axis by `endPosition`, `Peaks` over the wasm pyramid whose
`columns` reads a whole pixel row per crossing, and the measurements a view is
drawn with. The subscriptions rode `Server.onReply`, and **W8** folded them
onto `OscFunc` without changing their surface, as this slot said it would.

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
the script subscribes to both, reads a generated buffer, and draws a meter, a
scope and a waveform on its own canvas — with the columns asserted to be the
min/max of the very samples read, the trigger locked, the spectrum's peak on
the tone, and the canvas carrying ink. Examples: `examples/scope.html`, the
three paths in one page drawn by the script; and `examples/editor.html`, the
port of the Python client's `gui_editor.py` — a decoded file in a server
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
`examples/automation-lane.html`, the port of `automation_lane.py` with the
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
  clients name the same thing — and `transport_sync.py`'s `next_bar_sample`
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
`examples/transport-sync.html`, the port of `transport_sync.py` with the
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
  `Box` leg has nothing to coerce until the box algebra lands (**W7**).
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
Example: `examples/offline.html` — a phrase rendered at 60x real time, looked
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
Example: `examples/lifecycle.html` — instruments added and removed by hand, with
the pools' occupancy on screen.

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
- Most of what is left is **not** blocked on porting effort but on a surface this client does not have yet — MIDI, automation, the transport grid, an offline render, the box algebra, the UGens outside the shipped set. Each such example lands with (or after) the milestone that opens its surface, which is why this slot is a destination rather than a queue.
- The examples that are Python-process shaped by nature (a launcher, a live UDP peer, a native GUI shell) have no page counterpart and stay unported; the catalog says so rather than leaving a hole.

**Acceptance:** every Python example either has a web page of the same name or a stated reason in the catalog for having none, and each ported page runs on the in-page engine with the carrier line marked.

The largest single item behind this, named here because it is a **track and not
an example**: `gui_composer.py` needs the **arrangement layer** — `clausters.form`
(elements placed recursively, and the rendering that flattens them) plus the
multitrack `Editor` and its transport, roughly two thousand lines of Python with
no TypeScript counterpart — and it pulls W13 (the offline bounce of its take)
with it; the automation lane it also needs is here since W11. No design is staged for that port, and the
cross-client rule says the reference client is finished and polished first, then
ported, with whatever is language-agnostic pushed into the shared core as it is
written; deciding *what* goes down there is the first question of that milestone,
not a detail of it. Until then `clients/web/examples/composer.html` covers the
**host** half — the lanes, clips, shared axis, drag grid and gestures, built as
widgets with no model behind them — which is what the host's own bugs need to be
reproducible in a browser, and it is where G32b was found.

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
- **The factories are named for the carriers, not for Python's.**
  `Session.page()` and `Session.connect(url)`, matching `GuiHost.page()`/
  `GuiHost.connect()` and the `Server` carriers this package already has,
  rather than `embed`/`live(host, port)` — whose parameters (a host, a port, a
  process to boot) a page has none of. `nrt`/`render` and `join_transport`
  wait on **W13** and **W12**.
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
`examples/verbs.html`, the port of the Python client's `verbs.py` — a session
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

**Verified:** `tests/hosts.html` under the headless-Chrome harness — two hosts in one page both draw; widget `1003` holds `0.2` in one and `0.8` in the other at once; a gesture on one leaves the other's outbox empty and its widget unmoved; each bound knob reaches its own engine and not its sibling's (`220 → 984.7` while the other stays `220`); closing one leaves the other drawing, answering and driving its engine. Example: `examples/two-hosts.html`.

## Parity gaps carried from the Python client

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

## Future directions

- **Node target.** Already true in the harness, not yet a supported target: the `node --test` suites drive a real `clausters --ws` server and a real `clausters-gui --ws` host, so `WsConnection` runs under node's global `WebSocket` (`src/base/connection.ts` says so) and the wasm core loads there (`loadCore(bytes)`, node's `fetch` not reading `file://`). What remains is making it a *product*: a load path that finds the core's `.wasm` without the test's manual read, a documented entry point for headless scripting/CI the way `clients/python` runs without a display, and the boundary written down — the def, sequencing and GUI-driver layers port, the in-page engine (AudioWorklet) and the page host (canvas) do not.
- **Type-safe GuiDef/def schemas.** Generate TS types for the widget/def vocabularies from a single source shared with the server, so an invalid GuiDef is a compile error, not a runtime warning. Two things have since appeared that change the shape of the answer rather than the want: the frozen parity vectors (`tests/gen-*-vectors.py`) already catch a drifted *builder* at test time, and M30's `/def_query`/`/ugen_query` make the server's own catalogue readable at run time — so the open question is narrower, which source generates the types and when, not whether one exists.
- **A remote-server standalone page.** The in-tab standalone (a bundle booting against the embedded wasm engine) **shipped with the B track** and grew up in W4 (the bundle contract, the resolver, the pools, the components); what remains is the same mount against a **remote `--ws` server** — a one-file instrument front for a server running elsewhere. The old note called this cheap "once W1/W2 exist"; they exist, and W4 is what actually decides the work: `openBundle`/`startBundle` reach the page's `guiHost()` and `engine` singletons directly, so the step is giving the mount a **destination seam** (a `Server` + `GuiHost` pair, both already carrier-agnostic since W1/W2) in place of those singletons. The boot replay itself stays carrier-agnostic above the W0 seam, as it always was.


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
misplaced code, and each is already owned: `defs/boxes.py` (**W7**;
`defs/pv_expr.py` came with **W6**, which is what `pvKernel` takes),
the MIDI half of `responders.py` (**W9** — its OSC half is ported),
`session.py`/`play.py`/`base/main.py`/`base/environment.py`/`defs/_wire.py`
(**W18**), `render.py`/`defs/asdef.py` (**W13**), `form/` and `gui/editor.py`/
`gui/transport.py`/`gui/notation.py` (**W16**'s named track), `defs/patch.py`
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
so both clients gain it at once. The blocker is unchanged and is a packaging
one: the page needs an engraver, and libverovio is not in the bundle — so this
lands with **W16** and not before.

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
charged to the piece. Example: `examples/transport-freeze.html`, the port of the
Python client's `transport_freeze.py` — a generative texture frozen mid-gesture
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
  not an error. `examples/offline.html` had been calling
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
named and yielding to a registered host. Example: `examples/scoping.html`, the
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

### W24 - The completeness pass

The slot for what the milestone-by-milestone port leaves behind: differences
that are nobody's feature. It is deliberately last and deliberately open — it
gathers loose ends rather than opening a layer, and an entry leaves it as soon
as some other milestone has a better claim on it.

- **`defs/patch.ts`** — `GraphPatch` and `DefPatch`, the models behind
  `def.plot_def()`, which open a def's **structure** (not its sound) as a
  `patch` view. The widget has existed since W2 and `examples/composer.html`
  drives one by hand; what is missing is the model that reads a def and emits
  `{boxes, cords}`, plus the `PatchWindow` handle. W21's inventory listed
  `defs/patch.py` as unclaimed and it still is; it lands here unless the def
  layer is opened for something else first.
- **`Session.connectGui(url)` is a verb this client invented**, and it does two
  things at once: *connect* and *adopt*. Its adopting half briefly had a
  reference counterpart (`session.adopt_gui` / `adoptGui`), which left with the
  notebook track that was its only caller, so the whole verb is invented again
  and the question is back to one: does the reference client want a session to
  install a host it did not open? Answer that first — the standing rule says
  the reference leads — and `connectGui` follows from it, either dropped in
  favour of the explicit pair or matched by a shortcut there.
- **Two names the sweep of 2026-08-03 left over**, each too small to own a
  milestone and neither a difference with a reason: `Routine.run(func, clock,
  quant)`, the classmethod that constructs and starts in one call (the instance
  `play` is here, the shortcut is not), and `ServerError`, which W21 already
  listed as missing and portable and which nothing has since claimed.
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

## Found by use: the running list of fixes

These are not milestones and they are not future directions. They are what
**using the thing** turns up — an eye pass over an example, a path read while
doing something else, a behavior that is correct and unclear — recorded the day
it is found so it is not rediscovered, and kept here rather than inside whichever
milestone happened to be open at the time.

Two conventions, because the section only works if both hold. Every entry is a
**checkbox**, so what is open reads as open at a glance and nothing has to be
inferred from where it sits. And a fixed one **stays**, with the record of what
was wrong and why the fix is the shape it is — that is what makes the list worth
reading rather than a queue that empties.

Anything unresolved lives here or under "Future directions", both **after** the
tracks: never inside the milestone that happened to be open, and never among
finished work, where a pending item reads as done.

- ✅ **A page can follow a take while it records** *(shipped 2026-08-19 with the core's `write_buckets` door, closing the GUI plan's "A page cannot fold a streamed overview into the picture it holds")*. `data.RecordingStream` is the receiving end of `/buffer_stream`: one `Peaks` per take, allocated at the buffer's full length and empty, each report folded in through `Peaks.writeBuckets` (the core's own door, so nothing is measured on this side) with `written` saying how far the reports have got. `tests/recording.html` is the acceptance and it asserts the claim rather than the mechanism — a take recorded by the in-page engine, followed while it fills, and then compared **column for column** against a pyramid built from the samples read back. The two limits are the wire's and are documented as such: the summary is the resolution (zoomed inside a bucket the picture is that bucket), and the server keeps one buffer subscription per client, so on a page a script's stream and the GUI host's `fills` view cancel each other.

- ⬜ **A bulk read of more than one chunk gets no reply on the in-page carrier** *(found 2026-08-19 writing `tests/recording.html`, which reads a take back to compare it: `buffer.getSamples()` over 25 600 frames raised `ReplyTimeout: no reply to /buffer_getRange within 5s`, while the same call with `{ chunk: 4096 }` returned every sample)*. So it is not the size of the buffer but the size of one **request**: the chunk `getSamples` picks by default (`server.bulkChunk()`, the server's own maximum) produces a reply the page's ring does not deliver, silently — no `/fail`, no partial, just nothing. The test works around it by naming a chunk, which is what makes the workaround visible; what it means for an ordinary page is that reading a buffer of any size fails by default and works when a number nobody should have to know is passed. Worth taking with the next thing that touches the carrier's framing: either the ring's frame limit is what `bulkChunk` should answer, or the reply has to be split where it is written.

  **The mechanism is no longer a guess** *(2026-08-20, from the GUI host hitting the same wall)*: the server drops a reply whose ring has no room — deliberately, "backpressure, not loss ... all we can do without blocking the server" — and the ring is 64 KiB. So the loss is by design and every reader over the carrier owes itself the arithmetic. The host now does the first of the two fixes named above for its *own* requests: it sizes a chunk so several fit the ring, bounds how many it has in flight, and asks again for what never came back. This entry is the same reasoning applied to `bulkChunk`, which is the number a *user* of the client gets handed.

- ✅ **The bundle vectors were frozen against a prop name that had been renamed** *(found 2026-08-17 by re-running every generator while taking a change in the document format, which is the rule the packages-move-together section states and is the only thing that would have found this)*. `bundle-vectors.json` still carried `"layout": "col"` on a `layout` widget, from before the prop became `flow` in both clients' builders. The vector is generated from the Python builders and compared against this client's, so it had been describing a bundle neither client writes any more. Regenerated with nothing else in the commit, since it predates the work that found it.

- ✅ **The UGen builders are verified against the server on the Python side and against nothing here** *(fixed 2026-08-16, the day it was found)* *(found 2026-08-16, when the Python contrast test caught eleven kinds whose builder no longer matched the wire — and every one of them had been ported to TypeScript with the same defect, unnoticed)*. `clients/python/tests/test_session.py::test_ugen_catalog_matches_the_python_callables` asks a running server for `/ugen_query` and asserts, kind by kind, that each builder's parameter **names and defaults** are the server's own, with a declared exception list for the kinds whose signature cannot line up (variadic tails, static fields) that is itself asserted exact. There is no such test here, and the failure it catches is invisible to every other check — though it is worth being exact about **what** it catches, because the eleven were not wrong defs. Each builder assembles its wire list by hand (`new Ugen("BufDelayC", [bufnum, chan, signal, delaytime])`), so what they emitted was correct and what they sounded like was right. What was wrong was the **signature**: the parameter a caller reads and names. Two things depend on that and nothing else does — the patcher's Def view labels a box's inlets with the builder's parameter names, position by position (`ugen_input_names`, `defs/patch.py`), so a misaligned builder mislabels three inlets out of four; and a default that disagrees with the server's means the two disagree about what "left alone" means, in a UI reading `/ugen_query` beside a call that reads the builder. Neither is caught by a compiler, a def compile or an ear, which is exactly why it needs a test rather than attention.

  It is not a port of the Python test — that one reads signatures with `inspect` and maps a callable to its kind by parsing the `Ugen("Kind", ...)` literal out of the source. The TypeScript equivalent has the same two halves available: the emitted `Ugen.kind` and `Ugen.inputs` are readable by **calling** each builder with a sentinel per required parameter, which contrasts the *emitted list* rather than the signature and needs no AST at all. The exception list would then be about defaults only, since a sentinel marks every slot the caller had to fill.

  **What shipped** *(2026-08-16)*. `tests/ugen-catalog.test.ts`, against `ugen-vectors.json` — the odd generator of the set, frozen from the **server's** `/ugen_query` rather than from the Python client, because here the server is the reference and not a peer. The sentinel plan above was not needed: `Function.prototype.toString()` under node's type stripping still carries parameter names *and* defaults, so the contrast reads the signature after all, exactly as the Python one does. A destructured parameter or one defaulting to `{}` is an options object and ends the positional contrast — but only for a kind declaring a departing tail, so an undeclared `{}` cannot quietly truncate the check. Four declared lists (no builder / trailing tail / signature differs / aliased name) carry a reason each and are asserted exact by a second test.

  **It found three things on its first run.** Ten builders spelled the wire's `trig` as `trigger`; `TransportPos` had no builder at all (T5 reached the server, the Python client and the reference, and stopped there); and two suites still asserted the pre-T5 `transportState()` contract, where no grid meant no state — the runtime had been updated and the tests had not, and nothing had run them since. **Verified by mutation, not by passing**: reordering a builder's parameters, changing one default and removing an export each make it fail.

- ✅ **The browser bundle carries the glyph rasterizer** *(enabled 2026-08-11, with the GUI track's K10)*. `build.sh` compiles the host's wasm with `--features font-atlas`, so a page may draw text with a real typeface: it fetches a **raw TrueType/OpenType** file (the rasterizer does not decompress WOFF2, so a Google Fonts CSS URL is not one) and hands the bytes over with `(await guiHost()).bridge.font(bytes)`. A CSS `@font-face` cannot serve here — the host draws into a canvas and never reads the document's fonts.

  **The bundle ships no face**, which is what makes this affordable: the cost is the rasterizer alone, **+130 KB uncompressed, +46 KB gzipped** on a 5.8 MB / 1.7 MB bundle, and a page that hands over nothing draws the embedded bitmap face exactly as a build without the feature does. Loading one relayouts nothing — the sizing table never followed the typeface — so it may be handed over at any point, before or after the first `/gui_def`.

  Checked by eye rather than by a suite, and deliberately: verifying it needs a face, and committing one to the repository is the cost this whole design avoids. Two headless-Chrome screenshots of the same tree, one with a fetched face and one without, showed the same layout in the two faces — which is also what proves the floor holds in the browser.

- ✅ **The square wave's edge is still missing on the web, and the join that
  draws it is only in the host** *(found 2026-08-20 by the user, on the web
  version, after the fix landed for the native window)*. A column measures a
  group of samples and groups partition the samples where the curve does not, so
  a one-sample jump landing on a column boundary is drawn by neither column and
  the trace comes apart exactly at an edge — and it comes and goes with the zoom,
  since where the jump falls is a fact about the magnification. The rule that
  closes it is that **a column is inked over what it measured, extended to reach
  the column before it**, and it shipped in `3a7ce0fe` (with the record moved in
  `514ce96d`) inside `trace::draw_channel`, the host's one renderer — so the wasm
  host carries it and every widget-drawn picture in a page is fixed by the same
  commit.

  What is **not** fixed is a page that draws columns itself. This client hands
  out `Peaks.columns()` — measurements, and no renderer — so the rule is on
  whoever draws them, and it was copied into `examples/scope.html` in the same
  commit while `tests/data.html` still draws the gap. Two questions a fix has to
  settle: whether the rule stays a recipe each page repeats (documented where
  `columns` is) or the client grows the drawing side of it — a small helper over
  a `PeakRow`, or a `columns` variant returning joined spans — and, since the
  measurements must stay exactly what the core measured, that the join lives in
  `data/` rather than in the core or the wire.

  **What is not established is which surface the report is about.** A page of the
  user's own and `tests/data.html` fit the entry; `examples/scope.html` fits it
  only if it was looked at before the rebuild; and the wasm host fits it not at
  all — if the edge is missing in a `waveform` widget on a page, the join is
  reaching the picture and something else is, and this entry is the wrong place
  to look. Reproducing it against a named page is the first step, not the fix.

  **Reproduced, and it is the page that draws its own columns** *(2026-08-20)*:
  a square wave of 8192 frames with a 512-frame period, read at widths 16, 32
  and 64, comes back with **every column flat and fifteen disjoint boundaries** —
  a dashed top, a dashed bottom, no vertical anywhere — because at those widths
  a transition always lands between two columns. The wasm host was never in it:
  a widget draws through `trace::draw_channel`, which had the join.

  **Fixed by moving the rule down rather than by copying it again**, which is
  what the entry's second question actually needed. The join is now
  `clausters_core::peaks::join` (plus `join_columns` over a whole measured row),
  so there is **one implementation** and both drawings call it: the host per
  column, keeping only the walk a run of columns needs, and this client through
  `data.joinColumns(row)`. The measurement is untouched and stays what the two
  clients compare and the cache stores — `columns` answers what the pyramid
  measured, `joinColumns` answers what to ink — which is the constraint the
  entry set, met by putting the rule *beside* the measurement rather than inside
  it. `tests/data.html` and `examples/scope.html` (which loses its hand-copied
  recipe) both go through it, the web book's waveform section teaches it, and
  the case is asserted on both sides: `peaks::join_tests` in the core, "a square
  wave's edge is inked once the columns are joined" in `data.test.ts`.
  `docs/bindings.md` carries the wasm row; the C ABI has none, since nothing on
  that side strokes pixels.
