# Clausters

A real-time audio synthesis server in the style of SuperCollider's scsynth,
written in Rust and controlled over OSC (UDP, default port 57110).

- Forward roadmaps and milestones: `PLAN.md` (server), `clients/PLAN.md` and
  `clients/gui/PLAN.md` (client/GUI tracks), all in English — a roadmap plus a
  checkbox status per milestone, not an expanded completion narrative.
- The record of *what shipped* is the git history (clear commit messages); there
  is no separate per-milestone log. Non-obvious decisions and upstream-bug
  findings are curated in `docs/decisions.md` (ADR spirit); the frozen historical
  journal is `docs/history/build-log.md`, no longer maintained.
- English documentation is **two mdBooks, one per platform**, both Markdown and
  ReadTheDocs-deployable (each has a `.readthedocs.yaml` driving the build with
  `build.commands` — RTD has no native mdBook builder). Keep both current.
  - **Server / workspace** — mdBook in `docs/` (`docs/SUMMARY.md` the table of
    contents, the repo-root `book.toml` the config, `README.md` the front door;
    build with `mdbook build .`, output `book/` git-ignored). Reuses the
    existing `docs/*.md` in place. Key chapters: developer docs (threads, memory
    lifecycle, invariants, how to add a UGen) in `docs/architecture.md`;
    user-facing wire formats and OSC reference in `docs/schemas.md`; library/
    embedding use in `docs/using-as-a-library.md` and `docs/ipc.md`; the
    cross-language map in `docs/clients.md`. Crate API reference is the rustdoc
    (`cargo doc`).
  - **Python client** — mdBook in `clients/python/docs/` (its own `book.toml`
    and `src/`; `clients/python/docs/build.sh` builds it). The API-reference
    page `src/api.md` is **generated from the package docstrings by
    pydoc-markdown** (`clients/python/pydoc-markdown.yml`; both it and `book/`
    git-ignored). Install the generator in **user space** (no sudo) with
    `uv tool install --python 3.12 pydoc-markdown` — pin 3.12 because its deps
    lag the newest CPython, and that is also Read the Docs' version; then
    `clients/python/docs/build.sh` regenerates `api.md` and rebuilds the book
    (`uvx pydoc-markdown`, or `pip install` on a non-PEP-668 env, also work —
    see `clients/python/README.md`). The two books cross-link by their RTD URLs.
  - **Docstrings and published docs are plain Markdown**: **no Sphinx/RST
    directives** in docstrings (no `:role:` cross-refs, no `:param:` field lists
    — use backticks / Google-style sections), and **no milestone labels
    (`Mx`/`Cx`/`Fx`) in any published doc or docstring** — those live only in
    the `PLAN.md` roadmaps. `GUIA.md` is a personal Spanish smoke checklist, not
    part of the docs.
- **Closing a milestone always includes, whenever applicable**: code plus tests,
  a clear commit message, the `PLAN.md` roadmap checkbox updated, the developer
  documentation (`docs/architecture.md`, module docs) and the user documentation
  in `docs/` where the feature touches them, and a commented/explained example in
  `examples/` when the feature is user-facing. Add a `docs/decisions.md` entry
  **only** for a choice with non-obvious context, and a `GUIA.md` smoke step
  **only** for new human-audible/visual behavior — neither is a per-milestone
  obligation.
- Project skills live in `.claude/skills/` (realtime-audio, scsynth-osc,
  ugen-dsp, audio-testing, faust-embedding, faust-language, clausters-python,
  clausters-gui, documentation).

## Cross-client build strategy

Multiple clients exist or are planned (Python, a future JS/TS protocol client,
the GUI host native + browser/wasm). The rule: **finish and polish one reference
client at a time, then port** — never grow two full clients in parallel. What
makes a port cheap is keeping all language-agnostic logic in the shared core
(`clausters-core`/`clausters-ffi`, plus the agnostic `host` traits in
`clients/gui`) and pushing logic down there **as you write it**, so each client
stays a thin language-specific shell (idiomatic API + concurrency/scheduling).
Porting then = rebind the same core (ctypes/N-API/wasm), not reimplement it.
Full rationale in `clients/PLAN.md` ("Build strategy"). Always factor new work
with this modularity in mind.

## Language conventions

- Everything under `src/`, `tests/` and `examples/` (code, comments, strings,
  test names) is in English.
