# Clients and language bindings

Clausters is a server; clients drive it. This chapter is the **cross-language
map**: the one native contract every client sits on, the Python client built on
it, and the path to a JavaScript client and to distributable packages. The
client work lives in the `clients/` tree; this is the architectural overview.

## One contract: the C ABI

Everything a client needs that must be *native* lives behind a single,
versioned **C ABI** — the scsynth plugin-ABI lesson: every binary boundary is
versioned and checked. Two cdylibs expose it, and the boundary rule is the same
for both: **only flat data crosses** — `f32`/`f64`/integers, pointer+length
arrays, NUL-terminated error strings. Never a library type (a numpy array can
*view* a returned pointer, but that is the client's choice, not a dependency).

| cdylib | crate | what it is | key entry points |
|---|---|---|---|
| `libclausters_ffi` | `clausters-ffi` over `clausters-core` | the **shared numeric/timing core** | `clausters_core_abi_version`, `clausters_core_unary`/`_binary` (builtins), `clausters_core_whitenoise`, `clausters_core_beats_to_secs`/`_secs_to_samples`/… |
| `libclausters` | `clausters` (feature `embed`) | the **server as a library** | `clausters_abi_version`, `clausters_render` (offline), `clausters_open`/`_send`/`_poll`/`_clock`/`_ctl_*` (in-process live server) |
| `libclausters_midi` | `clausters-midi` over `midly`/`midi2`/`midir` | **MIDI I/O** | `clausters_midi_write_smf` (`.mid`), `clausters_midi_write_clip` (MIDI 2.0 clip), `clausters_midi_free`; with `--features live`: `clausters_midi_output_open`/`_send`/`_close` (virtual port) |

Beside the in-process embed path, the same OSC reaches the server over **UDP**
or **shared memory** (`--shm`); see [Local transports & embedding](ipc.md). So a
client has three ways to talk to the server (UDP, shm, embed) and one way to
reach the native core (`libclausters_ffi`) — all language-agnostic.

Why a shared core at all: the builtins, the seeded white noise and the
beat/second/sample math are compiled **once** in `clausters-core` and used by
both the server's UGens and every client, so client-side results match the
server **by construction** for the operations the server computes natively.

## The Python client

`clients/python/` is the reference client, a selective port of
SuperCollider's class library (sc3) covering both def formats (FaustDefs and
UGen-graph SynthDefs). It is **pure Python at runtime**: it
reaches the core through `ctypes` over `libclausters_ffi`, and speaks ordinary
OSC bytes to the server (UDP, TCP, or shm/embed via the transport module). It
mirrors the native contract in three layers — `base` (server-agnostic timing
and values), `seq` (events and patterns) and `defs` (the Faust/UGen definitions
and the `Server`, whose swappable interface is the live-RT / offline-NRT seam).

It has its **own documentation** — a guide and the generated API reference:
**[the clausters Python client book](https://clausters-python.readthedocs.io/)**.
<!-- Cross-link to the companion book; update the URL if the Read the Docs slug differs. -->

**This is also the proof the contract is language-agnostic**: Python — a
non-Rust language — already drives the whole system (core math, offline render,
live server) purely through the C ABI and OSC. Nothing in the boundary is
Python-specific.

## A JavaScript client (planned)

A JS client mirrors the Python one with **no new native work**: it sits on the
same C ABI and the same OSC.

- **Native bridge**: Node/Deno **N-API** (or `Deno.dlopen`) over
  `libclausters_ffi`/`libclausters` for desktop; **WebAssembly** for the
  browser (the core compiled to wasm; the server itself targets wasm only for
  the offline `render` path).
- **Mirror the layers**: `base`/`seq`/`defs` map across directly. The one
  language-specific piece is the coroutine driver — JS **generators / async**
  instead of Python's `yield`; the clock and the rest are the same value/time
  logic.

## Distribution

- **Python (done)**: a platform-tagged **wheel** that bundles the cargo-built
  cdylibs (`libclausters_ffi`, and `libclausters` with `embed,realtime`) inside
  the package (`clausters/_libs/`), so an installed package is self-contained —
  no `target/` directory, no build step at import. The runtime stays
  stdlib-only; the loaders prefer the bundled copy, falling back to the
  workspace `target/` in a source checkout. A `setup.py` build hook runs `cargo
  build` and stages the libraries; `python -m build --wheel clients/python`
  produces the wheel. See the [Python client
  book](https://clausters-python.readthedocs.io/) for the install recipes and
  the env knobs (`CLAUSTERS_WORKSPACE`, `CLAUSTERS_CARGO_FEATURES`, …).
  Cross-platform CI wheels (cibuildwheel / manylinux) and a Faust-enabled build
  are still future work.
- **JavaScript (planned)**: an npm package with prebuilt N-API addons per
  platform and a wasm build for the browser.
- **Reproducible Faust build (planned)**: the `faust` feature needs libfaust
  built with the LLVM backend; for the wasm target and for Faust-enabled
  wheels/npm builds this should be vendored under `third_party/` with documented
  build steps. The current wheel ships the core embed build (no `faust`
  feature).

## Status at a glance

| Piece | State |
|---|---|
| Shared core + C ABI (`clausters-core`/`clausters-ffi`) | done |
| Python client (`base`/`seq`/`defs`, incl. UGen `SynthDef`) | done |
| Cross-language docs + sequencing example | done |
| Python wheels packaging | done |
| MIDI interfaces in the Python client (`MidiServer`, SMF / MIDI 2.0 clip export, live port) | done |
| Client-side OSC/MIDI responders (`OscFunc`/`MidiFunc`) | done |
| JavaScript client + npm | planned |
| `third_party` Faust build + Faust-enabled wheels | planned |
