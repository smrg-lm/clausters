# Clausters

A real-time audio synthesis server in the style of SuperCollider's scsynth,
written in Rust and controlled over OSC (UDP, default port 57110).

- Forward roadmaps and milestones: `PLAN.md` (server), `clients/python/PLAN.md`,
  `clients/gui/PLAN.md`, `clients/web/PLAN.md` (client/GUI/web tracks) and
  `crates/clausters-document/PLAN.md` (the document: the arrangement's model, the
  edit vocabulary and the edit log, shared by every client and by the
  `standalone` host), all in English — a roadmap plus a checkbox status per
  milestone, not an expanded completion narrative.
  - **`ROADMAP.md` (root) orders them and defines nothing.** It says which of
    the milestones already written in the `PLAN.md` set is taken next and why
    that one first, naming each by its own label. A milestone's content, its
    decisions and its acceptance are only ever in its plan; when the two
    disagree, the plan wins. Never move content into `ROADMAP.md` — a milestone
    that grows a decision grows it in its plan. It also orders work that is not
    a milestone: an entry of a plan's "Found by use" or "Future directions"
    list, named by its own title and by the plan that holds it.
  - **`ROADMAP.md` is a temporary file and is a record of nothing.** It holds
    one working sequence over what is still pending, gathered from the several
    plans it is distributed across — so **rewriting it erases what has been
    done** and reorganizes only what is left. A closed phase leaves no line
    there: the plan's checkbox and the git history already say it shipped. When
    the sequence runs out, the file goes away.
  - **Anything unresolved goes at the end of its plan, and carries a checkbox.**
    A gap found by use — an eye pass over an example, a path read while doing
    something else, a behavior that is correct and unclear — is written down the
    day it is found, in the plan's **"Found by use"** section (fixes) or
    **"Future directions"** (a design that is not a fix), both of which live
    *after* the tracks and after "Definition of done". Never inside the
    milestone that happened to be open, and never in a section of finished work:
    a pending item filed among done ones reads as done, which is how it gets
    lost. Every entry there is `⬜`/`✅`, so what is open reads as open without
    inferring it from where it sits, and a fixed one **stays** with the record
    of what was wrong — that is what makes the list worth reading rather than a
    queue that empties.
- The record of *what shipped* is the git history (clear commit messages); there
  is no separate per-milestone log. Non-obvious decisions and upstream-bug
  findings are curated in `docs/decisions.md` (ADR spirit); the frozen historical
  journal is `docs/history/build-log.md`, no longer maintained.
- English documentation is **three mdBooks, one per platform** — server/
  workspace, Python client, web client — all Markdown and ReadTheDocs-deployed.
  Keep all three current. Where each book lives, how it builds, and how its API
  reference is generated (rustdoc, pydoc-markdown, TypeDoc) are in the
  `documentation` skill; doc comments in `clients/web/src` are TSDoc (`/** */`),
  never Rust-style `///`, which TypeScript tooling does not read. All three
  build locally in one command: `scripts/check-docs.sh` (optionally
  `server`/`python`/`web` to pick).
  - **Docstrings and published docs are plain Markdown**: **no Sphinx/RST
    directives** in docstrings (no `:role:` cross-refs, no `:param:` field lists
    — use backticks / Google-style sections), and **no milestone labels
    (`Mx`/`Cx`/`Fx`/`Ux`/`Sx`/`Gx`) in any published doc, docstring, example or
    comment** — a label is a roadmap coordinate and means nothing to a reader;
    they live only in the `PLAN.md` roadmaps.
- **Closing a milestone always includes, whenever applicable**: code plus tests,
  a clear commit message, the `PLAN.md` roadmap checkbox updated, the developer
  documentation (`docs/architecture.md`, module docs) and the user documentation
  in `docs/` where the feature touches them, and a commented/explained example in
  `examples/` when the feature is user-facing. Add a `docs/decisions.md` entry
  **only** for a choice with non-obvious context — not a per-milestone
  obligation.
- **The examples are the manual test surface.** There is no separate smoke
  checklist: new human-audible/visual behavior is checked by ear and by eye
  through an `examples/` entry (root `examples/` for the server,
  `clients/python/examples/` for the client and the GUI), so an example that
  exercises the new behavior *is* the manual test, and keeping it runnable is
  part of closing the work. **How one is written — a closed script, a `# %%`
  notebook or a page, by which package it belongs to — is the `examples`
  skill**; consult it before adding or editing one.
