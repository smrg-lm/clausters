# Plan - Clausters web client (TypeScript, browser-first)

The TypeScript client: a high-level client of `clausters-server` and of the browser GUI host, the web sibling of `clients/python`. It is its **own package, docs, examples and tests** under `clients/web`, exactly the way the Python client lives under `clients/python` - a parallel client track, not part of the GUI host (`crates/clausters-gui`) and not part of the server.

> **Read alongside `clients/python/PLAN.md`.** That plan already covers the *shared* client rationale (the native Rust core, the C-ABI/wasm seam, the "only flat data crosses" boundary, the coroutine-driver-stays-in-the-language rule) and was written from the start "to serve a future JavaScript client too". This file is the web-specific track; it does not restate the shared design, it points at it and records only what is different in the browser. As there, **sc3 is the reference model** for module structure, clock/routine behaviour, events, patterns and OSC/MIDI semantics; deviate only with an explicit reason.

## Why a separate track (and why now)

The browser GUI host (`crates/clausters-gui`, milestones G11-G16) runs the `/gui_*` widget system under WebGPU and is driven *through* a small wasm binding surface or over WebSocket. Those milestones deliberately use a **throwaway** in-page harness to test the host; the real, product driver - the thing a web app actually programs against - is this TypeScript client. Keeping it here, separate from the host, preserves the same boundary the rest of the system keeps: the host is a front, the client is a consumer of the protocol, and either can change without dragging the other.

So the web client is one more consumer of the exact same wires the Python client uses: OSC-in-JSON GuiDefs to the GUI host, OSC to the audio server, the same `decode_packet` framing - only the carrier (browser `WebSocket`), the binding technology (wasm, not ctypes) and the host language (TS, not Python) differ.

## `FormEditor` was removed (2026-09-06)

The multitrack driver written over `clausters.form` is gone, with its examples,
its tests and the book chapter built on it. Every `FormEditor` mention below
sits inside a **closed** milestone and is left as the record of what shipped;
nothing there is a pointer to code that exists.

`clausters.form` itself is retained, frozen: a small set of client-side data
structures for placing elements in time, with no view, taking no new work.

**`Editor` and `Editing` are untouched and stay**, with `View`, `Domain`, `Echo`,
`edit(x)` and the three editors it opens. What was removed is the *arrangement*
driver above them, not the seam - and they now have other priorities to adapt to,
decided in `O23`/`O24` rather than guessed at from this side.

The reason is structural rather than a defect count. A multitrack's own state -
which track a thing is on, its order within the track, its placement and its
identity - is authored, durable and undoable; projected out of a general tree it
had nowhere to live but the widget tree, which is drawn, and drawing frees. It
is being rebuilt as a **session** in `crates/clausters-document` - source,
region, lane, track, automation - with three classic applications over it
(audio editor, multitrack editor, score editor). See that crate's `PLAN.md`,
"The turn: the arrangement stops being a projection" (`O21`-`O24`).

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
    form/                 #   the arrangement (mirrors clausters/form)
      element.ts  aggregate.ts  render.ts  document.ts
    gui/notation/         #   engraving (mirrors clausters/gui/notation)
      engraver.ts  mei.ts  view.ts  _verovio.ts (the wasm toolkit)
    gui/transport.ts      #   play/pause/stop/locate + the view's playhead line
    gui/editor.ts         #   the multitrack editor (mirrors clausters/gui/editor.py)
    defs/patch.ts         #   the directed patcher, level 1 (GraphPatch)
  vendor/                 # the engraver, built by third_party/build-verovio-wasm.sh
                          #   (git-ignored; build.sh stages it into dist/)
    data/                 #   the data paths: what a view reads off the server
      buses.ts  taps.ts   #     the streamed sources (/bus_stream, /bus_tapStream)
      samples.ts  peaks.ts analysis.ts
    responders.ts         #   OscFunc and MidiFunc dispatch (mirrors responders.py)
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
- **Biome lints, adopted 2026-11-12 (root `PLAN.md`, `R21`)**, under the same lens: one package whose rules run in a native binary, delivered as one optional dependency per platform, with no JavaScript dependency of its own, pinned exactly (`npm run lint`, `biome.json`). The established alternative, a linter plus its TypeScript parser and rule set, is several hundred packages for the same two rule groups. Only those groups are on: correctness and suspicious, less what is a style of typing rather than a likely bug (an explicit `any`, an assignment inside a condition, a callback's return, an unused parameter), and its formatter stays off. It reads what `tsc` does not: the pages and the node scripts.
- The **Emscripten SDK** (`emcc`, user-space via `emsdk`) is the one heavy addition this lens admits, and it **arrived on 2026-08-21 with notation rather than with W7**: the engraver a page loads is built from `third_party/verovio.pin` by `third_party/build-verovio-wasm.sh`, because the published npm artifact is a different build of the same version and a build is what decides which score formats a client reads (`docs/decisions.md`, which also records what would reverse the choice). It stays out of the JS toolchain proper on the terms this bullet always set: nothing in `src/` or the test loop touches it, `build.sh` only stages its output, and the slim run-time entry never loads it. What follows is the original entry, still accurate about the *other* artifact: it builds `libfaust-wasm` so a Faust def compiles in the page (`third_party/BUILD-FAUST.md`, "WebAssembly parts" — documented, never built here). It stays out of the JS toolchain proper — nothing in `src/` or the test loop touches it, `build.sh` only stages its output as static assets, and the slim run-time entry never loads them. W7 evaluated it under the same lens on 2026-08-27 and the answer held for both artifacts: the SDK stays out of the JS toolchain, but the *release* carries it (`.github/actions/wasm-vendor`), and the SDK itself is pinned (`third_party/emsdk.pin`) — a package published without either artifact loads and runs and then cannot compile a def or engrave a score, which is why `check-package` now refuses one.

## Closed tracks

The tracks whose every milestone shipped are in [`docs/history/plans/web.md`](../../docs/history/plans/web.md),
moved there whole; the plan keeps what is open. They are, by their headings:

- Milestones
- Parity gaps carried from the Python client
- ✅ W21 - The module tree mirrors the Python client's *(done 2026-08-03)*
- ✅ W22 - The governing transport *(server T1, 2026-08-02; ported 2026-08-03)*
- ✅ W23 - `scope`: the live views *(done 2026-08-03)*
- W25 - The notebook front end is a client of the package - moved with W19
- ✅ W26 - The client stops drawing: the screen-shaped exports go *(done 2026-08-20)*
- ✅ W27 - It is written down who draws *(done 2026-08-20)*
- ✅ W24 - The completeness pass *(done 2026-08-25)*

## Future directions

Every entry carries a checkbox, like the plan's "Found by use" below: an
open direction has to read as open, and one that converges into a milestone leaves
this list rather than being ticked here.

- ✅ **An engine in a page has no clock, so every meter it reports reads zero**
  *(named 2026-09-15 with the server's `M34`, which is where it became visible:
  the load table joins `/server_status`'s CPU fields in reporting nothing here)*.
  `Instant::now` panics on `wasm32-unknown-unknown`, so the engine's per-block
  CPU meter is compiled out and `server::meters` stamps inertly -- a page can
  ask a native server over WebSocket what its roles cost and cannot ask its own
  engine, which is the one it actually runs. That is also why
  `basics/server_load` has no page twin (`docs/example-parity.md`). The fix is
  not a wasm clock in Rust but a **clock handed down by the host**: the page has
  `performance.now()` and the worklet has `currentTime`, both monotonic, so the
  shape is a function pointer installed at boot (beside the other host doors)
  that `meters::stamp` and the block meter read through, with the native build
  keeping `Instant`. Worth doing with a second reader in mind -- a page that
  wants to know why *its own* engine is late has nothing else to read.

  **Measured 2026-10-07: the fix above cannot be built as written, and what
  can be is a decision.** The engine runs inside the AudioWorklet, so that is
  where a block and the serving turn before it would be bracketed, and a
  worklet's global scope has no clock that can time one. In Chrome 154:

  - `performance` is **undefined** there. The page has `performance.now()`;
    the thread that runs the engine does not.
  - `currentTime` is the audio clock. It does not move inside one `process`
    call, so it says when a block is and nothing about how long it took.
  - `Date.now()` exists and steps by **1 ms**. A block's budget is 1.33 ms (64
    frames at 48 kHz), so no single block can be timed with it: `peakCPU` and
    `late_blocks` have no source at all.
  - Accumulated over many blocks it is a statistical estimate, and it does not
    hold up. A fixed piece of work, timed over a span of 400 repetitions, cost
    about 40 µs; the same work bracketed once a block read as about 125 µs
    over 5789 blocks (a tick caught in 13% of them, where 4% is what 40 µs
    against 1 ms predicts), at 48 kHz and at 44.1 kHz alike. A factor of three,
    and nothing in the page can say which of the two is right — which is the
    one thing a meter must not be.
  - `AudioContext.renderCapacity`, the browser's own load meter and the source
    this should have had, is **absent**. `AudioContext.playbackStats` is
    present: underruns, which is what `late_blocks` approximates, and not
    load.

  Firefox was not measured: a headless probe of it reported nothing.

  What *is* measurable exactly is the work a page does **outside** the
  worklet, where `performance.now()` exists: the NRT jobs the engine hands the
  Worker (`take_delegated` / `finish_delegated`), a Faust compilation, the
  disk streams the host feeds. The elapsed time could come back with each
  result and land in its own row.

  So the open question is no longer how to hand a clock down but **what a
  page's table should say**, and it is not this entry's to settle alone:
  exact rows for `nrt`, `faust` and the disk roles beside an `audio` row that
  still reads zero; underruns from `playbackStats` as `late_blocks`, where a
  browser has them; an `audio` row estimated with `Date.now()` and said to be
  an estimate; or the table as it is, every row zero and documented as such.
  A table half filled reads "idle" on the one row a late engine is asked
  about, which is the argument against the first on its own.

  **Decided 2026-10-07 (the user): the table stays as it is, and reports that
  the information is not available.** No estimate, no half-filled table. What
  was wrong with the table as it was is that it said *zero*, and zero seconds
  reads as an idle engine — so the fix is the reading, not a clock:

  - The server's load table knows whether it has a clock (`Meters::timed`,
    false on `wasm32`). Without one it still **counts** every run and keeps no
    time for it.
  - The replies say so field by field, with OSC **nil** where a measurement
    would be: `uptime` and every `busy` in `/server_load.reply`; `avgCPU`,
    `peakCPU` and `late_blocks` in `/server_status.reply`. `calls` stays, since
    a run is counted and not timed.
  - Both clients hand a nil on as no value — `None` there, `null` here — in
    `Load.busy`, and in `ServerStatus`'s three figures; no `share` is ever
    derived from an untimed reading, and one is no baseline for the next.
    `format_load`/`formatLoad` and the status line print `not available (this
    server cannot time itself)` and keep the counts.

  Nil is a tag the Python decoder did not read (it skipped what it did not
  know, which would have shifted every argument after it); it reads `N` as
  `None` now. Neither client *sends* one, so neither encoder grew a tag.

  Held by the server's own tests of the two replies over an untimed table, by
  one test in each client that walks the same replies to the same lines, and
  by `tests/defs.html`, where the engine a page boots answers `null` for all
  of it and a count of the blocks it ran. `basics/server_load` still has no
  page twin: there is no load in a page to look at.

- ✅ **The score keeps a second history, and nothing joins the two** *(the port
  of `clients/python/PLAN.md`'s entry of the same name, fixed 2026-09-02 in the
  same commit and by the same calls)*. `Score` registers in `Editing.of(score)`
  under the ``"score"`` vocabulary, attaches itself as a participant, records
  each edit as the MEI it produced with the previous one as its inverse, and
  walks the context's pile through `Editing.step`/`Editing.distribute` — so a
  page and a lane in one page have one Ctrl+Z. `JsScore.load` is the wasm half
  of the new core door (`clausters_score_load` natively), declared in
  `docs/bindings.md` like every other pair.

  One spelling is `idiom`: `Adopting` grew `projectLegs`, since TypeScript
  states what a participant must answer where Python asks the object.

- ✅ **An OSC marker dragged in a roll changes the picture and nothing else**
  *(the port of `clients/python/PLAN.md`'s entry of the same name — the gap was
  identical here, which at least made it a shared hole and not a divergence;
  fixed 2026-09-02, in the same commit and by the same calls)*. `NotesDomain`'s
  state is the whole timeline rather than its notes, a marker is an event of the
  same `events` vocabulary, and what an item is as data is written once in
  `seq/timeline.ts`'s `itemData` / `itemFromData` — which the document's
  `timelineMember` reads too, so a marker survives a save here as well.
  **Adding** one is refused with the reason, the host having no way to name it;
  the plan entry for that half is the Python client's, since the missing piece
  is the host's.

  Two spellings are `idiom` and are the only difference: an item the projection
  keeps is matched by a **sorted-key JSON string** where Python compares dicts
  (which are order-independent already), and a note's parameters still travel
  through `plain`'s own `EVENT_KEYS` mapping, which is why the write side hands
  an `Event` over as itself and only a marker through `itemData`.

