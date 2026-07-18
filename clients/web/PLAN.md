# Plan - Clausters web client (TypeScript, browser-first)

The TypeScript client: a high-level client of `clausters-server` and of the browser GUI host, the web sibling of `clients/python`. It is its **own package, docs, examples and tests** under `clients/web`, exactly the way the Python client lives under `clients/python` - a parallel client track, not part of the GUI host (`clients/gui`) and not part of the server.

> **Read alongside `clients/PLAN.md`.** That plan already covers the *shared* client rationale (the native Rust core, the C-ABI/wasm seam, the "only flat data crosses" boundary, the coroutine-driver-stays-in-the-language rule) and was written from the start "to serve a future JavaScript client too". This file is the web-specific track; it does not restate the shared design, it points at it and records only what is different in the browser. As there, **sc3 is the reference model** for module structure, clock/routine behaviour, events, patterns and OSC/MIDI semantics; deviate only with an explicit reason.

## Why a separate track (and why now)

The browser GUI host (`clients/gui`, milestones G11-G16) runs the `/gui_*` widget system under WebGPU and is driven *through* a small wasm binding surface or over WebSocket. Those milestones deliberately use a **throwaway** in-page harness to test the host; the real, product driver - the thing a web app actually programs against - is this TypeScript client. Keeping it here, separate from the host, preserves the same boundary the rest of the system keeps: the host is a front, the client is a consumer of the protocol, and either can change without dragging the other.

So the web client is one more consumer of the exact same wires the Python client uses: OSC-in-JSON GuiDefs to the GUI host, OSC to the audio server, the same `decode_packet` framing - only the carrier (browser `WebSocket`), the binding technology (wasm, not ctypes) and the host language (TS, not Python) differ.

## Guiding principles

