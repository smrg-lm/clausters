# Plan - Clausters web client (TypeScript, browser-first)

The TypeScript client: a high-level client of `clausters-server` and of the browser GUI host, the web sibling of `clients/python`. It is its **own package, docs, examples and tests** under `clients/web`, exactly the way the Python client lives under `clients/python` - a parallel client track, not part of the GUI host (`clients/gui`) and not part of the server.

> **Read alongside `clients/PLAN.md`.** That plan already covers the *shared* client rationale (the native Rust core, the C-ABI/wasm seam, the "only flat data crosses" boundary, the coroutine-driver-stays-in-the-language rule) and was written from the start "to serve a future JavaScript client too". This file is the web-specific track; it does not restate the shared design, it points at it and records only what is different in the browser. As there, **sc3 is the reference model** for module structure, clock/routine behaviour, events, patterns and OSC/MIDI semantics; deviate only with an explicit reason.

## Why a separate track (and why now)

The browser GUI host (`clients/gui`, milestones G11-G16) runs the `/gui_*` widget system under WebGPU and is driven *through* a small wasm binding surface or over WebSocket. Those milestones deliberately use a **throwaway** in-page harness to test the host; the real, product driver - the thing a web app actually programs against - is this TypeScript client. Keeping it here, separate from the host, preserves the same boundary the rest of the system keeps: the host is a front, the client is a consumer of the protocol, and either can change without dragging the other.

So the web client is one more consumer of the exact same wires the Python client uses: OSC-in-JSON GuiDefs to the GUI host, OSC to the audio server, the same `decode_packet` framing - only the carrier (browser `WebSocket`), the binding technology (wasm, not ctypes) and the host language (TS, not Python) differ.

## Guiding principles

- **Maximum reuse; the browser only adds its I/O.** What is value or time transformation is shared, not re-implemented: OSC assembly/decode, TempoClock arithmetic, the numeric builtins and the analysis kernels (peaks/FFT) come from **`clausters-core` compiled to wasm** (via wasm-bindgen), so the client is **numerically equivalent to the Python client and to the server by construction**. The GuiDef/`/gui_*` protocol and the def specs are the same JSON the Python builders emit. New TS code is confined to: the language-side control flow (generators/async routines), the browser carriers (`WebSocket`, Web MIDI, Web Audio clock, `fetch`), and the ergonomic builder/typed API.
- **The seam is the same as `clients/PLAN.md`.** The Rust core owns builtins, TempoClock (queue + arithmetic) and OSC bundle/timetag assembly + sample-clock conversion; the coroutine driver (`function*`/async in TS, sc3-style routines) stays in the language; no Rust callbacks into JS - the loop asks the wasm queue "what's next and when?", sleeps on the browser clock, resumes the routine. "Only flat data crosses" the wasm boundary (typed arrays / numbers / strings in and out, no callbacks).
- **Client/server separation, as in the Python client.** Timing/sequencing/GuiDef authoring is transport-agnostic; only a `Server`/`GuiHost` object knows the connection. The `TempoClock` must not talk to the server (the same rule corrected in the Python client's C4).
- **Browser realities are first-class, not afterthoughts.** WebSocket is the only *network* transport (no UDP, no shared memory, no mmap); since the server's B track, the browser also has a second, process-free carrier — the **in-page engine** (the server compiled to wasm in an AudioWorklet, reached through the B4 package's `server()` singleton) — and the client stays carrier-agnostic above a small connection seam. Bulk data arrives by `fetch`/`/b_getn`; meters/scopes read control buses over the wire; the sample-clock timebase uses the Web Audio clock (`AudioContext.currentTime`). These are the same "async fallbacks" the server/gui plans reserved for the browser.

## Target architecture

