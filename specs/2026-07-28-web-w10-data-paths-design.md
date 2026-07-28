# W10 — The browser data paths: buses, bulk, and the analysis exports

Design for the milestone `clients/web/PLAN.md` labels W10. Approved 2026-07-28.

## What this milestone is

The GUI host already reads the server's live data: a `meter` names a control
bus, a `scope` names an audio tap, a `waveform` names a URL, and the host
subscribes, fetches and reduces on its own. That is why a GuiDef naming any of
them works in the browser today.

This milestone gives the **script** the same three paths, so a page can read
what the host reads and draw it itself:

- **control buses**, streamed over the connection (`/c_stream` → periodic
  `/c_set` snapshots),
- **audio taps**, streamed the same way (`/tap_stream` → `/tap_data` windows),
- **bulk buffers**, read and written by `/b_getn`/`/b_setn` or fetched and
  decoded by the browser, with the **peak pyramid built in wasm**,

plus the **analysis exports** the views are made of: correlation, Lissajous,
the FFT magnitudes, and the oscilloscope's trigger alignment.

The user-facing point is the audio-editor and scope views: any page view that
needs to *read data from the server in wasm* has its source here.

## The rule that governs the whole milestone

> **No number is computed in TypeScript.**

Every figure the script draws comes out of the same `clausters-core` function
that produces the figure the host draws. The acceptance's "numerically
matching what the GUI host draws from the same source" is then true by
construction rather than by tuning — the same reason the sequencing layer (W3)
computes no time of its own.

This has one structural consequence, below: a piece of signal logic that today
lives inside `clients/gui` moves down into the core.

## Vocabulary

| Term | What it names |
|---|---|
| **snapshot** | one periodic `/c_set` (buses) or `/tap_data` (a tap window) |
| **window** | a run of consecutive tap samples on the tap's own sample axis |
| **column** | one screen pixel column of a waveform: a `(min, max)` pair |
| **pyramid** | the min/max peak cache the columns are read from |

A subscription is **subscribed** and **cancelled**, a buffer is **read** and
**written**, a def is **sent**, a node is **freed**. A view **draws**.

## 1. Rust: the core, and its wasm door

### 1.1 `clausters_core::oscil` — the trigger moves down

`clients/gui/src/host/oscil.rs` holds the audio-rate oscilloscope's signal
logic: `display_frames` (how many frames a `window_ms` display holds),
`raw_frames` (that plus the trigger's search slack) and `align` (the rising
crossing the trace locks onto, and whether it locked). It is already pure — no
GPU, no shared memory — and already shared verbatim by the native and browser
fronts.