- **Maximum reuse; the browser only adds its I/O.** What is value or time transformation is shared, not re-implemented: OSC assembly/decode, TempoClock arithmetic, the numeric builtins and the analysis kernels (peaks/FFT) come from **`clausters-core` compiled to wasm** (via wasm-bindgen), so the client is **numerically equivalent to the Python client and to the server by construction**. The GuiDef/`/gui_*` protocol and the def specs are the same JSON the Python builders emit. New TS code is confined to: the language-side control flow (generators/async routines), the browser carriers (`WebSocket`, Web MIDI, Web Audio clock, `fetch`), and the ergonomic builder/typed API.
- **The seam is the same as `clients/PLAN.md`.** The Rust core owns builtins, TempoClock (queue + arithmetic) and OSC bundle/timetag assembly + sample-clock conversion; the coroutine driver (`function*`/async in TS, sc3-style routines) stays in the language; no Rust callbacks into JS - the loop asks the wasm queue "what's next and when?", sleeps on the browser clock, resumes the routine. "Only flat data crosses" the wasm boundary (typed arrays / numbers / strings in and out, no callbacks).
- **Client/server separation, as in the Python client.** Timing/sequencing/GuiDef authoring is transport-agnostic; only a `Server`/`GuiHost` object knows the connection. The `TempoClock` must not talk to the server (the same rule corrected in the Python client's C4).
- **Browser realities are first-class, not afterthoughts.** WebSocket is the only transport (no UDP, no shared memory, no mmap, no embedded server); bulk data arrives by `fetch`/`/b_getn`; meters/scopes read control buses over the wire; the sample-clock timebase uses the Web Audio clock (`AudioContext.currentTime`). These are the same "async fallbacks" the server/gui plans reserved for the browser.

## Target architecture

A TS package mirroring `clients/python`'s shape (concern-named modules, an mdBook, examples, tests), so a reader who knows one finds the other:

```
clients/web/
  package.json            # the `clausters` npm package (TypeScript)
  tsconfig.json
  src/
    base/                 # the low-level seam (mirrors clausters/base)
      osc.ts              #   OSC encode/decode over the wasm core (mirrors _osclib)
      connection.ts       #   a WebSocket carrier to a --ws server/host (mirrors _oscinterface/netaddr)
      clock.ts            #   the routine driver over the wasm TempoClock queue
      timebase.ts         #   monotonic vs Web Audio sample-clock timebase
      builtins.ts         #   numeric builtins from the wasm core
    defs/                 # the def model + server client (mirrors clausters/defs)
      server.ts           #   the Server object: /d_recv, /s_new, /n_set, /sync, ...
      node.ts  bus.ts  buffer.ts
      signals.ts  ugens.ts  synthdef.ts  faustdef.ts  graphdef.ts
    gui/                  # the GUI host driver (mirrors clausters/gui)
      guidef.ts           #   window/panel/knob/slider/waveform builders
      host.ts             #   GuiHost: drive the wasm host in-page or a --ws host
    seq/                  # sequencing (mirrors clausters/seq)
      event.ts  eventstream.ts  pattern.ts  timeline.ts
    responders.ts         # OscFunc/MidiFunc dispatch (mirrors responders.py)
    session.ts            # the Session facade
    core/                 # the wasm-bindgen bindings to clausters-core (generated + thin wrapper)
  docs/                   # an mdBook (mirrors clients/python/docs), API ref from TSDoc via typedoc
  examples/               # the Python examples, ported (ws-only)
  tests/                  # the test suite (vitest)
```

The wasm `clausters-core` build is **shared with the GUI host** (G11-G16 already needs core compiled to wasm); this client links the same artifact, it does not produce a second one.

## Milestones

Labels (`Wx`) live only here, never in published docs or docstrings - the same rule as the other plans.

### W0 - Package skeleton + OSC over WebSocket (the carrier)

The smallest round trip, and the toolchain.

- The `clausters` TS package scaffolding: `package.json`/`tsconfig`, a bundler (esbuild/vite), a test runner (vitest), lint/format, and the wasm-bindgen build of `clausters-core` wired in.
- `base/osc.ts` + `base/connection.ts`: encode/decode one OSC packet **through the wasm core** (numerically equal to the server/Python) and carry it over a browser `WebSocket` to a `--ws` server. The TS sibling of `examples/ws_ping`.

**Acceptance:** a browser page connects to a `--ws` Clausters server, sends `/status`, and decodes the reply through the shared core; an automated test covers encode/decode parity against a known vector.

### W1 - Server client + the def model

Drive the audio server.

- `defs/server.ts`: the `Server` object - send `/d_recv`/`/d_graph`/`/d_faust` specs, `/s_new`, `/n_set`/`/n_free`, groups, the `/sync` barrier, buses and buffers; receive replies through `responders` (W4 hardens this).
- The def builders (`signals`/`ugens`/`synthdef`/`faustdef`/`graphdef`): start by sending the **same spec JSON the Python builders emit** (reused verbatim), then grow the typed TS builder API for parity, with the Python builders (both def families) as the reference.

**Acceptance:** from a browser page, define a synth/Faust def over WS and play it (`/s_new` then `/n_set`), with `/sync` ordering and an audible/queryable result on a `--ws` server.

### W2 - GUI host driver (`GuiHost` + GuiDef builders)

The product driver the GUI track (G13) deferred here - this closes the loop with G11-G16.

- `gui/guidef.ts`: the GuiDef builders (`window`/`panel`/`knob`/`slider`/`number`/`toggle`/`menu`/`waveform`/`meter`/`scope`/`canvas`/...), mirroring `clausters.gui.guidef`, emitting the same JSON.
- `gui/host.ts`: a `GuiHost` mirroring `clausters.gui.host` that drives the **wasm GUI host in-page** (through its wasm-bindgen binding surface) and/or a remote `--ws` host: `/gui_def`/`/gui_set`/`/gui_free`/`/gui_bind` out, `/gui_event`/`/gui_closed` in.

**Acceptance:** a TS app builds a panel GuiDef and drives the browser GUI host with it; interactions return as `/gui_event`; a `bind`-ed widget drives a `--ws` audio server with no round-trip through the page - the same examples the Python client runs against the native host, now in the browser.

### W3 - Sequencing: clock, routines, events, patterns

The timing layer, transport-agnostic, sc3-modelled.

- `base/clock.ts` + `base/timebase.ts` + `seq/*`: the routine driver over the **wasm TempoClock** queue (arithmetic and timetag/sample-clock conversion in the core; the `function*`/async driver in TS), with both timebases - the monotonic clock and the **Web Audio sample-clock** (`AudioContext.currentTime`) - and events/patterns mirroring `clausters.seq`.
- Keep the C5 lesson: the clock advances by yielding; the monotonic clock only computes sleeps, so relative timing is exact; the clock never talks to the server.

**Acceptance:** a routine schedules events that play on a `--ws` server with exact relational timing under both timebases, matching the Python client's behaviour on the shared vectors.

### W4 - Responders + MIDI (OscFunc/MidiFunc), buses and bulk over the wire

Receiving, dispatch, and the browser data paths.

- `responders.ts`: OscFunc/MidiFunc-style dispatch over the WS reply stream (and Web MIDI for `MidiFunc`), mirroring `responders.py`/`base/_midiinterface`.
- The browser data paths the GUI client needs: control buses read over WS (the message-based counterpart of shared memory, G14) feeding meters/scopes; bulk buffers via `fetch`/`/b_getn` (G15), with the peak pyramid built in wasm from fetched samples. **The server side already exists** (landed with G14): `/c_stream periodMs bus...` subscribes the client to periodic `/c_set` snapshots (one subscription per client, replaced per call, `periodMs <= 0` cancels, 10 ms floor, ≤128 buses; see `docs/schemas.md`). The TS client only consumes it — subscribe, decode the `/c_set` stream in a responder, feed the GUI host — exactly as the browser GUI host does in `clients/gui/src/host/web.rs`.

**Acceptance:** a TS app registers OscFunc/MidiFunc handlers that fire on server/MIDI events, and drives a browser GUI whose meters/waveforms read buses/buffers over WS.

### W5 - Docs, examples, tests, packaging

Make it a real, shippable client.

- An mdBook in `clients/web/docs` (mirroring `clients/python/docs`), with the API reference **generated from TSDoc by typedoc** (the TS counterpart of the Python client's pydoc-markdown), and the GUIA-style manual-testing notes kept current. The two client books and the two GUI books cross-link by their RTD URLs.
- The Python examples ported to TS (WS-only), a vitest suite, and the npm package build/publish; a parity pass against the Python client on the shared vectors (OSC, clock arithmetic, GuiDef JSON).

**Acceptance:** `npm install clausters` (or the workspace build) yields a usable client; the ported examples run in a browser against a `--ws` server and the browser GUI host; the docs build and deploy like the Python client's.

## Future directions

- **Node target.** The same package outside the browser (a Node `WebSocket` carrier and the same wasm core) for headless scripting/CI, the way `clients/python` runs without a display.
- **Type-safe GuiDef/def schemas.** Generate TS types for the widget/def vocabularies from a single source shared with the server, so an invalid GuiDef is a compile error, not a runtime warning.
- **Bundled standalone web app.** A built page that boots a GuiDef bundle (the web analogue of `--standalone`) against a remote `--ws` server - no embedded engine in the browser, but a one-file instrument front. **Update (2026-07-18):** the embedded-engine limitation is being lifted by the server's B track (root `PLAN.md`: wasm engine + AudioWorklet + in-tab bundle boot); B4 seeds the npm package in this directory, which W0 then adopts.
