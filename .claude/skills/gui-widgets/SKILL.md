---
name: gui-widgets
description: Domain knowledge for designing and implementing each Clausters GUI graphical element — the widget extension recipe and data-source table, oscilloscope triggering and the audio-tap gap, goniometer/phasescope geometry, live FFT spectrum, editor-grade waveform/spectrogram (multichannel, LOD crossfade, rulers, selection/playhead), BPF envelope editing, and the edit-back-to-data pattern. Consult when designing or implementing any new GUI widget or deepening an existing view.
---

# GUI widgets: per-element domain knowledge

This skill is the widget-level companion of [[clausters-gui]] (the track map: protocol, architecture, rendering machinery) — what [[ugen-dsp]] is to the server's UGens. It covers what each graphical element *is*: its algorithm, visual parameters, data source and implementation pattern. `clients/gui/PLAN.md` wins on milestone scoping and staging; `DESIGN.md` on rationale.

## The extension recipe (every new widget follows it)

Adding a widget is **never a protocol change** — the generic GuiDef node (`{id, type, ...props, children}`) already carries it; an unknown type is laid out but not painted. The checklist, all inside `clients/gui/`:

1. **Typed kind** — a new `WidgetKind` variant in `src/host/widget.rs` plus its `Widget::from_node` parse (keep the int/float distinction; props are flat OSC-primitive JSON).
2. **Renderer** — two shapes exist; pick by cost:
   - *Cheap*: pure functions over the flat-geometry `Mesh` (`src/host/paint.rs` `Painter`: rect/quad/line/disc + the `src/host/font.rs` bitmap text), like `meters`/`nodetree`/`plot`. GPU-free, unit-testable without a window.
   - *Heavy GPU*: an own pipeline/view like `waveform`/`spectrogram`/`canvas` (the `TimelineView` shape). Only when a mesh per frame cannot express it (textures, shaders, very dense geometry).
3. **Frame plumbing** — live inputs reach the shared per-window render `src/host/frame.rs` (`FrameInputs`); both fronts (native `gui.rs`, web `web.rs`) call the same `frame::render`, so a widget drawn there is browser-correct by construction.
4. **Interaction** — hit-test + mutation in `src/host/interact.rs` (shared by both fronts); a value-bearing interaction goes through the one `deliver` path: binding `forward` first, `/gui_event` when unbound.
5. **Python builder** — one function in `clients/python/clausters/gui/guidef.py` (mirror the existing ones: keyword props, `_drop_none`), plus a commented example in `examples/`.
6. **Tests** — GPU-free logic (models, math, parse) in `cargo test` from `clients/gui/`; window binaries and E2E follow the single-Bash-invocation rule.

## Data sources (what can feed a widget)

| Source | Mechanism | Native | Browser |
|---|---|---|---|
| Inline `data` / trailing OSC `blob` | rides the `/gui_def` message | yes | yes |
| Mapped file `path` / peaks `cache` | zero-copy bulk path, no OSC | mmap (`src/host/bulk.rs`) | fetched as a URL (`ArrayBuffer`) |
| Server buffer `buffer` | `/b_query` → chunked `/b_getn` (`src/host/fetch.rs`, shared state machine) | yes | yes (WS leg) |
| Control bus, per frame | shm segment (`src/host/shm.rs`: `control(i)`, `sample_clock()`, `sample_rate()`) | yes | `/c_stream` → `StreamedBuses` (`src/host/live.rs`) |
| **Audio-rate tap** | **does not exist yet** — the shm segment carries only control buses | — | — |

