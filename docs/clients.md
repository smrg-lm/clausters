# Clients and language bindings

Clausters is a server; clients drive it. This chapter is the **cross-language
map**: the one native contract every client sits on, the Python client built on
it, and the path to a JavaScript client and to distributable packages. The
client work and its milestones live in [`clients/PLAN.md`](https://github.com/)
(the `clients/` tree); this is the architectural overview.

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

Beside the in-process embed path, the same OSC reaches the server over **UDP**
or **shared memory** (`--shm`); see [Local transports & embedding](ipc.md). So a
client has three ways to talk to the server (UDP, shm, embed) and one way to
reach the native core (`libclausters_ffi`) — all language-agnostic.

Why a shared core at all: the builtins, the seeded white noise and the
beat/second/sample math are compiled **once** in `clausters-core` and used by
both the server's UGens and every client, so client-side results match the
server **by construction** for the operations the server computes natively (see
the C0 notes in `NOTAS.md`).

## The Python client

`clients/python/` is the reference client, a Faust-first port of
SuperCollider's class library (sc3). It is **pure Python at runtime**: it
reaches the core through `ctypes` over `libclausters_ffi`, and speaks ordinary
OSC bytes to the server (UDP, or shm/embed via the transport module). Layering:

- `clausters.base` — server-agnostic timing and value work: `builtins`
  (scalar/list math via the core), `absobject` (operator overloading), `stream`
  (`Routine`, the `yield` coroutine layer), `clock` (`TempoClock`, timing only),
  `timebase` (monotonic or the server's sample clock), the OSC/MIDI destination
  interfaces, `netaddr`, `main` (a thread-local execution context — **no global
  state that blocks RT and NRT in one script**).
- `clausters.seq` — sequencing: `Event`, the value patterns and `Pbind`,
  `EventStreamPlayer`.
- `clausters.defs` — the **server side**: `signals` (lowercase callables mapping
  Faust's Signal API into the JSON graph) + `FaustDef` (`/d_faust`), and their
  UGen-graph counterpart `ugens` (lowercase callables → `Ugen`/`Control`) +
  `SynthDef` (`/d_recv`) — both built **instance-based, no global build
  context**; the `Node`/`Bus`/`Buffer` handles and allocators, and `Server`
  (owns the communication interface and emits; swapping its interface retargets
  a routine from live RT to an NRT score — the seam).

What is implemented (C0–C5) and what is planned (C6 UDP sample-clock anchoring,
C7 MIDI interfaces, C8 TCP, …) is tracked in `clients/PLAN.md`; the hands-on
guide is `clients/python/clausters/GUIA.md`.

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
  logic. Per `clients/PLAN.md`, the client is written so a JS client "shares the
  same characteristics".

## Distribution (planned)

- **Python**: a wheel that bundles the prebuilt cdylibs (`libclausters_ffi`,
  and `libclausters` with `embed`) per platform; the package stays stdlib-only,
  locating the libraries it ships. (`pyproject.toml` already declares the
  package and the `dev` dependency group.)
- **JavaScript**: an npm package with prebuilt N-API addons per platform and a
  wasm build for the browser.
- **Reproducible Faust build**: the `faust` feature needs libfaust built with
  the LLVM backend; for the wasm target and for reproducible wheels/npm builds
  this should be vendored under `third_party/` with documented build steps
  (a backlog item — see `NOTAS_PARA_CLAUDE` / `clients/PLAN.md`).

## Status at a glance

| Piece | State |
|---|---|
| Shared core + C ABI (`clausters-core`/`clausters-ffi`) | done |
| Python client base/seq/defs (C0–C5) | done |
| UDP sample-clock anchoring, MIDI, TCP (C6–C8) | planned |
| JavaScript client | planned |
| Wheels / npm / wasm distribution, `third_party` Faust | planned |