- ✅ **Who builds `libfaust-wasm`** *(left over from **W7**/**B5**, decided and
  done 2026-08-27)*. The question was whether CI grows an **emsdk** leg or the
  artifact is **fetched** and pinned by digest, and fetching turned out never to
  have been on the table: `@grame/faustwasm` binds the Faust *source* API alone,
  so a box-API def would compile in a window and fail in a tab —
  `faust-wasm-bindings.patch` is why our build exists, and "pinned by digest"
  could only ever pin our own build, which is what a pin file already is.

  **What the delay actually cost was measured, and it was live**: `files` lists
  `dist/`, so a runner with no SDK printed `build.sh`'s two `note: vendor/…
  missing` lines and published a package **with no compiler and no engraver** —
  one that installs, loads and plays, and then cannot compile a def or engrave a
  score. So the release builds both, through `.github/actions/wasm-vendor`
  (restore-or-build, the composite already proven for the two native libraries,
  running the same vendored recipes a maintainer runs), and
  `tools/check-package.mjs` requires them so the silence cannot return — a tree
  without them stays perfectly developable, since nothing in `src/`, the suites
  or the smokes reaches either. The **SDK is pinned too**
  (`third_party/emsdk.pin`, read by both recipes and by the cache key): these
  artifacts are almost all toolchain output, and `emsdk install latest` meant the
  published package was built by whatever resolved that morning. `ci.yml`
  deliberately does not build them (~20 minutes cold, and its smokes reach
  neither). Recorded in `docs/decisions.md` ("The two vendored wasm artifacts are
  built by the release, from three pins").
- ⬜ **Type-safe GuiDef/def schemas.** Generate TS types for the widget/def vocabularies from a single source shared with the server, so an invalid GuiDef is a compile error, not a runtime warning. Two things have since appeared that change the shape of the answer rather than the want: the frozen parity vectors (`tests/gen-*-vectors.py`) already catch a drifted *builder* at test time, and M30's `/def_query`/`/ugen_query` make the server's own catalogue readable at run time — so the open question is narrower, which source generates the types and when, not whether one exists.
- ⬜ **A remote-server standalone page.** The in-tab standalone (a bundle booting against the embedded wasm engine) **shipped with the B track** and grew up in W4 (the bundle contract, the resolver, the pools, the components); what remains is the same mount against a **remote `--ws` server** — a one-file instrument front for a server running elsewhere. The old note called this cheap "once W1/W2 exist"; they exist, and W4 is what actually decides the work: `openBundle`/`startBundle` reach the page's `guiHost()` and `engine` singletons directly, so the step is giving the mount a **destination seam** (a `Server` + `GuiHost` pair, both already carrier-agnostic since W1/W2) in place of those singletons. The boot replay itself stays carrier-agnostic above the W0 seam, as it always was.

- ⬜ **Native plugins and the in-page engine** *(named 2026-10-13 with the server plan's J track)*. Port `PluginDef`, discovery, parameters, state, events, recovery and editor control alongside the Python client as each server capability lands, with the same calls and examples. A page connected to a native server controls its CLAP/VST3 instances; a native editor opens on that server's machine. The in-page wasm engine cannot load native plugin binaries and must report that capability explicitly while keeping the same builder surface. A possible WAM2 integration is an open design question: a different plugin binary/runtime is not automatically the same capability, and no independent DSP, mapping or drawing implementation is authorized by this entry. The native-server client port closes with the J track; any remaining in-page design stays pending here.

## Found by use: the running list of fixes

- ✅ **A formatter rounds an exact tie differently in the two clients** *(found
  2026-10-07, writing the test for a status with no clock: `4.25` printed
  `4.3% peak` here and `4.2% peak` in the Python client)*. Every record prints
  one line and the two clients are meant to print the same one
  (`tests/info-vectors.json`), but a number formatted to a fixed count of
  decimals is rounded by two rules: Python's `f"{x:.1f}"` rounds the exact
  binary value correctly and takes an exact tie to the **even** digit, while
  `Number.prototype.toFixed` takes a tie to the **larger** one. They differ
  only where the value is exactly halfway — `0.25`, `4.25`, `2.5` at no
  decimals — which a float off the wire seldom is and a test value often is.
  The sites are `formatServerStatus` and `formatLoad`
  (`src/defs/server/options.ts`), and any other formatter that calls
  `toFixed`. The fix is one helper that rounds as the reference client does,
  and it has to decide a tie on the value's exact binary expansion: scaling by
  a power of ten first makes `0.05` look like a tie it is not.

  **Fixed 2026-10-07**, and it was wider than the tie. `src/base/format.ts`
  is the reference client's rounding written in integers: `formatFixed` is
  `f"{x:.Nf}"` and `formatGeneral` is `f"{x:g}"`, both worked on the float's
  own bits (`mantissa * 2^exponent`, as a ratio of big integers), so a tie is
  decided on the exact value and never on a product that already rounded.
  Reading the sites found the second half: `formatNumber`, which every record
  line calls and whose comment said "Python's `%g`", was
  `Number(value.toPrecision(6)).toString()` — the same tie the other way
  (`100000.5` printed `100001` against `100000`), and **no exponent** where
  `%g` writes one, so a control at `1e6` printed `1000000` here and `1e+06`
  there. Both are one function now. The sites: `formatNumber`
  (`defs/info.ts`, and through it the host's widget record), the three
  formatters of `defs/server/options.ts`, and a patch's value caption
  (`defs/patch.ts`, which also writes every digit of a whole number as
  `str(int(f))` does).

  The reference is the language, so that is what the test is against:
  `tests/gen-format-vectors.py` asks Python for 795 values — exact ties in
  both parities, values that only look like ties once scaled, the edges of
  `%g`, zero and its negative, the largest and smallest floats, what is not
  finite, and a seeded spread as doubles and as the float32 a figure off the
  wire is — and `tests/format.test.ts` matches every line. The wording
  vectors gained what they did not cover: controls on a tie and past the
  exponent's edges, and the status and load lines, timed and not, which had
  no vector at all.

  Left as it is, on purpose: `defs/boxes.ts` writes a whole number with
  `toFixed(0)` when it splices one into Faust source. That is `repr`, not a
  record's line, and a whole number is never a tie.

- ✅ **`panels/host.html` promises a native host changes nothing, and its
  bound widgets and meters stop working there** *(found 2026-09-26, checking
  the WebSocket leg by hand)*. The page's comment says that with
  `transport: "ws"` "nothing below this line would change". The scripted
  controls hold to that -- `cutoff` and `amp` come back as `/gui_event` over the
  socket and the page applies them to its own engine. The bound ones do not:
  `freq`'s `/node_set` and the meter view's `/bus_set` go from the host
  straight to *its* audio-server leg, and the meters read a `/bus_stream` from
  that leg too. The engine is in the tab, which no native process can reach,
  so the native window's `freq` changes nothing and "a bus, metered" stays
  still. Deciding first: share one real audio server between the page and
  the host (the page's `Server` over WebSocket, the host with `--server`), or
  keep the tab's engine and have the page say that bound widgets and meters
  need the page's own host.
  **Fixed** by sharing a real server: "drive a native host" attaches the
  page's `Server` to a `clausters --ws` over WebSocket, sends the def there and
  then attaches the host, which is started with `--server` at that server and
  `--shm` at its segment -- a native host reads meters and scopes out of the
  segment, not over `/bus_stream`, which is the browser's path. The page's
  comment gives the two command lines.

- ✅ **`edit(curve)` could not be told the value range** *(found 2026-09-21;
  the finding and the reasoning are one entry in `clients/python/PLAN.md`,
  "Found by use" -- "`edit(curve)` could not be told the value range, so an
  amplitude envelope's zero was unreachable by hand" -- and are not restated
  here)*. This side's half: `PointsEditor` takes a `PointsEditorOptions` with
  `min`/`max`, `PointsView` holds the declared axis as the floor its kept axis
  starts from, and `edit` passes the pair through to the curve alone -- the
  other editors never see it. `examples/editors/edit-env.html` declares
  `[0, 1]`.

- ✅ **`Editor.window` returned the bare id while the handle sat beside it**
  *(found 2026-09-20; the finding and the reasoning are one entry in
  `clients/python/PLAN.md`, "Found by use" -- "`Editor.window` handed back a
  handle in one client and a bare number in the other" -- and are not restated
  here)*. This side's half: the getter returns `windowHandle` rather than
  `windowId`, so `editor.window.widget("play")` reaches a widget passed as
  `extra`, the way the reference client's `editor.window["play"]` does. `id`
  still returns the number. One caller moved (`gui/editing/samples.ts`, a null
  check).

- ✅ **`seq/automation.ts` goes, and `Bpf` comes** *(found 2026-09-20 by the
  user; the finding, the analysis and the decision are one entry in
  `clients/python/PLAN.md`, "Found by use" -- "`seq.Automation` is three things
  fused, and one of them is a second implementation of what the crate already
  plays", and are not restated here)*. This side's half of that pass: `Bpf`
  beside `Env` in `defs/ugens`, interchangeable with it; `seq/automation.ts`
  deleted with its second copy of `clausters.auto_lane`; the `Automation`
  branches out of `play.ts` and `plot.ts` and the import out of
  `gui/editing/points.ts`; the re-exports out of `seq/index.ts` and
  `src/index.ts`; `examples/transport/automation-lane.html` and the sweep of
  `examples/basics/verbs.html` gone, `examples/editors/edit-curve.html` rewritten
  over a `Bpf`; the `Automation` sections and rows out of
  `docs/src/routines-and-clocks.md`, `docs/src/verbs.md` and
  `docs/src/composition.md`; and the vectors `tests/gen-seq-vectors.py` and
  `tests/gen-form-vectors.py` build from one regenerated. It closes in the same
  commit as the Python side, which is what the surface being one means.

- ✅ **One WebSocket test fails only when the whole suite runs** *(found
  2026-09-18, running `./test.sh` after adding the manual ordering verbs; fixed
  the same day, and it was not the test)*.
  `tests/seq-ws.test.ts`, "a timeline on the server's transport follows the
  conductor over the wire", failed two of four full-suite runs today with
  `ReplyTimeout: the server was closed` (from `withServer`'s `close`, so a
  request was still in flight when the test ended), and passed every time it was
  run alone -- 877 ms against the 812 ms it failed at, which is the tell: the
  test is near its own timeout and the full suite, which runs the files
  concurrently, is what pushes it over. Nothing in the change it was found under
  touches timelines or the transport; a run of the same suite without it failed
  nothing, and a later run with it failed nothing either.

  It was written down rather than fixed because the fix looked like a decision:
  raise that test's timeout, cut what it waits for, or stop running the
  server-backed files concurrently with the rest.

  **None of the three was it.** `TransportPlayer.queue` put each step on one
  chain (`this.chain = this.chain.then(work)`) and left the chain unobserved
  until the next `refresh` awaited it. That is right for a **verb**, whose
  caller refreshes -- it is how a refusal reaches the caller rather than the
  console. It is wrong for a **re-cue**, which is queued by `broadcast`, a
  responder callback with no caller at all: when the one that lands while the
  server is closing fails with every request in flight, the rejection had
  nobody, and node reported it as an unhandled rejection against whatever test
  was running. A page would have printed it to the console for the same reason.
  So `queue` now marks the chain observed (`this.chain.catch(() => {})`) while
  leaving the failure on the chain, which is still where `refresh` finds it;
  `tests/timeline-transport.test.ts` pins both halves at once -- no unhandled
  rejection, and the next `refresh` still rejects.

  **What it is worth keeping** is not the bug: it is that an intermittent
  failure in a suite is worth one run of the file alone (877 ms, passing) and
  one of the suite without the change (nothing failing) before it is filed as
  flaky. Both readings were right and neither was the cause, and the entry that
  said "the test is near its own timeout" was wrong about a test that has never
  been slow.

- ✅ **The GuiDef sweep vectors are stale, and regenerated they show `waveform`
  disagreeing on two loudness options** *(found 2026-09-17, regenerating every
  parity vector after the timeline work; fixed 2026-09-18)*.
  `tests/gen-gui-vectors.py` writes entries the committed
  `tests/gui-sweep-vectors.json` does not have: the
  `scope` and `signal` builders' `loudness_target`, `loudness_scale`,
  `loudness_ruler` and `loudness_stats`, and the same for `waveform`. With the
  file regenerated, `gui-parity.test.ts` fails two cases,
  `waveform(loudness_ruler)` and `waveform(loudness_stats)`: this client's
  `waveform` emits `loudness_ruler: 1` / `loudness_stats: 1` on the tree and the
  Python client's does not, while `scope` and `signal` agree. Which one is right
  is not decided; the fix is that one, in the lagging client, and the
  regenerated vectors committed with it. The committed file was left as it was,
  so the suite passes today only because it is stale.

  **The lagging client was Python, and the code said so**: `waveform` declared
  `loudness_ruler` and `loudness_stats` as keywords and then forwarded neither
  — its tail flagged `overlay` and `fills` one `if` at a time, where `scope`
  and `signal` run the whole pair through a flag loop, and the two options fell
  through the gap that third spelling left. So the option was accepted and
  silently dropped, which is the worst of the three ways to be missing a prop:
  the caller sees no error and no ruler. `waveform` now flags the four together,
  the vectors are regenerated and committed, and the sweep fails again when a
  surface drifts rather than when somebody remembers to regenerate it.

- ✅ **The page suite is one browser, and the second one found a defect it had
  been passing over** *(found 2026-09-16: the user opened
  `examples/basics/group-order.html` in Firefox and got `Uncaught (in promise)
  DOMException: Can't close an AudioContext twice` -- the fix is below, the gap
  is that nothing here would have found it)*. `test.sh` runs every acceptance
  page under one headless Chrome. That is the right default -- a second browser
  doubles the slowest part of the suite -- and it is not free: the two engines
  disagree about what an API **refuses**, so a page that is wrong everywhere
  passes here whenever Chrome is the lenient one. Two known instances now, and
  they are the same shape: the wheel's units (recorded above, and answered with
  a page a person opens by hand) and closing an `AudioContext` twice, which
  Firefox throws on and Chrome shrugs at.

  What to do about it is a decision, not typing, which is why this is written
  rather than fixed. The cheap half is a **rule for the pages**: assert the
  *mechanism* rather than the absence of an exception -- `defs.html` now checks
  that two concurrent closes are one promise, which fails in either browser,
  where "it did not throw" only fails in one. The expensive half is a second
  browser in the suite, and it is worth pricing rather than assuming: a
  Firefox-only pass over the same pages, run by hand before a release the way
  the feature matrix is, would probably do.

  **Decided 2026-10-01 with the user, and done.** Priced first: the suite is
  about 128 s, 42 of them node and 85 the 26 pages, so a second browser is
  +65 % on every run -- too much for a step repeated many times a day.
  Development stays on Chrome; `./test.sh --browsers chrome,firefox` runs the
  pages once per browser and is **required before a release** (the
  `release-versioning` skill's procedure, `docs/contributing.md`, CLAUDE.md)
  and run on request otherwise. The harness is portable: each browser's
  profile is made in the package (`browser-profiles/`, git-ignored), which a
  confined install (a snap, a flatpak) can read where it cannot read the
  system temp directory, and a system without `setsid` reaps the browser by
  its pid. The first passes found two things Chrome had been passing over: a
  component announced `clausters-ready` before its canvas had its size --
  Firefox delivers the first `ResizeObserver` callback after the event, so
  the host drew its first frames at 1x1 and the page saw 1x1 four runs in six
  -- fixed in the component, which measures once before the event; and
  `disk.html` waited a fixed 1.2 s for a disk stream to start, which a
  browser under load missed, so it now waits for the sound, up to a bound.
  The rule for the pages -- assert the mechanism, not the absence of an
  exception -- stands as written above.

- ✅ **A handle whose server stopped keeps a dead carrier, and this client had
  no way to reopen one** *(the twin of the fix made in `clients/python/PLAN.md`
  on 2026-09-15, named the same day and fixed the day after, when the user
  asked for it: step through it and fix what persists in the web client)*.
  `openCarrier` returned `this.conn` whenever it was set, and nothing ever
  cleared it -- so after `quit()` (which closes the page engine's
  `AudioContext`) a later `boot()` or `attach()` handed back the dead one, and
  every request failed the way the reference client's did before its fix: a
  timeout naming the command, then nothing. `close()` had the same shape from
  the other side, leaving a stopped receiving door on a carrier it kept.

  **Why it was not the same one-line change**, and what it became instead. In
  Python an interface knows how to reconnect -- it holds a host and a port --
  so dropping the connection is enough. A `Connection` here does not: it is an
  object, sometimes built by the caller and handed to the constructor, and
  reopening one is not something a `Server` can do in general. So the fix is an
  **ownership rule plus a capability**, and it is sharper than the reference's:

  - `Connection.gone?()` -- optional, asked not required, the same posture the
    Python event loop takes to a source's `gone`. A `WsConnection` answers from
    its socket's `readyState`, the in-page carrier from its engine's
    `AudioContext.state`, a score leaves it out and is assumed live. It exists
    because **a send cannot find this out**: a closed socket takes a packet and
    drops it, a dead context takes one into a worklet nobody renders.
  - `Server.openedCarrier` -- whether the carrier is one this handle opened or
    one it was lent, which is the class's own ownership rule applied to the
    carrier. `dropCarrier()` replaces what it opened (and lets go of the engine
    with it, so `Server.engine` never hands a second handle a closed one) and
    leaves a lent one exactly as it was.
  - `boot`/`attach` ask `gone?()` before reusing a kept carrier: a handle
    replaces its own, and one built around a carrier that has since closed is
    told so plainly instead of timing out. They also re-arm the receiving door
    when a `close()` left the carrier without one.

  `quit()` drops the carrier, so `boot()` on the next line works -- which is
  the reported sequence, and it is now the last phase of `tests/defs.html`:
  quit, re-boot, `status()` and `queryInfo()`, verified to fail on the build
  without the fix (`FAIL: quit kept naming a closed engine`) rather than only
  to pass with it. The other half of the Python fix, waiting for the process
  to exit, has no twin by nature: a page launches no process.

- ✅ **Two playback fixes never crossed over, and the page was the older
  program** *(found 2026-09-11 while porting a third; fixed the same day)*.
  `handPorts` (a curve owns the port it names, so the hand does not write it)
  and `staysPut` (a clip that changed track is made again, not set) were
  written in Python and left there, so the page went on taking automations back
  with a plain set on every edit and leaving a dragged box sounding through the
  track it came from. Neither is a platform difference and neither shows in a
  build: the two files simply drifted. Both are now module functions of the
  same names, doing the same thing in the same place.

- ✅ **A page spelled with an underscore had no twin at all** *(found and fixed
  2026-09-05)*. `views/tempo_ruler.html` was named the way its script is, and a
  pair is found by path with `_` becoming `-`, so the audit saw two examples
  with no counterpart rather than one pair — and reported it as two missing
  twins, which reads like a gap in the port instead of a typo in a filename.
  Renamed to `views/tempo-ruler.html`; the pair then differed by one call, the
  map's `dumps` against `dump` — declared an idiom row at first and then fixed
  properly, by renaming the Python client to the `dump`/`load` every other
  layer already used (`docs/decisions.md`).

- ✅ **Every page opened in a browser, and what that turned up** *(done
  2026-09-05)*. The pair audit reads what a page **calls**; it cannot say
  whether the page runs. Driving all 80 of them under headless Chrome — open,
  press every button in the order they are laid out, read the page's own log and
  the console — found eleven that did not, and nothing about them was visible to
  a reader:

  - **`registry_new` / `__wbindgen_free` / `scheduler_new` of `undefined`** in
    eight pages: they reach the core before anything has fetched it. A `Session`
    verb and `Server.boot`/`attach` await `loadCore` for you; a page that boots a
    bare `GuiHost`, engraves a `Score` or builds a `Timeline` first does not, and
    the failure named a wasm-bindgen symbol instead of the missing line. The
    pages await it now, and `requireCore` — the guard written for exactly this,
    and until now wired only into `time.ts` and `rand.ts` — is on all five
    `Registry` constructions, so the message names the verb and the fix. The
    guide's claim that "`boot()` and `attach()` load it themselves" was true of
    the audio `Server` only, and is now written as the list it actually is.
  - **`chans` imported from the wrong module** (`basics/multichannel`): the page
    did not load at all.
  - **A truthy `{}`** (`views/take`): the read-out on the application clock
    guarded a selection the way the script does, but an empty selection is falsy
    in Python and truthy here, so the routine threw on its first tick and was
    dropped.
  - **Arity taken for keywords** in two pages: `Bus.control({ server })` put the
    options where `channels` goes and `fft(src, { fftSize })` where `active`
    does. The script writes both as keyword arguments, which is why neither read
    as wrong.
  - **`session.play(take)` on a buffer** (`editors/edit-samples`, and its script
    with it): `Session.play` is the pattern verb in **both** clients — the free
    `play` is the one that sounds a buffer through the stock reader. The same
    wrong call in both languages, which is what a faithful port looks like when
    the call itself was never run.
  - **A transport join that answered itself** (`io/responders`): a script's
    receiver is a second socket, so the reply to its own `/transport_query` never
    reaches the responder; a page has one carrier, so it did, and re-aligned
    forever.
  - **A double `close`** — `Session.close` closes the server it owns, which now
    quits the engine it booted, and then the engine: the second
    `AudioContext.close()` threw. The engine's `close` is idempotent now.

  The one page that still reports an error under the sweep is `panels/host`,
  which offers a button that attaches to a `clausters-gui --ws` host: nothing is
  listening, and that is the page working. The engine panic the sweep also found
  is the server's and is written in the root `PLAN.md`.

- ✅ **A page cannot write a Faust constant that is *real* and integral**
  *(found 2026-08-27 porting the box API, measured and closed the same day)*. A def builder's numeric constant
  travels as a bare JSON number, and the server reads it back with
  `serde_json`: an integral one becomes a Faust **int**, anything else a
  **real** (`src/faust/boxes.rs`, `number_box`; the signal schema does the
  same). Python distinguishes them by the literal's type — `box(2)` is `int 2`
  and `box(2.0)` is `real 2.0` — and JavaScript has one number type, so `2.0`
  *is* `2` by the time `boxes.box`/`signals.signal` sees it and a page can only
  say the int.

  **It is older than the box API**: `signals.ts` has had it since W1, unwritten
  down, and porting `boxes.ts` is only where it was noticed. The parity vectors
  cannot catch it either — they are compared as parsed JSON, where `2.0` and
  `2` are the same value — which is worth knowing about what those vectors
  prove.

  **Measured, and it costs nothing**, which is what closed it: compiling both
  spellings of every operator that could care (`faust -lang c`, the pinned
  compiler) says the constant's type never decides a value. `/` is real
  whatever its operands are, so there is no integer division to fall into; any
  real operand promotes the whole expression, so a constant against an audio
  signal is the same either way; where int is *required* (a table index, `@`,
  `select2`, the bitwise family) a real is cast down silently, so the int a
  page emits is the one those wanted. What is left is the operator pair chosen
  by type against an *int* signal — `%`, `min`, `max`, the comparisons — and
  both members compute the same number for an integral constant, diverging only
  past 2^24, where the exact one is the int.

  **So no `real()` builder**, which would have been a name in one client the
  other has no use for. The spelling already exists in a verb both clients
  have: `asfloat`. `box(2).asfloat()` is Faust's `float(2)`, folded to `2.0f`,
  and `int(x) % float(4)` compiles byte-identically to `int(x) % 4.0` — so the
  reference client's `box(2.0)` has an exact page-side equivalent after all.
  Written in `docs/src/platform.md`, in `boxes.box`'s own doc comment, and
  recorded in `docs/decisions.md` ("A page has one number type, and Faust's
  promotion makes that cost nothing").

- ✅ **`Buffer.fromSamples` empties the array it is given** *(found and fixed
  2026-08-26, writing a `/buffer_write` round trip)*. The in-page carrier's `bufferLoad`
  posts the samples to the worklet with `samples.buffer` in the **transfer
  list** (`src/engine/loader.ts`), so the caller's `Float32Array` comes back
  detached: reading it afterwards throws `Cannot perform Construct on a detached
  ArrayBuffer`, and a view taken of it earlier silently reads zero length. The
  failure lands wherever the array is next touched, never at the call that took
  it.

  **The reference client does not do this** — a numpy array handed to
  `Buffer.from_samples` is still there afterwards — so it is a divergence and
  not a browser limit, which is why it is here rather than on
  `docs/src/platform.md`.

  **It is a copy at the boundary**, and the `{consume: true}` option this entry
  floated was the wrong idea: it would be a knob the Python client has no
  counterpart for, which is a surface one client has and the other does not —
  the thing the non-divergence rule exists to stop — bought to save a copy the
  socket carrier pays anyway. `fromSamples` slices, `Connection.bulkLoad` says
  in as many words that it consumes what it is handed, and `Buffer.load` still
  passes it an array built for the call, so the fast path is exactly where it
  always was.

  Worth naming: this was **also an inconsistency inside one client**. The socket
  path goes through `setSamples`' blob runs, which copy, so the same call left
  the array alone over a socket and emptied it in a page.

  `tests/nrt.html` asserts the array survives, and it is asserted rather than
  commented because a detached array **does not throw when it is read** — its
  length is 0 and every element is `undefined`, so nothing downstream reports
  it. Checked the way a regression test has to be: it fails without the fix.

- ✅ **A clock could not be locked to a server without spelling the two steps**
  *(found 2026-08-25, porting `transport/conductor.py`; fixed the same day)*.
  The reference client has `TempoClock.lock_to(server)` — one verb, idempotent,
  graceful when no master answers — and `Session.lock_to_server` is the
  session-wide spelling of it. Here only the session verb existed, so a page
  driving a **bare** clock (which is exactly what a conductor's followers do)
  had to write `clock.timebase = await server.sampleTimebase()` and know that
  this is what locking means. `TempoClock.lockTo(server, timeout?)` and
  `unlock()` are here now, with `Session.lockToServer` calling the first, so
  there is one implementation and the session verb is what it always claimed to
  be — a spelling.

- ✅ **`Server.open`, and `GuiHost.page`/`connect`, were the reference client's
  verbs merged and renamed** *(found 2026-08-25 when the user asked what `open` was doing
  beside `boot` and `attach`; fixed the same day)*. No client had all three:
  the reference has a cheap constructor plus `boot()` (start the process),
  `attach()` (reach one already running, verify, reconcile) and `reconcile()`;
  this one had only `Server.open(connection, …)`, which was the constructor,
  `attach` and `reconcile` merged and renamed — with `attach`'s **refusal** made
  opt-in (`verify`), so a page talking to nothing carried on by default where
  the reference raises.

  It is the four verbs now, and the pairs read as pairs:

  - `new Server(connection)` **reaches nothing**, as there. The carrier comes in
    already built (`await pageConnection()`, `await WsConnection.open(url)`),
    which is the one shape that cannot be shared: opening a carrier is
    asynchronous in a browser and a constructor cannot await. That, and only
    that, is `idiom` here.
  - `boot()` brings up what the carrier points at, and **what that means is the
    carrier's to say** — the same sentence the reference client's `boot` is
    written with. `pageConnection` boots by starting its audio (an
    `AudioContext` is created suspended, so this is the resume a gesture pays
    for), a `ScoreConnection` has nothing to start and boots as a no-op, and a
    WebSocket refuses and names `attach`, because a tab starts nothing on
    another machine. `Connection.boot?()`/`quit?()` is where that lives.
  - `attach()` verifies, always. Not a flag: it is what the verb is for.
  - `reconcile()` has its own door, and both verbs call it.
  - `quit()` is `boot`'s pair and now finishes the job on a carrier whose server
    is not a process listening for `/server_quit`: the page's engine *is* the
    server, so quitting it closes it. `close()` stays the other end — this
    handle lets go and the server stands.

  Twenty-three pages and every suite moved with it, and the two-carrier examples
  (`basics/synth`, `transport/sequencing`) now show the pair being *chosen*,
  which is the thing they were always about. `SessionOptions.verify` is gone:
  `Session.page` boots and `Session.connect` attaches, so refusing silence is
  no longer something a caller has to ask for.

  **The same defect sat one layer up**, and went with it: `GuiHost.page()` and
  `GuiHost.connect(url)` were static factories where the reference client has
  `boot()` and `attach()` on a handle. They are those verbs now, over a carrier
  the constructor already took — `new GuiHost(await pageGuiConnection()).boot()`
  for this page's wasm host, `new GuiHost(await WsConnection.open(url)).attach()`
  for a `clausters-gui --ws`. `boot` refuses a socket and names `attach`;
  `attach` verifies with a `/gui_query` for an id nobody defined, since a host
  that is up answers it and one that is not answers nothing. The page carrier
  carries the surface it goes to (`PageGuiConnection.gui`, declared in
  `gui/page.ts` where everything the DOM is lives), which is how `boot` knows it
  is driving a host in this document and where its canvases come from.

- ✅ **Opening a server did not adopt it, so the ambient verbs had nothing to
  resolve** *(found 2026-08-25, porting `hello_note.py`; fixed the same day)*.
  The reference client's `Server.boot()` and `Server.attach()` both take
  `adopt_default=True` and claim the default session's slot when it is free —
  which is what makes `Server().boot()` followed by a bare `play(Event(...))`
  the first example a reader ever runs. Here `Server.open` claimed nothing, so
  the same two lines ended in *"no server to play on: open one with
  Session.page() or Session.connect(url)"* — advice that names two verbs the
  reader did not want, for a server they had just opened.

  `Server.open` adopts first-wins now, with the same opt-out under the name the
  language gives it (`adoptDefault`). Both places that open a carrier *without*
  it being the reader's act of opening a server pass `false`, exactly as
  `Session.live` does there: `Session.nrt` (an offline score has no business
  being the page's default) and `Session.over`, since a session is **asked** for
  the slot with `adoptDefault()` rather than taking it by being constructed.

  It went unnoticed because no page did it: every example opened a `Session`
  first, which resolves through its own field. That is precisely what a port of
  the *simplest* example turns up, and the reason W16 is worth the effort beyond
  the pages themselves.

- ✅ **A running clock resumes what it is handed inside `play`, where the
  reference client's returns first** *(found 2026-08-25, writing `Routine.run`;
  fixed 2026-08-26)*.
  `TempoClock.sched` ends in `pump()`, and an item due now is resumed there —
  so `play()` on a started clock runs the routine's first pass **before it
  returns**. The Python clock pushes and lets its own thread pick the routine
  up, so `play()` returns first. Reproduced side by side: a routine printing on
  its first resume, with `print("after play() returned")` under the call, orders
  the two lines the other way round in each client.

  It is not a page's price for having one thread. The wake could be deferred to
  the ticker (a 0-delay tick) or to a microtask, and either would restore the
  reference ordering; both are changes to the driver, and every clock test drives
  it synchronously through a `ManualTicker`, so the choice is what the manual
  harness then has to await. That is why this is written down rather than fixed
  under a milestone about names.

  What it costs today, exactly: `Routine.run(function* () { ... melody ... })`
  cannot read its own binding on the first pass — the Python idiom the shortcut
  is *for*, since there the same call reads as a decorator. Its doc comment says
  so and points here.

  **Fixed with the microtask**, on the user's rule that the choice is whatever
  is cheapest *without adding anything to the user's API*: `play(routine)` is
  unchanged, and when a scheduling call defers its own pump is an implementation
  detail of this language. `sched`/`schedAbs` end in `wakeSoon`, which queues
  the pump and returns; inside a wake it does nothing, since `pump`'s loop
  re-reads the queue before it sleeps. Deliberately **not** the ticker: an item
  with no wait left is due, and making it wait for a tick would be saying it is
  not. And not a worker either — the pacing already lives in one
  (`workerTicker`); what cannot move off the page thread is the *resume*, since
  a routine closes over page objects a worker cannot be handed.

  **The cost was the harness, as predicted, and it landed in one place per
  file**: `tests/flush.ts` is the one thing a hand-driven test now awaits, and
  each file's `run(seconds)` awaits it once at the top, so about sixty tests
  changed by a keyword and an `await` rather than in their substance. Two tests
  changed in substance, and both were asserting the old order: the default
  session's bare `Routine.play()` now asserts that the first pass is *not* on
  that call's stack and then awaits it.

  **And it turned up a divergence underneath**: `Timeline.fromPattern` bounced a
  pattern by building a `manualTicker` and driving it by hand, where the Python
  one queues the pattern on an unstarted clock and calls `clock.render(dur)` —
  the offline drive that exists for exactly this and that walks the queue
  itself. Two programs producing a similar result by different calls, which is
  what the non-divergence rule names. It calls `render` now, in Python's order,
  and the deferral cannot reach it. What that dropped is a TS-only guard (a
  step cap that threw "the pattern did not end — pass { dur }"); an unbounded
  bounce of an endless pattern now hangs in both clients, identically, and the
  entry below carries the question of whether either should refuse it.

- ✅ **An unbounded bounce of an endless pattern hangs, in both clients**
  *(named and fixed 2026-08-26, the entry above having removed the one client's
  guard for being one client's)*. `Timeline.fromPattern(p)` with no `dur` over
  an endless pattern never returned, here or in Python.

  **Where the guard went, and the two places it could not go.** Not inside the
  recorder, which is where it looks like it belongs: a routine that raises
  **loses its own place and nothing else** — the clock catches it by design, so
  it can go on driving every other routine — so a guard there is swallowed and
  the bounce returns a short timeline instead of refusing. (Found by writing
  it that way first; the Python test failed with the traceback printed and the
  call returning normally.) And not in `render`'s default, because a long
  offline render of a real score is exactly the thing that is meant to run for
  a very long time.

  So `render` gained an **optional** bound on resumes — `max_steps` /
  `{ maxSteps }`, defaulting to none — for the caller who knows its source
  might never end, and `fromPattern` passes it whenever no `dur` was given,
  re-raising in its own terms ("the pattern did not end after N events — pass
  `dur=` to bound an endless one"). The number is the caller's too
  (`max_events` / `maxEvents`, `MAX_BOUNCED_EVENTS` = 1,000,000 by default),
  which is what makes it **testable**: the test proves the refusal at 32 events
  rather than recording a million. Both clients, same commit, same message.

- ✅ **The operator vocabulary had no home, so the class trees differed**
  *(found 2026-08-24, adding the range maps to both clients and having to
  decide where the TypeScript ones could go; fixed the same day)*. Python
  overloads `+ - * /`, so its operator base is a neutral `AbstractObject` in
  `base/absobject.py`, and every expression family hangs off it: the UGen
  graph, the Faust one, the per-bin `PvExpr`. TypeScript has no overloading, so
  the equivalent is a class **full of methods** — and that class had never been
  given a home: it was written inside `defs/ugens/graph.ts` and called
  `SynthExpr`, so `PvExpr`, which wants exactly the same vocabulary per bin,
  extended the UGen module to get it. Nothing was wrong with the *surface*; the
  two trees differed because of where the code happened to live, which is the
  kind of asymmetry that decides the next question wrongly.

  It decided one immediately: a range map is a method that composes a
  `RangeMapUGen`, so putting the six on `SynthExpr` would have given `PvExpr` a
  `.linexp()` that Python's has not — a divergence created by the shape of the
  file. That is what `GraphExpr` was for when the maps landed, and it is gone
  now.

  **`base/absobject.ts` is the home, `AbstractObject` the name it already has
  in the other client**, beside the value functions that compute the same
  operators. `PvExpr` extends it directly; `SynthExpr` means the UGen-graph
  branch again and carries what only that branch has. The one thing a typed
  language needs here and an untyped one does not is `Fan` — which operand kind
  fans a result out and into what (a channel list in the graph, nothing per
  bin) — kept as one type because the pair only ever travels together. No
  surface moved and no emitted spec changed, which the parity vectors say
  without being touched.

- ✅ **The composer example was a second program, not the same one in another
  language** *(found 2026-08-22, when the user asked why the two examples were
  not identical after the curve was fixed in one and not the other)*. The
  arrangement matched lane for lane, which is what the earlier pass fixed — but
  everything around it had been improvised: the page assembled `engine()`,
  `Server.open(pageConnection())` and a bare `TempoClock` where the script opens
  **one `Session`**; it bounced its takes through the free `render()` instead of
  an **offline session**; it called `editor.render(...)` where the script calls
  `editor.play(...)`, so the transport parked at the end read as a dead button
  with no rewind; it had no `status` label, so a press reported to a log below
  the window instead of *in* it; and it never located the cursor at the top. The
  page now makes the same calls in the same order (`Session.page` is this
  client's `live`), and the **one** line it cannot share is named where it sits:
  a tab has no folder, so the bounce is installed with `Buffer.fromSamples`
  where the script writes a WAV and loads it with `Buffer.read`. That difference
  is a real gap in both directions and is written down as one (root `PLAN.md`,
  W24) rather than settled by an example.

- ✅ **The document writer took every wrapped object for a preserved node**
  *(found 2026-08-22 chasing the curve that would not edit, and the more serious
  of the two)*. `preserved()` asks whether a base `Element` is holding a **raw
  document node** — the format's own rule that what a writer does not understand
  it preserves. The Python client spells that `isinstance(element.wraps, dict)`;
  this one spelled it `typeof wraps === "object"`, which is true of *every*
  instance there is. So a base `Element` wrapping an `Automation` — the
  arrangement's own way of holding a curve — was written out as the
  automation's **fields** (`levels`, `times`, `buf`, `bus`, …) instead of as the
  `generator` node with its break-points, and the two clients wrote **different
  documents for one composition**. Downstream: `leafConfig` answered `{}`, so
  the editor's curve edit was refused by the crate with "this body holds no
  configuration" — while `applyPoints` still reported success, changed nothing
  and left no undo behind. Fixed by testing for a **plain** object (its
  prototype is `Object.prototype` or null), and the parity suite gained the case
  that would have caught it: `gen-form-vectors.py` had no `Automation` in any of
  its five compositions, which is the whole reason a divergence in the document
  format survived a suite built to compare documents.

- ✅ **A curve edit named the clip that drew it instead of the leaf that carries
  it** *(found the same day, in both clients)*. `applyPoints` addressed its
  `configure` intent to `placed.member.element` — which for a layered clip is
  the **aggregate**, not the member holding the `Automation`. `automationOf`
  descends into a simultaneous aggregate, so the curve was found and the sound
  followed, but an aggregate is not a leaf: the intent replaced an empty
  configuration, the crate had nowhere to keep the points, and the edit reported
  success with nothing changed and nothing to undo. The notes route had had
  `rollOwner` for exactly this since it was written; the curve route never grew
  its mirror. Now it has one (`curveOwner`/`_curve_owner`), and both clients
  carry the test.

- ✅ **A clip's curve body drew nothing — in every client.** A `clip` carrying
  `points` laid out as an empty rectangle: the automation lane of the multitrack
  editor, in a page *and* in a native window. `Element::draw_body` defaults to
  drawing nothing and `Curve` never overrode it, so the body was built
  (`clip_bodies`), placed (the layout test asserts it) and collected for the
  pass — which then called a method that painted nothing. What hid it for so
  long is that every test drove `Curve::draw` with a `TimeSpace`, the
  *standalone* door taking its body-shaped branch, and the two doors are only
  the same when something connects them. Found by porting the editor and
  comparing the two clients' screens side by side; fixed in
  `crates/clausters-gui/src/host/elements/curve.rs`, pinned by
  `a_clip_body_draws_the_line`, which goes through the body door.

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

- ✅ **The wheel had no instrument, so its calibration was a guess**
  *(added 2026-08-21, taking the GUI plan's "The wheel zooms faster in a page
  than in a window")*. What one notch reports is neither a constant nor the same
  in two browsers — Firefox sends `DOM_DELTA_LINE` with the viewer's own scroll
  preference as the count, Chrome sends `DOM_DELTA_PIXEL` with about a hundred
  CSS pixels, and winit multiplies a pixel report by the window's scale factor
  before the host sees it — so the host's `Wheel::BROWSER` cannot be written
  without a browser. `tests/wheel.html` is that browser: three facts, no wasm,
  no engine, no server, open it and turn the wheel.

  It is the one page in `tests/` deliberately **outside** `test.sh`'s `run_page`
  list, and the list says so rather than leaving the absence to be read as an
  oversight: it needs a hand and has no verdict to beacon. It stays in the tree
  because the figure is per browser and per machine, so it is asked again rather
  than once: Firefox 153 answered 6 lines a notch, Chrome 151 answered 120 CSS
  pixels, on the same machine and the same day.

  **The page's own arithmetic was wrong first time out**, which is worth keeping
  because it is the kind of error a measuring instrument hides rather than
  announces: it divided a pixel report by `devicePixelRatio` and reported 80
  where the answer was 120. `deltaY` is already in CSS pixels, and the ratio
  belongs to undoing winit's conversion to physical ones — which the *host*
  does, not the page. The two cancel. Read wrongly it even reversed the sign of
  the error, showing a notch as short when it was long.

- ✅ **A bulk read of more than one chunk gets no reply on the in-page carrier** *(found 2026-08-19 writing `tests/recording.html`, which reads a take back to compare it: `buffer.getSamples()` over 25 600 frames raised `ReplyTimeout: no reply to /buffer_getRange within 5s`, while the same call with `{ chunk: 4096 }` returned every sample)*. So it is not the size of the buffer but the size of one **request**: the chunk `getSamples` picks by default (`server.bulkChunk()`, the server's own maximum) produces a reply the page's ring does not deliver, silently — no `/fail`, no partial, just nothing. The test works around it by naming a chunk, which is what makes the workaround visible; what it means for an ordinary page is that reading a buffer of any size fails by default and works when a number nobody should have to know is passed. Worth taking with the next thing that touches the carrier's framing: either the ring's frame limit is what `bulkChunk` should answer, or the reply has to be split where it is written.

  **The mechanism is no longer a guess** *(2026-08-20, from the GUI host hitting the same wall)*: the server drops a reply whose ring has no room — deliberately, "backpressure, not loss ... all we can do without blocking the server" — and the ring is 64 KiB. So the loss is by design and every reader over the carrier owes itself the arithmetic. The host now does the first of the two fixes named above for its *own* requests: it sizes a chunk so several fit the ring, bounds how many it has in flight, and asks again for what never came back. This entry is the same reasoning applied to `bulkChunk`, which is the number a *user* of the client gets handed.

  **Fixed 2026-08-21 by giving the carrier the answer**, which is what the
  reference client had and this one did not: `Connection.stream` is the port of
  `OscInterface.stream` — whether a packet on this carrier is free of the
  ceiling one delivery imposes. A `WsConnection` frames its own packets and says
  `true`; `pageConnection` is the 64 KiB ring and says `false`. `bulkChunk` reads
  that and nothing else, so a page keeps the classic 1024 and a socket keeps the
  server's frame ceiling, and `getSamples()` over a take of any size now works
  without being handed a number. It is a capability rather than a list of carrier
  types in `Server` for the same reason it is one there: a carrier that module
  never heard of answers the question for itself, which is what
  `tests/server-open.test.ts` asserts against the reference client's own
  `test_bulk_chunk_reads_the_carrier_capability_not_its_type`.
  `tests/recording.html` reads its take back with no `chunk`, so the workaround
  that made the defect visible is gone rather than left as an idiom.

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
  whoever draws them, and it was copied into `examples/views/meters.html` in the same
  commit while `tests/data.html` still draws the gap. Two questions a fix has to
  settle: whether the rule stays a recipe each page repeats (documented where
  `columns` is) or the client grows the drawing side of it — a small helper over
  a `PeakRow`, or a `columns` variant returning joined spans — and, since the
  measurements must stay exactly what the core measured, that the join lives in
  `data/` rather than in the core or the wire.

  **What is not established is which surface the report is about.** A page of the
  user's own and `tests/data.html` fit the entry; `examples/views/meters.html` fits it
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
  it. `tests/data.html` and `examples/views/meters.html` (which loses its hand-copied
  recipe) both go through it, the web book's waveform section teaches it, and
  the case is asserted on both sides: `peaks::join_tests` in the core, "a square
  wave's edge is inked once the columns are joined" in `data.test.ts`.
  `docs/bindings.md` carries the wasm row; the C ABI has none, since nothing on
  that side strokes pixels.

  **And then the web half went** *(W26, 2026-08-20)*. The fix above was right
  about *where* the rule lives and wrong about who calls it: a page strokes no
  columns at all any more, so `data.joinColumns` and the pixel row it joined
  (`Peaks.columns`) were removed rather than kept in step with a host that has
  the only renderer. The core's `peaks::join`/`join_columns` and the assertions
  under them are untouched — this entry's record of the defect stands, and the
  square wave is drawn correctly by the one thing that draws it.

- ✅ **Two pairs read side by side turn out not to be pairs** *(found
  2026-08-24, in C45's pass over both example directories)*. The reorganization
  put every script beside its page for the first time, which is what made these
  legible; both are recorded rather than patched, because the rule is that a
  divergence found by reading a pair is a plan entry and not a local edit to
  whichever file was in front of you.

  - **`panels/standalone` is two different programs.** The script *authors* a
    bundle — it writes the defs and the GuiDef to a data directory and prints
    the `clausters-gui --standalone` command that would launch it, talking to
    nothing. The page *boots* one — it fetches a pre-built bundle and runs it in
    the tab. They share one verb. Each half is worth showing and neither client
    shows both.

    **Decided with the user, 2026-08-26: a page does not write bundles, and
    that is the design rather than a platform limit.** A bundle is an *input*:
    it is produced ahead of time and saved, so a **static** page can boot it
    against the gui client and a standalone server **with no interpreter at
    all** — which is the whole point of the standalone shape. Authoring it in
    TS is a convenience (not changing language for one step), and W15 already
    says so: *"Runs in Node (a file writer) and in the page (an in-memory
    bundle mounted without a round trip through disk)"*. The page's half of
    W15 is the in-memory mount, which is a different thing from writing one.

    **So the two halves were paired wrong, not missing.** The counterpart of
    `standalone.py` is a **node script**, and the page is the counterpart of
    *running* `clausters-gui --standalone` — the half the Python example ends
    by printing a command for. Nothing is added to the page.

    **What it leaves open** is where a node example lives:
    `clients/web/examples/` holds pages, and this would be the first script
    there. Its own subdirectory, or beside the suites — a question for the
    `examples` skill, since that is where the three directories and their forms
    are written down.

    **W15 shipped without answering it** *(2026-08-27)*, so it became the one
    thing between the writer and its examples rather than a note beside a
    milestone. Three scripts waited on the answer, not one: the counterpart of
    `standalone.py`, and the two `examples/panels/*/make_bundle.py` that write
    the bundles this package's own pages mount — a Python script authoring the
    web client's examples is itself the divergence, and it was only still there
    because there was no TypeScript writer.

    **Answered the same day, and the rule is in the `examples` skill.** A web
    example is a page for anything a page can do, and a **node script**
    (`.mjs`) for authoring, which is the one thing it cannot: writing a bundle
    is not something a page does, because a bundle is an input a static page
    boots with no interpreter. The script sits **beside the page it feeds**
    (`panels/piano/make_bundle.mjs` next to that example's `index.html`), and
    the page's header says which command to run, since a page whose bundle has
    not been written shows nothing.

    With that, the pair is `panels/standalone.mjs` ↔
    `clients/python/examples/panels/standalone.py`, and `standalone.html` is a
    lone page whose sibling is *running* `clausters-gui --standalone`; the two
    `make_bundle.py` are gone, ported verb for verb, and their output is
    **byte-identical** to what the Python writer emitted — which is the
    canonical format proving itself outside a test. Two things fell out of it:
    `./test.sh` and `scripts/smoke-web.sh` no longer skip or fail when the
    Python client is not importable (this package now writes its own example
    bundles), and every generator in the three example trees writes into an
    ignored `out/` rather than into the working directory.
  - ✅ **`transport/sync`'s page has a half the script does not**, and says so in
    its own prose: a playhead that obeys the shared grid. That is not a platform
    difference — a script can follow a playhead — so it is the script that is
    behind. **Closed 2026-08-26 by moving the script forward**, which is the
    direction the rule names: `sync.py` gained the same `Timeline` of four
    quarter notes, the same `Playhead(...).loop(0, 4)`, the same
    `follow_transport(sa, quant=4)`, and the conductor's three broadcasts —
    `transport_play`, `transport_locate(4.0)`, `transport_stop` — in the page's
    order. Where the page makes each step a button, the script runs the sequence
    through, which is the one difference and is the platform's: the page's extra
    is a *hand*, not a call. Its prose no longer claims a half the script lacks.
    Verified by running it: the playhead advances 0 → 3.44, wraps on the locate,
    and halts.

  Neither blocks W16. They are named here because a reader who opens the pair
  expecting one example in two languages currently finds two, and nothing else
  says so.

- ✅ **Forty-seven scripts have no page, and twelve pages have no script**
  *(counted 2026-08-24; closed 2026-08-25, when every script that can have a
  page had one)*. W16's real size, stated as a number rather than as "about
  forty", and then paid: it went 19/47/12 → **55 pairs, 11 scripts alone, 11
  pages alone**, counting a directory with an `index.html` as its own name. It
  has moved once since, and only by a milestone paying its own acceptance:
  **W7** and **W9** wrote the three pages named below on 2026-08-27, so the
  count today is **58 pairs, 8 scripts alone, 11 pages alone** and every one of
  the eight has its reason in this entry.

  **The eleven scripts left are each accounted for**, which is what the
  milestone's acceptance asks: three belonged to another milestone's acceptance
  (`faust/boxes-library` to **W7**, `io/midi-responder` and
  `editors/pianoroll-midi` to **W9**, all three paired on 2026-08-27 when those
  milestones landed); six have **no page by nature** and a seventh,
  `editors/session`, has none and could not, each with its reason below; and
  `io/osc-responder` is **paired under another name**, `io/responders.html`,
  which says so in its own header.

  **And the eleven pages left are too.** Six are a page's own subject with no
  script behind it — `components/demo`, `components/lifecycle`,
  `editors/document`, `panels/graph-controls` (a bundle mounted as a component),
  `panels/two-hosts`, `basics/engine` (the platform itself). One,
  `io/responders`, is the pair above. The remaining four turned out not to be
  gaps at all: **their siblings are the *server's* examples**, in the root
  `examples/` directory that the pairing count never looks at —
  `basics/demand` ↔ `examples/demand.py`, `basics/synth` ↔
  `examples/synthdef.py`, `transport/sequencing` ↔ `examples/sequencing.py` —
  and `panels/host` is the carrier contrast that `panels/skeleton` and
  `panels/attach` show between them. All four now say which example they are, in
  their own headers, which is what the layout rule asks of a page whose name
  cannot be read back to a script's.

  **What the 33 are, so "a destination rather than a queue" is a statement with
  a list under it.** 27 are portable with the surface this client has today —
  `views/` (12), `editors/` (6), `spectral/` (3), `transport/` (2),
  `buffers/` (2), `io/osc-destination` and `io/osc-responder` (the web has
  `OscDestination`, over a WebSocket bridge where the script has UDP). Three are **no longer
  counted here at all**: `faust/boxes-library` belonged to **W7**'s acceptance and
  `io/midi-responder`/`editors/pianoroll-midi` to **W9**'s — all three written with
  their milestone on 2026-08-27, since no page could be written before it and
  writing it is how each was shown to work. One more has **no page and could not**: `editors/session` is *the third
  writer* — this client writes a session file, `clausters-gui --session` opens
  it as a **separate process**, edits it with no language attached and saves it
  back, and the script reads it again. A tab has neither the file nor the
  process, and what it does have (a document written and read in the page) is
  `editors/document/index.html`, which is a page of its own subject rather than
  this one's port.

  One is **paired under another name**, which the layout rule allows and the
  page itself states: `io/osc-responder`'s page is `io/responders.html`, because
  a tab has no socket to bind and what it responds to is the carrier it already
  has — the page's header says so in as many words. Six have **no page by
  nature**:
  `io/live-udp` (a UDP peer) and `io/embedded` (the server in this process
  through the cdylib — in a tab that *is* the engine); `views/recording-mapped`,
  already stated in its own docstring — it is `recording.html`'s window drawn
  from a **mapped** segment, and "a page can map nothing"; and `views/bulk`,
  whose subject is precisely the three *shared-resource* forms a `waveform`
  accepts (a peak-cache file, a raw `f32` file, a server buffer exported to
  one). All three are files the host memory-maps, and a tab has no filesystem
  and no mapping: its bulk path is the blob beside the `/gui_def`, which
  `views/plot`, `views/linked` and `views/rulers` each show in place of a
  mapped file. Porting it would be writing a different example under the same
  name. The sixth is `io/osc-destination`: the surface exists here
  (`OscDestination` rides a `Connection`, and the plan records the carrier
  difference — UDP there, a WebSocket bridge here), but a browser has no
  out-of-the-box peer to send to, and an example that needs a bridge nobody is
  running is not one that runs. The fifth is `buffers/render-then-load`, and for
  the same reason: its
  subject is the **file** the server writes and then reads back
  (`render(path=…)` → `/buffer_allocRead` → `read_soundfile`, "the file needs no
  conversion in between, because the same process wrote it and reads it"). A tab
  has no file at any of those three steps; the *change of state* the example
  opens with is `buffers/offline-render.html`, which is paired and does exactly
  that with the samples in hand. The 12 are not all gaps — `components/` is a
  surface the page has and the script cannot, `basics/engine` is the platform
  itself — but `panels/host`, `basics/synth`, `basics/demand`,
  `transport/sequencing` and `io/responders` are pages whose subject the Python
  client covers under other names or across two files, and pairing them is part
  of W16 rather than a separate job.

- ✅ **All three web smoke scripts failed before they asserted anything**
  *(found 2026-08-24 during C45, fixed the same day; the third one turned up
  while fixing the two)*. `smoke-web-standalone.sh` ended in "meter stream never
  moved (0 snapshots)", `smoke-web-components.sh` in "shared namespace never
  confirmed", and `smoke-web.sh` in "clock stuck at 512". Two different causes,
  both invisible to every build and to CI, which runs none of the three.

  **The stream two of them watched was addressed to somebody else.** The engine
  keeps a reply queue **per client**, and `addReply(fn)` with no peer hears only
  the default client's replies. The subscription those pages were watching is
  the **GUI host's**, on its own leg — so the listener was installed, nothing
  raised, and the callback simply never fired. `ANY_PEER` is the observer door
  and `tests/components.html` already used it, with a comment saying exactly
  why: when replies became per-client the test was updated and the four pages
  around it were not. Fixed in `examples/panels/standalone.html`,
  `examples/components/demo.html`, and the two bundle pages
  (`examples/panels/piano/`, `examples/panels/graph-controls/`) whose live
  readout had been sitting at 0 with nobody to notice.

  **The failure mode is the reason this needs a guard**: it fails silently and
  in the *worst* direction. An assertion waiting for movement times out with an
  empty list and blames the audio; an assertion waiting for **silence** passes
  for entirely the wrong reason. So `tests/peer-observers.test.ts` now reads
  every page under `examples/` and `tests/` and requires any `addReply` that
  mentions `/bus_stream.reply` to name the client it means — `ANY_PEER` to watch
  somebody else's subscription, `DEFAULT_PEER` for its own. Declared rather than
  forbidden, the way `docs/gui-props.md` treats a prop divergence: watching your
  own stream is legitimate, and saying so is what a reader needs. A textual
  check on purpose — a page is a plain module no type-checker reads, and the
  mistake is an omitted optional argument, which is precisely what a type cannot
  catch.

  **The third was a race, not a wire.** `tests/smoke.html` bounded its clock
  wait by an *iteration count* — 4000 MessagePort round trips — and 512 frames
  is under 11 ms at 48 kHz, so on a context that had only just begun rendering
  the loop finished before the next quantum. It passed whenever a debugger's own
  warm-up had already paid that wait, which is why it looked like a real engine
  failure: driving the same page over CDP reported `clock 1024 -> 1536`. Bounded
  by the clock now, with a yield between polls.

  What is still true and unfixed: **none of the three runs in CI**, and neither
  do the `?smoke=1` verdict modes the two bundle pages carry, which nothing
  invokes at all. That is the next entry.

- ✅ **Nothing runs the web smokes, and two verdict modes are invoked by
  nobody** *(named 2026-08-24, once the three scripts passed again; fixed
  2026-08-25)*. Three scripts existed, none of them ran anywhere, which is how
  all three came to be broken at once and stayed that way. And the two bundle
  pages (`examples/panels/piano/`, `examples/panels/graph-controls/`) each
  carried a full `?smoke=1` mode that beacons a verdict — the voice chain end to
  end, the allocated bus streaming — with no script to run them: dead
  assertions, written and never fired.

  One runner now walks a list of five cases: `scripts/smoke-web.sh` (`--list`
  names them, an argument runs a subset), with the two other scripts gone. It
  builds what the selection needs — the package once, the demo bundle, each
  authored bundle through its own `make_bundle.py` — and reads each verdict out
  of the HTTP access log, the posture every web acceptance is written in. Two
  things it does *not* inherit from the scripts it replaces: the browser is
  reaped by its **process group**, the hardening `test.sh` grew after a killed
  suite left engines running, and a failing case does not stop the run, since
  these are precisely the assertions that go unnoticed one at a time. What the
  three scripts got wrong the same way is also fixed: none passed
  `--enable-unsafe-swiftshader`, so the pages that mount a GUI host were relying
  on whatever adapter headless Chrome happened to offer.

  The other half was the claim in this entry itself: it said CI ran
  `clients/web/test.sh`. **It ran neither** — no workflow mentioned either one.
  The `web smokes (headless Chrome)` job in `.github/workflows/ci.yml` runs the
  runner now: the wasm toolchain, npm, a plain Python and one `cargo build -p
  clausters-ffi`, since writing a bundle validates it through the core's C ABI
  — which the first CI run said and no local run could, the workspace `target/`
  having had the library all along.

  Both authored pages passed on their first firing, which is the good outcome
  and not the expected one — an assertion never run is an assertion never
  checked.

- ✅ **`open()` with no host resolves the ambient one, and a page that meant
  another handle gets a plausible wrong answer** *(found 2026-08-27, in the
  manual review, reading `panels/attach.html` against `attach.py`; that page is
  fixed in the commit — what is left is the class)*. A `View.open(element)`
  with no `host` falls back to `resolveHost()`. That is the right default and
  it is what makes `plot()` work with no ceremony. What has no answer is the
  case the attach example is built on: a page holding **two** handles, one of
  which deliberately did not become the ambient one. Leaving `host` out there
  is never what the author meant, and nothing says so — the window opens, it
  draws, its ids are valid, and the only symptom is that they came out of the
  wrong allocator.

  It is worth an entry because of how it hid. The page looked right: two
  canvases, two panels, a log line reporting two different ids. It even printed
  a line *claiming* the two shares do not collide, and the numbers it printed
  (1002 and 1006) were both from the owner's share, so the check that was there
  to catch exactly this passed on a comparison of a share with itself. What
  found it was reading the two halves of the pair side by side and noticing
  that the Python one said 525288 where the page said 1004.

  The options, none of them obviously right: warn when a second `GuiHost`
  exists and `open` had to guess; make a non-adopting `attach` poison the
  ambient fallback for windows opened after it; or leave the resolution alone
  and treat this as an example's bug, on the grounds that a default that
  sometimes asks is worse than one that always answers. The Python client has
  the same default (`plot` resolves a session's host) and the same gap, so
  whatever is decided is decided for both.

  **Closed 2026-10-07: the resolution stays, and it was already settled.**
  The same question came up again when a scope had to reach a server over a
  socket ("`resolveHost` makes one for a server whose connection names an
  address", below), and was answered there: the caller hands what it means,
  so nothing is guessed. That is the third option above. A warning or a
  refusal cannot be triggered by "more than one host is alive" without firing
  on the legitimate cases — `plot` keeps a host of its own per server, and two
  hosts in one program is `panels/two-hosts` — so the defect is the call that
  leaves `host` out while meaning another handle, which the attach page was
  and no longer is. What the class needed was the rule said where the call is
  written: `View.open`'s documentation in both clients now says that leaving
  `host` out always means the ambient one, and that a second handle that did
  not adopt it is named.

- ✅ **A page turns a script's sequence into a button, and then explains why**
  *(found 2026-08-27 and 2026-08-28, in the manual review, on `panels/style`
  and `panels/text`; both pages are fixed in their commits — what is left is
  the class and the sweep)*. Both twins replaced a timed list with one button
  that fired every change at once, and both lost steps doing it: `style`
  dropped the theme clear and merged `opacity` with `radius`, `text` dropped
  the first of the two alignments. What each was showing — that a prop is
  *live*, one change at a time — is precisely what a single button cannot show,
  so the page produced a similar picture and demonstrated nothing.

  The reason it is a class and not two bugs is the second page's prose, which
  had written the drift down as a rule: the button was "one the script has no
  counterpart for … because a page has an event loop and no reason to sleep
  through a list". A page has a clock; nothing was impossible. That sentence is
  a scaffold that became a norm, and the next page would have inherited it.

  **What is not yet done is the sweep.** Only these two Python examples pace
  with an explicit `CHANGES` list, and both are answered; what nothing has
  looked at is the rest of the pairs, where the script paces with `sleep`s
  inside its own loop and the page may again be waiting for a hand. The review
  will reach them one by one, and this entry closes when it has — or sooner, if
  somebody reads the pairs for this one property alone.

  **The rule it leaves behind**, worth stating once because both pages broke
  it the same way: a button in a page is legitimate when it stands for
  something a person does (the autoplay gesture, "open the window") or for a
  step the script also takes on its own. It is not legitimate as the page's
  answer to a script's clock.

  **The sweep, done 2026-10-07**, over every pair whose script sleeps on the
  main thread (the 83 pages read against their scripts for that one property).
  It found the class twice over: the button standing in for a clock, and the
  prose that wrote the drift down as a rule — "a page must not block", "each
  verb is a button", "a page has one thread it must not block". A page awaits,
  which blocks nothing, so none of those sentences was ever true.

  - **A sequence that had become buttons**, now the script's run on the
    script's clock: `buffers/buffer-edit` (four stages were four buttons; the
    script's `run()` is one call with its 6.5 s and 8 s in it, and the page now
    has the two buttons the script has — bring it up, call `run()`),
    `panels/attach` (the attached handle let go when a hand pressed a button,
    where the script lets go two seconds after the windows open) and
    `views/window` (the second signal was a button, where what the script shows
    is a view changing under a repointed source with nobody touching it).
  - **A cell kept as a button, but its own clock lost**: `transport/conductor`
    read the followers' positions on a button press, where the script reads
    them three times 0.7 s apart after the play, and split the locate and the
    stop the script does 1.5 s apart into two buttons; `transport/timeline`
    read the position when the next button was pressed, where the script reads
    it 1.2 s after the play; `io/servers` closed the attached handle the
    instant every node was freed, so the fifth dropping to one note could not
    be heard, and stopped `a` with no wait after freeing its note;
    `basics/hello-note` said the phrase's synths had freed themselves before
    the phrase had played; `panels/panel` set the cutoff in the same turn the
    panels opened, where the script waits half a second so the change is seen.
    A cell is what a hand runs in the script too, so these keep their buttons
    and await what the cell sleeps.
  - **Prose that described a page that no longer existed**:
    `transport/sync` already ran the script's tour on one button, and its note
    still told the reader to press a "join late" button it does not have.

  What was read and left: every pair whose script loops with `sleep` until a
  window closes (`views/analyzer`, `meters`, `nodetree`, `scope`, `vumeter`,
  `panels/canvas`) already drives the page from a frame clock, which is that
  loop's own spelling; `basics/tempo-canon` and `tempo-map` poll the clocks on
  an interval as the scripts do; `transport/freeze` keeps a button per cell
  and awaits the three seconds inside the one that has them. The eight pages
  changed were run in a browser by pressing their buttons in order, each to
  its last line, with no error.

- ✅ **No page in the suite exercises a `canvas`, and three bugs lived behind
  that** *(found 2026-08-28, in the manual review; the three are fixed in their
  commit — what is left is the coverage; the page written 2026-10-07)*. The widget did nothing in a browser
  and had never done anything: its view was never built, and once it was, the
  host panicked twice on the way to the first frame (a `block_on` a browser
  cannot serve, `std`'s `Instant`). None of the three is subtle. All three
  needed somebody to open a page with a `canvas` on it, and no page in
  `tests/` has one.

  The gap is bigger than this widget, and it is what makes the entry worth
  keeping: the acceptance pages cover the families that were being *built* when
  each was written — plots, scopes, components, lifecycle, sequencing, MIDI,
  Faust, buffers — and a widget that shipped on the desktop before the browser
  front existed is covered by nothing. `canvas` is the one that was caught. The
  cheap answer is a page that mounts one of every built-in element and asserts
  only that the host draws a frame and does not panic, which is exactly the
  shape the desktop's own squeeze test took (`crates/clausters-gui/PLAN.md`, "Nothing
  resizes a window, so nothing tests a squeeze") and is what would have caught
  all three of these at once.

  *(Written: `tests/elements.html`, in the suite after `scope.html`. It mounts
  one of every element — the controls, the containers that draw something of
  their own, every view of a signal, the curve, the keyboard, the node tree,
  an empty score and patcher, a shader `canvas`, and the roll and the
  multitrack through the generic `node`, since no builder makes those two —
  lets the host draw them for a second and a half, and then asks the host for
  each: a `/gui_query` answers with a type, the stage holds a canvas with a
  size, and the page raised nothing, which a panic inside a frame does. A new
  element is one row of the page's `elements()`.)*

- ✅ **`boot()` adopts the page's engine where the reference client refuses**
  *(found 2026-08-28, auditing the client's abstractions against the reference
  after a reader asked why a page's boot takes three lines)*. **This is the
  root of four of the entries below and is fixed first.**

  In the Python client ownership is carried by the **verb**. `Server()` is a
  handle on an address; `boot()` starts a server this handle owns and *raises*
  if something already answers there ("Booting is for a server that is **not
  there yet**"); `attach()` takes one already running, verifies it, and owns
  nothing. There is no shared-versus-own axis anywhere, because the verb
  already says it: `io/servers.py` boots two servers side by side with
  `Server().boot()` and `Server(port=57130).boot()`, and needs no id share
  between them.

  This client made the page's memoized engine the **default target of `boot`**,
  so `new Server().boot()` twice returns two handles on one server, silently.
  Everything that follows is compensation for that: `own` had to be invented on
  `Session.embed`, `IdShare` moved into the common path, and a destination
  cannot close a carrier it never opened.

  **The shape:** `boot()` brings up an engine this handle owns — a second
  `boot()` is a second server, as in the reference. `attach()` reaches the one
  already running in this page and verifies, raising when none is. The
  page-wide sharing a component wants is `attach`, and boot-or-attach already
  has a name in the reference (`Session.live(boot=True)`, "starting a server if
  none is up"). The one real platform limit — an engine is an `AudioContext`
  and Chrome caps them at six — is **documented**, not designed around, exactly
  as `io/servers.py` documents needing a backend that mixes several streams.

- ✅ **The carrier is a positional argument where the reference builds its own**
  *(found 2026-08-28, same pass)*. `new Server(connection)` and
  `new GuiHost(connection)` make opening a carrier the caller's first step, so
  a page's boot reads `await loadOsc(); new Server(await pageConnection())`
  against the reference's `Server().boot()`. The constructor's own doc admits
  it ("That is the one shape this client's `Server` does not share with the
  reference") and blames a constructor that cannot await — but `boot()` is
  already async, so the carrier can be *named* in the constructor and *opened*
  in the verb.

  **The shape**, mirroring `Server(host, port, interface=None, transport=None,
  …)` and `GuiHost(host, port, transport="tcp", interface=None, share=None)`:
  `new Server({ transport, url, latency, sizing, timeout, share, connection })`
  with `transport` defaulting to the page's engine and `connection` the escape
  the reference spells `interface=` (it is how a score is passed, as the
  reference passes `OscNrtInterface`). `GuiHost` takes the same. Then
  `loadOsc`, `pageConnection`, `pageGuiConnection`, `engine` and `newGuiHost`
  stop appearing in examples at all.

- ✅ **`Session.embed` inverted the reference's default with a flag the
  reference does not have** *(found 2026-08-28, same pass)*. Python is
  `embed(tempo, latency, workers, timebase, server=None)` — no `own`, and every
  session opens and owns a fresh server unless handed one. This client is
  `embed({ own = false, engine, channels, … })`: shared by default, own by
  flag. Falls out of the entry above — with the verb carrying ownership again,
  `own` has nothing left to say and goes; `server=` is the handed-one case and
  keeps its meaning under whatever `engine`/`connection` ends up named.

- ✅ **`Session.activate`/`deactivate` are missing, and `adoptDefault()` is
  surface the reference does not have** *(found 2026-08-28, same pass)*. They
  are not the same capability: `activate` makes the **whole** session ambient
  (server, clock and random root) and `deactivate` gives the slot back, while
  `adoptDefault` lends only the server and cannot be undone. The scoped form
  this client does have (`use()`) is the reference's `with session:` — the half
  a page needs *least*, since a page's handlers and console outlive every
  block, which is precisely the case the reference's own docstring gives for
  the unscoped pair existing.

- ✅ **`WindowHandle` is missing three verbs and renamed a fourth** *(found
  2026-08-28, same pass)*. Against `clausters/gui/handle.py`: `set(**props)`
  (`/gui_set` on the window root) and `free()` (free the window's subtree) do
  not exist here, and `handle()` (a `WidgetHandle` for the root, so its props
  can be set) does not either; `names()` is spelled `widgetNames()`. The
  capability exists on the host (`GuiHost.set`, `GuiHost.free`) — it is the
  handle, which is how a caller reaches it, that lacks them. `free` missing is
  the worst of the four: it is the verb the repo fixes for the action.

- ✅ **`TempoClock` is missing `play` and `run`, and `lockTo` its `warmup`**
  *(found 2026-08-28, same pass)*. `TempoClock.play(routine, quant)` snaps a
  routine's start to a beat grid and is pure client-side logic with no platform
  obstacle. `run(seconds)` blocks in the reference, but `Session.run(seconds)`
  is already async here, so the clock's can be too. `lock_to(server,
  warmup=True, timeout=2.0)` lost `warmup`. Also `Session.render` does not take
  `workers`, `path` or `sample_format` — `path` being arguable rather than
  impossible, the page having OPFS.

- ✅ **`OscDestination` cannot close its carrier** *(found 2026-08-28, same
  pass)*. The reference's `close()` closes the interface **if this destination
  opened it**; here the comment reads "The `Connection` is borrowed, never
  closed here", which is true only because nothing in this client opens a
  carrier of its own. Closes with the carrier entry above.

- ✅ **Two names for one concept across the clients** *(found 2026-08-28, same
  pass)*. `SampleClockTimebase` here is `SampleTimebase`, an unforced rename.
  `main.current_tt` is `currentRoutine` — the *mechanism* legitimately differs
  (a thread-local against a module slot, justified in `base/context.ts`) but
  the concept's name did not have to. And `IdShare`/`shareOf` live in
  `base/core.ts` where the reference has `base/ids.py`, against this book's own
  promise that "a reader who knows one client finds the other at the same
  relative path".

- ✅ **`ManualTimebase` is surface only this client has** *(found 2026-08-28,
  same pass)*. A hand-advanced timebase, exported from `index.ts`. Either the
  reference has a gap (a deterministic timebase is useful to a user, not only
  to a test) or it should not be public here. Undecided, and that is the entry:
  by the non-divergence rule a surface in one client and not the other is a
  defect whichever direction it runs in.

**What this audit did *not* find, recorded so the next pass need not redo it.**
Compared class by class against the reference and clean: `defs` entire (`bus`,
`buffer`, `node`, `synthdef`, `faustdef`, `graphdef`, `patch`, `info` — reply
records being interfaces rather than classes is idiom), `seq` entire (every
pattern class pairs), `form` entire, `gui/guidef`, `gui/editor`,
`gui/transport`, the responders, `Routine`/`Stream`, and the routine context
with its random streams — which is the best port in the package: it replaces
the reference's thread-local with a module slot and writes down why that is
sound.

- ✅ **`basics/verbs` opens a session where its twin opens a bare server**
  *(found 2026-08-28, by ear, when `activate` started lending the clock)*. The
  script is `Server().boot()` and nothing else — no `Session`, no tempo, so it
  plays on the default clock at 1.0. The page opened `Session.embed({ tempo:
  2.0 })`, and its whole prose is framed on the session.

  **The tempo was fixed the day it was heard** and is not the entry: while
  `adoptDefault` lent only the server, the page's 2.0 never reached an ambient
  `play`, so the two ran at the same speed *by accident*; making `activate`
  lend the clock — which is the reference's behaviour — made the page run
  twice as fast, and a listener caught it in seconds. The 2.0 is gone.

  **Decided 2026-08-28: the script moves.** The page's session is the better
  example — a `Session` is what the ambient verbs resolve against, and the
  example is about the ambient verbs — so `verbs.py` opens
  `Session.embed().activate()` and the pair reads the same call for call. Both
  ends also gained the offline third: a `Session.nrt()` played into and
  rendered, next to the free `render`, so the tour ends on the three carriers
  a session has rather than on one.

  **And the fix that decision uncovered**: neither leg **started** the
  session's clock. While the ambient clock was the default session's, that one
  starts itself on first use; a session you hold is yours to start, and
  nobody did. So since `activate` began lending the clock, every step of the
  tour that needed one — the generator, the pattern, the timeline, the
  automation — was queued on a clock that never woke, and the tour ran with
  four of its nine verbs silent. Python says so out loud at exit ("this
  program ends with routines queued on a clock that was never started"), which
  is how it was caught; the page said nothing at all.

  **And the class it belongs to**: a page can declare a parameter its twin
  does not have and stay silent about it for as long as nothing reads that
  parameter. Nothing in CI would have caught this one; an ear did.

- ✅ **`loadOsc` outlived the reason it existed, and the pages kept awaiting
  it** *(found 2026-08-28, when a reader opened `basics/hello-note.html` after
  the carrier work and the boot preamble was still there)*. The entry above
  ends "then `loadOsc`, `pageConnection`, `pageGuiConnection`, `engine` and
  `newGuiHost` stop appearing in examples at all" — and they did not. `boot`
  and `attach` had started awaiting the core themselves, but eighty-odd pages
  still opened with `await loadOsc()`, and most still imported a carrier they
  no longer called. The name went too: `loadOsc` was an alias for `loadCore`
  over the same wasm (`bundle.ts` awaited *both*, one line apart, which is
  what an alias that means nothing looks like), and the codec is the smallest
  thing the core does. One name now, and only a caller that uses the core
  **without** opening a server — encoding a packet by hand, authoring a
  bundle — awaits it.

  **Two defects fell out of removing the awaits**, and both are the same
  shape: the pages were carrying the earlier work's mistakes and nothing ran
  them.

  - `new Server()` died inside its own constructor on a page that had not
    loaded the core: all four allocators are core registries, so moving the
    load into the verbs left the constructor needing a module the verbs had
    not yet fetched. Every page hid it by awaiting `loadOsc` first, so it
    surfaced only when that line went. The allocators are built on first use
    now, which is after a verb has run in every path and is where `reconcile`
    replaces them anyway.
  - `views/editor.html`, `views/recording.html` and `editors/score.html` had
    a two-line boot block sitting at module scope, above the `let` that
    declares the variable it assigns — left there by the mechanical pass that
    migrated them, which no build and no test reads. They had been broken on
    `main` since that commit.

  **The class**: an `await` moved into a verb does not move what the
  constructor already needed, and a page nothing runs stays broken until
  somebody opens it. The second half is the standing one — the examples are
  the manual test surface, and a sweep across ninety of them is exactly where
  a mechanical edit goes wrong quietly.

- ✅ **`transport/sync` stopped demonstrating anything the day `Session.embed`
  started booting its own engine** *(found 2026-08-28, reading the four pairs
  that put a `Routine` on a `TempoClock` after being asked where that pair
  lives)*. The page opened three sessions — a conductor and two clients — on
  what its own prose called "the page's engine (the same server two tabs would
  share)". That was true while `embed` defaulted to the shared engine; once
  ownership became the verb's to say, each `Session.embed()` brought up an
  engine of its own, so the three had **three transports** and the alignment
  the page exists to show was between numbers nobody was comparing. Nothing
  failed, nothing threw, and no suite opens this page.

  Ported properly it is also the script's shape, which it had drifted from:
  the conductor `boot`s the shared server and the two clients `attach` to it
  with `new Server({ engine: conductor.engine, share })`, each with its own
  `TempoClock`, `lockTo`, `joinTransport` and `clock.play(routine, 4)` — the
  script's `Server(share=...)`, `TempoClock(1.0)`, `lock_to`, `join_transport`
  and `clock.play(Routine(...), quant=4)`, call for call. The page now prints
  the **absolute sample** of each client's next bar, as the script does: that
  the two numbers match is the alignment, where the grid beat it printed
  before matched trivially.

  **The class**: a default that moves changes what every caller of it means,
  and an example is a caller no build reads. The audit's own entries named the
  clients it fixed; what it did not do is re-read the pages that had been
  written against the old default.

- ✅ **Three `basics` pages built by hand the clock their scripts get from a
  session** *(found 2026-08-28, same reading)*. `graph_maths.py`,
  `multichannel.py` and `typed_controls.py` all open `Session.nrt(tempo=2.0)`
  and play on `session.clock`; the three pages opened a bare `Server().boot()`
  and then wrote `new TempoClock(2.0, { timebase: await server.sampleTimebase()
  })` — which is exactly what the session does internally, spelled out. Same
  music, different calls, which is what "one example in two languages" forbids.
  The pages open `Session.embed` now and take `session.clock`.

  The one line that stays different is named in each page's prose: the script
  renders offline (`Session.nrt`) and the page plays live (`Session.embed`),
  which is the standing "a page turns a script's sequence into a button" item
  above and not a divergence of surface.

  **And the same pair said one thing two ways**: `multichannel.py` sounds each
  voice with `Event(..., has_gate=True).play(session.server)` and lets the
  event release it; the page wrote `new Synth(...)` and then a hand-built
  `/node_set gate 0` bundle, yielding `dur - 0.5` and `0.5` where the script
  yields `dur`. The script's is the reference and the page has it now. What
  the page was doing by hand is exactly what `sustain` and `hasGate` do, which
  is why the divergence was invisible: both sounded right.

- ✅ **`Routine.run` is what the pages use and what no script does** *(found
  2026-08-28, checking which examples use a clock's `run` after fixing the
  four `Routine`-on-a-`TempoClock` pairs)*. Five pages —
  `editors/pianoroll`, `buffers/buffer-edit`, `spectral/cross`,
  `spectral/kernel`, `spectral/convolution` — write
  `Routine.run(fn, session.clock)`, and all five twins write
  `Routine(fn).play(session.clock)`. Both spellings exist in both clients
  (`Routine.run` is a classmethod in `base/stream.py`, used there as a
  decorator), so this is not a missing surface: it is the same call written
  differently on each side, which is the thing "one example in two languages"
  is about. The pages write `new Routine(fn).play(session.clock)` now — the
  script's spelling, and the one the other four pairs already used.

  **The rule that came with it, and holds for both clients**: inside a session
  whose clock is the one being played on, `play` is called with **no clock** —
  the ambient ladder resolves it. It is only spelled out where the session is
  not ambient, which is the case in all five of these pairs (none of them
  activates) and is why the argument stays here. Checked across both clients'
  examples in the same pass: no site passes a clock it could have left out.
  Worth knowing that **no Python example activates a session at all**, while
  four pages do — `views/scoping`, `basics/verbs`, `buffers/offline-render`,
  `io/responders` — which is the `basics/verbs` framing question again, one
  file wider than it was written.

  Checked in the same pass and **not** a divergence: `TempoClock.run(seconds)`
  takes seconds in both clients (the one that takes a routine is
  `Routine.run`), and exactly one example uses it — `transport/sync.py`'s
  `ca.run(3.0)`, to let the bar arrive before winding the clock down. Its page
  cannot: `run` starts, waits and *stops*, and the page's clock has to stay up
  for the buttons that follow. That is the standing "a page turns a script's
  sequence into a button" item, not a gap.

- ✅ **Both clients named the clock they were already standing in** *(found
  2026-08-28, when the `Routine.run` fix raised the question of when a clock
  has to be passed at all)*. Fifteen scripts and eleven pages wrote
  `Routine(fn).play(session.clock)` while holding exactly one session — and
  they had to, because **none of them activated it**: verified by running it,
  an unactivated session is not ambient, so a bare `play()` falls through to
  the default session's clock, which offline means a real-time clock in a
  script that renders. So the argument was not redundant; the missing
  `activate()` was.

  The criterion, now rule 5 of the `examples` skill: an example built around
  one session activates it and then names nothing it owns — `send()`,
  `Synth(...)`, `play()`. What comes back is only what is *not* the default (a
  second server, another session's clock). A method on the server is not an
  argument and stays, and an example with no session at all is right as it is:
  `main` is a session too, and its server and clock are the defaults there.

  **What the pages needed and the scripts did not**: four render an offline
  take and then sound it on a live server, so they `deactivate()` the score
  session after rendering. Left ambient, their `play(buffer)` would have
  reached the score instead of the engine — silence, with nothing to read.
  The scripts end there, so they never meet it.

- ✅ **A second page put its followers on an engine that was not there**
  *(found 2026-08-28, writing the Python equivalent of the sync page's client
  line and checking who else builds one)*. `transport/conductor.html` made its
  followers with `new Server({ share }).attach()` — no engine named, so the
  verb looked for the *page's* memoized engine while the conductor had booted
  one of its own. Nothing in the page ever brings the page engine up, so the
  first follower raised "no engine is running in this page". Same regression as
  the sync page and the same cause: `boot()` stopped meaning "the page's
  engine" and the pages that assumed it were never opened again.

  Its script was the other half of the pair: `sync.py` and `conductor.py` both
  built their clients with a bare `Server(share=share)`, which **verifies
  nothing** — a handle pointing where nobody answers drops every later message
  into a UDP void — and leaves the allocators on compiled defaults instead of
  the running server's capacities. Both use
  `Server(share=share).attach(adopt_default=False)` now, which is what the
  pages say.

  **The class**: `attach` is the verb that says "this was already running", and
  a bare constructor says nothing at all — so a client that skips it is not
  simpler, it is unverified. Both scripts were run; both pages need an eye.

- ✅ **The examples clicked at both ends, and two of them started on a step**
  *(found 2026-08-28, when a listener asked whether the envelope-free synths
  in the test pages were deliberate)*. They are — a test measures one thing
  and an envelope ramps exactly what is being measured — but the question is
  the right one to ask of the **examples**, where what is taught is how the
  API sounds. Measured rather than guessed: the first and last sample of every
  WAV the Python examples write.

  Two were true onsets, both noise switched on at full amplitude:
  `spectral/chain` at 0.175 (-15 dBFS on the first sample) and
  `spectral/kernel` at -0.164. Every other file started at 0.0000 — the
  8-frame window of the first pass had been catching legitimate 2 ms attacks
  and reporting them as steps, which is why the first count of "three" was
  wrong.

  Eight ended on a step: `chain` 0.198, `offline_render` 0.064, `envelope`
  -0.042, `wavetables` 0.041, `cross` 0.040, `boxes_library` -0.032,
  `multichannel` 0.027, `graph_maths` 0.015. Two causes, not one — a
  `/node_free` cutting a sounding node mid-sample, and a render ending at the
  score's last event with the release still to come. The defs that had no
  envelope got one (`chain`, `kernel`, `cross`, `graph_maths`; the Faust voice
  in `boxes_library` got `si.smoo` on its `amp`, which is the same idea from
  that library), the routines close a gate where they freed a node, and the
  offline ones schedule `/node_free 0` after the tail as the score's closing
  event. All thirteen WAVs now measure 0.0000 at both ends, and both legs of
  each pair carry the same change.

  **What is left is not an example's to fix**: the free `render()` verb cuts
  the release of anything it bounces, in both clients — written up in
  `clients/python/PLAN.md`, since the fix is one decision for both.

- ✅ **The port improvised the roll's OSC lane, and the root barrel exported a
  name Python does not** *(found 2026-08-30 while renaming `OscEvent`/`MidiEvent`
  to `OscItem`/`MidiItem`, reading the two clients' timeline and editor side by
  side; fixed the same day)*. Three divergences in the one seam, none of which
  any build could see:

  - `Editor.oscOf` labelled only `OscItem`s, so a raw MIDI item on a timeline
    drew **no marker** in the web roll while the same piece drew one labelled
    `midi` in Python (`Editor._osc`). The lane is display-only, so nothing
    failed — the picture was simply missing a flag.
  - `OscItem` stored the whole message as one `message` tuple where Python has
    `addr` and `args`, which is why the editor read `item.message[0]` for what
    Python calls `item.addr`. Same result, different object: it now carries the
    two attributes Python's does.
  - the root barrel re-exported `MidiItem`, which `clausters/__init__.py` does
    not: both clients re-export `Event`, `rest`, `Timeline` and `Playhead` at
    the top level and keep the two raw-message items under `seq`.

- ✅ **Three places call beats what is measured in seconds** *(found 2026-08-30
  by the user; written in full in `clients/python/PLAN.md`, "Three places call
  beats what is measured in seconds")*. The port carries the same three: the
  automation's curve naming an `Env`'s seconds beats, a take's length stored in
  beats (`Vector`/`Segments`/`Segment` and the event's `dur`), and the segment
  tuple mixing frames with beats. The rule is the same — an onset is in the unit
  of what contains it, a duration in the unit of the material — and so is the
  place the conversion belongs, the flattening for playback. It lands in the same
  commit as the Python fix, since it is one arrangement model in two languages.

  **Closed 2026-08-30** with the Python entry, whose fix landed in one pass over
  the four packages (`75445faa`): `durationUnit` in `form/` and the conversion
  at the flattening.

- ✅ **The editing path said nothing in the page** *(found and fixed
  2026-09-12)*. Python's `clausters/gui/editing/trace.py` said the path out loud
  at five joints — an event routed, an entry recorded, a step of the pile, a
  publish, an acknowledgement — and the page had no counterpart and no logging
  module at all under `src/base`, so an editing session that went wrong in front
  of a person was visible to nobody on this side.

  The gap was wider than editing, so what landed is the whole of it:
  `base/log.ts` with the same three areas (`server`, `gui`, `gui.editing`), the
  same `watch`/`unwatch` doors and the same nesting rule — an ancestor answers
  for its children and arming both prints once, which is the defect Python's
  `_already_watched` exists for. `gui/editing/trace.ts` is the narrowed door, and
  every call site is the Python one, argument for argument: the two sends and the
  reply in `defs/server`, the send and the event in `gui/host.ts`, and the five
  joints.

  **The environment is the one thing that could not be ported as it stands**: a
  page has no environment variables, so it reads `globalThis.CLAUSTERS_LOG` or a
  `?clausters-log=` in its own URL, and `process.env.CLAUSTERS_LOG` under node.
  Same spelling, same values.

  `unwatch` came out of writing it — the page needed it to leave a test as it
  found it, and Python had no such verb, so it grew one too rather than the two
  surfaces differing by one.

- ✅ **The intents an edit projects were collected and nobody read one**
  *(found and collapsed 2026-09-12; written in full in
  `clients/python/PLAN.md`)*. `moved` is `changed` and `adopt` takes nothing, in
  both clients. The page's own evidence that the seam never fitted was the cast
  it needed to use it: `this.editing.moved(moved as unknown as Intent)`.

- ✅ **A box entered from a piece got its own `Application`** *(found and fixed
  2026-09-12; written in full in `clients/python/PLAN.md`)*. What blocked sharing
  was that the `Echo` sat on the `Application` while a conversation's floor is
  **one view's**; it is the editor's now and `enter` passes `app`, so a piece and
  its entered boxes are one window set with a floor each.

- ✅ **The session format was restated in both clients, and it had already
  drifted** *(found and fixed 2026-09-12; written in full in
  `clients/python/PLAN.md`)*. `SESSION_FORMAT` and `Session.format` said 1 while
  the crate has been at 2 since `O31`. Both say 2 now, and the number stays a
  **literal** on purpose: a `Session` is plain data and `multitrack.ts` opens the
  core for nothing else, so reading it from wasm would make writing a session
  need an `await loadCore()`. `document-parity.test.ts` asks the new
  `sessionFormat` door and compares instead.

- ✅ **Four readers the crate exported and nobody called** *(found and fixed
  2026-09-12; written in full in `clients/python/PLAN.md`)*. `O29` deleted this
  package's wrappers over `multitrackPicture`/`multitrackRead` and its two
  siblings; the prose in `gui/editing/edit.ts` and `gui-multitrack-edit.test.ts`
  that still named them is corrected to `multitrackProps`/`editingIntake`, and
  the four **wasm exports** are gone with their C ABI twins (core ABI **v55**).

- ✅ **A box closed blocked the piece's whole undo order** *(found by use
  2026-09-12 in the Python example and fixed in both clients the same day;
  written in full in `clients/python/PLAN.md`)*. The applier was a **view**, so a
  box whose window had closed left an entry nobody could apply — and a refused
  step puts the cursor back, so the pile blocked rather than missing one step.
  The participants are structures now: `Editing.identity` takes an `Applier` (a
  `Domain`, or a `Score` for a page) and `distribute` asks those. `projectLegs`
  left the `Adopting` interface with it.

- ✅ **The page's own host takes no id share** *(found 2026-09-13 while giving
  the GUI host its share of the server's ids; written in `clients/python/PLAN.md`,
  `C55`)*. A `Session` holding its own engine splits its ids with the host it
  opens (`splitShare`, the bridge's `id_share`), as a script does with
  `--id-share`. The page's shared engine has no such launcher: `guiHost()` boots
  a host that allocates from the whole space, beside the page's `Server` and
  beside `pagePools()`, which also take the whole space. Before `C55` the host's
  fixed window kept it clear of both by accident of its base; now nothing does.
  What is to be decided is who splits on the page: the pools already defer to a
  client's allocators when handed them, so the likely shape is one page-level
  split between the page's `Server`, its pools and its host, stated once.
  Fixed the same day (`C55`, step 7): `pageIds` is the page engine's one space,
  `pagePools` and an attached `Server` with no share of its own draw from it,
  and `guiHost()` splits it once (`splitPageIds`), handing the host its half
  through the bridge's `id_share`.

- ✅ **`editors/edit-multitrack.html` is not call for call its script** *(fixed 2026-09-14: of the three, one was a port that drifted and two were spellings nobody had declared. The page had lost the script's hot take -- the `sine` at 1.4 that fills a meter, source 6 and box 25 -- and has it now. `Part` is a named tuple in Python and an interface in TypeScript, so a page's span is an object literal: a row in the idiom table. And `push` is `Array.push`, the script's `list.append`, which the audit reads as the host's verb: a row in the pair's own table. The audit reports the pair clean.)* *(found 2026-09-13, auditing the pair while the script learned to write its piece as a session)*. `scripts/audit-example-pairs.py editors/edit_multitrack` reports three differences nobody declared and nobody decided: the script calls `sine` where the page does not (the hot take's oscillator), the script builds its join's two spans with `Part` where the page does not, and the page calls `push` where the script does not. Each is either a port that drifted or a spelling the idiom table is missing; which is to be read side by side before either file changes. The session cell the script gained the same day is declared (`docs/example-parity.md`), because a tab has no filesystem to write a take into.

- ✅ **The web book's document chapter still opens with `form`'s pages, and
  holds two sections the Python book has nowhere** *(found 2026-09-18, retiring
  "the composition" from both books)*. `docs/src/composition.md` begins with
  `form`'s elements, grouping and rendering -- the tutorial the Python book
  deleted on 2026-09-08 and reduced to `form.md`, "a frozen layer" -- and then
  "What an editor is" and "`edit(x)`: one verb over the four structures", which
  describe both clients and appear in neither of the Python book's chapters.
  So the two books teach the same surface in different places, and one of
  them still presents `form` ahead of the document. **To do:** a `form.md` in
  this book the size of the Python one, and the two editor sections in both
  books, in the chapter where each reader looks for `edit`.

  **Fixed 2026-09-18, the same day.** This book has a `form.md` the size of
  the Python one, and the tutorial pages are gone from here as they are from
  there. Both document chapters open the same way and then carry "What an
  editor is" and "`edit(x)`: one verb over the four structures", before the
  multitrack sections -- where a reader of either book looking for `edit`
  arrives. The two chapters' shared opening also lost "Everything above is the
  general tree", a sentence about the `form` pages that had stood over nothing
  in the Python book since they were deleted.

- ✅ **The web book and README name a surface the package does not export**
  *(found 2026-09-21, reviewing the tree)*.
  `docs/src/composition.md`, "Editing a multitrack", imports `Arrangement` and
  `Content` from `"clausters"` and edits with `doc.ARRANGEMENT`: the root
  exports `Multitrack` and the vocabulary is `MULTITRACK`, and `Content` is
  reachable only as `multitrack.Content` — the root's re-export list from
  `./multitrack.ts` leaves it out, where `Session` and `Source` are left out
  because they would collide with the client's own names and `Content` would
  not. `README.md` lists `Playhead` in `seq`, retired 2026-09-17, and the
  docstrings of `form/element.ts:352` and `form/render.ts:125,146-147` say a
  concrete element plays through a `Playhead` and `render` returns one — it
  returns the `Timeline`. **To do:** export `Content` from the root (the
  Python book imports it from `clausters.multitrack`, and a page reads the
  root), fix the snippet and the four sentences; the check that would have
  caught the first is the Python entry's ("The book's examples import names
  that no longer exist, and nothing reads a book's code"), written once for
  both books.

  **Fixed 2026-09-21.** `Content` is a root export, the snippet edits through
  `doc.MULTITRACK` and reads back with `Multitrack.read`, the README says a
  `Timeline` plays itself and is its own transport, and the two `form`
  docstrings say `render` returns the timeline. The check is not one test for
  both books after all: `tests/book-imports.test.ts` resolves each
  `import { ... } from "clausters..."` in a javascript or typescript block
  through the built package's `exports` map, and every `alias.Name` a block
  reads off an imported namespace — the second half is what catches
  `doc.ARRANGEMENT`, which no import names. On the old pages it reports the
  three this entry found.

- ✅ **The engine in a page caches no ephemeral Faust def** *(found
  2026-10-01, making generated defs `tmp_`)*. A native server writes an
  ephemeral def's record and bitcode to a temp directory, so a replayed
  expression skips the recompile; the in-tab engine has no temp directory
  wired and compiles it every time (`src/osc/server/async_pipes.rs`). The
  page has storage for it -- the origin private file system its file commands
  already use -- so this is the port's gap: a directory of that storage as
  the ephemeral cache. **Fixed 2026-10-01, and the premise was half wrong**:
  the native cache was written and never read -- a live `/def_send faust`
  always compiled, and the bitcode was filed under the def's name, which for
  an ephemeral def is a fresh uuid every send. Decided with the user to make
  it real on both: an ephemeral def is cached **by its content**
  (`faust::cache::content_key`), the bitcode in the OS temp directory
  natively and the compiled module with its JSON in the page's storage
  (`engine/nrt-worker.ts`), keyed there by the commit the vendored compiler
  was built from (`vendor/faust/version`, written by
  `third_party/build-faust-wasm.sh`), so a hit loads no compiler at all.

- ✅ **The offline score and its carrier are named apart from Python's**
  *(found 2026-09-27, listing every class named after a score)*. The page's
  `Score` (`src/base/connection.ts`) is Python's `OscScore` — the same
  accumulated bundles, the same binary it serializes to — and its
  `ScoreConnection` is Python's `OscNrtInterface`; both are exported from the
  package under the names that differ (`src/index.ts`). A divergence of
  surface, so the fix is a rename to Python's names, with the TSDoc, the web
  book and any page or test that spells them. `Score` with no prefix is
  notation's in both clients (the engraver's editable score) and stays so.
  **Related:** the server's own `Score` (root `PLAN.md`, Found by use, "The
  render's score is named `Score`, and the clients call it `OscScore`").
  **Fixed 2026-10-01** with the server's, in the same commit: `OscScore` and
  `OscNrtInterface`.

- ✅ **No client binds a MIDI channel** *(found 2026-09-29, closing root
  `PLAN.md` `T9`)*. The port of `clients/python/PLAN.md`'s entry of the same
  title: `/midi_bind`, `/midi_unbind` and `/midi_map` get their builders on
  the server beside `laneSet`, in the shape Python's take. **Fixed
  2026-09-30** with Python's, in the same commit: `ServerMidi`'s `midiBind`,
  `midiBindZone`, `midiUnbind`, `midiMap`, `midiUmp` and `midiQuery`.

- ✅ **A played event's curves are stepped at 48 kHz off the sample clock**
  *(found 2026-10-04, porting `C62`'s emitter; fixed 2026-10-07)*. The grid a curve's values
  are sent on is the engine's block at the server's sample rate
  (`seq/curves.ts`, `CurveEmitter.sampleRate`). The Python client reads the
  rate off the clock when it is on the server's samples and off the server's
  options otherwise; this client's `Server` keeps no rate, so off the sample
  clock it assumes 48000. Against a server at another rate the step is not
  the engine's block: the values still land, a block apart on average, but
  not on its boundaries. The port's gap: the handle would keep the rate
  `/server_query` already answers and the emitter would read it.
  *(Fixed: `Server.reconcile` keeps the nominal rate the query answers, on
  an internal `sampleRate` of the handle, and `CurveEmitter.sampleRate`
  reads it off the sample clock. A handle that never asked — an offline
  score, one sized by hand — still steps at 48000, as Python's does at its
  options' default.)*

- ✅ **A page's own host is never a client of a server over a socket**
  *(found 2026-10-07, giving `resolveHost` the server an editor plays on;
  done the same day)*.
  `resolveHost(server)` answers a host that is a client of the server's
  engine: the page's own host for the page's engine, one opened for an
  engine a server holds itself. A `Server` reached over a WebSocket has no
  engine in the tab, so `scope` and an editor that plays on it get the
  page's host, a client of the page's engine: the taps drawn and the
  transport followed are another server's. The host can be pointed at one
  (`GuiBridge.connect_server(url)` attaches that leg) and nothing in this
  client calls it. What it
  needs first is the question the entry "`open()` with no host resolves
  the ambient one" already asks: which handle a page with two servers
  meant.
  *(Done. `newGuiHost({ url })` makes a host whose audio leg is a WebSocket
  of its own to that server — `connect_server`, which nothing called — and
  opens no engine in the page for it; `resolveHost` makes one for a server
  whose connection names an address and keeps it by that address, with the
  server's ids split as they are for an engine's host. The question it was
  said to wait on did not stand in the way: the caller hands the server it
  means, so nothing is guessed. Checked against a real `clausters --ws`
  with its OSC log on: a scope on a session over the socket opened on the
  host kept for that address, the page opened no engine, and the server
  received the host's own `/bus_tap`, `/bus_tapStream` and `/clock_query`.
  No page of the suite runs against a socket server, so this stays a check
  made by hand.)*

- ✅ **A verb called on the score editor's handle told nobody the score
  changed** *(found 2026-10-06, writing the chrome-less window's test as the
  twin of Python's)*. `ScoreEditor`'s verbs marked the turn changed and
  closed no turn: Python's `_act` runs inside `Editing.turn`, whose way out
  is what brings every other view of the score in step and calls
  `on_change`, and the port's `#act` ran bare. So `onChange` stayed silent
  after `editor.move(1)` — and fired late, with the next gesture on the
  page, which closed the turn the verb had left marked. **Fixed 2026-10-06**:
  `#act` runs in `editing.turn(this, ...)`, as a gesture's delivery does; the
  two tests are now one test in two languages.

- ✅ **The audit reads `transport/conductor`'s two halves as different** *(found
  2026-10-08, running `scripts/audit-example-pairs.py` over every pair while
  closing other work)*. The page's call sequence ends with a `free` the
  script's does not: both `close()`s free the governed group, so either the
  script's `close` is not reached where the audit reads the sequence or the
  page frees once more. Nothing in this pair was touched since `75685cc5`;
  read the two side by side, decide which one is the program, and make the
  other the same -- or declare the difference in `docs/example-parity.md` if
  it is the platform's.

  **Closed 2026-10-08, the same day: the two halves were one program, and
  the declaration was stale.** Both `close()`s free the governed group; what
  differed was `docs/example-parity.md`, which still said only the script
  freed it (from before the page's `close()` did) and held a second
  `transport/conductor` block naming an `unfollow_transport` the script no
  longer calls. The audit merged two blocks under one name silently, which
  is how the stale one hid; it now refuses a block written twice, and that
  found a second pair written twice (`faust/boxes_library`), merged into one.

- ⬜ **The `worklet` smoke timed out once in CI** *(found 2026-10-12, run
  `38281229363`, `web smokes` job)*. `scripts/smoke-web.sh` reported `worklet
  FAILED: no verdict within 90 s` while the four other cases passed in the same
  job; a rerun of the job passed, and the case passes locally three times out
  of three on the same tree. So it is intermittent and unexplained:
  `tests/smoke.html` never reached a verdict, and the log says nothing about
  where it stopped -- whether the AudioWorklet never started on the runner's
  headless Chrome, the engine never ticked, or the page hung after. What is
  open is the diagnosis, and before it what lets one be made: the case should
  say where it stood when the 90 s ran out (the last step reached), so the
  next timeout is a fact rather than another rerun.

  **Instrumented 2026-10-12; the diagnosis is still open.** The page beacons
  `/smoke-step-<stage>` as each stage starts (`core`, `boot`, `resume`,
  `status`, `clock`, `sine`), and the resume and every clock read now have a
  deadline of their own, so a hang there becomes a `FAIL` that names it. On a
  timeout `scripts/smoke-web.sh` prints the last step reached, the page's
  console from the browser's log (`--enable-logging=stderr`) and the tail of
  the rest of that log. Checked by forcing a hang at `status` and at `clock`.
  What is left: read the next CI timeout and fix what it names.

  **Watched 2026-10-15; not reproduced.** No CI run since the instrumentation
  has timed out in this case (the three `web smokes` failures of 2026-10-15
  were the job cancelled by the process-group reap, since fixed), and twenty
  runs of the case in a row pass locally. The entry stays open on the same
  terms: the next timeout's output names the step.

- ✅ **The example-pair audit misreads the persistent plugin pair** *(found
  2026-10-14)*. The extractor was not at fault: the pair's rows were stale.
  They declared an `Array.find` and two `Array.push` the pages no longer made,
  so `find` hid the page's `catalog.find`, which is the pair's real call. Those
  rows are gone, the pages' own `click` went with them, and a row that finds
  nothing to drop now fails the audit. What stays declared is `embed`/`live`:
  the script boots its server and the page attaches to one.

- ✅ **A client's own file write makes its directories in a tab, and not
  under node** *(found 2026-10-22, accepting the audio editor)*.
  `SoundFile.write`, `render({ path })` and a MIDI score's `write` go through
  `base/files.ts`, whose page half makes every directory above the file and
  whose node half -- like the Python client's -- refuses a directory that is
  not there. The server's own writes now refuse one in a tab too
  (`docs/decisions.md`, "A path in a page is its own storage"). Several pages
  rely on the tab's behavior to make `out/`, where their scripts call
  `os.makedirs`; settling it means one rule for both halves and the pages
  making the directory as their scripts do.
  *Fixed 2026-10-22:* no write in a tab makes a directory, as under node;
  the pages that write make `out/` with `opfs.makeDirectory` where their
  scripts make it.

- ✅ **An offline render in a page reads and writes no file** *(found
  2026-10-22, opening `X28`)*. The page's offline render is one synchronous
  call into the engine's wasm (`engine/render.ts`), which hands back every
  sample it rendered: a render of an hour holds the hour, and the streams
  that read and write files have nobody to serve them, since in a page the
  host is their reader (`dsp::disk_web`) and nothing drives it during a
  render. So a `DiskIn`, a `TransportDiskIn` over a list of files, a `DiskOut`
  and a `PeaksOut` in an offline score do nothing in a page, and
  `render({ path })` writes the file only once the whole take is in memory.
  The audio editor's processing is a render that reads a list from files and
  writes a file with its peaks (`crates/clausters-apps/PLAN.md`, `X28`), so
  its page half needs this first. What it takes: the render run in the NRT
  worker, a chunk of blocks at a time, with the streams served between two
  chunks through sync access handles (the worker can read and write at an
  offset synchronously), the render's own output written at an offset as it
  comes, and the same refusal and the same file a native `--nrt` render gives.
  *Fixed 2026-10-23, opening `X28`:* the render is the engine's
  `OfflineRender`, a run of frames at a time, in a Worker of its own (one per
  render); the files the score reads are opened with sync access handles,
  read-only where the browser shares them, and the engine reads them through
  a synchronous reader inside the block that needs them, so a list is there
  from its first frame as natively; each run is written where the body ends
  and the outputs are drained between runs. The live engine's span reads take
  no lock any more (a `File` slice), so a render reading a take does not stop
  the take streaming. An offline score also learned the transport commands
  the engine carries out itself, which a render over a list needs, natively
  too.

- ✅ **A recording or a write in a tab is held whole and rewritten at every
  flush, so a take of hours does not fit** *(found 2026-10-22, closing the
  root plan's `S22`)*. The worker keeps a `DiskOut`'s whole body in memory
  and writes the file again from its first byte at each flush, so that it is
  valid at every moment, and a `/buffer_write` gathers its runs the same way
  before it writes. The W64 and RF64 headers are the server crate's and are
  right at any length, but the memory and the rewrite grow with the take: an
  hour of stereo floats is 1.4 GB held and rewritten, and the 4 GB the
  containers exist for is out of reach. What it takes: an OPFS sync handle
  that writes at an offset -- the body appended where it ends and the header
  written again at the start -- so the worker holds a run and not the take.
  *Fixed 2026-10-22, opening `X28`:* each run is written where the body
  ends and a recording's header again at the start, through a sync handle
  closed after each flush so a reader finds the file free between two;
  a `/buffer_write` writes its header only at the end, so until then it
  reads as no sound file. One path's reads and writes are serialized in
  the worker. `PeaksOut` still holds its base buckets to the end, about
  2.3 % of the samples, as natively.

- ✅ **An editor's working folder under node is in the temporary directory,
  and nothing sweeps one a tab left** *(found 2026-10-24, with the native
  fix)*. A reverse of a day filled a machine's memory: the temporary
  directory was a memory filesystem, and folders left by sessions that did
  not close through their window held it after every process had gone.
  Natively the folder is now the core's (`clausters_core::workspace`):
  under the user's cache, held by a lock file beside it, removed when the
  session ends and swept by the next editor when it was killed. The node
  client's `temporaryFolder` still names the temporary directory, and node
  has no file lock to hold one by; a tab's folders in its origin's storage
  are removed by the editor's close and by nothing when the tab dies. The
  shape to follow is the core's: the same root and the same sweep, by
  whatever a page and node can hold a folder with. A render that fails
  already deletes what it wrote, in every client.
  *Fixed 2026-10-24:* `src/base/workspace.ts` holds the folder. Under node
  it is the core's root and lock file, the lock naming its process and
  touched while held, so either client sweeps what the other left; in a tab
  it is under `clausters/work` in the origin's storage, held by a Web Lock
  and swept when no tab holds it.

- ✅ **A render's file streams are drained twice, in script and in Rust**
  *(found 2026-10-24, the audio editor in a page)*. The page's render
  worker serves an offline render's streams in script
  (`src/engine/file-render.ts`: the inputs read through two functions it
  defines, each `DiskOut` written a run at a time, each `PeaksOut`'s
  buckets kept and its file written once its synth is gone). An
  application's render in the applications' build does the same in Rust,
  through `clausters_core::files` (`server::render`, `page_streams`),
  since its threads reach the page's storage synchronously. Two
  implementations of one rule: the render worker should serve its
  render through the Rust one, with a file layer of its own over the sync
  access handles it already opens.
  *Fixed 2026-10-24:* the render worker calls one Rust render
  (`renderFile`, the native `render_to_wav_with`), which serves its streams
  and writes its output through `clausters_core::files`; the worker opens
  the files the score reads and writes first (`scoreFiles`,
  `scoreOutputs`) and answers each file call on them synchronously. The
  stepwise `OfflineRender` binding and the script's drain are gone, and the
  request's layout is one module for both of its readers
  (`src/base/file-requests.ts`).

- ✅ **`edit(file, { stage })` opens the audio editor somewhere else**
  *(found 2026-10-28, writing `tests/app-window.html`)*. `EditOptions.stage`
  says the window's canvas takes the element's box, and the multitrack and
  the other editors honor it; the audio editor opens through `openApp`,
  which takes no element, so the host appends a canvas of its own below the
  page and the stage stays empty (`tests/audio-editor.html` passes one and
  draws outside it). Either the application's open attaches the stage's
  canvas to the window it opens, or the option says it is not for an
  application.
  *Fixed 2026-10-28.* `openApp` takes the page's `element` as `open` does:
  the window the host opened on a canvas of its own takes the element's
  canvas and box, and the host removes the canvas it had appended.
  `tests/app-window.html` opens one into a stage.

- ✅ **A page's multitrack reads the window only after its own next change**
  *(found 2026-10-29, porting the live multitrack handle)*. The Python client
  reads the window's multitrack at every read; a page's objects answer at once,
  so they read a copy brought to the window's after each of the page's changes
  and steps, and when the window closes (`docs/decisions.md`, "A script's
  multitrack is the window's while it is open"). What the hand does in the
  window between those is not in the page's reads. Closing it needs the
  application to tell the page each change of the window's, or reads that are
  promises.
  **Fixed 2026-10-30, by the first** (decided by the user, `docs/decisions.md`):
  the host tells the opener `/gui_app_changed` after a hand moves the context,
  and the page brings its copy to the window's as after its own changes.

- ✅ **`editors/span_selected` differs from its twin outside the
  multitrack** *(found 2026-10-29, auditing the pair after rewriting its
  multitrack cells)*. `scripts/audit-example-pairs.py` reports `job` at the
  page's start and `file` and three `span` reads in the audio cells, none of
  them declared in `docs/example-parity.md`; they predate the multitrack's
  rewrite.
  **Fixed 2026-10-30**, with `editors/audio_lifecycle` and
  `editors/edit_audio`, which differed the same way and had no entry. The
  reads of an audio editor's application (`files`, `dirty`, `undoLabel`,
  `sampleRate`, `span`, `parts`) and the `layer` setter are properties in a
  script and awaited methods in a page, the shape `selected` and `setSpan`
  already declare, and are idiom rows now; `file` is declared per pair,
  since `Source.file` shares the name. `job` was the page's own helper
  parameter, read as a client name, and is renamed.

- ✅ **`seq.html` fails in Firefox on a note's start** *(found 2026-10-29,
  the Firefox pass over the pages)*: `note starts[1] = 0.418s, expected
  ~0.25s`, in headless Firefox only; Chrome passes it. **Fixed
  2026-10-30:** the clock's first wake came late by however long the tick
  worker took to load, which in a fresh Firefox profile was often longer
  than the wait; a wake is now armed for an instant, and on the page's own
  timer too until the worker says it runs.