The audio tap is the prerequisite the scopes below share: a server-side tap (SuperCollider's `ScopeOut2` shape — a UGen writing the recent samples of its input into a shared ring the host reads each frame). The segment ABI is versioned (`src/server/ipc.rs`: `MAGIC`, `ABI_VERSION`), so extending it is a loud, checked bump; the browser needs a streamed sibling (the `/c_stream` pattern at audio-frame granularity). Design and staging: see PLAN.md.

One principle bounds all host-side analysis: **the host computes only what plotting needs** (peaks, a display FFT). Audio *processing* happens once, in the server — which already owns a spectral chain (`FFT`/`PV_*`/`IFFT`, `src/dsp/spectral.rs`); a client wanting processed audio runs it there (live or NRT) and hands the host the result resource.

## Placement analysis: every general compute function goes to the core

The established rule (peaks, the forward FFT and the windows already took this path): **an algorithm used by more than one Clausters process lives once, in `clausters-core`, and is exposed over `clausters-ffi` when a non-Rust client needs it** — never reimplemented per client. So every new compute function a widget introduces gets an explicit placement analysis, recorded with the milestone:

- **General** — useful to another client (Python plotting/authoring the same data) or to a future server feature (a UGen, an analysis command) → `clausters-core`; add the FFI export if Python (or a later JS client) consumes it.
- **Display-only** — meaningful only against a screen (hit-testing, tick spacing, trigger alignment of a drawn window) → the gui crate.

Standing candidates from the widgets below: the **envelope shape math** (today only in the server's `src/dsp/envgen.rs::shape_value` — the BPF editor must draw the same curves and the server crate cannot be linked from the gui, so it moves to the core with the server delegating, the same move the FFT made); **multichannel peaks** (the Python client must build the identical multichannel cache through the existing peaks FFI); the **correlation metric** (a general audio measurement, not a pixel concern). Counter-examples that stay gui-side: trigger search, breakpoint hit-testing, ruler tick math.

## Oscilloscope (audio-rate scope)

A rolling window of recent audio samples, redrawn each frame. Distinct from the existing `scope` widget, which plots a **control-bus** history polyline (`src/host/meters.rs::draw_scope`, history advanced by the front at tick rate) — the oscilloscope reads the audio tap.

- **Data**: a ring of the last N samples per tapped channel; the display window is time-based (e.g. 1–100 ms → `window_ms * sr` samples), resampled to the pixel width (never resolve finer than the screen — per-column min/max when samples-per-px > 2, polyline otherwise; the waveform's regime logic in miniature).
- **Triggering** makes the display stable instead of a rolling blur: search the newest data for a **rising crossing of a trigger level** (default 0.0) and align the drawn window to it. Add **hysteresis** (arm below `level - h`, fire at `level`) so noise near the level doesn't retrigger mid-window; bound the search (one display window); **free-run fallback** when no crossing is found (silence, DC) — draw the newest window. Trigger level and edge are widget props; a "hold" prop freezes the trace.
- **Multi-channel**: overlay traces with per-channel color, or one lane per channel sharing the time axis.

## Phasescope / goniometer

A stereo phase view: plot recent `(L, R)` sample pairs as x/y points/segments — a Lissajous figure.

- **Geometry**: rotate 45° so the axes are mid/side — `x = (L − R)/√2` (side), `y = (L + R)/√2` (mid). A mono (in-phase) signal draws a vertical line; anti-phase draws horizontal; a wide stereo field fills the lozenge. This rotated form is the audio-engineering convention.
- **Persistence**: the figure only reads as a shape with a decaying trail — draw the last K pairs with age-faded alpha (cheap: one polyline/point set per frame from the ring), or accumulate-and-decay if it ever becomes a heavy view. K spans a few display windows.
- **Companion**: a correlation readout (Pearson r of L,R over the window; +1 mono, 0 decorrelated, −1 anti-phase), drawn as a small bar under the field. The metric itself is general audio measurement — a core candidate per the placement analysis.
- **Data**: the same audio tap, two channels.

## Spectrum (live FFT / spectroscope)

One forward FFT per frame over the newest window of tapped samples — a single spectrogram column, drawn as a curve.

- **Math is already shared**: `clausters_core::fft::rfft_magnitudes_into` (power-of-two sizes only — `fft::supports`; wasm-safe) over a `clausters_core::window::Window` Hann window; magnitudes to dB (`20·log10`, floor −120 dB) with the same normalization the `spectrogram::Stft` uses, so the two views agree.
- **Axes**: log frequency by default (the audibly useful one; same display→bin mapping as the spectrogram), dB vertical with an adjustable window (a `db_floor`/`db_ceil` pair, like the spectrogram's contrast controls).
- **Stability**: raw per-frame FFTs flicker. Exponential averaging per bin (`s = α·new + (1−α)·s`, α a prop) and/or a **peak-hold** trace decaying a few dB/s, drawn over the live curve.
- **Resolution**: FFT size is a prop (1024–4096 typical); one curve point per pixel column — interpolate bins to columns, never draw per-bin geometry finer than the screen.

## Editor-grade waveform / spectrogram

Deepening the two existing heavy views to audio-editor quality; all of it is view-side (data and analysis unchanged).

- **Multichannel**: today `channels` de-interleaves channel 0 only. Editor-grade keeps all channels: **stacked lanes** (one sub-rect per channel, shared time axis — the default) or **overlaid** traces with per-channel color. One `Pyramid` per channel; whatever multichannel cache shape is chosen extends `clausters_core::peaks` and its FFI export, so the Python client keeps building the identical cache the host maps (the placement analysis above).
- **LOD crossfade**: switching pyramid levels on zoom pops. Blend the two adjacent levels weighted by the fractional part of `log2(spp / base_bucket)` so the transition is continuous — a per-frame data choice in `upload_geometry`, not a new pipeline.
- **Rulers**: a time axis with adaptive ticks (a 1-2-5 progression over samples/ms/s; labels as `hh:mm:ss.mmm` or samples, a prop) under the view; the spectrogram adds a Hz ruler (decade-spaced 100/1k/10k with log subdivisions, matching the shader's display→bin mapping). Bitmap font + mesh lines — cheap chrome, no GPU work.
- **Selection**: a `[start, len]` range in sample units, drawn as a translucent overlay, dragged with the pointer, emitted as a `/gui_event id "selection" start len` (the existing `"view" start len` event is the model) and settable via `/gui_set`. The selection is what edit-back and transport commands will consume.
- **Playhead**: a vertical line at the playing sample position. Natively read from the shm segment each frame (`sample_clock()` u64 + `sample_rate()` — zero messages); in the browser, from a streamed clock (the `/c_stream` family). Mapping to the view is just `sample → x` through the existing `viewport::View`.
- **Cursor readout**: time/value (waveform) or time/frequency (spectrogram) under the pointer, in the corner — pure math over the view mapping.

## BPF / envelope editor

A drawable break-point function: the cleanest edit-back widget.

- **Model**: breakpoints `(time, value)` plus a per-segment **shape** — exactly the server's envelope segments (`src/dsp/envgen.rs::shape_value`: SC shape numbers — step/linear/exponential/sine/welch/squared/cubed and the custom-curvature shape with its `curve` value). Reusing the same shape math means what the editor draws is what `EnvGen` plays — which requires the placement move above: `shape_value` lives only in the server crate today, and the gui cannot link the server, so the shape math relocates to `clausters-core` (the server delegating), FFI-exposed if the Python client wants to evaluate/plot envelopes client-side.
- **Rendering**: evaluate the segment shape once per pixel column between breakpoints (never finer than the screen); discs for the points, the curve as a polyline — all `Painter` geometry.
- **Interaction** (through `interact.rs`): hit-test **points first** (a device-pixel radius), then segments. Drag a point to move it (clamp times monotonic between neighbors); drag a segment vertically to adjust its curvature; modifier-click adds a point on the curve, another removes. Times are normalized or in seconds (a prop); values live in `[min, max]` with an optional exponential display scale for frequency-like params.
- **Output**: the edited breakpoint list flows back per the edit-back pattern below.

## Edit-back-to-data (the pattern, not a widget)

The heavy views receive data today; edit-back is the reverse direction, and it must not fork the protocol. Two flows, both flat:

- **To the script**: `/gui_event id <tag> <values...>` with the edited structure as flat OSC primitives (a breakpoint list as `t0 v0 shape0 curve0 t1 v1 ...`, keeping ints int and floats float), or as one compact blob for bulk data (the `samples_to_blob` little-endian `f32` layout — the existing client↔host convention). New event *payloads*, not new addresses.
- **To the server**: the binding path — a bound editor forwards its edit straight to the audio server (`/b_setn` for a drawn buffer region; a flat envelope list to whatever server destination consumes it), the widget-value analogue of `/gui_bind`, bypassing the script exactly as a bound knob does.

Two invariants: **the host's mapped resources stay read-only** (the mmap bulk path is a read path by design — writes go through the server or the script, never by the host scribbling on a shared file), and **flat primitives at the boundary** (no structured/nested encoding on the wire beyond the JSON-in-OSC def itself).

## Forward pointers (no firm design yet — do not elaborate)

- **Timeline / DAW view**: tracks with audio and MIDI/OSC sequencing; audio lives in the server so the view reads it from there; the reference shape is an OSC-controllable transport. Builds on selection/playhead above and [[scsynth-osc]] semantics.
- **Score / notation**: Verovio (C++ → wasm/JS) rendering MEI/MusicXML to interactive SVG — off the GPU path entirely, web-first.

Staging for both, and for everything above, lives in `clients/gui/PLAN.md`.

## Conventions

The track's rules apply unchanged (see [[clausters-gui]]): build/test from `clients/gui/`, fmt/clippy clean, English throughout, GPU-free logic unit-tested, the E2E single-invocation rule, and closing work means docs + `GUIA.md` + a commented example, not just code. Windowing/FFT correctness patterns are in [[ugen-dsp]] and [[audio-testing]]; the shm/RT boundaries in [[realtime-audio]]; the Python driving side in [[clausters-python]].
