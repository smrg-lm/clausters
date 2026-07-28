# W4 components — implementation plan

Executes `specs/2026-07-27-web-w4-components-design.md`. Tasks are ordered by
dependency; each ends with a green test slice and a commit.

**Goal:** the gui host draws one canvas per `window`-rooted GuiDef, those
canvases are elements of an HTML document, and a bundle authored from the
Python client mounts into one — several per page, without colliding.

**Tech stack:** Rust (`clausters-core`, `clausters-core-web`, `clausters-ffi`,
`clients/gui`), TypeScript 7 emitted by `tsc`, Python 3, `node --test`,
`cargo test`, headless Chrome for the page acceptance.

## Global constraints

- The resolver is **pure and language-agnostic**: it lives in `clausters-core`,
  takes an allocation the caller made, and returns flat data. No allocation
  inside it, no host state, no new `/gui_*` command.
- Placeholders appear **only** in the GuiDef record and `boot.json`. A `@` or
  `$` reaching a def payload is a design error, not a shortcut.
- One format on three legs: browser, `clausters-gui --standalone`, loopback.
  A bundle with no `bundle.json` keeps working natively (directory listing).
- The run-time entry never imports the authoring builders. Task 5 asserts it.
- `cargo fmt` + `cargo clippy --all-targets` clean before any Rust commit; the
  def-family matrix and the doc build per `.claude/skills/feature-matrix`.
- TS: `npm run check` clean before any commit.
- Vocabulary: **window** is the desktop's; the browser has **components** and
  **canvases**; `window` stays the GuiDef root node type on the wire.
- Prose uses the API's verbs: a node is **freed**, a def **sent**, a component
  **mounted** and **resolved**.

---

### Task 1 — `clausters_core::bundle`: the format and the resolver

**Files:** create `src/bundle/mod.rs` (or `crates/clausters-core/src/bundle.rs`
per the crate layout), with its unit tests.

**Produces:** the serde types (`Manifest`, `ParamSpec`, `SymbolTable`,
`Allocation`, `Requirements`, `Resolved`), plus `requirements(&Manifest)`,
`resolve(template, allocation, params)` and `validate(manifest, template)`.

- [ ] Define the manifest and template types; `serde` round-trip tests.
- [ ] `requirements`: widget count, node/bus/buffer symbols.
- [ ] `resolve`: offset widget ids through the nested tree; substitute `@` and
      `$` in props and in boot messages; merge attribute → preset → default;
      type/range-check every parameter.
- [ ] Error cases as tests: unknown symbol, missing parameter with no default,
      type mismatch, value out of range, a placeholder in a def payload.
- [ ] `validate` for the writers.
- [ ] `cargo test -p clausters-core`, fmt, clippy. Commit.

### Task 2 — The doors: wasm and C

**Files:** modify `crates/clausters-core-web/src/lib.rs`,
`crates/clausters-ffi/src/lib.rs`.

**Consumes:** Task 1.

**Produces:** `bundle_requirements`, `bundle_resolve`, `bundle_validate` on the
wasm side (JSON in, JSON out — flat data only), and the C counterparts for the
Python writer's validation.

- [ ] Add the exports, mirroring the shapes the two crates already use.
- [ ] Keep both crates building natively; `#[cfg(target_arch = "wasm32")]` on
      the wasm exports.
- [ ] `./build.sh` stages `dist/core/` (the `.d.ts` is the proof).
- [ ] fmt, clippy, commit.

### Task 3 — The host: one canvas per def

**Files:** modify `clients/gui/src/host/web.rs` and the wasm binding surface.

**Consumes:** nothing above. **Produces:** `canvases: HashMap<i32, CanvasSlot>`
replacing `window`/`render`/`current_def`; `attach(def_id, canvas)`,
`detach(def_id)`, `resize(def_id, w, h)`, `set_visible(def_id, bool)`.

- [ ] Turn the singular fields into a map: surface, size, gesture state and
      visibility per canvas.
- [ ] Attach to a canvas the caller supplies (winit `with_canvas`) instead of
      hunting for the one winit appended to `<body>`.
- [ ] Route pointer/modifier events and repaints by def id.
- [ ] Skip the tick for a canvas marked hidden, and drop its buses from the
      `/c_stream`/`/tap_stream` subscription sets.
- [ ] Rust tests: two defs render and gesture independently; a hidden canvas is
      skipped and unsubscribed.
- [ ] fmt, clippy (including `cd clients/gui`), commit.

### Task 4 — The native leg reads the same manifest

**Files:** modify the `--standalone` path in `clients/gui`.

**Consumes:** Tasks 1, 3.

**Produces:** the native host reads `bundle.json` when present, allocates, and
goes through `resolve` — so a symbolic bundle runs on the desktop, and two of
them can be loaded without colliding. Absent manifest keeps today's listing.

- [ ] Read + allocate + resolve; fall back to listing when there is no manifest.
- [ ] Test: the example bundle mounts twice with distinct buses and node ids.
- [ ] fmt, clippy, commit.

