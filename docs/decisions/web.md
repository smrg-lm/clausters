# The web client and the engine in a page

Part of the [design record](../decisions.md), whose index lists every
entry's title.

## Native↔wasm render parity is a tight tolerance, not bit-identity

Context (the B track): the engine compiles to `wasm32` and the B0 acceptance
compares the wasm render of a score against the native NRT render of the same
bytes. Two platform facts make strict bit-identity the wrong bar there — and
only there (same-platform RT ≡ NRT stays bit-exact, unchanged):

- **Different libm implementations.** Native Rust lowers `f64::sin` & co. to
  the system libm; `wasm32-unknown-unknown` uses Rust's own libm port. The two
  legitimately round a few ULP apart — the same fact `tests/golden.rs` already
  records for its cross-platform tolerance. UGens call transcendentals per
  sample (`Sine` is `phase.sin()`), so the difference is unavoidable without
  forcing one libm on every target (a rejected option: pinning the Rust libm
  natively would change the native sample stream and change nothing users can
  hear).
- **No flush-to-zero on wasm.** wasm has no FTZ/DAZ mode, so a render that
  enters the denormal range diverges from the FTZ-armed native engine. The
  parity scene is therefore required to be denormal-free (the generator
  asserts it), which keeps this factor out of the comparison entirely.