A TS package mirroring `clients/python`'s shape — the `src/` module tree at the same relative paths as `clausters/`'s modules, `dist/` reproducing it 1:1, and `examples/`/`tests/`/(W5) `docs/` beside them — so a reader who knows one finds the other. This is the **only web directory in the repo**: every browser JS/HTML artifact (package modules, the engine's worklet/loader runtime, examples, test pages, tools) lives here, and the wasm crates stay Rust-only, their wasm-bindgen bundles staged in by `build.sh` (see `docs/decisions.md`, "The web front-end lives in one package"). The layout as it stands after W1 (parenthesized entries are where the later milestones grow):

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
    (responders.ts        #   OscFunc/MidiFunc dispatch (mirrors responders.py) — W8/W9)
    (session.ts           #   the Session facade — W5)
    engine/               #   browser-only: the in-page engine runtime
      worklet.ts  loader.ts  worklet-shim.ts  server.ts (the server() singleton)
    bundle.ts elements.ts #   browser-only: bundle boot + the custom elements
  dist/                   # emitted src/ 1:1 (.js + .d.ts + maps) + the staged wasm
                          #   bundles engine/ gui-host/ core/ — the _bin/_libs analog
  examples/               # synth.html, demo.html, engine.html, gui-host.html,
                          #   standalone.html (the Python examples port here — W5)
  tests/                  # node --test suites + parity vectors (osc, def, gui)
                          #   + the browser acceptance pages (client/defs/gui/
                          #   smoke/parity)
  tools/                  # bundle-manifest.py, demo-bundle.sh
  (docs/                  # an mdBook (mirrors clients/python/docs), API ref via typedoc — W5)
```

The wasm `clausters-core` build is **shared with the GUI host** (G11-G16 already needs core compiled to wasm); this client links the same artifact, it does not produce a second one.

## Tooling (decided 2026-07-18, at W0's start; the no-heavy-deps rule)

The repo-wide posture — minimal, user-space, reproducible — applied to the JS toolchain. B4 already established the package's shape (plain browser-native ES modules, wasm bundles as static assets, served as-is); the toolchain must preserve it, not fight it.

- **node LTS under `~/.local`, no sudo** — the same pattern as libfaust. The recipe (kept current in `clients/web/BUILD.md` once W0 lands): download the `linux-x64.tar.xz` of the newest LTS from nodejs.org/dist, verify against `SHASUMS256.txt`, extract to `~/.local/lib/`, symlink the versioned dir to `~/.local/lib/node`, and symlink `node`/`npm`/`npx`/`corepack` into `~/.local/bin` (already on `PATH`). Installed 2026-07-18: v24.18.0 (npm 11.16.0).
- **`typescript` is the only package dependency** (dev-only; v7, the native compiler — a single package, no transitive deps; `@types/node` rides along for the test files, type declarations only). `tsc` does both jobs: **type-checking** (`tsconfig.json`, src + tests, no emit) and **emitting** (`tsconfig.build.json`: `src/` → `dist/` module-per-module, with declarations and source/declaration maps — the browser interface is JS with a type map). Imports between our modules are written with `.ts` extensions and rewritten on emit (`rewriteRelativeImportExtensions`), which is what lets node run the sources directly; the output is the same plain servable ESM the B4 modules were. The dev loop is `tsc -p tsconfig.build.json --watch` + `python3 -m http.server`.
- **No bundler.** Nothing here needs one: the package ships unbundled, the wasm bundles and the worklet module must stay static assets anyway (`AudioWorklet.addModule` and bundlers are a known friction), and the browser loads bare ESM natively. Evaluated and not adopted: **vite** (a dev server with HMR plus rollup/esbuild underneath — tens of MB of dev machinery whose two roles are already covered by `http.server` and `tsc --watch`; revisit only if HMR-grade DX is genuinely missed), **esbuild** (only earns its place when bundling), **vitest** (pulls vite in as its platform).
- **Tests: `node:test`, built into node — zero dependencies.** Node runs `.ts` directly (native type stripping, default since 23.6), so pure-logic tests (codec parity, clock arithmetic, builders) run straight from source with `node --test`, no compile step, no runner package. Browser-only behavior (audio, canvas, the elements) keeps the B-track posture: headless-Chrome smoke scripts with the access-log beacon.
- `typedoc` (the W5 API-reference generator) gets evaluated under this same lens when W5 starts.

## Milestones

Labels (`Wx`) live only here, never in published docs or docstrings - the same rule as the other plans.

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

Not in scope here, by the plan's own division: the exhaustive UGen/`signals`
catalogue (the set the acceptance and the examples exercise is in — sources,
filters, delays, panning, envelopes, triggers, bus and buffer I/O, the demand
pair, the full operator tables), the box API, and the bulk/streaming data
paths.

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

Not in scope here, by the plan's own division: the browser data paths the
heavy views feed on (`/c_stream` decoding client-side, `fetch`/`/b_getn` bulk,
the wasm peak pyramid) and the `correlation`/`lissajous` analysis exports —
the host already reads those paths itself, so a GuiDef that names a bus, a tap
or a URL works today.

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

Not in scope, by the plan's own division: `automation` (a break-point control
curve; it pulls in buffers, `Env` and a control def), `MidiEvent` and MIDI
destinations, the shared `/transport` grid, and an NRT/score drive - the
client has no score interface, and `Timeline.fromPattern` bounces by driving
the ordinary clock through its manual seams.

### W4 - Reserved

An intentionally empty slot, held open here because whatever lands in it
belongs in the client **before** the documentation and the first publication.
Its content is not specified yet. What this milestone used to name — the
responders, MIDI, and the browser data paths — moved to the milestones after
W5, none of which is a prerequisite of W5.

### W5 - Docs, examples, tests, packaging

Make it a real, shippable client.

- An mdBook in `clients/web/docs` (mirroring `clients/python/docs`), with the API reference **generated from TSDoc by typedoc** (the TS counterpart of the Python client's pydoc-markdown), and the GUIA-style manual-testing notes kept current. The two client books and the two GUI books cross-link by their RTD URLs.
- The Python examples ported to TS (either carrier), the `node --test` suite, and the npm package build/publish; a parity pass against the Python client on the shared vectors (OSC, clock arithmetic, GuiDef JSON).

**Acceptance:** `npm install clausters` (or the workspace build) yields a usable client; the ported examples run in a browser over either carrier (the in-page engine, or a `--ws` server) with the browser GUI host; the docs build and deploy like the Python client's.

### Milestones deferred out of W1-W3

Everything below was left out of W1, W2 or W3 as those closed, and is now its
own milestone. They have **no fixed sequential order** — each is independent
and gets tackled when it is wanted, the same convention the client track uses
past C10 — and each *widens a layer that already exists* rather than opening a
new one, which is why none of them blocks W5: the client is shippable without
them, just narrower than the Python one.

### W6 - The full UGen / `signals` catalogue

W1 shipped the def model with representatives of each family; this fills the
families out, so a graph written against the Python client ports by
transcription rather than by lookup.

- `defs/ugens.ts`: the rest of the server's UGen catalogue — sources, filters, delays, panning, envelopes, triggers, bus and buffer I/O, the demand pair, the spectral chain (`fft`/`ifft`/`pv_*`, the client side of S8), the output-less roots (`sendReply`/`sendTrig`/`poll`), and the complete unary/binary operator tables.
- `defs/signals.ts`: the same for the Faust signal API, up to the whole surface `clausters.defs.signals` exposes.
- The W1 composition rule is unchanged: TypeScript has no operator overloading, so operators stay methods and parity is asserted on the **emitted spec**, never on the source.

**Acceptance:** every builder the Python client exposes has a TS counterpart emitting the same spec JSON, checked by extending the frozen vectors (`tests/gen-def-vectors.py`); a graph transcribed from a Python example plays over either carrier (the Faust half WS-only, as ever).

### W7 - The box API: Faust's box algebra

The TS counterpart of `clausters.defs.boxes` (C22) — the third def-authoring
surface, beside the UGen graph and the signal API.

- `defs/boxes.ts`: the point-free algebra (`seq`/`par`/`split`/`merge`/`rec`, `wire`/`cut`, controls, tables) emitting the same box-tree JSON the Python builders emit, plus `faust(src, ...)` to fold a Faust source expression — its libraries (`fi.`/`os.`/`re.`/`pm.`) included — into a composable `Box`.
- WS-only by nature, like every Faust path in the browser: the wasm engine is the `synth,embed` build with no LLVM JIT, so a box def compiles only against a native server.

**Acceptance:** the Python box-API examples rebuilt in TS emit byte-identical spec JSON (new frozen vectors), and one of them compiles and plays against a `clausters --ws` server.

### W8 - Responders: `OscFunc` over the reply stream

The client's input path and its role as a general OSC hub (sclang's `OSCFunc`),
mirroring the half of `responders.py` that does not involve MIDI.

- `responders.ts`: pattern-matched dispatch over the connection's reply stream — either carrier exposes it through the W0 seam (`addReply`), so nothing here names a transport — with handlers scheduled on a clock rather than run from the socket callback, the browser counterpart of the Python client's "never block the clock thread".
- The reply handling W1 grew ad hoc inside `Server` (the dispatch table and the `/sync` barrier) folds onto this one door, so everything arriving comes in the same way.

**Acceptance:** a TS app registers and unregisters `OscFunc` handlers that fire on server notifications (`/n_go`/`/n_end`, `/done`, `/tr`) over either carrier, and the W1/W3 end-to-end suites stay green through the new door.

### W9 - MIDI: `MidiFunc` in, `MidiEvent` and MIDI destinations out

Both directions of MIDI in one milestone, since in the browser they are one
API: Web MIDI is the only MIDI I/O a page has.

- `MidiFunc` over `navigator.requestMIDIAccess`, mirroring `responders.py`/`base/_midiinterface` — note/cc/program dispatch, port selection, handlers scheduled on a clock; convenience responders turn notes into `/s_new`, as C13 does.
- `MidiEvent` and MIDI as a **destination** of the sequencing layer: an event stream plays to a MIDI output exactly the way it plays to a `Server`, over the `play(destination)` seam W3 established, mapping `Event` → channel-voice messages the way `clausters-midi` already defines them for C11.
- Timing stays **best-effort by design**, as C18 settled for the Python client — but the browser gives it back cheaply: `MIDIOutput.send(data, timestamp)` takes a `performance.now()` deadline, so the driver hands over the deadline it has already computed instead of sleeping to it.

**Acceptance:** a pattern plays to a browser MIDI output on the same beat grid it plays to the audio server, and a `MidiFunc` on an input port drives defs on the server, over either carrier.

### W10 - The browser data paths: buses, bulk, and the analysis exports

The paths the heavy views feed on, read by the **script** this time. The host
already reads them itself (that is why a GuiDef naming a bus, a tap or a URL
works today); this is the client getting the same numbers.

- Control buses over the connection: `/c_stream periodMs bus...` subscribed from the client and its periodic `/c_set` snapshots decoded in a responder (W8's door) — the message-based counterpart of the native host's shared memory (G14). The server side exists on both carriers already: one subscription per client, replaced per call, `periodMs <= 0` cancels, 10 ms floor, ≤128 buses (`docs/schemas.md`), and B3 left `/c_stream`/`/tap_stream`/`/b_getn`/`/clock` streaming over the in-page leg too.
- Bulk buffers by `fetch`/`/b_getn` (G15), with the **peak pyramid built in wasm** from the fetched samples, so a waveform draws at screen resolution without a second implementation of the reduction; plus the fetch + `decodeAudioData` → `bLoad` sample path B3 left in `bundle.ts`, folded into the client's buffer API.
- The core's `correlation`/`lissajous` analysis exports surfaced to TS.

**Acceptance:** a TS app reads a control bus and a buffer over either carrier and draws them **itself** (a canvas the script feeds, not a host-fed widget), numerically matching what the GUI host draws from the same source.

### W11 - Automation: a break-point curve as a control vector

The TS side of C23 — deferred out of W3 because it is the one sequencing piece
that is not pure timing: it pulls in buffers, `Env` and a control def.

- `seq/automation.ts`: a break-point curve discretized into a control buffer on the server (`/b_gen "env"`, the server's own `envshape`) and read back onto a control bus by a lane synth (`OutCtl`), prepared without blocking the driver, then played and freed like any other element.
- The `bpf` builder already in W2's GuiDef catalogue becomes its editor, so the curve is authored, heard and edited over the same loop the multitrack editor uses.

**Acceptance:** a curve authored in TS drives a synth's control over either carrier with the same values the Python client produces for the same break points, and dragging it in the browser GUI's `bpf` widget moves the sounding value.

### W12 - The shared `/transport` grid

Phase alignment across clients — the TS counterpart of C15 and of C16's
`follow_transport`.

- `quant` honored when a routine starts (snap to a beat boundary), and joining the server's `/transport` grid so pages started at different moments share one bar line.
- Following the `/transport_play|stop|locate` broadcasts, so a page's playhead rolls in lockstep with every other client: the server broadcasts control, never audio.
- W3's rule is not bent: the clock still never talks to a server. The `Server` feeds the grid inward, the way `sampleTimebase()` already anchors the timebase.

**Acceptance:** two pages — or a page and a Python client — join the same transport and land on the same bar; a `/transport_locate` moves the page's playhead with it.

### W13 - An NRT / score drive

The third `Server` interface, beside the two carriers: a destination that
*writes* time instead of waiting for it.

- A score destination for the sequencing layer — the same pattern or timeline that plays live emitted as a timestamped score — rendered either by a native server's NRT mode over WS, or in-page by the wasm engine running faster than real time into a buffer the page can play or download.
- W3 already *bounces* without one (`Timeline.fromPattern` drives the ordinary clock through its manual seams); what is missing is the interface, not the arithmetic.
- Score parity is the check C5 keeps on the Python side: one piece, one score, compared byte for byte.

**Acceptance:** a piece written once emits a score byte-identical to the Python client's for the same input, and renders from the browser to a WAV that matches the native NRT render.

## Future directions

- **Node target.** The same package outside the browser (a Node `WebSocket` carrier and the same wasm core) for headless scripting/CI, the way `clients/python` runs without a display.
- **Type-safe GuiDef/def schemas.** Generate TS types for the widget/def vocabularies from a single source shared with the server, so an invalid GuiDef is a compile error, not a runtime warning.
- **A remote-server standalone page.** The in-tab standalone (a bundle booting against the embedded wasm engine) **shipped with the B track** (B3/B4: `bootBundle`, `<clausters-bundle>`, `examples/standalone.html`); what remains as a future direction is the same page against a **remote `--ws` server** — the bundle's GuiDef and defs replayed over the WS carrier instead of the in-page one, a one-file instrument front for a server running elsewhere. Cheap once W1/W2 exist: the boot replay is carrier-agnostic above the W0 seam.
