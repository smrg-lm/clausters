# Contributing

Working on Clausters itself. The internals — threads, memory lifecycle, invariants, how to add a UGen — are in [Architecture](architecture.md); this chapter is the practical setup around it.

## Build and test

```sh
cargo build                 # core, no audio device needed to compile
cargo test                  # the full core suite
cargo clippy --all-targets  # lints (kept clean)
cargo doc --no-deps         # the API reference
```

The core **must always build and test without any feature and without libfaust installed**, and with any combination of the def-family features `synth`/`faust` (both, either alone, or neither — most integration suites are gated on `synth`, so the featureless run is thin by design).

## System build dependencies (Ubuntu 26.04)

```sh
# default build (PipeWire audio + ALSA-seq MIDI)
sudo apt install build-essential pkg-config libasound2-dev libpipewire-0.3-dev libdbus-1-dev clang
# only for the matching optional feature:
sudo apt install libjack-jackd2-dev          # --features midi-jack
# plain-ALSA build (no PipeWire libs):
#   cargo build --no-default-features --features synth,realtime,midi
```

`pipewire` is a default feature (the target systems always ship PipeWire), so
the default binary hard-links `libpipewire` and expects it at runtime. `clang`
is only used at build time by bindgen (it parses the PipeWire C headers to
generate bindings); the toolchain itself stays `rustc` + `gcc`. `libdbus-1-dev`
is there for `rtprio`, also a default feature: cpal promotes the audio callback
thread to `SCHED_FIFO` through RTKit over DBus. The `midi-jack`
build links against jackd2's `libjack` but resolves to PipeWire's `libjack`
under `pw-jack` at runtime.

## The `faust` feature (default)