- **Git commit messages are in English** (subject and body), ASCII-only.
- The `PLAN.md` roadmaps (`PLAN.md`, `clients/PLAN.md`, `clients/gui/PLAN.md`),
  `docs/decisions.md` and the frozen `docs/history/build-log.md` are in English.
  `GUIA.md` (root + `clients/python/GUIA.md`) and the conversation with the user
  are in Spanish.
- **Type and class names** are CamelCase, with an acronym inside a name
  taking only its first letter in uppercase — `OscFunc`, `MidiFunc`,
  `OscUdpInterface`, `OscTcpInterface`, `NodeIdAllocator` — never all-caps
  (`OSCFunc`, `OscUDPInterface`). This holds both for the Python client (and
  any later class-based client) and for Rust, which already follows it by
  idiomatic style (clippy's `upper_case_acronyms`). The only all-caps
  acronyms left are verbatim external-API symbols, kept as cited: Faust's C
  API FFI in `src/faust` (`UIGlue`, `CsigFConst`, `CboxHSlider`, ...) and, in
  docstrings, sclang's `OSCFunc`/`MIDIFunc`.
- **Naming the components** (in the documentation and in our conversations),
  keep these three distinct — each form, English or Spanish:
  - `clausters-server` / "servidor clausters" / "clausters server" — the
    **Rust server**.
  - `clausters-python` / "clausters python" — the **Python client**.
  - `clausters-cliente` / "clausters cliente" / "clausters client" — **an
    unspecified client** of the clausters server (a protocol consumer in
    general, not the Python one specifically).
- **The arrangement model / multitrack editor** — the client-side layer that
  places elements in time, groups them recursively and renders them
  (`clausters.form`), and its DAW-style view (`track`/`clip` widgets +
  `clausters.gui.Editor`). **The module name is dissociated from the prose**:
  `form` is a code name only — the documentation never says "the form". In prose
  the layer is *"the arrangement"* (or *"the arrangement model"* when naming the
  layer as such) / "el arreglo", "el modelo de arreglo", and its view is *"the
  multitrack editor"* / "el editor multipista"; the work itself is *"the
  composition"* / *"the piece"*, and the data is *"the tree"* / *"the elements"*.
  Never the bare "the model" (it reads as the node tree or a def).
  The layer's **internal vocabulary** for its contents is the **element**
  (`Element`), deliberately general — it spans both a **generated element** (the
  rendered thing: an audio file, a bounced timeline — random-access, so it can be
  read backwards, sliced, edited in place) and a **generator element** (the
  algorithm that renders it: a def, a pattern — forward-only, it can just be
  evaluated), with the *change of state* between them. The verb for that change of
  state to sound is **render** (`Element.render`, `Editor.render`), never
  "realize"; the editor's *graphic* direction is **draw** (`Editor.draw`, the
  GuiDef). A `Group` has two **kinds**: **concrete** (its members relate in time)
  and **logical** (they relate by processing). Its documentation:
  - **User** — the composition chapter of the Python client's book
    (`clients/python/docs/src/composition.md`): elements, grouping, rendering,
    and how the editor maps and edits them.
  - **Wire** — `docs/gui-protocol.md` (the `/gui_*` reference: widgets, props,
    edit-back payloads).
  - **Development** — `docs/architecture.md` ("The arrangement layer: where it
    lives", and the GUI host's structure + how to add a widget).
  - **Rationale** — `docs/decisions.md` (the framework itself; the beats↔samples
    unit bridge; a buffer sounds through an instrument; the patcher shows a
    connection, not a direction).
  The framework's own record is `docs/decisions.md` — there is no source document
  behind it to consult.

## Commit workflow

**Work directly on `main`.** This is a single-maintainer repo; commit to
`main` unless the user explicitly asks for a branch or a PR. Do **not** create
a feature branch on your own — this overrides the default "if on the default
branch, branch first" behavior.

Before generating any commit that touches Rust, run `cargo fmt` (or at least
`cargo fmt --check`) and include the formatting fixes — the tree must be
`cargo fmt --check`-clean. Do not hand-format Rust against rustfmt; rustfmt is
the source of truth. (Likewise keep the build green: the core must compile and
test with any combination of the def-family features `synth`/`faust` — both,
either alone, or neither — and without the `embed` feature.)

## Testing via the Python launcher: refresh the bundled binaries first

`Session.gui()`, the `clausters` console script and the FFI loaders resolve
their native artifacts with precedence **env override → bundled copy inside
the package (`clients/python/clausters/_bin/`, `_libs/`) → workspace
`target/`**. In this source checkout the package is installed editable, so
the *bundled* copy wins and goes stale the moment a crate is rebuilt — a
manual test can silently exercise pre-change binaries. Before any manual or
visual test launched through Python, refresh the bundled copy (e.g.
`cargo build --release` in `clients/gui/`, then copy
`target/release/clausters-gui` over `clients/python/clausters/_bin/`), or
point the override env vars (`CLAUSTERS_GUI_BIN`, `CLAUSTERS_BIN`,
`CLAUSTERS_LIB`, `CLAUSTERS_FFI_LIB`) at the workspace build.

## E2E testing rule

The Bash sandbox isolates the network between invocations: a server started in
one invocation is unreachable from the next, and UDP packets to localhost are
silently lost. Always run server and client in the **same** Bash invocation
(server in background with `&`, then the client, then kill), e.g.:

```sh
(./target/debug/clausters & PID=$!; sleep 1.5; \
 ./target/debug/examples/osc_ping status vibrato quit; kill $PID 2>/dev/null)
```

## OSC decoding

All incoming OSC bytes — UDP datagrams and IPC ring contents alike — decode
through `osc::decode_packet`, the single entry point (a thin wrapper over
rosc's `decoder::decode_udp`). Keep that one door so decoding and any future
hardening stay in one place.

## The def-family features: `synth` and `faust`, both default

The two def families are independent Cargo features, and **both are on by
default**: `synth` (SynthDef/UGen graphs, `/d_recv`) and `faust` (FaustDefs,
`/d_faust`). They are **peers** — never treat one as the fallback of the other,
in code or in docs. They combine freely; a custom build can ship either alone
(see `BUILD.md` for the matrix). The node tree only sees `dyn SynthNode`, which
keeps the families symmetrical — feature-gate new work accordingly.

## The `faust` feature and libfaust

Because `faust` is default, a plain `cargo build` / `cargo test` needs libfaust
built **with the LLVM backend** — Ubuntu's `libfaust2t64` ships without it and
without headers, so it is built from source and installed under `~/.local` (see
`BUILD.md`, "Building libfaust from source", for the reproducible recipe).
`build.rs` locates it through `FAUST_PREFIX`, falling back to `~/.local`, then
`/usr/local`. The core must still **build and test without the feature and
without libfaust installed** (`--no-default-features`); keep that path green.

`build.rs` writes a `DT_RPATH` of `$ORIGIN`, `$ORIGIN/../_libs` and the build
prefix, which keeps the artifacts relocatable: the Python wheel bundles
`libfaust.so` and the `libLLVM.so` it JITs with in `clausters/_libs/` (staged by
`clients/python/build_native.py`), so an installed package compiles FaustDefs
with nothing else on the machine. It must stay `DT_RPATH` (not `RUNPATH`): only
that one is inherited by transitive deps, and it is libfaust that needs libLLVM.

The faust suites run in the **ordinary parallel harness** — every compilation
FFI call goes through `faust::ffi_lock()`, which serializes libfaust in-process.
(They historically needed `--test-threads=1`; that is obsolete. A SIGSEGV in a
`faust_*` suite means an FFI path skipped the lock — look there first.)

## RT-safety (non-negotiable)

The audio thread (`Engine::process_block` and everything it calls) must never
allocate, free, lock or do I/O. Commands arrive fully pre-built over a
lock-free FIFO; freed memory leaves through the garbage FIFO and is dropped on
the network thread. `tests/rt_safety.rs` guards this with `assert_no_alloc`.

Denormals: every processing thread runs in flush-to-zero mode —
`dsp::denormals::flush_to_zero()` is re-armed in the cpal callback and armed
in `render()` (both, so NRT stays sample-identical to RT) — and Faust
factories are compiled with `-ftz 2`. Keep all three call sites if you touch
them; `tests/denormals.rs` and the Faust tail test in `tests/golden.rs` guard
this. The one exception: the Faust *compiler* path runs inside
`dsp::denormals::normal_precision` (libfaust's interval typing aborts under
FTZ/DAZ; the guard restores the armed mode on exit) — keep that bracket too.
