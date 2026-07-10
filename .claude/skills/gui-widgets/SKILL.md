---
name: gui-widgets
description: Domain knowledge for designing and implementing each Clausters GUI graphical element — the widget extension recipe and data-source table, oscilloscope triggering and the server audio taps, goniometer/phasescope geometry, live FFT spectrum, editor-grade waveform/spectrogram (multichannel, LOD crossfade, rulers, selection/playhead), BPF envelope editing, and the edit-back-to-data pattern. Consult when designing or implementing any new GUI widget or deepening an existing view.
---

# GUI widgets: per-element domain knowledge

This skill is the widget-level companion of [[clausters-gui]] (the track map: protocol, architecture, rendering machinery) — what [[ugen-dsp]] is to the server's UGens. It covers what each graphical element *is*: its algorithm, visual parameters, data source and implementation pattern. `clients/gui/PLAN.md` wins on milestone scoping, staging and design rationale.

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
| Audio tap `tap`, per frame | segment tap rings (ABI v3): `/tap tapIndex bus` routes a bus in; `SharedSegment::tap_read_latest` reads the newest window lock-free | yes | `/tap_stream` → `/tap_data` windows → `StreamedTaps` (`src/host/live.rs`) |

The audio tap is the prerequisite the scopes share, and it exists: a trailing region of the versioned shm segment (ABI v3; `--taps` single-channel rings of `--tap-frames` samples, each a cache-line-aligned cursor + ring). `/tap tapIndex bus` routes any audio bus into a ring — a command flipping engine routing state, not a UGen (where SuperCollider reaches for `ScopeOut2`); the audio thread appends a block per tap per block, RT-safe. The browser sibling is `/tap_stream periodMs frames tapIndex...` → `/tap_data tap endPosition blob` (newest-window snapshots with a stream position; the `/c_stream` posture). Python: `Server.tap` / `Server.stream_taps` (headless capture, no mmap needed).

One principle bounds all host-side analysis: **the host computes only what plotting needs** (peaks, a display FFT). Audio *processing* happens once, in the server — which already owns a spectral chain (`FFT`/`PV_*`/`IFFT`, `src/dsp/spectral.rs`); a client wanting processed audio runs it there (live or NRT) and hands the host the result resource.

## Placement analysis: every general compute function goes to the core

The established rule (peaks, the forward FFT and the windows already took this path): **an algorithm used by more than one Clausters process lives once, in `clausters-core`, and is exposed over `clausters-ffi` when a non-Rust client needs it** — never reimplemented per client. So every new compute function a widget introduces gets an explicit placement analysis, recorded with the milestone:

- **General** — useful to another client (Python plotting/authoring the same data) or to a future server feature (a UGen, an analysis command) → `clausters-core`; add the FFI export if Python (or a later JS client) consumes it.
- **Display-only** — meaningful only against a screen (hit-testing, tick spacing, trigger alignment of a drawn window) → the gui crate.

Done so far: the **correlation metric** and the **Lissajous geometry** (`clausters_core::measure`, FFI v7); the **multichannel peak cache** (`clausters_core::peaks::MultiPyramid` — one CLPK-v2 file for all channels, core-side de-interleave, FFI v8 `clausters_core_peaks_multi_*`, `clausters.gui.peaks_cache_file(channels=N)`); the **perceptual frequency scales** (hz↔mel O'Shaughnessy, hz↔bark Traunmüller — chosen for its exact analytic inverse — `clausters_core::scale`) and the **bar:beat grid reads** (`tempoclock::bar`/`beat_in_bar`, the display complement of `quant_delay`), all FFI v9, Python `_native` wrappers + `Clock.bar`/`beat_in_bar`; the **envelope shape math** (`clausters_core::envshape::shape_value`, the SC shape numbers — relocated from the server, whose `EnvGen` delegates to it, so the BPF editor draws exactly what the server plays; its FFI export deferred until a client evaluates envelopes client-side — the Python leg only maps breakpoints to `Env`). Counter-examples that stay gui-side: trigger search (`src/host/oscil.rs`), the breakpoint model/hit-testing/drag clamps (`src/host/bpf.rs`), ruler tick math (`src/host/ruler.rs`), the LOD crossfade blend. Deferred with its decision recorded: the tap-ring layout/reader stays host-side because Python captures taps over `/tap_stream` — promote it to the core + FFI only when a client needs to map-read taps.

## Oscilloscope (audio-rate scope) — implemented

The audio-rate form of the one `scope` widget: `tap` (or `rate: "audio"`) selects it; without a tap the widget stays the **control-bus** history polyline (`src/host/meters.rs::draw_scope`). The signal logic is `src/host/oscil.rs`, pure and shared by both fronts; `live::update_tap_windows` refreshes each scope's aligned window per tick (native: the segment's rings; browser: the `/tap_data` store), and `meters::draw_wave` draws it.