- **An example documents itself, and the documentation never enumerates the
  examples.** The examples travel with the *repository*, not with the wheel or
  the npm package, so a catalog of them inside a book serves a reader who does
  not have them and rots for the one who does. Each example's module docstring
  (or a page's header comment) says what it shows, what it needs and how to run
  it; the books' `examples.md` pages say only where the directories are, how to
  run each family, and at most name one or two entry points. A topic page may
  still point at *one* example that shows what it is explaining — that is a
  cross-reference, not a catalog.

## Cross-client build strategy

Multiple clients exist or are planned (Python, the web client in TypeScript,
the GUI host native + browser/wasm). The rule: **finish and polish one reference
client at a time, then port** — never grow two full clients in parallel. What
makes a port cheap is keeping all language-agnostic logic in the shared core
(`clausters-core`/`clausters-ffi`, plus the agnostic `host` traits in
`clients/gui`) and pushing logic down there **as you write it**, so each client
stays a thin language-specific shell (idiomatic API + concurrency/scheduling).
Porting then = rebind the same core (ctypes/N-API/wasm), not reimplement it.
Full rationale in `clients/python/PLAN.md` ("Build strategy"). Always factor
new work with this modularity in mind.

### The packages move together

Four packages share every surface worth changing — the **server** (root crate +
`crates/`), the **Python client**, the **web client**, the **GUI host** — and
each wire has an end in two or more of them. So a change is not finished when
its own package builds: **it closes with a pass over the packages it touches**,
in the same commit. Porting is not a task to schedule later.

The wires and where their ends live:

- An **OSC command** (`src/osc`) — its reference in `docs/schemas.md`, its
  builder in `clients/python/clausters`, its builder in `clients/web/src`.
- A **`/gui_*` widget or prop** — the host in `clients/gui/src`, the reference
  in `docs/gui-protocol.md`, the builders in both clients' `gui` modules.
- A **client API** — the Python client is the reference and the web client
  ports it. If the surface does not exist in TS yet, say so in the commit and
  leave `clients/web/PLAN.md` naming the shape the port must follow, so the two
  do not re-derive it differently.
- A **shared-core function** (`clausters-core`) — every client binds the same
  one; new numeric or timing logic belongs there rather than in a client. Its
  bindings are declared in `docs/bindings.md` and enforced by
  `tests/bindings.rs` (C ABI ↔ wasm) and
  `clients/python/tests/test_native_parity.py` (C ABI ↔ ctypes): a new symbol
  that reaches only one of them fails a test until the table says why.

**What is actually checked, and what is not.** The compiled Rust is safe:
cargo refuses to build a caller of a signature that moved. Everything else is
on trust, so that is where drift accumulates:

- **Nothing runs any example.** Not the root `examples/` (mostly Python
  driving the server), not `clients/python/examples/` (including the GUI ones),
  not `clients/web/examples/`. CI runs none of them, and a Python signature
  change breaks them at a call site no build ever reaches. They are the manual
  test surface, so run the ones the change touches, by hand. **After changing
  any Python signature, also run `npx pyright` in `clients/python`** (nothing
  vendors it; npm's cache is the install) — its
  `pyrightconfig.json` turns every rule off but the four that catch exactly
  this (it is a call-site check, not a type check), over the package, the tests
  and both example directories. The baseline is zero, so anything it prints is
  yours; `docs/contributing.md` explains the rule choice.
- **The web package is typechecked against nothing else.** Its Python (the
  parity generators `clients/web/tests/gen-*-vectors.py`, the bundle authors
  `clients/web/examples/*/make_bundle.py`) imports the Python client; its pages
  (`clients/web/tests/*.html`, `examples/**`) are plain modules no
  type-checker reads. `./build.sh && ./test.sh` from `clients/web` is the only
  thing that proves the package works — and `dist/` is git-ignored, so build
  before testing, always. Re-run the generators and commit whatever vectors
  move.
- **CI does not lint everything.** It skips the def-family feature matrix and
  never builds the docs, so between a push and a tag rustdoc's lints are watched
  by nothing: `.claude/skills/feature-matrix/check.sh`. A release runs the whole
  matrix before it publishes, which catches these — but only once they are
  already on `main`.
- **The staged artifacts go stale silently** — the binaries bundled in the
  Python package (`scripts/refresh-bin.sh`) and the web package's `dist/`.
- **The books drift last and loudest.** A concept renamed in one client is
  renamed in all three books, and a command's reference page changes with the
  command. And the doc build is the one check the compiler cannot help with at
  all: a dangling TSDoc `{@link}`, a page missing from a `SUMMARY.md`, a
  docstring pydoc-markdown chokes on — all of it compiles, tests and lints
  clean, and only CI's `docs` job (minutes after the push, in a job nobody
  watches) says otherwise. **Run `scripts/check-docs.sh` before committing
  anything a book reads** — its own pages, a Python docstring, a TSDoc comment:
  it runs the same three builds CI does, with the same pinned tools, in about
  five seconds. The pre-commit hook runs it for the books the tree touches, but
  it is a convenience, not a gate.

## Language conventions

- Everything under `src/`, `tests/` and `examples/` (code, comments, strings,
  test names) is in English.
- **Git commit messages are in English** (subject and body), ASCII-only.
- The `PLAN.md` roadmaps (`PLAN.md`, `clients/python/PLAN.md`,
  `clients/gui/PLAN.md`, `clients/web/PLAN.md`), `docs/decisions.md` and the frozen
  `docs/history/build-log.md` are in English. The conversation with the user is
  in Spanish.
- **Prose uses the API's own verbs for API actions.** When the protocol or
  API has a verb for an action, the documentation (books, docstrings,
  comments) names the action with *that* verb: a node or widget is **freed**
  (`/node_free`, `/gui_free`, `node.free()`) — never "destroyed", "deleted" or
  "killed"; a def is **sent** and **loaded**; a server is **booted**; an
  element is **rendered** (never "realized" — see the arrangement vocabulary
  below). Everyday synonyms make the prose drift from the surface the reader
  actually types against.
- **Name the structure, not the category.** "Material" is a category — it can
  be samples, a def, a pattern, a routine — so it never names a thing that has
  a name: a buffer is a **buffer**, its contents are **samples** (data), a
  def is a def, a pattern a pattern. Where the general term is genuinely
  needed, the arrangement already has one (**element**, and a clip's
  **contents**), and the concrete word is still preferred when the code knows
  which it is. The same rule catches "the material" standing in for "the
  piece", "the audio" or "whatever is played" — say which.
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
  GuiDef). An `Aggregate` has two **kinds**: **concrete** (its members relate in
  time) and **logical** (they relate by processing). Its documentation:
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

