# Building Faust from `third_party/faust`

How to build and install the Faust toolchain vendored in `third_party/faust`
(a full clone with `--recurse-submodules`, currently the `master-dev` tip
**versioned 2.86.0** — the unreleased dev version right after the 2.85.9
release: GRAME bumps the version string first thing after tagging), entirely in
user space (no sudo). Clausters needs **libfaust with the LLVM backend**;
Ubuntu's `libfaust2t64` ships without it and without headers, so building from
source is mandatory.

## The vendored, reproducible build (`build-faust.sh`)

The Faust source is **not committed** (it is heavy and git-ignored under
`third_party/faust`); reproducibility comes instead from two committed files:

- **`third_party/faust.pin`** — the single source of truth: the exact commit
  (`FAUST_SHA`, the unpatched `master-dev` base) and the LLVM version the shipped
  libLLVM is built against (`FAUST_LLVM_VERSION`, pinned to **18** — Ubuntu
  24.04's default, a baseline-x86-64 build that keeps the bundled libLLVM
  portable). A developer on a different LLVM (this machine has 21) builds locally
  with `LLVM_CONFIG=llvm-config-21 third_party/build-faust.sh`; only the *shipped*
  artifact is pinned, and the hand-written FFI surface is version-independent.
- **`third_party/build-faust.sh`** — one recipe, run identically by local dev,
  CI and the release wheel: fetch the pinned commit (+ submodules), build the
  dynamic `libfaust.so`, install into a prefix, and stage the libLLVM beside it.

```sh
third_party/build-faust.sh              # installs into ~/.local
third_party/build-faust.sh --prefix /some/where
```

It **protects an existing working clone**: if `third_party/faust` is checked out
somewhere other than the pin (e.g. the `fix-boxcos-boxfmod` branch below) it
refuses to move it — pass `FAUST_ALLOW_CHECKOUT=1` to check out the pin, or
`FAUST_SKIP_FETCH=1` to build the current checkout as-is. Override the LLVM
binary with `LLVM_CONFIG=…` if yours is not named `llvm-config-21`.

The rest of this document explains what the script does, step by step, and the
manual commands behind it.

## Requirements (native build)

System packages (these are the only ones that may need `sudo apt install`;
everything else is user-space):

- `cmake`, `make`, `g++` — the build system.
- `llvm-XX-dev` (tested with **LLVM 21**, `llvm-21-dev` on Ubuntu; LLVM 20
  also known-good) — the JIT backend. The build is driven by
  `llvm-config-XX`.
- `zlib1g-dev`.
- `libzstd-dev` — **only** if you link LLVM statically
  (`LINK_LLVM_STATIC=on`); with the dynamic `libLLVM.so` (recommended, and
  what the commands below use) it is not needed.
- **Not** needed: `libpolly-XX-dev` (only required by static LLVM linking),
  `libmicrohttpd` (HTTPD support is optional and skipped automatically if
  absent).

## Native build: compiler + libfaust (dynamic) + stdlib

This is the command `build-faust.sh` runs — from `third_party/faust`:

```sh
CMAKE_BUILD_PARALLEL_LEVEL=$(nproc) make most \
  CMAKEOPT="-DINCLUDE_DYNAMIC=ON -DINCLUDE_STATIC=off -DLINK_LLVM_STATIC=off -DLLVM_CONFIG=llvm-config-21"
```

- `make most` selects the `most.cmake` backends/targets: the `faust` CLI
  compiler, `libfaust.a` (static) and — in `most.cmake` — **not** the shared
  lib, hence `-DINCLUDE_DYNAMIC=ON` to add `libfaust.so`. Clausters'
  `build.rs` links `-lfaust` against the shared lib.
- `-DINCLUDE_STATIC=off`: skips the static `libfaustwithllvm.a`, which Clausters
  never links; it embeds LLVM's static component libs (needs Polly on some
  LLVM builds). Only the dynamic `libfaust.so` is built and installed.