- **Data**: the newest raw window of the tap, sized `window_ms * sr` (clamped 16..4096, 48 kHz fallback before the rate is known) with a 2× slack for the trigger search; drawn per-column min/max when denser than the pixels, polyline otherwise (never resolve finer than the screen).
- **Triggering** (`oscil::align`) makes the display stable instead of a rolling blur: the **latest** rising crossing of `trigger` that still leaves a full window, re-armed below the level minus a 2%-of-peak-to-peak **hysteresis** so noise riding the level doesn't fire mid-window; **free-run fallback** (the newest window) when no crossing exists — silence, DC. `window_ms`/`trigger`/`hold` are live props (`/gui_set`); `hold` freezes the trace.
- **Multi-channel** (future): overlay traces with per-channel color, or one lane per channel sharing the time axis — today one tap is one single-channel trace; a stereo pair is two taps.

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

## Editor-grade waveform / spectrogram — implemented

The two heavy views at audio-editor quality; all view-side (data and analysis unchanged). The `spectrogram` is a host widget (`WidgetKind::Spectrogram`) with the waveform's source surface (`path`/`cache`/`buffer`/`data`/`blob`); both share an `EditorProps` (ruler, `sample_rate`, selection, playhead) and their chrome rides a second **overlay `Painter` pass** drawn after the GPU views in the one shared `frame::render` (browser-correct by construction; drag interactions are native-only today, display + `/gui_set` everywhere).