**No trailers in commit messages.** Not `Co-Authored-By:`, not a "generated
with" line, not attribution of any kind, whoever or whatever wrote the commit.
The subject and body are the entire message, and the last line is prose. The
history records *what shipped and why*; how it was typed is not part of that
record. The same holds for a PR description.

Before generating any commit that touches Rust, run `cargo fmt` (or at least
`cargo fmt --check`) and include the formatting fixes — the tree must be
`cargo fmt --check`-clean. Do not hand-format Rust against rustfmt; rustfmt is
the source of truth. (Likewise keep the build green: the core must compile and
test with any combination of the def-family features `synth`/`faust` — both,
either alone, or neither — and without the `embed` feature.)

**Clippy must come back clean, always — zero warnings, not "no new ones".**
Run `cargo clippy --all-targets` before committing Rust and fix whatever it
reports, **including warnings the commit did not introduce**. A warning that is
genuinely wrong gets a scoped `#[allow(...)]` **with a comment saying why** —
never a silent pass.

Why this is a standing rule rather than a one-off cleanup: CI pins nothing
(`dtolnay/rust-toolchain@stable`, no `rust-toolchain.toml`), so **every rustc
release can turn a green tree red with no code change** — a new lint, or an
existing one widened, lands on code nobody touched. So a clean tree is not a
state you reach once; expect to find warnings you did not cause, and treat that
as normal rather than as a sign something is wrong. Left alone they compound:
one stale warning becomes background noise, the next hides behind it, and the
output stops being read.

Clearing warnings that predate the work at hand goes in its **own commit**,
separate from the feature, so the feature's diff stays readable.

Note CI lints `--workspace --all-targets` and the GUI crate, but **not** the
def-family feature matrix — a warning that only appears under
`--no-default-features` (or under one family alone) will not be caught there —
and it never builds the **docs** at all, so rustdoc's own lints (a `[`link`]`
to an item that was renamed, moved or made private) are caught nowhere but
here. Run the matrix locally when the change touches feature-gated code or doc
comments: `.claude/skills/feature-matrix/check.sh` runs all of it in one go, and
the `feature-matrix` skill lists every command it covers.

The doc build walks the def families for the same reason clippy does, which
settles how a doc comment names something across a feature seam: **in
backticks, not as a link** (`dsp::denormals` naming `server::backend`,
`server::defstore` naming `faust::cache::FaustRecord`) — a link there resolves
only in the build where the target is compiled in.

## Versioning

Three version numbers answer different questions and must stay distinct: the
package SemVer (`Cargo.toml`, the wheel), and the two binary ABI counters
`ABI_VERSION` (embed/IPC) and `CORE_ABI_VERSION` (core FFI), which are the source
of truth for binary compatibility. The release rules — the pre-1.0 breaking tier,
when each counter moves, the one-way linkage between them — are in the
`release-versioning` skill; the rationale is in `docs/decisions.md`.

## Testing via the Python launcher: refresh the bundled binaries first

`Session.gui()`, the `clausters` console script and the FFI loaders resolve
their native artifacts with precedence **env override → bundled copy inside
the package (`clients/python/clausters/_bin/`, `_libs/`) → workspace
`target/`**. In this source checkout the package is installed editable, so
the *bundled* copy wins and goes stale the moment a crate is rebuilt — a
manual test can silently exercise pre-change binaries. Before any manual or
visual test launched through Python, refresh the bundled copy — one command:
`scripts/refresh-bin.sh` (wraps `clients/python/build_native.py`: rebuilds
server + FFI + GUI host in release and stages everything into `_bin`/`_libs`;
`scripts/refresh-bin.sh gui_shell` also runs that example with the `.venv`
Python, `--skip` skips the rebuild) — or point the override env vars
(`CLAUSTERS_GUI_BIN`, `CLAUSTERS_BIN`, `CLAUSTERS_LIB`, `CLAUSTERS_FFI_LIB`)
at the workspace build.

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
default**: `synth` (SynthDef/UGen graphs, `/def_send synth`) and `faust` (FaustDefs,
`/def_send faust`). They are **peers** — never treat one as the fallback of the other,
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