The FaustDef family is **on by default**, so a plain `cargo build` / `cargo test` needs **libfaust built with the LLVM backend** on the machine. Distro packages (e.g. Ubuntu's `libfaust2t64`) ship without it and without headers, so it is built from source and installed under `~/.local`, once. The reproducible recipe is in `BUILD.md` (the "Building libfaust from source" section). `build.rs` locates the library through `FAUST_PREFIX`, falling back to `~/.local`, then `/usr/local`.

```sh
FAUST_PREFIX=~/.local cargo test           # the prefix is only needed if it is not ~/.local
cargo test --no-default-features --features synth,realtime   # no libfaust needed
```

Every compilation FFI call goes through `faust::compiler::ffi_lock()`, which serializes libfaust within the process (instantiating from an already-compiled factory is concurrency-safe). That lock is what lets the Faust suites run in the ordinary parallel test harness; a SIGSEGV in a `faust_*` suite is the signature of an FFI path that skipped it.

`build.rs` also writes a `DT_RPATH` of `$ORIGIN`, `$ORIGIN/../_libs` and the build prefix, so the artifacts are relocatable and a distribution can bundle libfaust and its libLLVM beside them (which is exactly what the Python wheel does — see `clients/python/build_native.py`). `DT_RPATH` rather than `DT_RUNPATH`: only the former is inherited by transitive dependencies, and it is libfaust, not our binary, that needs to find libLLVM.

## Real-time safety (non-negotiable)

`Engine::process_block` and everything it calls must **never allocate, free, lock or do I/O**. Commands arrive fully pre-built over a lock-free FIFO; freed memory leaves through the garbage FIFO and is dropped on the network thread. `tests/rt_safety.rs` guards this with `assert_no_alloc` — run it after touching anything on the audio path. Denormals are flushed to zero on every processing thread (`dsp::denormals::flush_to_zero()` plus `-ftz 2` for Faust); see the RT-safety notes in [Architecture](architecture.md).

## End-to-end testing in a sandboxed shell

Some CI/sandbox environments isolate the network **between** shell invocations: a server started in one invocation is unreachable from the next, and UDP packets to localhost are silently lost. Always run the server and client in the **same** invocation — server in the background with `&`, then the client, then kill it:

```sh
(./target/debug/clausters & PID=$!; sleep 1.5; \
 ./target/debug/examples/osc_ping status quit; kill $PID 2>/dev/null)
```

## OSC decoding

All incoming OSC bytes decode through `osc::decode_packet`, the single entry point (every transport funnels through it). It is a thin wrapper over `rosc::decoder::decode_udp` — keep that one door so decoding and any future hardening stay in one place.

## Fuzzing the network edge

Because every transport funnels through that one door, one fuzz target covers
the whole inbound parse surface. The harness lives in `fuzz/` (a cargo-fuzz
crate, deliberately **not** a workspace member — it needs nightly and only
builds through cargo-fuzz):

```sh
rustup toolchain install nightly --profile minimal
cargo install cargo-fuzz
cargo +nightly fuzz run decode_packet fuzz/corpus/decode_packet fuzz/seeds/decode_packet -- -max_total_time=300
```

Run it from the repo root. `fuzz/seeds/` holds a few versioned valid packets
(message, args, blob, bundle) to start coverage from; the growing corpus and
any crash artifacts land in `fuzz/corpus/` and `fuzz/artifacts/` (both
git-ignored). Arbitrary bytes must decode to `Ok` or `Err` — a panic, hang or
memory blow-up is a bug; minimize it with `cargo +nightly fuzz tmin` and fix
it (or report it upstream to `rosc`) before release. Run a few minutes of
fuzzing before publishing a release that touched the OSC path.

## Continuous integration

GitHub Actions runs the same checks this page asks for by hand
(`.github/workflows/ci.yml`); every job maps to a local command, so a red job
is reproducible with the same line:

- **lint** — `cargo fmt --check` and `cargo clippy --workspace --all-targets
  -- -D warnings`, for the root workspace and for `clients/gui` (its own
  workspace). The tree is clippy-clean; keep it that way.
- **test** — `cargo test --workspace` across the def-family feature matrix:
  default, `--no-default-features`, `--no-default-features --features synth`,
  and default plus `embed`.
- **gui** — `cargo test` in `clients/gui` plus the wasm build gate
  (`clients/gui/check-wasm.sh`).
- **python** — `python clients/python/build_native.py --debug`, then `pytest`
  in `clients/python`. pytest is the client's one development dependency,
  declared as the `dev` group in its `pyproject.toml`
  (`pip install --group ./clients/python/pyproject.toml:dev`); the package
  itself has none. On Linux the staging step also needs **patchelf**.
- **docs** — both mdBooks with the same mdBook version Read the Docs uses,
  and the pydoc-markdown API page for the client book.
- **faust** — the default `cargo test` covers it, with libfaust built from
  source at the commit pinned in the workflow (the recipe in
  `third_party/BUILD-FAUST.md`) and cached; a cache hit makes the job cheap.
  Upgrading libfaust = bumping `FAUST_SHA` there after verifying locally. The
  featureless / SynthDef-only runs (`--no-default-features`) are what keep the
  build green on a machine with no libfaust.

## Releases and publishing

- **Tagged releases** (`.github/workflows/release.yml`): pushing a `v*` tag
  builds the self-contained Python wheel (client + embedded server +
  standalone binary; Linux x86_64 for now) and a server-binary tarball,
  publishes the wheel to PyPI and attaches both to a GitHub release. PyPI
  auth is [Trusted Publishing](https://docs.pypi.org/trusted-publishers/)
  (OIDC — no stored token): the PyPI project must list this repository with
  workflow `release.yml` and environment `pypi` as a trusted publisher, and
  the repository needs a `pypi` environment. There is deliberately **no
  sdist**: the package compiles cdylibs from the Rust workspace, which an
  sdist of `clients/python` would not contain.
- **Read the Docs** hosts the two books as two projects pointing at the same
  repository, each selecting its config under *Settings → Advanced → Path to
  configuration file*: the server/workspace book uses the repo-root
  `.readthedocs.yaml`, the Python client book uses
  `clients/python/.readthedocs.yaml`.

## Conventions

- **Language**: everything under `src/`, `tests/` and `examples/` (code, comments, strings, test names) is in **English**, as are the roadmap files `PLAN.md` / `clients/PLAN.md` and the design record `docs/decisions.md`. `GUIA.md` (root + `clients/python/GUIA.md`) stays in **Spanish** (maintainer-facing manual smoke checklists); this book and the rustdoc are the English documentation.
- **API verbs in prose**: when the protocol or API has a verb for an action, the documentation names the action with that verb — a node or widget is **freed** (`/n_free`, `/gui_free`), never "destroyed" or "deleted"; a def is **sent**/**loaded**; a server is **booted**. Everyday synonyms make the prose drift from the surface the reader actually types against.
- **Closing a milestone** means, where applicable: code plus tests, a clear **commit message** (that is the record of *what* shipped — there is no separate per-milestone log), the `PLAN.md` roadmap checkbox updated, developer/user docs where the feature touches them (`docs/architecture.md`, `docs/schemas.md`, module docs), and a commented `examples/` entry for user-facing features. Add a short entry to `docs/decisions.md` **only** when a choice has non-obvious context, and a `GUIA.md` smoke step **only** when a new human-audible/visual behavior appears — neither is a per-milestone obligation.

## Project skills

Domain knowledge lives in `.claude/skills/`: `realtime-audio` (RT thread rules, lock-free patterns, cpal), `scsynth-osc` (the OSC protocol and node-tree model), `ugen-dsp` (UGen DSP algorithms), `audio-testing` (testing audio without ears: NRT, golden files, signal asserts, no-alloc), `faust-embedding` (the libfaust C API and lifecycles), and `faust-language` (writing Faust and transposing it to the Signal/Box APIs — sample-level feedback, physical modeling). Process skills: `clausters-python` (idiomatic client use), `clausters-gui` (the GUI host and GuiDefs) and `documentation` (how to write and place docs — the Diataxis split, the generated API references, and the dev/decision docs).

One skill is a workflow rather than knowledge: `feature-matrix` runs the fmt + clippy configurations of the commit workflow, including the three the CI never builds (`--no-default-features`, and each def family alone). Run it whenever a change touches feature-gated code.

## The commit hook

`.githooks/pre-commit` stops a commit whose `cargo fmt --check` or `cargo clippy` is dirty, for the default feature set of the workspaces the working tree touches (root, `clients/gui`, `fuzz` are three separate workspaces). A commit with no Rust in it costs nothing.

It is versioned in the repo but git only looks at it once you point git at it, so **enable it once per clone**:

```sh
git config core.hooksPath .githooks
```

**It does not enforce anything, and cannot.** `git commit --no-verify` skips it without running it; so does unsetting `core.hooksPath`, or editing the file, which sits in the working tree of the person it is checking. A client-side hook is a convenience, never a gate. What it buys is speed: the same failure in two seconds instead of five minutes into a CI run. **CI is the gate** — it runs fmt, clippy, the test matrix and the Python suite, and the person committing cannot skip it. So `--no-verify` is not a licence to land a warning; it just moves the discovery to CI, and the rule in CLAUDE.md ("zero warnings, always") is unchanged either way.

A *git* hook rather than an editor's or an agent's, because git is the one thing that knows with certainty that a commit is happening: anything upstream has to guess it from the text of a command line, and a guess that errs permissive is a check that silently is not there. It also covers every commit — from any terminal, editor or script.

It checks the **working tree**, not the index: cargo reads the filesystem, so the tree is what gets linted either way, and the rule in CLAUDE.md is about the tree. A dirty experiment left beside a clean staged change therefore blocks the commit; that is the rule working, not the hook overreaching.

## Claude Code hooks and settings

`.claude/hooks/` holds one hook, versioned so a clone gets it and containing no absolute paths: **`fmt-rust.sh`** runs `rustfmt` on every `.rs` file as it is written, so the tree cannot drift out of `cargo fmt --check`. It only formats files inside this checkout — "every crate is edition 2024 with no `rustfmt.toml`" is a fact about *this* tree, not about someone else's.

**Versioned vs. local.** `.claude/settings.json` (the hook wiring, plus the permissions any contributor would grant: `cargo build`/`test`/`fmt`/`clippy` and the venv's Python) is checked in. `.claude/settings.local.json` is git-ignored and is where machine-specific permissions belong — absolute paths, one-off commands, anything naming your home directory. So is `.claude/projects/`, the assistant's per-session memory.

**What the hooks need on a fresh clone**, beyond the build dependencies above:

- **`jq`** (`sudo apt install jq`), which `fmt-rust.sh` uses to read its input, and a `find` with `-mmin` (GNU or BSD, not POSIX) for the once-every-twelve-hours throttle on its warning. The git hook needs neither — git hands it the event directly.
- **`cargo` on the PATH of a *non-interactive* shell, with the `rustfmt` and `clippy` components installed.** The PATH is the one that bites: rustup adds `~/.cargo/bin` from a shell profile, and hooks do not necessarily run under one. The components are a separate question — rustup leaves a `cargo-clippy` proxy in `~/.cargo/bin` whether or not the component is there, so finding the binary proves nothing; ask cargo instead. If in doubt, `bash -c 'cargo fmt --version && cargo clippy --version'`, and `rustup component add rustfmt clippy` if either fails.

A hook whose dependencies are missing says so on stderr (once every twelve hours) and then stands down. It never blocks the work — but read the warning, because until it is fixed the check is simply not running.

## Editing this book

The book sources are the Markdown files in `docs/`; `docs/SUMMARY.md` is the table of contents and `book.toml` the config. Build and preview with [mdBook](https://rust-lang.github.io/mdBook/):

```sh
cargo install mdbook --version 0.4.40
mdbook serve     # live preview at http://localhost:3000
mdbook build     # generates ./book (git-ignored)
mdbook test      # type-checks Rust code snippets
```

**Pin the version.** CI and both `.readthedocs.yaml` builds fetch the prebuilt mdBook 0.4.40; an unpinned `cargo install` gets whatever is current, and a page that looks right locally can then render differently in what actually gets published. Upgrading means changing all three together. The prebuilt binary is also the faster route to the same thing:

```sh
curl -sSL https://github.com/rust-lang/mdBook/releases/download/v0.4.40/mdbook-v0.4.40-x86_64-unknown-linux-gnu.tar.gz | tar -xz -C ~/.local/bin
```

The Python client's book is a second, separate mdBook in `clients/python/docs/`, built by `clients/python/docs/build.sh`. Its API-reference page is **generated** from the package docstrings by pydoc-markdown, so it needs that tool as well — installed in user space, pinned to Python 3.12 because its dependencies lag the newest CPython and 3.12 is also what Read the Docs builds with:

```sh
uv tool install --python 3.12 pydoc-markdown
```

Both books have to stay current: a change that touches the server and the client surfaces belongs in both. See [`clients/python/README.md`](https://github.com/smrg-lm/clausters/blob/main/clients/python/README.md) for the alternatives to `uv` (`uvx`, or a plain `pip install` in an environment that is not externally managed).