- **Multichannel**: `channels` keeps **all** channels — **stacked lanes** (one viewport per channel, shared time axis, divider lines — the default) or **overlaid** traces (`overlay: 1`) with per-channel colors (the waveform shader takes per-vertex color; one vertex range per channel). One `Pyramid` per channel; the cache is one `MultiPyramid` file (see the placement analysis). The spectrogram computes one `Stft` per channel (`frame::stft_lanes`; `spectrogram::hop_capped` raises the hop so long buffers fit the 8192-frame texture). The buffer fetch keeps the interleaved download whole; fronts build the right view by widget lookup at completion.
- **LOD crossfade**: `WaveformData::column` blends the two pyramid levels adjacent to the zoom weighted by `log2(spp / bucket)` clamped 0..1 — continuous exactly where `level_for` switches, so zooming never pops. A per-frame data choice, not a new pipeline.
- **Rulers** (`src/host/ruler.rs`, pure): every axis is a configurable ruler in **its own strip** beside the view (`frame::timeline_body` subtracts the bottom `RULER_H` / left `RULER_W` strips; each axis independently `"off"`), every unit a live `/gui_set` prop — so GUI menus retune them through script glue, zero recompute. The layout is **adaptive by measured labels**: each unit owns a ladder of candidate steps and the smallest one whose *own formatted labels* (`font::width`/`height` at `ruler::RULER_SCALE`, device pixels) fit without collision wins — no fixed pixel constants — and every generator lays out over the **visible sub-range** of its axis (the horizontal `View`, the vertical `y_start`/`y_len` window), so any zoom keeps revealing finer non-colliding rungs (property-tested). Time axis (`ruler`): 1-2-5 `"time"` (`h:mm:ss.mmm`) / `"samples"` (integral steps) / `"beats"` — musical `bar:beat` labels (1-based) on the client's grid (`tempo` beats/sec, `beat_at`, `quant`), a musical ladder (binary beat fractions → beats → bars → 2^k bars, bar lines always majors; `bar`/`beat_in_bar` from `clausters_core::tempoclock`). Waveform amplitude axis (`ruler_y`, default `"norm"`): `"norm"`/`"percent"` (1-2-5 over ±1), `"bits"` (integer sample values at `bit_depth`, plus full-scale endpoints), `"db"` (round dBFS rungs mirrored about the −inf center at `±10^(dB/20)`, thinned outward from the center); positions respect `waveform::AMP_MARGIN`. Spectrogram frequency axis: tick positions follow `freq_scale` = `"linear"`/`"log"`/`"mel"`/`"bark"` (`log_freq` legacy alias) by inverting the shader's exact display→bin mapping — decades when the visible span is wide, a 1-2-5 hertz ladder when zoomed under a decade; the perceptual forms live once in `clausters_core::scale` (`ruler::display_to_hz`/`hz_to_display` is the shared inversion, also the cursor readout's). Bitmap font + mesh lines.
- **Vertical zoom/pan**: each heavy view carries a **vertical display window** `y_start`/`y_len` on `EditorProps` (normalized display units, `0, 1` = full axis; non-positive `y_len` resets) — the waveform maps its geometry through the visible amplitude slice (`WaveformView::set_amp_window`), the spectrogram drives `SpectrogramView::set_freq_window` (display coordinates, so the cursor anchor holds across linear/log/mel/bark). Gestures live on the y-ruler strip: wheel zooms (anchor-preserving via `viewport::zoom_span`), drag pans (absolute from a snapshot), `R` resets both axes; changes emit `/gui_event id "view_y" y_start y_len` and everything is settable via `/gui_set` (the browser's path — see the decision in `docs/decisions.md`).
- **Selection**: `sel_start`/`sel_len` in sample units (`f64` — sample-accurate past f32), drawn as a translucent overlay; **plain drag selects** (Shift+drag pans — the recorded gesture decision), emitting `/gui_event id "selection" start len` live and settable via `/gui_set`. This is what edit-back and transport commands will consume.
- **Playhead**: a line at `sample_clock - playhead_at` (the script anchors `playhead_at` from `/clock` when it starts a synth; negative = off). Natively from the shm header via `BusSource::sample_clock` (zero messages); in the browser from `/clock` polled once per tick. A playhead makes the window animate.
- **Cursor readout**: time+amplitude (waveform) or time+frequency (spectrogram, inverting the display mapping) in the body's corner — pure math over the view mapping.

## BPF / envelope editor — implemented

A drawable break-point function: the cleanest edit-back widget (`WidgetKind::Bpf`; the model in `src/host/bpf.rs`, pure and unit-tested, drawn in the shared `frame::render` base mesh).

- **Model**: breakpoints `(time, value)` plus a per-segment **shape** leaving each point — exactly the server's envelope segments, evaluated through `clausters_core::envshape::shape_value` (SC shape numbers — step/linear/exponential/sine/welch/squared/cubed/hold and the custom-curvature shape with its `curve` value), the very function `EnvGen` plays. Deliberately the future **automation-lane shape**: values in any `[min, max]` (bipolar/unipolar; an on/off lane is the **hold** shape over 0/1 — SC's step jumps to the *target* at segment start, so a step segment draws the next point's value), an optional exponential display scale (`exp`, frequency-like ranges), times in the envelope's own units over `[0, duration]` (0 = fit the last point); the first point may sit past 0 — the first value holds before it (`IEnvGen`'s offset semantics; `points_to_env` realizes it as a leading constant segment, since `EnvGen` has no offset input). The wire form everywhere (props, `/gui_set`, events) is the flat quad list `t v shape curve …`; a `/gui_set` carries it as a JSON-string scalar.
- **Rendering**: evaluate the segment shape once per pixel column between breakpoints (never finer than the screen); discs for the points, the curve as a polyline — all `Painter` geometry.
- **Interaction** (native gestures; the browser keeps display + `/gui_set` parity, the editor-view posture): hit-test **points first** (a device-pixel radius, nearest wins), then the segment under the cursor's x. Drag a point to move it (times clamped monotonic between neighbors); drag a segment vertically to bend its curvature (it becomes the custom shape, the midpoint following the drag); Ctrl+click adds a point at the cursor (inheriting the split segment's shape, then dragging until release), Ctrl+click on a point removes it (never below two).
- **Output**: the edited breakpoint list flows back per the edit-back pattern below. Python: the `bpf` builder plus `env_to_points`/`points_to_env` round-tripping `clausters.defs.Env`; `examples/gui_bpf.py` draws an envelope and hears an `EnvGen` play it.

## Edit-back-to-data (the pattern, not a widget) — established

The heavy views receive data; edit-back is the reverse direction, and it does not fork the protocol. Two flows, both flat (recorded in `docs/decisions.md`):

- **To the script**: `/gui_event id <tag> <values...>` with the edited structure as flat OSC primitives (the `bpf`'s list as `"points" t0 v0 shape0 curve0 t1 v1 ...`, keeping ints int and floats float), or as one compact blob for bulk data (the `samples_to_blob` little-endian `f32` layout — the existing client↔host convention). New event *payloads*, not new addresses.
- **To the server**: the binding path — a bound editor forwards its edit straight to the audio server, the widget-value binding generalized to a flat list (`Binding::message_args`/`Host::forward_args` send `addr prefix… values…`; `/b_setn` for a future drawn buffer region), bypassing the script exactly as a bound knob does.

Two invariants: **the host's mapped resources stay read-only** (the mmap bulk path is a read path by design — writes go through the server or the script, never by the host scribbling on a shared file), and **flat primitives at the boundary** (no structured/nested encoding on the wire beyond the JSON-in-OSC def itself). The later drawn-buffer and automation cases are applications of this pattern, not new designs.

## Forward pointers (no firm design yet — do not elaborate)

- **Timeline / DAW view**: tracks with audio and MIDI/OSC sequencing; audio lives in the server so the view reads it from there; the reference shape is an OSC-controllable transport. Builds on selection/playhead above and [[scsynth-osc]] semantics.
- **Score / notation**: Verovio (C++ → wasm/JS) rendering MEI/MusicXML to interactive SVG — off the GPU path entirely, web-first.

Staging for both, and for everything above, lives in `clients/gui/PLAN.md`.

## Conventions

The track's rules apply unchanged (see [[clausters-gui]]): build/test from `clients/gui/`, fmt/clippy clean, English throughout, GPU-free logic unit-tested, the E2E single-invocation rule, and closing work means a clear commit message + the `clients/gui/PLAN.md` checkbox + docs + a commented example (a `GUIA.md` smoke step only for new visible behavior), not just code. Windowing/FFT correctness patterns are in [[ugen-dsp]] and [[audio-testing]]; the shm/RT boundaries in [[realtime-audio]]; the Python driving side in [[clausters-python]].
