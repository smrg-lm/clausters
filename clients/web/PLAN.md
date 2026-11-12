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
- **Biome lints, adopted 2026-11-12 (`crates/clausters-server/PLAN.md`, `R21`)**, under the same lens: one package whose rules run in a native binary, delivered as one optional dependency per platform, with no JavaScript dependency of its own, pinned exactly (`npm run lint`, `biome.json`). The established alternative, a linter plus its TypeScript parser and rule set, is several hundred packages for the same two rule groups. Only those groups are on: correctness and suspicious, less what is a style of typing rather than a likely bug (an explicit `any`, an assignment inside a condition, a callback's return, an unused parameter), and its formatter stays off. It reads what `tsc` does not: the pages and the node scripts.
- The **Emscripten SDK** (`emcc`, user-space via `emsdk`) is the one heavy addition this lens admits, and it **arrived on 2026-08-21 with notation rather than with W7**: the engraver a page loads is built from `third_party/verovio.pin` by `third_party/build-verovio-wasm.sh`, because the published npm artifact is a different build of the same version and a build is what decides which score formats a client reads (`docs/decisions.md`, which also records what would reverse the choice). It stays out of the JS toolchain proper on the terms this bullet always set: nothing in `src/` or the test loop touches it, `build.sh` only stages its output, and the slim run-time entry never loads it. What follows is the original entry, still accurate about the *other* artifact: it builds `libfaust-wasm` so a Faust def compiles in the page (`third_party/BUILD-FAUST.md`, "WebAssembly parts" — documented, never built here). It stays out of the JS toolchain proper — nothing in `src/` or the test loop touches it, `build.sh` only stages its output as static assets, and the slim run-time entry never loads them. W7 evaluated it under the same lens on 2026-08-27 and the answer held for both artifacts: the SDK stays out of the JS toolchain, but the *release* carries it (`.github/actions/wasm-vendor`), and the SDK itself is pinned (`third_party/emsdk.pin`) — a package published without either artifact loads and runs and then cannot compile a def or engrave a score, which is why `check-package` now refuses one.

## Closed tracks

The tracks whose every milestone shipped are in [`docs/history/plans/web.md`](../../docs/history/plans/web.md),
moved there whole; the plan keeps what is open. The ticked entries of "Found by
use" and "Future directions" are at the end of the same file. The tracks are,
by their headings:

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

An entry that was done without becoming a milestone is ticked and moves whole
to [`docs/history/plans/web.md`](../../docs/history/plans/web.md) ("Future directions: the entries that were done"), so this list
holds what is open.

- ⬜ **Type-safe GuiDef/def schemas.** Generate TS types for the widget/def vocabularies from a single source shared with the server, so an invalid GuiDef is a compile error, not a runtime warning. Two things have since appeared that change the shape of the answer rather than the want: the frozen parity vectors (`tests/gen-*-vectors.py`) already catch a drifted *builder* at test time, and M30's `/def_query`/`/ugen_query` make the server's own catalogue readable at run time — so the open question is narrower, which source generates the types and when, not whether one exists.

- ⬜ **A remote-server standalone page.** The in-tab standalone (a bundle booting against the embedded wasm engine) **shipped with the B track** and grew up in W4 (the bundle contract, the resolver, the pools, the components); what remains is the same mount against a **remote `--ws` server** — a one-file instrument front for a server running elsewhere. The old note called this cheap "once W1/W2 exist"; they exist, and W4 is what actually decides the work: `openBundle`/`startBundle` reach the page's `guiHost()` and `engine` singletons directly, so the step is giving the mount a **destination seam** (a `Server` + `GuiHost` pair, both already carrier-agnostic since W1/W2) in place of those singletons. The boot replay itself stays carrier-agnostic above the W0 seam, as it always was.

- ⬜ **Native plugins and the in-page engine** *(named 2026-10-13 with the server plan's J track)*. Port `PluginDef`, discovery, parameters, state, events, recovery and editor control alongside the Python client as each server capability lands, with the same calls and examples. A page connected to a native server controls its CLAP/VST3 instances; a native editor opens on that server's machine. The in-page wasm engine cannot load native plugin binaries and must report that capability explicitly while keeping the same builder surface. A possible WAM2 integration is an open design question: a different plugin binary/runtime is not automatically the same capability, and no independent DSP, mapping or drawing implementation is authorized by this entry. The native-server client port closes with the J track; any remaining in-page design stays pending here.

## Found by use: the running list of fixes

These are not milestones and they are not future directions. They are what
**using the thing** turns up — an eye pass over an example, a path read while
doing something else, a behavior that is correct and unclear — recorded the day
it is found so it is not rediscovered, and kept here rather than inside whichever
milestone happened to be open at the time.

Every entry is a **checkbox**, so what is open reads as open at a glance and
nothing has to be inferred from where it sits. A fixed one is ticked, with the
record of what was wrong and why the fix is the shape it is, and moves whole to
[`docs/history/plans/web.md`](../../docs/history/plans/web.md) ("Found by use: the entries that were fixed"): this list holds what is
open, and that file is where a search for a fixed one ends.

Anything unresolved lives here or under "Future directions", both **after** the
tracks: never inside the milestone that happened to be open, and never among
finished work, where a pending item reads as done.

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