It **moves to `clausters-core` as `oscil`**, and the GUI host consumes it from
there. Nothing about the host's behaviour changes; what changes is who can
reach it. A script that draws its own oscilloscope gets the *same* trace, phase
lock included, instead of a second trigger implementation in TypeScript — the
placement rule the core exists for ("an algorithm used by more than one
process lives once"), applied the moment a second process wants it.

The unit tests move with the code.

### 1.2 `crates/clausters-core-web` — the new doors

Each one a mechanical shell over the core, the shape every door in that crate
already has:

| Door | Core behind it |
|---|---|
| `correlation(l, r)` → `number \| undefined` | `measure::correlation` |
| `lissajous(l, r)` → `Float32Array` (`[x, y]` interleaved) | `measure::lissajous_into` |
| `fft_magnitudes(samples, fft_size, wintype)` | `fft::rfft_magnitudes_into` + `window::Window` |
| `oscil_display_frames`, `oscil_raw_frames`, `oscil_align` | `oscil` (1.1) |
| `JsPyramid` | `peaks::MultiPyramid` |

`JsPyramid` is the one with state, so it crosses as a class:

```
JsPyramid.build(samples, channels, base_bucket)   // interleaved samples
  .frames() .channels() .num_levels() .base_bucket()
  .level_bucket(level) .level_for(samples_per_px)
  .column(ch, level, s0, s1)          -> [min, max] | undefined
  .columns(ch, level, s0, s1, width)  -> Float32Array, [min, max] per column
  .to_bytes() / JsPyramid.from_bytes(bytes)
```

`columns` is the door a view actually calls: **one crossing per frame** for a
whole pixel row, never one per column, and never a resolution finer than the
screen. `to_bytes`/`from_bytes` are the same cache format the host maps and the
Python client writes, so a pyramid computed anywhere is readable everywhere.

### 1.3 What does *not* move

`clausters-ffi` is untouched: Python already reaches `peaks`, `correlation` and
`lissajous` through it, and nothing here changes that surface. So
**`CORE_ABI_VERSION` does not move**, and neither does any package version —
this milestone is not a release.

## 2. TypeScript: what `Server` learns, and `src/data/`

### 2.1 `Server` — the commands, mirroring the Python client verb for verb

| TS | Python | Wire |
|---|---|---|
| `streamBuses(periodMs, ...buses)` | `stream_buses` | `/c_stream` |
| `taps` (a registry) + `tap(tap, bus)` | `taps`, `tap` | `/tap` |
| `streamTaps(periodMs, frames, ...taps)` | `stream_taps` | `/tap_stream` |
| `getSamples(buf, {start, count, chunk})` | `get_samples` | `/b_getn` → `/b_setn` |
| `setSamples(buf, start, samples, {chunk})` | — | `/b_setn` |
| `loadSample(url)` | — | see below |

The tap registry is the core's occupancy map like every other allocator here,
sized from `/server_info`'s tap count, so two views never fight over one ring —
the same posture `clausters.scope` keeps in Python.

`getSamples`/`setSamples` chunk by the **transport's own bound**: the frame
ceiling the server advertises in `/server_info` (queried once and cached),
which is megabytes per round trip on a stream carrier. The rule is the Python
client's `_bulk_chunk`, ported.

`loadSample(url)` is one API over two carriers: `fetch` +
`decodeAudioData` gives interleaved samples, which the in-page engine takes
through `bLoad` (the path `bundle.ts` already walks, folded in here) and a
socket carrier takes as `setSamples` chunks after a `/b_alloc`.

### 2.2 `src/data/` — the sources

A new module tree, one file per source, each a small object with a
subscribe/read surface and no drawing of its own:

- `data/buses.ts` — `BusStream`: subscribes `/c_stream`, decodes the periodic
  `/c_set` snapshots into a `Float32Array` in the order the caller asked for,
  notifies subscribers, `stop()` cancels.
- `data/taps.ts` — `TapStream`: decodes `/tap_data tap endPosition blob` into
  the newest window per tap, placed on the tap's own sample axis by
  `endPosition` (consecutive snapshots overlap or gap by exactly the position
  delta). Beside it `scopeWindow(...)`, the trigger-aligned trace, computed by
  the core's `oscil`.
- `data/samples.ts` — the bulk read/write helpers and the fetch/decode pair.
- `data/peaks.ts` — `Peaks` over `JsPyramid`: `Peaks.build(samples, {channels,
  baseBucket})` and `columns(channel, {start, end, width})` → `{min, max}`,
  ready for a canvas.
- `data/analysis.ts` — `correlation`, `lissajous`, `spectrum`.
- `data/index.ts` — the barrel, re-exported from `src/index.ts`.

### 2.3 The reply door, and W8

The subscriptions consume replies through `Server.onReply`, the door W1 already
built. **W8** (responders: `OscFunc` over the reply stream) folds that ad-hoc
dispatch onto one door; when it lands, these sources move onto it without
changing their own surface, because a source only ever asks "give me the
decoded messages".

## 3. Verification

- **`node --test`** — new parity vectors (`tests/gen-data-vectors.py`, the
  generator pattern the OSC/def/clock/GuiDef vectors already use, computing
  through the Python client's FFI): peak columns, correlation, Lissajous,
  trigger alignment and FFT magnitudes. Plus, against a fake connection, the
  chunking of `getSamples`/`setSamples` and the decoding of bus and tap
  snapshots.
- **The WS suite** — against a real `clausters --ws`: a control bus and an
  audio tap read live, and a `setSamples` → `getSamples` round trip.
- **The page acceptance** (`tests/data.html`, headless Chrome, in-page engine,
  no server process) — a synth moves a control bus and feeds a tap; the script
  subscribes, **draws it itself** on a canvas, reads a buffer with `/b_getn`,
  builds the pyramid, and asserts that its columns equal a direct min/max over
  those same samples and that the streamed bus value matches `/c_get`.

## 4. Documentation

- A new chapter in the web client's book (`clients/web/docs/src/data.md`, with
  its `SUMMARY.md` entry): the three paths, what each costs, and how a view is
  fed.
- `docs/architecture.md`: the trigger alignment now lives in the core.
- `docs/decisions.md`: why the oscilloscope's signal logic moved down — the
  moment a second process (a page's own drawing) wants an algorithm, the
  algorithm belongs to the core.
- `clients/web/PLAN.md`: the W10 checkbox and its "What shipped".
- `examples/scope.html`: a meter, an oscilloscope and a waveform, all drawn by
  the script from these sources.

## Out of scope

- **Editing back** beyond `setSamples`: an edit-back protocol for a view that
  owns data (the `/gui_*` edit payloads) is the GUI track's, not this one.
- **The spectrogram's STFT cache** — the heavy offline analysis the editor-grade
  view wants. `fft_magnitudes` is the per-frame door; a cached STFT is its own
  piece of work.
- **`/b_export` + mmap**: the local shared-resource path has no browser
  counterpart (a page has no filesystem); `fetch` is its analogue and is here.
