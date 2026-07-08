# Clausters

A real-time audio synthesis server in the style of SuperCollider's scsynth,
written in Rust and controlled over OSC (UDP, default port 57110).

- Implementation plan and milestones: `PLAN.md` (server) and `clients/PLAN.md`
  (client track), both in English.
- Completion log per milestone: `LOG.md` (English) — update it when a
  milestone is finished.
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
    `PLAN.md`/`LOG.md`. `GUIA.md` is a personal file, not part of the docs.
- **Closing a milestone always includes, whenever applicable**: the
  developer documentation (`docs/architecture.md`, module docs), the user
  documentation in `docs/` for new features, manual testing steps and
  counts in `GUIA.md`, and a commented/explained example in `examples/`
  when the feature is user-facing — not just code and LOG.md.
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
- `PLAN.md`, `clients/PLAN.md` and `LOG.md` (the dev-history files) are in
  English. `GUIA.md` (root + `clients/python/GUIA.md`) and the conversation
  with the user are in Spanish.
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

## Commit workflow

Before generating any commit that touches Rust, run `cargo fmt` (or at least
`cargo fmt --check`) and include the formatting fixes — the tree must be
`cargo fmt --check`-clean. Do not hand-format Rust against rustfmt; rustfmt is
the source of truth. (Likewise keep the build green: the core must compile and
test with any combination of the def-family features `synth`/`faust` — both,
either alone, or neither — and without the `embed` feature.)

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

## The def-family features: `synth` (default) and `faust` (optional)

The two def families are independent Cargo features: `synth` (SynthDef/UGen
graphs, `/d_recv`, on by default) and `faust` (FaustDefs, `/d_faust`). They
combine freely; a custom build can ship either alone (see `BUILD.md` for the
matrix). The node tree only sees `dyn SynthNode`, which keeps the families
symmetrical — feature-gate new work accordingly.

## Optional `faust` feature

`cargo test --features faust` needs libfaust built **with the LLVM backend**
— Ubuntu's `libfaust2t64` ships without it and without headers, so it is
built from source and installed under `~/.local` (see the F0 section of
`LOG.md` for the reproducible recipe). `build.rs` locates it through
`FAUST_PREFIX`, falling back to `~/.local`, then `/usr/local`. The core must
always build and test without the feature and without libfaust installed.

**Run the faust tests single-threaded:** `cargo test --features faust --
--test-threads=1`. libfaust/LLVM is not safe for **concurrent** compilation in
one process, so the default parallel test harness SIGSEGVs in the faust suites
(`faust_compiler` and friends) — a known libfaust limitation, not a bug in our
code. This only affects the test harness creating factories in parallel: the
server itself compiles on a single thread holding `faust::ffi_lock()`, so
production is unaffected.

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