### Task 5 — The slim run-time entry

**Files:** create `clients/web/src/runtime.ts`, `clients/web/tests/runtime-graph.test.ts`;
modify `build.sh`, `tsconfig.build.json` as needed.

**Consumes:** nothing above.

**Produces:** `dist/runtime.js` exporting `defineComponent` and the element
classes, reaching the engine, the host, the OSC codec and the mount — and
nothing else.

- [ ] Write the entry; keep `dist/index.js` (the full facade) unchanged.
- [ ] Write the module-graph test: walk the emitted imports of `dist/runtime.js`
      and assert it never reaches `defs/`, `gui/guidef.ts` or `seq/`. Run it —
      it fails if the entry is wired wrong.
- [ ] `npm run check`, commit.

### Task 6 — The component element

**Files:** modify `clients/web/src/elements.ts`; modify `clients/web/src/gui/host.ts`.

**Consumes:** Tasks 3, 5.

**Produces:** `<clausters-bundle>` owning its own canvas, with declared
parameters as attributes, `preset`, the two-phase mount, and the observers.

- [ ] The element creates its `<canvas>` and hands it to the host; drop the
      page-wide canvas adoption from `guiHost()`.
- [ ] `ResizeObserver` + `devicePixelRatio` → `resize`; `IntersectionObserver`
      → `set_visible`.
- [ ] Phase 1 on `connectedCallback`: allocate, resolve, `/gui_def`, draw.
- [ ] Phase 2 on the first page gesture: resume the context, send defs (one
      `/d_recv` per def name per page), buffers and boot.
- [ ] Per-component failure: the error shows on that component,
      `clausters-error` fires, the rest of the page comes up. `clausters-ready`
      carries the resolved def id.
- [ ] `npm run check`, commit.

### Task 7 — The mount path over `resolve`

**Files:** modify `clients/web/src/bundle.ts`.

**Consumes:** Tasks 2, 6.

**Produces:** the bundle mount going through `bundle_requirements` /
`bundle_resolve`, allocating from the page's `Server`/`GuiHost` allocators,
replacing today's `bundle_boot_packets` path; the page-level sent-def registry.

- [ ] Fetch the manifest, the template, the preset and the buffers.
- [ ] Allocate widget base, nodes, buses, buffers; resolve; send.
- [ ] Deduplicate def sends across components on the page.
- [ ] `node --test` over a served fixture bundle; commit.

### Task 8 — The Python writer

**Files:** create `clients/python/clausters/bundle.py`,
`clients/python/tests/test_bundle.py`;
create `clients/web/tests/bundle-vectors.json`, `clients/web/tests/bundle-parity.test.ts`.

**Consumes:** Tasks 1, 2.

**Produces:** `Bundle` — `param`, `bus`, `node`, `buffer`, `synthdef`,
`graphdef`, `gui`, `preset`, `write(dir, runtime=...)` — validating through the
core before emitting, and generating `index.js`.

- [ ] The symbol table and the placeholder strings (`b.bus("lfo")` → `"@lfo"`).
- [ ] Def-name prefixing with the bundle name.
- [ ] `write()` emits the directory, the manifest, the presets and `index.js`;
      it validates first, so an unmountable bundle is unwritable.
- [ ] Freeze a bundle vector and assert it from `node --test`, the way
      `def-parity`/`gui-parity` already do.
- [ ] `pytest`, `npm run check`, commit.

### Task 9 — Examples, acceptance, docs, close

**Files:** modify `clients/web/examples/piano/`, `clients/web/examples/graph-controls/`;
create `clients/web/examples/document/`; create `clients/web/tests/components.html`;
modify `docs/clients.md`, `docs/architecture.md`, `docs/decisions.md`,
`clients/web/README.md`, `clients/web/BUILD.md`, `clients/web/PLAN.md`,
`clients/PLAN.md`.

**Consumes:** everything.

- [ ] Port both examples to the writer — `piano_voice`'s baked `out_ctl(0.0,…)`
      becomes a control, which is what makes it mountable twice.
- [ ] `examples/document/`: an interactive text with components interleaved
      with prose — the milestone's shape, and the form the mdBooks can embed.
- [ ] `tests/components.html`: the acceptance. Two instances of one bundle plus
      one of another; three canvases draw; the two instances hold different
      buses and node ids and their def was sent once; a `freq` attribute makes
      one audibly different (asserted on a control bus); a component scrolled
      out of view stops streaming; one component's failure does not take the
      page down. Beacon the verdict, the standard web-smoke posture.
- [ ] `./test.sh` whole, plus the feature matrix and the doc build.
- [ ] Docs: the three `docs/decisions.md` entries, the format in
      `docs/clients.md`, the host structure in `docs/architecture.md`, the
      README/BUILD sections, an example exercising the new visual behavior.
- [ ] Tick W4 in `clients/web/PLAN.md`, write its "What shipped", update the W
      status paragraph in `clients/PLAN.md`. Commit.