- `-DLLVM_CONFIG=llvm-config-21`: Ubuntu installs only the versioned
  `llvm-config-21` binary (no plain `llvm-config`), so it must be named
  explicitly — the pinned version (`faust.pin`'s `FAUST_LLVM_VERSION`). The
  script derives it as `llvm-config-$FAUST_LLVM_VERSION`; override with
  `LLVM_CONFIG=…`.
- `-DLINK_LLVM_STATIC=off`: links the monolithic system `libLLVM.so`
  (`-lLLVM-21`). Result: `libfaust.so` ≈ 11 MB instead of a ≈ 35 MB
  `libfaustwithllvm.a`, and no Polly/zstd dev packages needed.
- `CMAKEOPT` extras are appended *after* the `-C` target cache files on the
  cmake command line, so they override the `FORCE`d cache defaults — no need
  to edit the cache in `build/faustdir` afterwards (an earlier recipe did it
  in two steps).
- The cmake configuration is cached in `build/faustdir`; on a re-run with
  different options, `make -C build distclean` first (it wipes only
  `faustdir`, keeping `build/bin` and `build/lib`).
- Wall time: ~10 min on 8 cores. Watch for the configure line
  `-- Found LLVM 21.1.8` to confirm the JIT backend is in.

## Install into `~/.local`

```sh
make install PREFIX=$HOME/.local
```

Installs `~/.local/bin/faust*` (compiler + the `faust2*` wrapper scripts),
`~/.local/lib/libfaust.{so,so.2,so.2.86.0}`, headers under
`~/.local/include/faust`, and the stdlib (`*.lib`) in `~/.local/share/faust`.
No `ldconfig` or `LD_LIBRARY_PATH` needed for clausters: its `build.rs` finds
the prefix (`FAUST_PREFIX` env var, falling back to `~/.local`, then
`/usr/local`) and embeds an rpath.

After installing, `build-faust.sh` also **stages the libLLVM** that
`libfaust.so` is NEEDED-linked against into `<prefix>/lib` (found via `ldd`,
copied with `cp -L`). That is what makes the prefix self-contained: Clausters'
`DT_RPATH` (`<prefix>/lib`, inherited by transitive deps) then resolves both
libfaust and its libLLVM with no LLVM runtime installed — the same layout the
wheel bundles and CI restores. Doing it by hand:

```sh
cp -L "$(ldd ~/.local/lib/libfaust.so | awk 'tolower($0) ~ /llvm/ {print $3}')" \
  ~/.local/lib/
```

Sanity check:

```sh
~/.local/bin/faust --version         # Faust 2.86.0, with LLVM backend listed
ldd ~/.local/lib/libfaust.so | grep LLVM   # => libLLVM.so.21.x (JIT present)
```

Uninstall: `make uninstall PREFIX=$HOME/.local` (uses the manifest in
`build/faustdir/install_manifest.txt`).

## WebAssembly parts

Two distinct wasm artifacts, both **excluded from `make most`** and both
requiring the [Emscripten SDK](https://emscripten.org/docs/getting_started/downloads.html)
(`emcc` on the `PATH`; user-space install via `emsdk install latest &&
emsdk activate latest` + sourcing `emsdk_env.sh`):

1. **`libfaust-wasm`** — the whole Faust *compiler* as a WebAssembly library
   (what faustwasm / the Faust IDE use to compile `.dsp` in the browser).
   From `third_party/faust`:

   ```sh
   make wasmlib          # or: make -C build wasmlib
   ```

   Produces `build/lib/libfaust-wasm.{js,wasm,data}` (the `.data` bundles the
   stdlib for Emscripten's virtual FS). Note this is *target* wasm ≠ the
   native build above; both coexist in the same tree (the wasm objects live
   under `build/faustdir/emcc`).

2. **wasm glue** — the small C++ runtime glue (audio driver/mixer bindings)
   for *running* Faust-generated wasm modules in WebAudio:

   ```sh
   make wasmglue         # or: make -C build wasmglue
   ```

   (Its own deps — libsndfile et al. — are prebuilt in `build/wasmglue/`.)

Related but separate: the native `faust` compiler already emits wasm/wast
*code* (`faust -lang wasm foo.dsp`) with no Emscripten needed — Emscripten is
only required to build the two artifacts above.

> Status in this repo: not built — `emcc` is not installed on this machine.
> The native build/install above is done and verified.

## The `fix-boxcos-boxfmod` branch

The clone carries a local branch fixing the two C-API bugs of [faust#1264]
(submitted upstream as
[faust#1272](https://github.com/grame-cncm/faust/pull/1272)):
`boxCos()`/`boxFmod()` returning the `abs` primitive — the reason for the
fragment workaround in `src/faust/boxes.rs` — and the missing `kLRsh` case
in the signal type checker — the reason `CsigLRightShift` is not bound in
`src/faust/ffi.rs`. Note: if you build and install libfaust **from that
branch**, the canaries `upstream_boxcos_still_computes_abs`
(`tests/faust_box.rs`) and `upstream_lrsh_still_fails_the_type_checker`
(`tests/faust_signal.rs`) fail *by design* — that is the signal to retire
the workaround and bind `lrsh`. The verified install below is from the
unpatched `master-dev` base (`56c9e678d`, "Set version to 2.86.0").

[faust#1264]: https://github.com/grame-cncm/faust/issues/1264

## Building clausters against it

From the repo root, with the install above in place:

```sh
cargo build            # faust is a default feature
cargo test
```

Notes (see also `BUILD.md` and the `faust` sections of `CLAUDE.md`):

- `build.rs` locates the prefix via `FAUST_PREFIX` → `~/.local` →
  `/usr/local`; with the `~/.local` install nothing needs to be exported.
- **No `--test-threads=1` needed.** Every libfaust compilation FFI call now
  goes through `faust::ffi_lock()`, which serializes the compiler in-process,
  so the faust suites run in the ordinary parallel harness. (A SIGSEGV in a
  `faust_*` suite means an FFI path skipped the lock — that is the signal to
  look there, not a reason to serialize the whole run.)

### Verified 2026-07-05 (this build)

- Faust 2.86.0 (`third_party/faust`) + LLVM 21.1.8 (`llvm-21-dev`), Ubuntu
  (Resolute), shared-LLVM link. Built with the exact commands above and
  installed to `~/.local`. `faust --version` lists the LLVM backend;
  `libfaust.so` = 11 MB, linked against the system `libLLVM.so.21.1`.
- Full Faust compile: ~9 min wall on this machine (parallel via
  `CMAKE_BUILD_PARALLEL_LEVEL`; the faust makefiles pass no `-j` of their
  own, so without that env var the build is serial and much slower).
- Clausters: `cargo build --features faust` links out of the box (no
  `FAUST_PREFIX` needed — `~/.local` is the default fallback), and
  `cargo test --features faust` passes **343 tests across 36 suites, 0
  failures** (~3 s of test time). No source changes were needed moving from
  the earlier toolchain (Faust 2.81.10 / LLVM 20) to 2.86.0 / LLVM 21 — the
  hand-written FFI surface is unchanged.