The bar chosen: max |delta| ≤ 1e-6 with the bit-exact count reported —
measured reality is 47990/48000 samples bit-exact and max delta 1.5e-8 (one
f32 ULP at signal scale) on the B0 scene, so the tolerance is three orders
above the legitimate noise and three below any plausible DSP bug. The harness
is `scripts/parity-web.sh` (native fixture generator + headless-Chrome page,
the same scripted-page pattern as the GUI's `web/parity.html`).

**Revisited 2026-10-09, with the cost measured, and kept.** The user asked for
the same bits in every client on every platform, and one libm on every target
was built and timed (`clients/python/PLAN.md`, `C63`). The values were not the
obstacle — the native stream moves by an ulp, inaudibly — the audio thread
was. The system's `f32` functions (glibc 2.43, x86-64) are correctly rounded
and fast, and the `libm` crate is not: per call `sin`/`cos` 1.2x, `exp`
1.4-1.8x, `tan`/`tanh` 2.4-2.8x, `powf` 7.5x, and its `exp10f` is off a
correctly rounded result on ~40% of arguments. With every cheaper route
written in — `exp2` for powers of two, `f64` rounded once for `exp10`, `tanh`
and a positive base's `pow` — an operator over an audio-rate signal still cost
1.5x to 1.8x per sample, and `pow` with a moving base 3.4x. The tolerance
stays; what the attempt left in the tree is the measurement
(`examples/bench.rs`, `bench_math`) and two optimizations it uncovered, which
stand on any libm: `EnvGen` prepares a segment once (`envshape::Segment`), and
a fixed-base power is one `exp2` (`warp`). Reopening this is a question of a
portable libm that matches the system's speed, and the bench section is what
answers it.

## The browser engine is one wasm instance inside the AudioWorklet, spoken to over the MessagePort

Context (the B track): the live browser backend has to host GUI
components that embed on **arbitrary pages** — documentation sites, notebooks,
pages we do not control the headers of.

Decision: one wasm instance — OSC translate + engine together — lives inside
the `AudioWorkletGlobalScope`. OSC bytes travel over the node's MessagePort in
both directions; commands cross into the engine through the same in-memory
ring the native embed mode uses (`Segment::in_memory`), and each 128-frame
render quantum runs one serving turn before its two 64-frame engine blocks
(the pulled `step()` pacing, tested natively in `tests/headless.rs`).

- *(Revised 2026-10-23: this requirement did not hold up, and an editing
  application in a page requires cross-origin isolation and shares memory;
  see "An editing application holds its engine, and the server holds no
  application's lists".)*
- **No SharedArrayBuffer, by requirement.** SAB needs COOP/COEP isolation
  headers, which an embeddable component cannot demand of its host page. The
  MessagePort path has no header requirement at all. The ring seam keeps a
  later SAB/wasm-threads build (a zero-message in-page `BusSource`) open as an
  optimization, not a redesign — and the optimization has since been priced,
  which is what settles it. On a page holding forty canvases, each drawing a
  control-rate meter and a control-rate scope, one `/bus_stream` frame is 824
  bytes and taking it in costs about 29 microseconds of the main thread per 33
  ms frame: **a tenth of a percent of the frame**, some 25 kB/s, with the
  page's frame rate and event-loop lag unable to tell a streaming phase from
  the same page with the subscription cancelled. The cost is per call rather
  than per byte (sixty-four canvases: 1304 bytes, the same 28 microseconds).
  So the zero-message path would save a tenth of a percent of a frame, and
  cost isolation headers an embedded component cannot ask for plus a second
  backend for one seam; the MessagePort carrier is not a compromise at
  this scale, it is the right size. The measurement is reproducible —
  `clients/web/tools/profile-bus-stream.sh` — and reopening the question means
  a profile that shows the messages costing something, not an argument.
- **The one relaxation vs. the native RT rules**: OSC→Cmd translation
  allocates on the worklet (audio) thread. wasm malloc is a bump over linear
  memory — no page faults, no locks, no priority inversion — and the DSP
  itself stays allocation-free, so the native no-alloc discipline keeps its
  value without being extended to a heap that cannot misbehave the same way.
  That holds only while the bump stays inside the memory the module already
  has: a bump past the end is a `memory.grow`, which is a host call that may
  copy the whole heap and **detaches** the `ArrayBuffer` and every JS view over
  it. So the engine reserves its linear memory at link time — 16 MB, against a
  boot that lands at 4.3 MB and a rustc default of 1.5 MB, with a 256 MB
  ceiling well under the ~350 MB where iOS Safari kills a tab
  (`--initial-memory` / `--max-memory` in `crates/clausters-web/build.rs`,
  asserted by `clients/web/tests/memory.test.ts`). The claim above is about a
  heap that does not grow; the reservation is what makes it one.
- **Synchronous instantiation, async compilation.** The main thread
  `WebAssembly.compileStreaming`s the module and passes it through
  `processorOptions` (a `WebAssembly.Module` is structured-cloneable); the
  processor constructor runs wasm-bindgen's `initSync` — the worklet never
  awaits anything, and is live from its first quantum.

Two findings worth keeping with this record:

- **The worklet scope has no `TextDecoder`**, and the wasm-bindgen glue
  instantiates one at module-evaluation time. The fix is import order:
  `worklet.js` imports `worklet-shim.js` (a minimal UTF-8 `TextDecoder`)
  *before* the glue — ES modules evaluate dependencies in import order, so
  the shim is installed when the glue's top level runs.
- **Chrome's `--virtual-time-budget` races timers ahead of the audio clock**,
  so the B0 parity trick (dump the DOM after a virtual-time budget) cannot
  drive a smoke that waits on real audio progress. `scripts/smoke-web.sh`
  instead runs Chrome in real time and has the page beacon its verdict as a
  `fetch` of `/smoke-verdict-…`, read from the HTTP server's access log. Every
  web acceptance since is written that way, and one runner walks them all.

## The browser bundle boot replays the persisted files over the wire; the only new artifact is a manifest

Context (the B track): a standalone bundle must boot in a tab exactly as
`clausters-gui --standalone` boots it natively. Natively the *embedded server*
loads the data directory itself (defs, `boot.json`) and the host only replays
the GuiDef; a browser has neither a filesystem nor an embedded loader.

Decision: the browser boots the **same persisted files, fetched as URLs and
replayed as ordinary OSC** to the in-page engine — no browser-specific bundle
format. The split of labor is deliberate: the boot's **ordering and encoding**
live in the GUI host's platform-agnostic `host::bundle` module (natively
unit-tested; it mirrors the server's own boot order — defs → graphdefs → boot
preset → the GuiDef's `boot` messages), while the **fetching** stays in page
JS (`crates/clausters-gui/web/bundle.js`). The replay is bracketed by two `/server_sync`s: the
engine serves strictly in order, so the trailing `/server_sync.reply` is the page's
"bundle is up" signal — no per-command acking.

- **The one addition is `bundle.json`**, a manifest at the bundle's root
  naming the def files, because HTTP cannot list a directory the way the
  native store lists it. It is generated (`web/bundle-manifest.py`), never
  hand-maintained, and also carries the one genuinely browser-side mapping:
  which audio URL feeds which server buffer (fetch + `decodeAudioData` →
  the engine's `buffer_load` — the browser's `/buffer_allocRead`, decoded by the host
  page because the wasm engine has no sndfile).
- **The in-page leg is one more `ServerLink` variant, not a new protocol.**
  `ServerLink::Page` hands outbound packets to a page-registered callback and
  takes replies through `GuiBridge.server_reply`; the host's streamed data
  paths (`/bus_stream`, `/bus_tapStream`, `/buffer_getRange`, `/clock_query`) run over it
  unchanged — the acceptance smoke watches the meter's `/bus_stream.reply` stream arrive
  with moving values.
- MIDI bindings, the remaining thing the native data-dir boot restores, are
  deliberately not replayed: the browser has no MIDI leg.

## The web package is plain ES modules over per-page singletons; the canvas moves to the last-booted element

Context (the B track's capstone): the web components had a deferred design
("converges when the milestone starts"). What converged:

- **Plain ES modules, no toolchain.** The `clausters` package in
  `clients/web/` ships as ES modules a static server can serve as-is; a
  `build.sh` stages the two wasm bundles (`engine/`, `gui-host/`) next to
  them. No bundler and no node dependency today — the TS track (W0) adds its
  toolchain *around* this package later, rather than this milestone
  front-loading one the repo cannot even run (no node on the dev machine, and
  nothing here needs it).
- **Two lazy per-page singletons, wired once.** `server()` (the engine) and
  `guiHost()` (the GUI host) boot on first use; the in-page server leg —
  engine replies → `GuiBridge.server_reply`, host outbound → `engine.send` —
  is wired inside `guiHost()`'s first boot, exactly once. The engine handle's
  single `onReply` slot is owned by the singleton and fanned out to a
  listener set, so any number of components, watchers and REPL scripts
  coexist; per-boot `/server_sync` ids keep concurrent bundle boots from mistaking
  each other's `/server_sync.reply`.
- **One canvas, adopted by the last-booted element.** The browser GUI host
  shows one window-rooted def on one canvas (its long-standing shape), so
  `<clausters-bundle>` does not pretend otherwise: booting an element moves
  the page's host canvas into that element's shadow DOM (re-parenting
  preserves the GPU context and winit's listeners). Multiple elements share
  the engine namespace today and take turns showing; per-element windows are
  a host-side multi-window question, not a packaging one.
- **The gesture is the element's power button.** The autoplay affordance is
  standardized in the components themselves (`<clausters-bundle>`'s boot
  button, `<clausters-power>` alone for raw-singleton pages), not left to
  each embedding page.

## The web client toolchain is tsc + node:test, nothing else; OSC parity is a committed vector file

*Extended 2026-11-12: a linter joined it, under the same rule ("The web client
lints with Biome, a linter that is one package").*

Context (the TS track's start): the web client needed a JS toolchain, and the
default answer in that ecosystem (vite + vitest, or any bundler) contradicts
both the repo's no-heavy-deps posture and the package's already-settled shape
(plain servable ES modules, wasm bundles and the AudioWorklet module as
static assets — the things bundlers fight).

- **`tsc` is the whole toolchain**: type-checker and emitter (module-per-module,
  `.js`-extension imports, output identical in shape to the hand-written B4
  modules). TypeScript 7 (the native compiler) is a single package with no
  transitive dependencies; `@types/node` rides along as declarations only.
  The dev server stays `python3 -m http.server`; the evaluation of
  vite/esbuild/vitest and the reasons each was declined are recorded in
  `clients/web/PLAN.md` ("Tooling").
- **Tests run from source under `node --test`** — node's native type
  stripping (default since 23.6) runs `.ts` directly, so the pure-logic
  suites need no compile step and no runner package. Browser-only behavior
  keeps the B-track posture (headless-Chrome smokes, access-log beacon);
  `clients/web/test.sh` is the one entry that runs all three layers.
- **Codec parity is a frozen artifact, not a convention.** The TS codec goes
  through `clausters-core` compiled to wasm (`crates/clausters-core-web`), so
  it is the server's codec by construction — and the tie to the *Python*
  client is held by `clients/web/tests/osc-vectors.json`, generated from
  `clausters.base._osclib` and committed. Both clients answer to the same
  frozen bytes; regenerating the file is an explicit act (new cases), never
  part of a test run.

## The web client lints with Biome, a linter that is one package

*Decided 2026-11-12, extending "The web client toolchain is tsc + node:test,
nothing else".* The package's pages (`tests/*.html`, `examples/**`) and node
scripts are plain JavaScript that no type-checker reads, so an undefined name
or a variable left behind there is found by running the page or not at all,
and `tsc` itself does not report an unused import or variable. A linter closes
that, and the toolchain rule decided which one: **Biome**, whose rules run in a
native binary delivered as one optional dependency per platform, with no
JavaScript dependency of its own, the posture TypeScript 7 was taken for. The
established alternative is a linter plus a TypeScript parser and its rules,
several hundred packages for the same checks.

Two rule groups are on, the ones that name a bug rather than a style:
correctness and suspicious, less the rules that are a style of typing or of
writing (`noExplicitAny`, an assignment inside a condition, a callback's return
value, an unused parameter, an approximate constant, a declaration merged with
a class, a `let` with no type). The formatter stays off: the package has no
formatter and this does not add one. `npm run lint` runs it with warnings as
errors, `test.sh` runs it after the type-check, and CI's web job runs it. A
`biome-ignore` carries its reason on the line, as a `noqa` does in Python.

## The web front-end lives in one package that mirrors the Python client

Context: the B track left the browser JS/HTML where each milestone was born —
the worklet/loader runtime and its harnesses beside the engine crate
(`crates/clausters-web/web/`), the standalone page, the bundle fetch module
and the manifest generator in the GUI host's harness directory
(`crates/clausters-gui/web/`), and the package itself in `clients/web/` — with a
two-hop staging chain copying the engine bundle between them, the bundle boot
implemented twice (the harness's single-consumer `bundle.js` next to the
package's `bundle.ts`) and the interim page codec still shipped after the
core-backed one replaced it. Unmaintainable as the client track grows.

- **`clients/web` is the only web directory.** All browser JS/HTML — package
  modules, the worklet/loader runtime, examples, test pages, tools — lives in
  the `clausters` package; the crates (`clausters-web`, `clausters-core-web`,
  `crates/clausters-gui`) stay Rust-only and their wasm-bindgen glue is staged into
  the package by `clients/web/build.sh`, directly, with no intermediate
  copies. The duplicates died: `bundle.js` was absorbed by `bundle.ts` (the
  standalone page now boots over the package singletons) and the interim
  codec was deleted.
- **The package structurally mirrors `clients/python`.** Sources under
  `src/` at the same relative paths as `clausters/`'s modules (`base/`,
  `gui/`, later `defs/`/`seq/`/`responders`/`session`), with `examples/`,
  `tests/` and (later) `docs/` beside them; `dist/` reproduces the `src/`
  tree 1:1 and the wasm bundles staged inside it (`engine/`, `gui-host/`,
  `core/`) are the browser's `_bin`/`_libs` — binary artifacts inside the
  installable tree, no source beside them.
- **Node-TS conventions, servable output.** Sources import each other with
  `.ts` extensions (`rewriteRelativeImportExtensions`), so node runs them
  directly (tests never need a build); `tsc` emits `dist/` with declarations
  and source/declaration maps — the browser interface is JS with a type map,
  ready for in-page typed consumption (livecoding). Imports of the
  wasm-bindgen glue keep `.js` and resolve against staged copies (full
  bundles in `dist/`; `.d.ts` stubs — plus the core's glue `.js` for
  node-from-source — mirrored into `src/`).
- Consuming the package from node to control local servers (a native
  WebSocket carrier, headless scripting) is a **separate future feature**
  ("Node target" in `clients/web/PLAN.md`), deliberately not folded into the
  consolidation.

## The TypeScript graph composes by method, and the wire is what parity means

The Python client builds a UGen graph with operators — `sine(freq) * amp`,
`1 - env`, `sig % 2` — because Python lets a class define them. TypeScript does
not: there is no operator overloading, and no proposal close enough to plan
against. So the web client's graph composes by **method**: `sine(freq).mul(amp)`,
and every other operator or math function is a method carrying the same operator
**name** the wire uses (`.midicps()`, `.max(x)`, `.distort()`). Where the
constant is on the left, which a method cannot express, there are free functions
(`sub(1, sig)`) and, on the Faust side, the explicit `rsub`/`rdiv`.

Two consequences worth stating, because they look like drift and are not.

**A method name is not always the wire name — and the method's is
SuperCollider's, not the language's.** The selector *is* what crosses (`asint`,
`hypotapx`, `recip`, `lshift`), and it stays as the def format spells it: those
names are in stored SynthDefs, in `docs/schemas.md` and in the frozen parity
vectors, so renaming one is a format change and not a spelling preference. (This
paragraph said "renaming them would be a format change for a cosmetic gain" and
used `as_int`/`hypot_apx` as its examples, which is the sentence three
underscores hid behind: the wire's own convention is lowercase and **joined**,
those three broke it, and a convention with three exceptions is not one. They
were renamed on 2026-08-24, at the price the paragraph names and no more —
`from_name` resolves the underscored spellings for good, so a def stored before
the rename loads and nothing emits them again.) What a
*user* types is a different question, and it was answered wrong at first — the
web client took the idiomatic TypeScript spelling (`asInt()`, `hypotApx()`)
while the Python client took a third one (`abso`, `as_int`, `recip`), so the two
clients disagreed with each other **and** with the language they are a port of.

The rule now: **a client's name is SuperCollider's, all lowercase, joined** —
`asinteger`, `asfloat`, `reciprocal`, `leftshift`, `rightshift`, `hypotapx`,
`abs`. Lowercasing sclang's camelCase is not new; it is what the operator table
already did for `bitand`/`bitor`/`bitxor`, now applied to the six that had
drifted. So a reader who knows SuperCollider types the same call in both
clients, and neither client's idiom overrides the vocabulary. The mapping from
that name to the wire selector lives in the class, in one line each.

**Parity is asserted on the emitted spec, not on the source.** The two clients
cannot be compared expression for expression, so `clients/web/tests/def-parity.
test.ts` builds each reference graph independently in TypeScript and asserts the
`SynthDefSpec`/signal-tree/`GraphDefSpec` JSON is identical to what the Python
builders emit for the same graph (frozen by `gen-def-vectors.py`). That is the
right seam anyway: the def format is the shared contract, and two clients
agreeing on it is exactly what "numerically equivalent by construction" is for.
The vectors cover what the source alone cannot show — control dedup and
first-seen order, the topological walk, the fused `Sum4`/`Sum3` mix fold, the
`ir`/`tr` control types and lags, generic op naming, and the `sr()` clamp inside
a Faust recursion.

The same reasoning settles a smaller one: a JS number is a double, so the client
cannot infer int-vs-float from a value the way Python does. The `Server` therefore
tags **by position** — node ids, bus indices and add actions as int32, control
values as float32 — and only the free-form `sendMsg`/`genBuffer` guess (an
integral number is an int32), with an explicit `[tag, value]` pair as the escape
hatch where the guess is wrong (`/buffer_gen`'s flag word).

## A GuiDef from TypeScript: the options are the language's, the document is the wire's

The Python GuiDef builders take keyword arguments spelled exactly like the props
they emit (`text_size=3.0`, `base_bucket=512`, `sel_start=…`), because in Python
those are the same identifier. TypeScript's are not, and a client whose surface
reads as snake_case would be the odd one in its own language — the def builders
already took the other road (`control("cutoff", 800.0, { lag: 0.1, lagDown: 0.5 })`).
So the web GuiDef builders take **camelCase options** and write the host's
**snake_case props**: `waveform({ baseBucket: 512, selStart: 0.0 })` emits
`{"base_bucket": 512, "sel_start": 0.0}`. A prop this client does not name yet —
a newer host's — passes straight through under its wire name, the way Python's
`**props` does, so the vocabulary can grow without a client release.

**Parity is asserted on the emitted document, not on the source**, for the same
reason the def specs are (`clients/web/tests/gui-parity.test.ts` against vectors
frozen by `gen-gui-vectors.py`) — and there is a second reason here. A JSON
number from JavaScript carries no int/float distinction: `JSON.stringify(480.0)`
is `480`, where Python writes `480.0`. That looked like a parity problem and is
not, because the host reads every continuous prop through `as_f64` and every id,
index and count as an integer — an integer literal is a perfectly good float on
the way in. What the vectors compare is therefore the **parsed** document, which
is exactly the level at which the two clients have to agree. The int/float
distinction stays load-bearing where it actually rides OSC (`/gui_set`'s values,
tagged by the same inference rule the audio client uses).

The same milestone settles a smaller one: the browser client has **no pump**.
`GuiHost` subscribes to its connection once and routes `/gui_event`/`/gui_closed`
to the handles as they arrive (`win.widget("cutoff").onEvent(fn)`), with `query`
a promise — where the Python client drains the host from the script's own loop
because it must not block the clock thread. Same discipline, but in a language
where it is the only shape available.

## The web clock: the routine driver stays on the page, only the wake-up moves off it, and the server anchors the timebase

The Python client runs a `TempoClock`'s real-time drive on a **background
thread**, which shares objects with the rest of the program: the routine, the
`Server`, the session. A browser Worker shares nothing but structured-cloned
messages, and a routine is a closure over the script's own objects — it cannot
cross. So there is no direct port of that thread, and the coroutine driver stays
on the page's thread, which is what `clients/python/PLAN.md` already asks for ("the
coroutine driver stays in the language").

What *does* port is the property the background thread has: a wake-up the rest
of the program cannot starve. On the page, `setTimeout` is clamped once nested
and throttled to about a second in a background tab — longer than any usable
scheduling headroom, so a sequence would stutter the moment the user changed
tabs. A worker whose only job is `setTimeout` and `postMessage` is not throttled
that way. Hence the split: **queue and routines on the page, the wake-up behind
a `Ticker`** (a shared tick worker in the browser, `setTimeout` where there is
no `Worker`). The exactness never depended on the wake-up anyway — it rides on
the bundle's timetag, and the wake only has to arrive within `Server.latency`.

The same seam pays a second time: with `Ticker` and `Timebase` both injectable,
`node --test` drives the *real* driver by hand, so "a late wake-up does not
shift the music" is a deterministic assertion rather than a timing-dependent
one. That is also why there is no NRT/score drive in this client: a second drive
would be a second thing to keep correct, and the seams already give what it was
for (`Timeline.fromPattern` bounces a pattern by advancing the ordinary clock as
fast as the loop can go).

**Anchoring is the Server's job, not the clock's.** The Python client has
`clock.lock_to(server)`, which puts the clock in conversation with a server and
contradicts that client's own rule — the one its C5 corrected — that the clock
must not talk to the server. The web client inverts the relation:
`server.sampleTimebase()` returns a timebase, and the clock merely reads it. The
Server is the right owner because it is the object that knows the carrier, and
the two carriers need different work:

- **in-page** — the engine runs in this page's `AudioContext`, so one worklet
  round trip pairs the engine's counter with the context's `currentFrame` at the
  same instant. Their difference is a fixed integer (the engine advances one
  quantum per render quantum of that context), so from then on the counter is
  `currentTime` read synchronously: exact, and drift is not a thing between a
  clock and itself.
- **over a socket** — `/clock_query` round trips feed the core's `SampleClockModel`,
  which regresses local time against the server's counter. The warmup must
  **spread its anchors over time**: five back-to-back round trips all land
  inside a couple of milliseconds, and a regression over that span is noise, not
  a rate. (Measured: it read a slope 2× off, which showed up as a sample clock
  running at half speed.)

A server that does not answer leaves the clock on wall-clock time rather than
failing, so a page whose master is unreachable keeps working.

## The document places, the host draws: no window management in the browser

On the desktop `clausters-gui` opens one window per `window`-rooted GuiDef and
the system's window manager places, sizes and stacks them. In a browser tab the
drawing surface is a `<canvas>` in an HTML document, and the document does the
placing — CSS, the order of the markup, the flow of the page. W4 makes that one
substitution and nothing else: the browser host went from `window`/`render`/
`current_def`, all singular, to a map keyed by def id, and the model is the
native front's, ported rather than invented.

Two things had to change direction with it, and both are the same point. The
**element supplies the canvas** (`GuiBridge::attach(def_id, canvas)`, winit's
`with_canvas`) instead of the page hunting for whichever canvas winit appended
to `<body>`: that is the correct ownership, and it is the only way N of them can
exist. And the **size comes from the element** (a `ResizeObserver` times
`devicePixelRatio`, reported in device pixels), so the host never reads the DOM;
it is told "this def draws into this canvas, at this size, and right now it is
(not) visible", and knows nothing else about HTML.

What is deliberately absent is the *management* layer. Components are placed in
the markup by whoever writes the page, in the order they want; nothing opens,
moves or stacks them. Mounting happens in `connectedCallback`, so an element
inserted later works, and unmounting in `disconnectedCallback` — see below for
what that gives back.

One consequence is worth writing down because it was found the hard way: winit
focuses a canvas it creates, and a browser scrolls a freshly focused element
into view. In a document with several components the last one mounted yanked the
reader to the bottom of the page. Canvases are created with `with_active(false)`;
a click focuses them, which is when keyboard input is wanted anyway.

**Not rendering what is not seen.** A document can hold fifty canvases with
three in the viewport. The browser already skips compositing what is off screen,
but that does not stop *our* host from computing the frame — the spectrum
analysis, the scope advance, the FFT — nor, more expensively, from keeping its
`/bus_stream` and `/bus_tapStream` subscriptions alive, which is server CPU and wire
traffic for something nobody is looking at. So each component carries an
`IntersectionObserver`, and every per-frame and per-packet cost is derived from
the **visible** set (`host::live::demand`, platform-agnostic and natively
tested). The same waste exists on the desktop behind an occluded window; only
the browser front acts on it so far.

## An unmounted component gives back what it took, not what the page shares

The mount is two phases because the host does not need audio and the engine
does. The unmount is deliberately **one**: an element removed from the DOM is
removed whole, so its window and widgets (`/gui_free`), the nodes its boot
instantiated (`/node_free`), its canvas (`detach`, which also drops it from the
frame tick and the `/bus_stream` set) and every id it drew from the page's
pools go together. What it does *not* touch is anything shared: the
`AudioContext`, the host, and — the one that looks like a leak and is not — the
def payloads and the sample buffers. Both are keyed by URL and identical for
every instance of a bundle, so freeing them would be freeing a sibling's; they
stay, and a component mounted again finds them loaded.

Which settles the other asymmetry: a re-connected element **mounts afresh**
rather than resuming. The resolver runs again over a new allocation, so the
attributes and the preset are read again — there is no half-alive state to
resume, which is precisely what lets the teardown be complete.

Two things found the hard way. A custom element's constructor **may not touch
its attributes**: `defineComponent` filled in `src` there, which works when the
parser upgrades an element and throws `NotSupportedError` on
`document.createElement` — that is, on exactly the path a page adding
components from script takes. The default bundle is carried as a field and
reflected into the attribute on connect. And a component's canvas must follow
its element's box on **every** `ResizeObserver` firing, not only at mount: an
element appended and mounted inside one task can be measured before the browser
has laid it out, and the first firing is what corrects the 1x1 canvas that
leaves.

The way back — `/gui_closed`, a window closed by the host rather than by the
page — reaches the element that mounted the def, which unmounts and emits
`clausters-closed`. The wasm host in the tab never sends one (nothing there
closes a canvas but the page); a native host over a socket does, and its event
stream joins the page's through `ClaustersGui.deliver`, so an element hears a
window closing wherever the window was.

## The run-time entry excludes the authoring builders

Running a component is the browser equivalent of `clausters-gui --standalone`:
the host is the server's client, and there is no scripting client in between.
The builders ran earlier, in the authoring script; what the page fetches is
data. So the package has **two entry points**, and the difference is what a page
is made to download.

`dist/runtime.js` carries the engine, the GUI host, the OSC codec, the mount and
the element. `dist/index.js` — the whole facade — carries those plus the def
builders (`defs/`), the GuiDef builders (`gui/guidef.ts`) and the sequencing
layer (`seq/`). A page that embeds an instrument in an article needs the first;
a page that sequences, responds or edits live imports the second on top. Both
target the same element.

Making the split real needed one move in the sources: the page's own host — the
singleton, its canvases, the in-page carrier — came out of `gui/host.ts` into
`gui/page.ts`, because `GuiHost` is the transport-agnostic *client* object (it
also drives a native `--ws` host) and pulls the GuiDef builders with it, while
the run time needs only the host itself.

The exclusion is **asserted, not hoped for**: `tests/runtime-graph.test.ts`
walks the emitted module graph of `dist/runtime.js` and fails if it ever reaches
any of the three, with the facade's own graph as the negative control so the
walker cannot pass by finding nothing. An import added anywhere along the chain
— the entry, the element, the mount, the page host — shows up there.

## The web client's API reference: TypeDoc, and a second TypeScript to run it

The repository's documentation rule is that an API reference is *generated* from
the source's own doc comments — the rustdoc for the crates, pydoc-markdown for
the Python client. The web client's book needed the same for TypeScript, under
the toolchain posture the package was built with: minimal, user-space, no
bundler, one dependency.

**TypeDoc, installed as a tool rather than as a dependency.** It is the only
generator that reads the language's own doc format, and with the markdown plugin
it emits the pages an mdBook can host. Its weight — about forty megabytes,
mostly the TypeScript it parses with — does nothing at run time and nothing at
build time, so it goes where pydoc-markdown goes: a **user-space global
install**, never in the package's `node_modules` and never in `package.json`.
Read the Docs installs it the same way, into a directory of its own.

That placement also settles a version conflict rather than fighting it. The
package compiles with **TypeScript 7**, the native compiler, a single dependency
with no transitive ones; TypeDoc is built against the **5.x** compiler API and
resolves its own copy from beside itself. Two TypeScripts exist on the machine
and never meet: one compiles, one reads. Pinning the package back to 5.x to
share it would have been the wrong trade — the emit is the product, the docs
build is a tool.

**The consequence for the sources.** TypeDoc reads TSDoc (`/** */`). The client
had been written with Rust-style `///` doc comments, which are invisible to
*every* TypeScript tool — an editor shows nothing on hover, a generator sees an
undocumented symbol. The tree was converted wholesale; new code writes `/** */`.

**Warnings are errors** (`treatWarningsAsErrors`), the rustdoc posture: a
comment referring to a symbol that moved, was renamed or became private fails
the build instead of producing a dangling page. Getting there also widened the
public surface a little — the types that appeared in signatures without being
exported (`ParamSpec`, `SpecInput`, `TimedMessage`, `Props`, …) are exported
now, which is the honest reading of a warning that says a documented signature
mentions something the reader cannot name.

## A page holds one host because it wants one, not because it can hold one

`start()` used to be a page-wide singleton: a second call reached winit's
`EventLoop::build`, which answers `RecreationAttempt` — a panic inside the wasm,
not an error a caller could catch — so an embedder wanting a second client had
to refuse it rather than hand back a stack trace.

The refusal was reasoning from the wrong constraint. **The event loop is what a
page can hold one of; the host is not.** That loop already drives any number of
windows — it is how one host serves a document's canvases — so the fix is not to
partition anything, it is to stop conflating the two: build the loop once,
memoize it in the proxy the instances already shared, and let `start()` add an
instance to the app already running.

What made this worth getting right rather than working around is what the
workaround would have cost. Sharing one host between two clients means their
widget ids land in one namespace, so the ranges have to be partitioned — and the
allocating clients are *separate processes with no channel between them*. The
only arbiter that can see who shares a tab is the page, which appears late, after
the first `plot()` has already assigned ids. Every escape from that is bad in its
own way: a slot derived from a session key collides silently, a claim file in a
runtime directory puts protocol state on a disk the client may not share, and
rebasing an already-built tree onto a slot learned at mount is correct but
fiddly. A second host instance makes the question not arise: independent id
spaces by construction.

The same reasoning settles the audio side, and settles it the same way. Nothing
in the engine was page-global either — `bootClausters` has always built its own
`AudioContext` and worklet per call — so `server()`'s memo *was* the singleton.
It stays, because sharing is what a page wants (its components play into one
mix), and `engine()` sits beside it for the caller that must not share a node,
bus and buffer space with the rest of the document.

So the rule for both, and the reason the default reads as a default rather than a
limit: **one host and one engine per page unless a caller asks for another.** Two
of them in one tab are as independent as two tabs. `guiHost()` in `page.ts` keeps
its memo — its contract is *the page's host with the page's default canvas*, and
a second one of those is not a thing anyone wants — and `newGuiHost()` sits
beside it for the caller who needs an instance.

That second function was not there at first, on the reading that such a caller
could go one layer down to the binding surface. That reading was wrong, and the
evidence was in the tree: the front end that needed it went down there, importing
`clausters_gui.js` and calling `start()` itself, because the client offered
nothing else. A capability the client has and does not expose is a capability
every consumer re-implements. `newPools()` came with it for the same reason —
page-global id pools are right for components that share the page's engine and
wrong for a client that does not, and the page having exactly one client had
stopped being true.

The cost of an instance is worth naming, because it is what makes the default
defensible: the wasm module and its memory are shared, and the GPU was already
per canvas, so a second host downloads nothing and adds no device. A second
*engine*, on the other hand, is a second `AudioContext` — they mix in the browser
rather than in the engine, and browsers cap how many a page may have (Chrome at
six), which bounds how many independent clients a tab is worth holding.

One thing had to be added rather than merely unlocked. Nothing ever closed a
host, because a page that holds one until it unloads has nothing to close; a
caller that opens them over time does, and an abandoned instance keeps its
WebSocket open, its `setInterval` running and its GPU surfaces alive. Hence
`GuiBridge::close`.

## The third carrier writes time, and a page's render has no file at the end

*(Its second half -- "a page has no file at the end" -- is superseded by "A
path in a page is its own storage", below.)*

The web client's offline drive had two things to decide, and the second is the
one that does not follow from the first.

**Where the score lives.** The Python client puts it in the *interface* — an
`OscNrtInterface` that accumulates bundles instead of sending them — and the
seam this client has at that spot is `Connection`, which carries bytes and
nothing else. A score cannot be assembled from bytes: a bundle's timetag has to
say *when*, and by the time the packet is encoded that number is already inside
it. So `Connection` grew two optional members — a `timeMode` (`"unix"` or
`"score"`) and a structured `addBundle(secs, messages)` — and the `Server`
branches on the first exactly where the Python client branches on
`interface.time_mode`. The alternative was a second seam above the carrier, and
that would have made "which carrier" a question the layers above `Server` could
ask, which is the one property the seam exists to prevent.

The consequence is worth stating plainly: **only the connection changes**. The
same patterns, defs and routines write a score that a live session would have
sent, and the score comes out byte-identical to the Python client's for the
same piece.

**Where the audio goes.** Here the two clients genuinely part. Python's
`render(path=...)` hands the score to a `clausters --nrt` process that streams
straight to disk — the samples never cross into Python, which is what lets a
long bounce not build millions of floats, and what makes `int16`/`int24`
output meaningful. A page has no process to hand a score to and no filesystem
to write to: its renderer *is* the wasm engine already in the tab, and what it
produces is a `Float32Array` in this page's memory. There is no honest `path`
to offer, so the client offers none, and the intent is served twice over
instead — `wavBytes(stats)` for a download, `Buffer.fromSamples(...)` to put
the take straight back into the engine (the reference client's render-then-load
with the file taken out of the middle, since the carrier shares memory with the
server).

One consequence had to be repaired rather than documented. `render` with no
seed is supposed to draw a fresh one — a take of a noisy piece is a
performance, unpredictable first — but the engine's entropy source is
`SystemTime`, which wasm does not have, so a seedless render there takes a
**fixed** seed and every take is the same take. The platform that has entropy
is the caller's, so the client draws the word from `crypto.getRandomValues` and
forwards it. The wasm shell inventing one would have been the wrong place: it
owns no logic, and this is a capability of the edge, not of the renderer.

## A responder listens to the connection it has, since a page can bind no port

`OscFunc` is the client's input path in both clients, and the port keeps its
whole surface — the constructor, the `(msg, time, src)` callback, the argument
template, `enable`/`disable`/`free`/`oneShot`, the builder form. What could not
be ported is what sits *underneath* it, and the difference is not a design
preference: in the reference client a responder registers with a **receiver**
that binds a UDP socket of its own, so any application on the machine can target
it, and a browser tab can bind nothing and be addressed by nobody.

So a receiver wraps the **`Connection` the client already has**. Everything a
page can hear arrives on the carrier it opened — the in-page engine or a socket
to a server — and that is where the door goes. Three consequences, each visible
in the API rather than hidden:

- **`src` names a carrier**: a socket's URL, or `page` for the in-page engine.
  It answers the same question a `(host, port)` answers — who sent this — with
  what a browser actually knows, and it is still what a responder narrowed by
  `src` compares.
- **The default receiver is the ambient session's server**, resolved per call
  rather than cached as the reference client caches its module default. A page
  can hold two sessions on two engines, and each server owns one receiver, so
  resolving late is what makes a bare `new OscFunc(fn, "/done")` mean "this
  session's server" in both of them.
- **`time` had to be brought across the wasm boundary.** The decoding door
  flattened bundles and dropped their timetags, which is all a reply reader
  needs and not what a responder's callback is defined to receive, so the shell
  grew a second door carrying each message's containing-bundle time in Unix
  seconds (`null` for an immediate bundle or a bare message) — the rule the
  Python client's own decoder applies, now asserted on both sides.

The fold that comes with it is worth stating too: the client's *own* reply
handling — the node ids that recycle as `/node_end` arrives, the streams that
decode `/bus_stream.reply` and `/bus_tapStream.reply`, the playhead that follows
`/transport_query.reply` — are ordinary `OscFunc`s now, on the receiver a page
would use. They had each grown their own address test inside a raw subscription,
which is the shape a responder exists to remove. `Server.onReply` stays under
them as the unmatched seam, because a decoder that wants everything in arrival
order is a real caller and not a responder.

## The browser engine gets a second scope: a Worker for the work that is neither audio nor UI

*2026-08-25.*

The native server has four kinds of thread and the browser engine has had one.
Network, audio, and the auxiliaries that may not block either — the NRT runner
(every `/buffer_*`: allocation, decoding, file I/O, zeroing), the Faust
compiler, the disk streams — are four roles that the page collapsed onto the
AudioWorklet, because `OscServer::headless` serves a pulled turn (`step()`)
before each block and everything else was compiled out or made inline
(`NrtRunner::Inline`, `workers = 0`, `DiskIn`/`DiskOut` target-gated off).

That was right for B0–B4, whose question was whether the engine runs at all. It
is wrong now, and the cost is measurable rather than theoretical: **the render
quantum's budget is about 3 ms** at 44.1 kHz (2.67 at 48), and a buffer install
is a memcpy into wasm linear memory — a five-minute stereo take is 110 MB, tens
of milliseconds, on the thread that owes a block every 2.67 ms. Nothing has
glitched yet only because no example is that big.

**Decision: one dedicated Worker, beside the worklet.** It is the browser's
version of "the threads that are neither audio nor UI", and it takes exactly the
roles that are those threads natively:

- **The NRT runner.** `server::nrt::run_job` is already a `pub fn` whose own
  docstring names two callers — the NRT thread and the offline renderer, which
  calls it synchronously. The Worker is a third caller of the same door, not a
  new architecture: a slim wasm shell beside `clausters-web` exporting `run_job`,
  fed jobs over a port and answering with the built buffer's frames.
- **The Faust compiler** (revising `B5`, which had put it on the main thread).
  Compiling a def is tens of milliseconds; on the main thread that is the GUI
  host's frames. It belongs where the native compiler thread is — off both.

**Why the decoder is ours and not the browser's.** The obvious shortcut is
`decodeAudioData`. It is the wrong one: it is a different decoder from
symphonia, so the same file would become different samples in a tab and in a
window — a divergence in values, which is worse than one in surface because
nothing names it. The Worker runs our decoder.

**What travels, and what it costs.** No `SharedArrayBuffer`: that question is
settled and priced elsewhere in this file, and reopening it means a profile
rather than an argument. Samples cross as **transferred** `ArrayBuffer`s (a
move, not a copy), and the Worker holds a `MessagePort` transferred into the
worklet, so a chunk does not queue behind the main thread's event loop at 60 fps.
What remains on the audio thread is one bump allocation in linear memory and one
memcpy — which is the next decision.

**`step()` gets a budget, and that is the general form of the fix.** Everything
the worklet does outside `process_block` is bounded per turn, and work that does
not fit resumes on the next one. A buffer install is **chunked** across serving
turns; the swap-in stays one pointer store, which the "every job replaces the
buffer" rule already gives us, and `/buffer_*` already replies `/done` late, so
the protocol learns nothing. The same budget bounds an OSC burst or an oversized
bundle, which today have no ceiling either. This is the one invariant that
changes: the worklet's serving turn stops being "drain everything" and becomes
"drain what fits".

**OPFS is what makes the file-shaped commands mean something.** A page has a
private filesystem, and `createSyncAccessHandle()` — synchronous read and write,
the same shape `reader_thread`/`writer_thread` have natively — is available in
**dedicated Workers only**, by standard, precisely so nobody blocks the main
thread with it. So the Worker is also where files live: `/buffer_allocRead`
stops being a path with nothing behind it, `DiskIn`/`DiskOut` become possible,
and a persisted data dir — the browser twin of `--data-dir` — becomes possible
with them.

**`DiskIn`'s honest price.** Without SAB the stream cannot be a sample-accurate
SPSC ring; it is a chunk queue the Worker keeps ahead and transfers. Chrome's
own guidance is worth quoting against ourselves here: a ring buffer "only
reconciles the buffer size mismatch and does not give more time to run the
code", and Worker↔worklet synchronization stays loose enough that an
underrun shows up as a drop. So **the prefetch depth is the design**, not a
tuning constant, and a browser `DiskIn` is a deeper-latency instrument than the
native one. That is a difference to write on the page, not to hide.

**Browser compatibility, checked rather than assumed.** Every piece is
Baseline: AudioWorklet (Chrome 66, Firefox 76, Safari 14.1); OPFS sync access
handles, widely available since March 2023, dedicated-worker-only in all three;
`MessagePort` exposed to Window, Worker **and** AudioWorklet and transferable,
per the HTML standard. Two WebKit findings are worth carrying:

- **Passing a `WebAssembly.Module` to a worklet was broken in Safari** and is
  the mechanism B2 already relies on (`processorOptions`). Fixed in November
  2022 (WebKit 220038), so the floor for the engine in Safari is that release,
  not 14.1 — worth knowing before a bug report blames our code.
- **iOS Safari crashes a tab somewhere near 350 MB of wasm memory**,
  undocumented. It is the reason the buffer pool must stay modest on mobile and
  a long take must stream rather than pool — the same conclusion `DiskIn`
  reaches from the other side.

The one thing not verified from documentation is transferring a `MessagePort`
*into* the worklet on Safari specifically; WebKit's serialization fix above
covers the mechanism, but no test here can drive Safari. So the direct
Worker↔worklet channel is **feature-detected with a handshake at boot**, falling
back to relaying through the main thread. Same semantics either way — it is a
transport detail, like choosing a carrier — and the fallback costs latency, not
correctness.

**A number that does not hold cuts the feature, not the budget.** Each step of
this is gated on a measurement rather than on being finished, and the outcome of
a bad measurement is decided in advance: the browser keeps the **reduced**
capability — a ceiling on a buffer, a floor under a prefetch, a `DiskIn` that
only plays forward — and the reduction is written on the page of limits with the
number that caused it. A tab that does less than a window is a limit, and there
is now a place for limits; a tab that drops samples is a defect, and there is no
place for those. The temptation this forecloses is the other order: keeping the
full capability and widening the budget until the glitch is rare enough to be
somebody else's bug report.

**And what this does not fix, ever.** Parallel groups. DSP threads mean wasm
threads mean `SharedArrayBuffer` mean cross-origin isolation, and a component
embedding on a page we do not control cannot demand headers of it. The Worker
does not help: it is a second thread, not shared memory. `/group_parallel` is
accepted and serializes — bit-identically, so the samples are the ones the flag
promises — and `Session.embed` takes no `workers` argument in the browser. This
is now written where a reader meets it, in the web client's book ("What a tab
cannot do"), beside every other limit that is the platform's rather than the
port's.

## The bundle format's bytes are canonical, so two writers make one directory

*2026-08-27.*

W15 ported the bundle writer to TypeScript, and a second writer of one format
raises a question the first one never had to answer: what does it mean for the
two to agree? The milestone's acceptance said **byte-identical**, which is the
only version of the claim a test can check — compare shapes and two writers can
drift apart in everything JSON parsing throws away, which is exactly where a
port drifts.

Bytes cannot be agreed on by imitation, and the reason is the one already
recorded above about a page's single number type. Python spells an integral
float `220.0`; JavaScript has no way to say that at all, since `220.0` *is*
`220` by the time a writer sees it. So the shared spelling is the one both
languages can produce: **an integral float is written without its decimal**.
The rest follows the same rule of picking what neither side has to work to
reproduce — **keys sorted**, no space between tokens, two spaces of indent for
the two files a person reads (`bundle.json` and a preset). Sorting is not
cosmetic: the two GuiDef builders assemble their props in their own order, and
sorting is what keeps that out of the comparison instead of coupling one
client's builder to the other's.

Nothing downstream reads a type out of the spelling. A parameter declares
`"type": "float"` and the core's resolver coerces an integer to it (`clausters_core::bundle`); a
def payload's `{"const": 1}` deserializes into an `f32` field; the native leg's
`boot_messages` is the one place a JSON integer means something — it becomes an
OSC `Int` rather than a `Float` — and the server's own reader accepts an int
wherever it wants a float (`osc::translate`), which is what the page leg has
always relied on, since `JSON.parse` erased the distinction there long before
this.

The Python writer moved to meet the format rather than the format bending to
the reference client. That is the usual direction reversed, and it is right
here: what is being defined is a **file**, not an API, and a file that only one
language can write is not a format.

## A path in a page is its own storage

*(2026-09-30, superseding the second half of "The third carrier writes time".)*
The web client's `render` took no `path`, and the reason given was that a page
had no filesystem to write to and no honest path to offer. That stopped being
true when the engine's file commands moved into a Worker over the origin
private file system: `/buffer_allocRead`, `/buffer_write` and `Buffer.read` all
name paths in a tab, and they name that storage. A render that could not write
to the same place left a hole the examples filled by hand: a dozen pages
downloaded a blob with `wavBytes` where their scripts rendered to a path, one
script (`render_then_load`) had no page at all, and a global parity row
declared `Buffer.read` and `Buffer.fromSamples` the same call because "a tab
has no path".

**So a path means the same storage to every verb that takes one**: the page's
origin private file system in a tab, the disk under node (`base/files.ts`).
`render({ path, sampleFormat })` writes the take there and answers with
`samples: null`, as the Python client does; `readSoundfile` reads one back;
`RenderStats` gained `path` and `events`, and `channels`/`interleave` joined
`channel`. `wavBytes` is gone: it was a surface the Python client lacks, and its
WAV header was written in TypeScript.

**The bytes are the server crate's.** The WAV framing, the int16/int24
conversion and the decoder are the ones the NRT worker already binds from
`clausters-nrt-web` for `/buffer_write` and `/buffer_allocRead`, loaded on
demand on the rendering thread (`engine/codec.ts`). A second rounding or a
second reader would be a second answer, and a file that differs by a bit
between a tab and a window is the divergence nothing names.

**What is still the platform's**: a page cannot stream while it renders -- its
renderer is the wasm engine on the calling thread, so the samples exist in full
before the file is written -- and the page's GUI host maps no file, so a view
is still handed samples (`gui.source`) where a script names a `.f32` path.

*Revised 2026-10-22:* **a server's write makes no directory, in a tab as on a
disk.** The page's storage made every directory above a written file, so a save
into a directory that does not exist succeeded in a tab and was refused by a
native server -- the one failure the audio editor's acceptance needed both to
show. `/buffer_write` and a recording now leave a missing directory missing and
fail the command; a page that writes there makes the directory first
(`opfs.makeDirectory`, a script's `os.makedirs`), and the audio editor makes its
own default scratch directory as the Python one makes its temporary one. The
client's own writes -- `SoundFile.write`, `render({ path })`, a MIDI file's or
a symbolic score's -- went the same way the same day: no write makes a
directory, in a tab or under node, so a page makes `out/` where its script
makes `examples/out/`.

## The web client holds its working folder by what it has (2026-10-24)

Context: an editor's working folder is the core's
(`clausters_core::workspace`), held by a file lock the operating system lets
go of however the process ends, and swept by the next editor when nobody holds
it. The web client cannot bind that: wasm reaches no disk, and node has no file
lock to hold one by.

Decision: the web client holds the folder itself (`src/base/workspace.ts`), by
the core's root and rule. Under node the lock file **names its process** and is
**touched while it is held**; the core's lock names its process too. So the two
clients sweep each other's leftovers in the one root: a native sweep reads a
lock touched within the last minute as a session starting and leaves it, and a
node sweep reads a lock whose process is not running as nobody's, leaving one
that names no process to a native sweep, which can try it. In a tab the folder
is in the origin's storage and held by a Web Lock, which the browser lets go of
when the tab goes; the next editor in the origin removes every folder whose lock
no tab holds or waits for.

Rejected: a sweep by age alone, which would remove a live session's folder
after an hour of editing; and keeping node in the temporary directory, which on
many machines is memory, the reason the native folder moved.

*Superseded the same day.* The audio editor's application moved into the GUI
host, so no client holds a working folder any more: the host's application
holds it through the core, by its file lock natively and, in a page, by a Web
Lock through the file layer (`clausters_core::workspace`'s page side). The
web client's `src/base/workspace.ts` was removed with the clients' audio
coordinator.

## The applications in a page are a second build of the host, on a pinned nightly (2026-10-24)

Context: an editing application in a page runs as one program over one
shared memory: the host and the application on the main thread, the
application's threads in workers, the engine's processing half in an
AudioWorklet, with built commands crossing the engine's lock-free FIFOs as
they do natively. Shared memory in Rust's wasm needs the standard library
compiled with atomics, and the prebuilt one is not, so the build has to
rebuild it (`-Z build-std`), which only a nightly does; a spike compiled
the host, the application and the engine that way and drew the GUI suite's
page whole on it in an isolated page.

Decision *(the toolchain the user's, 2026-10-24)*: the web package builds
the GUI host twice. The ordinary build stays on stable, without shared
memory, and stays the one a component embeds with in a page whose headers
nobody controls. The **applications' build** compiles the applications in,
with atomics and a shared, imported memory, using the dated nightly pinned
in `crates/clausters-gui/wasm-apps-toolchain` and a target directory of its own; no
other build uses that toolchain. A page loads the applications' build when
it is cross-origin isolated (`crossOriginIsolated`), and the ordinary one
otherwise: the same host compiled twice, with one surface.
`clients/web/tools/serve.py` serves the package with the embedder policy on
every response and the opener policy on a document that asks for it
(`?isolated`), which is how the suite runs a page both ways.

Rejected: serializing the commands between an application in a worker and
the engine in the worklet, which would put a wire back where the second
turn removed one; and moving every build to nightly, which would put the
whole project on a toolchain only one artifact needs. The pin is moved by
hand, when a nightly is needed or a bug asks for it.

## A page whose engine has threads reads its lists with the native reader (2026-10-25)

Context: an editing application in a page runs its engine in an AudioWorklet
over memory it shares with its threads, and plays its takes through streams
its own thread fills (`dsp::fed`). The list reader a page's engine had
(`dsp::disk_web`) is the one of a page with no threads: the page reads each
stream's files and pushes them in, so it knows no fed stream, and nothing in
the application's page polls it. The audio editor in a page therefore played
silence while its transport ran -- its page test checked the transport, not
what was heard; read back through the editor's meter it read zero.

Decision: wherever the engine has threads -- natively, and in a page whose
build has `target_feature = "atomics"` -- `TransportDiskIn` is the native
reader (`dsp::stream`), its thread spawned through `clausters_core::threads`
(a worker in a page) and its files read through `clausters_core::files`, and
the I/O thread's account and offline waits are one module both build
(`dsp::io_thread`, which counts its naps where a page has no clock). Only a
page whose engine has no threads, a client's, keeps the host-fed reader. So
the application reads its takes with the same code bit for bit natively and
in a page, which is what the second turn asked, and its page test now asks
the editor what it plays (`{"query": "output"}`: its meter's levels and the
samples played as silence for want of a frame).

Rejected: teaching the host-fed reader the fed stream, which would have made
three implementations of one reader's real-time half where two already had
to agree.
