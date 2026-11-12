# Clausters GUI track - implementation plan: the closed tracks

The tracks of [`crates/clausters-gui/PLAN.md`](../../../crates/clausters-gui/PLAN.md) whose every milestone shipped, moved here
whole on 2026-11-12 so that the plan holds what is open. They are the record
of what each track decided and built; a track is not reopened here, and
work that changes what one built is written in the plan.

## Status: foundation in place

The `crates/clausters-gui` crate (an independent workspace, so it can never break the core build) already validates the heavy-rendering path - the `Gx` work below builds the protocol/host around it. The prototype layout:

```
src/lib.rs           module index
src/viewport.rs      View + zoom/pan (unit-tested)
src/peaks.rs         Pyramid peak analysis + cache (unit-tested)
src/spectrogram.rs   Stft analysis + FFT + cache + renderer (unit-tested)
src/spectrogram.wgsl full-screen quad, texture sample, viridis colormap
src/waveform.rs      WaveformData + WaveformRenderer (3 regimes)
src/waveform.wgsl    passthrough shader (columns + line pipelines)
src/view.rs          TimelineView trait (incl. optional char/vertical hooks)
src/bytes.rs         shared little-endian cache (de)serialization
src/native.rs        winit + wgpu harness driving any TimelineView
src/demo.rs          synthetic test signal
src/bin/waveform.rs      waveform binary
src/bin/spectrogram.rs   spectrogram binary
```

`cargo test` exercises the reusable machinery without a GPU: navigation math, peak correctness vs brute force, FFT correctness (impulse -> flat spectrum, cosine -> peak at its bin), STFT frequency localization, and cache round-trips (memory and temp file). `cargo run --bin waveform` and `cargo run --bin spectrogram` open the two windows (need a display and a Vulkan/Metal/DX12/GL adapter). The split is the point: the renderers take a `wgpu::Device`/`Queue` and a target format and own nothing windowing-specific, so `native` is just a driver, swappable for a `<canvas>` WebGPU surface; adding a view is a new module implementing `TimelineView` plus a one-screen binary, no new windowing or input code. This validates the load-bearing claim: the heavy, custom, GPU-bound widgets can be written once against `wgpu`/WGSL and run both natively and on the web, while the *composition* of widgets is a scripted protocol, not compiled Rust.

And on the core/server side, the transport the web direction needs is already done (G1, below).

## Completed milestones (foundation + desktop host, G1-G10)

Each landed with its detail in the git history; the design rationale that outlived them is in this file's design sections above and `docs/decisions.md`.

- ✅ **G1 - WsHub transport**: a WebSocket transport carrying the existing OSC encoding (`src/osc/ws.rs`, `ClientId::Ws`, the `clausters_ws_*` FFI client), one OSC packet per binary frame through `osc::decode_packet`.
- ✅ **G2 - GUI host skeleton** (`clausters-gui` binary): the dual-role process, headless first — links `clausters-core` (not the server crate) for the shared OSC seam and owns a thin UDP transport front; a generic GuiDef node, the widget registry (node-tree shape), the transport-agnostic command loop, and the Python driver `clausters.gui`.
- ✅ **G3 - `/gui_def` + JSON widget tree + a real window**: the typed widget schema (renderer's interpretation of the generic node), a pure layout engine, and the winit-on-main-thread windowed front rendering the heavy `waveform` with zoom/pan; `--headless` for no-display runs.
- ✅ **G4 - Standard control widgets + `/gui_set` + events**: `slider`/`knob`/`number`/`button`/`toggle`/`menu`/`text` over one mesh `Painter` + a bitmap font (no new GPU code); the typed tree as single source of truth so a live `/gui_set` and a user drag are one edit; interaction emits `/gui_event`/`/gui_closed`.
- ✅ **G5 - GUI as a client of the audio server + shared-memory meters/scopes**: a bidirectional client leg; a read-only `SharedSegment` mmapping the `--shm` segment (versioned ABI checked on attach) so `meter`/`scope` read a control bus each frame with zero OSC; server-buffer waveforms fetched over `/buffer_getRange`. The GUI mirrors the segment's binary ABI rather than linking the server crate.
- ✅ **G6 - Bindings (`/gui_bind`): bypass the script**: a `widget id -> Binding` map so a bound widget forwards its value straight to the audio server; the unbound case keeps emitting `/gui_event`; bindings pruned on free/redefine.
- ✅ **G7 - Bulk data path + shared DSP**: heavy data moves between processes via local shared resources (mmap files, exported server buffers), never re-encoded over OSC (the network reads stay the async fallback); the forward FFT (`microfft`) and the peak pyramid live once in `clausters-core`, peaks reachable over the FFI so a client builds the identical cache the host maps.
- ✅ **G8 - Node-tree view + NRT plots**: `nodetree` (a pure parser of `/group_queryTree.reply`, refreshed off `/server_notify`) and `plot` (a lightweight decimated static view), both cheap flat-geometry views added by extension, the server untouched.
- ✅ **G9 - Canvas + shaders**: a `canvas` widget running a script-supplied WGSL shader over its area, params driven from OSC and from a control bus read out of shared memory each frame; a failed compile is caught, not fatal.
- ✅ **G10 - Standalone GuiDef + GraphDef bundles**: a bundle (a named GuiDef beside its GraphDefs) boots as a self-contained instrument via `--standalone <name>` against an embedded server, no language client — GuiDefs persist like the server's defs, a root `boot` list and widget `bind` props make a saved tree self-driving. The `standalone` feature links `clausters` (`embed,realtime`); `ServerLink::{Udp,Embed}`.

## Browser / WebGPU target (G11-G17)

The earlier single milestone framed the web host as "swap the winit surface for a `<canvas>` surface; the renderers run unchanged" - true, but it describes the small part. The host is largely native-only I/O glue against a smaller core of genuinely portable renderers/analysis and *pure* host logic (the typed tree, layout, GuiDef parse, the mesh `Painter`/`font`, the registry, the node-tree/scope models, bindings) that only sat behind the `wasm32` exclusion because it was never separated from the I/O glue. So the browser target is not a rewrite: it is **factor the host along the platform seam, then write one new I/O impl per native coupling, reusing everything else** - the protocol, the decode door, the analysis, the renderers and the whole pure core shared verbatim; only the platform shell (transport, bus source, bulk loader, GPU/surface bring-up) gets a second, web impl.

- ✅ **G11 - Host platform seam** (agnostic core + `Platform` traits, wasm build kept green): `pub mod host` made unconditional; only the I/O shell stays `#[cfg(not(wasm32))]`, behind small traits (`Transport`, `DefStore`, `BulkLoader`, `BusSource`). `check-wasm.sh` is the build gate so no later milestone re-couples the core to native I/O. **Decision:** the browser host always talks to a *separate* audio server over WebSocket (no in-process engine in the browser) — a scope boundary the "in-browser audio engine" track below relaxes, and the `Transport`/`ServerLink` seam is shaped to take the wasm-`Embed` variant without a protocol change.
- ✅ **G12 - Web surface**: `<canvas>` WebGPU + async GPU bring-up (no `block_on`) + render loop; the per-window render factored into a shared `host::frame::render` so the browser is pixel-faithful by construction. `web/build.sh` + `web/index.html`.
- ✅ **G13 - Web transport**: drive the browser host live over WebSocket — the browser front runs the real `Host` (shared protocol dispatch + a shared `host::interact`); a wasm-bindgen `GuiBridge` (`feed`/`def`/`poll`/`connect_server`) is the in-page binding surface, and `ServerLink::Ws` gives a bound widget the server bypass. The full TypeScript client stays a separate `clients/web` track — the harness here only tested the host; its W2 has since grown the product driver (`GuiHost` + the GuiDef builders) over this same binding surface.
- ✅ **G14 - Browser meters/scopes**: control buses over the wire — the server grew `/bus_stream periodMs bus...` (one subscription per client, transport-agnostic), feeding a message-based `BusSource` (`StreamedBuses`) so the same meter/scope drawing runs from streamed values. Fixed a `/server_notify`/stream leak on WS/TCP disconnect.
- ✅ **G15 - Browser bulk data**: fetch/blob and the `/buffer_getRange` fallback — the native buffer-fetch state machine extracted into a pure `host::fetch`; on the web `path`/`cache` resolve as URLs (`window.fetch`), the pyramid built in wasm from `clausters_core::peaks`. Fetch fills the `BulkLoader` seam through the event loop (it cannot block).
- ✅ **G16 - Packaging and native/browser parity**: `web/build.sh` + the pinned wasm-bindgen CLI (no wasm-pack/trunk); `web/index.html` the documented harness and `web/parity.html` the scripted headless parity pass; browser quick-start in `docs/clients.md`.
- ✅ **G17 - WebGL2 fallback**: browser reach where WebGPU is disabled — the wasm build enables wgpu's `webgl` backend alongside `webgpu` and picks per-runtime via `new_instance_with_webgpu_detection` (WebGPU where truly supported, WebGL2 otherwise, ~99% reach). Cheap because the crate already avoids compute shaders/storage buffers, runs FFT/peaks on the CPU, and lets naga translate WGSL to GLSL ES.

## Widget deepening (G18-G21b): scopes, editor-grade views, edit-back

With the host complete on both platforms, the next arc deepens the **graphical elements themselves** - the catalog entries once marked *future* (`scope` as a real oscilloscope, `phasescope`, `spectrum`, `bpf`) plus the editor-grade refinement of the two heavy views and the edit-back-to-data pattern. The ordering criterion: every one of these is **driven from the Python client as it stands** (new `clausters.gui` builders over the unchanged `/gui_*` protocol - widgets are added by extension, never by protocol change), so each milestone lists its Python leg explicitly. The per-widget domain knowledge (trigger algorithms, goniometer geometry, LOD crossfade, envelope-shape math) lives in the `gui-widgets` skill; this plan stages the work. Packaging (Tauri) and the in-browser audio engine stay deliberately last - see "Future directions".

**Recurring analysis (every milestone in this arc, the G7b rule):** each new compute function gets an explicit placement decision, recorded with the milestone - **general** (useful to another client from Python, or to a future server feature) goes to `clausters-core`, with a `clausters-ffi` export when a non-Rust client consumes it; **display-only** (hit-testing, tick spacing, trigger alignment of a drawn window) stays in the gui crate. Peaks, the forward FFT and the windows already took the core path; of the known candidates below, the correlation metric (G19), the multichannel peak cache (G20) and the envelope shape math (G21, `clausters_core::envshape` with the server's `EnvGen` delegating; its FFI export deferred until a client evaluates envelopes client-side — the tap-reader precedent) have since landed core-side, and the G20c tick/label layout was analyzed display-only (gui-side) — leaving the per-bucket RMS descriptor (G20e, since folded into the A track's A1/A2 along with the rest of the loudness family) and the constant-Q transform (G20f, experimental).

- ✅ **G18 - Server audio tap + a real oscilloscope**: tap rings in the versioned `--shm` segment (ABI v2 → v3), created by a `/bus_tap tapIndex bus` command (not a UGen), with the browser sibling `/bus_tapStream` → `/bus_tapStream.reply` (a windowed blob per period). The `scope` gained an audio-rate mode (`tap`/`rate:"audio"`) with a hysteresis level trigger. The trigger search is display-only (gui-side); the ring reader's core move is deferred until a client needs to map-read taps.
- ✅ **G19 - Phasescope + live spectrum**: two tap-consuming views added by extension — `phasescope` (the 45°-rotated Lissajous with an age-faded trail + a Pearson-r bar) and `spectrum` (one forward FFT per frame, dB with adjustable window, log/linear axis, averaging + peak-hold). The new general functions — correlation and Lissajous geometry — landed in `clausters_core::measure` with FFI exports (ABI v6 → v7); display stays gui-side. Closes the catalog's four *future* scope entries with G18.
- ✅ **G20 - Editor-grade waveform + spectrogram**: the two heavy views at editor depth and the `spectrogram` finally host-wired. **Multichannel** as one cache file (`MultiPyramid`, CLPK v2, FFI CORE_ABI v7 → v8) so the client builds the identical cache; lanes stacked or overlaid. **LOD crossfade** (blend the two adjacent pyramid levels). **Rulers** (adaptive 1-2-5 time axis; the spectrogram's Hz ruler by inverting the shader's display→bin mapping). **Selection + playhead + readout** as shared `EditorProps` on both kinds, drawn in a second overlay pass through the one `frame::render` (plain drag selects, Shift+drag pans).
- ✅ **G20b - Configurable rulers: units per axis + side strips**: every axis is a configurable ruler in **its own strip** (bottom for x, left for y, independently optional). Time axis gains `"beats"` (`bar:beat` on the client's grid); the waveform amplitude axis gains `"db"`/`"bits"`/`"percent"`; `log_freq` grew into `freq_scale` = linear/log/mel/bark (new shader mappings). All live `/gui_set`. **Placement:** hertz↔mel/bark → `clausters_core::scale`, the bar/beat reads → `clausters_core::tempoclock`, six exported over FFI (CORE_ABI v8 → v9); tick spacing and the ladders stay gui-side.
- ✅ **G20c - Editor views: adaptive ruler layout + vertical zoom/pan**: the fixed pixel constants in `host/ruler.rs` gave way to a layout that **measures its labels** (`font::width`/`height` at the ruler scale, device pixels): each unit's ladder (1-2-5 decimal, musical, dB rungs, decades) is tried against its *own* formatted labels and the smallest colliding-free step wins (property test: no two labels overlap for any window/zoom/unit/strip size). Every tick generator now lays out over the **visible sub-range** of its axis, and the heavy views gained that vertical window: `y_start`/`y_len` in normalized display units on `EditorProps` (live `/gui_set`, `"view_y"` events; non-positive `y_len` resets) — the waveform maps its geometry through the visible amplitude slice, the host `spectrogram` drives `SpectrogramView`'s display-coordinate frequency window, so the cursor anchor holds across linear/log/mel/bark and the browser keeps display + `/gui_set` parity while gestures stay native (wheel on the y-ruler strip zooms, dragging it pans, `R` resets — see `docs/decisions.md`). **Placement:** tick spacing and label fitting are pixel concerns — display-only, gui-side.

- ✅ **G20d - Linked views: shared navigation groups, composable editor items**: the per-widget navigation/selection/playhead state (formerly a per-slot `View` in each front) extracted into one **group model owned by the host core** (`host/timeline.rs`: horizontal `View` + selection + playhead anchor per `GroupKey`); the GPU slots keep only pixels, per-widget state keeps only the y axis (G20c). Grouping is an **explicit `link` (int) prop** (decision recorded: an item may want *unlinked* views of one file, and a link may span sources), live via `/gui_set` (negative = unlink, carrying the current view along; joining adopts the group). A gesture on any member mutates the group and every member window repaints; `/gui_set` of `view_start`/`view_len`/`sel_*`/`playhead_at` on any member applies group-wide (the interception lives in the core `on_set`, so the browser gets it for free); `"view"`/`"selection"` events emit **once**, carrying the **interacted member's id** (decision: a designated group id is not a widget id and would break the `/gui_event` shape). Selection/playhead were **mirrored** group→members' `EditorProps` (single writer, no drift) — retired in E3, which reads the group directly; shared chrome stays plain composition - a linked lane under a ruled sibling sets `ruler:"off"`, no auto strip, no new container. The group navigates in **session-timeline units** with its length aggregated (max) over member extents and membership spanning windows - deliberately the seat of the future multitrack view, whose clip items become members with a per-member *placement* (offset), the one designed extension point (all members at offset 0 today). Placement: navigation/group state is protocol/display logic - gui-side host core, nothing for `clausters-core`. Python: `link=` on both heavy builders; `examples/views/linked.py` composes a waveform + spectrogram of one NRT render in lockstep, single event stream, live re-linking. See `docs/decisions.md`.

- ✅ **G21 - BPF envelope editor + the edit-back-to-data pattern**: the first widget that *writes data back*, and the pattern it establishes (folding in the former "Edit-back-to-data" and "Automation / BPF view" future directions). The **`bpf` widget**: breakpoints `(time, value)` plus a per-segment shape/curve **using the server's own envelope shape numbers**, evaluated once per pixel column (painter geometry); interaction hit-tests points before segments — drag to move (times clamped monotonic), drag a segment vertically for curvature, Ctrl+click adds a point / removes the one under it; the widget is deliberately the future automation-lane shape — any `[min, max]` range (bipolar/unipolar; an on/off lane is the hold shape — SC's step jumps to the target at segment start) with an optional exponential display scale (`exp`), times over `[0, duration]` (0 = fit). **Placement (recorded):** the shape evaluation relocated to `clausters_core::envshape` with the server's `EnvGen` delegating to it (the FFT move); its FFI export is **deferred** until a client actually evaluates envelopes client-side (the tap-reader precedent — the Python leg maps breakpoints to `Env`, it does not evaluate curves). Breakpoint model/hit-testing/drag stay gui-side (`host/bpf.rs`, pure and unit-tested). **The edit-back pattern (recorded):** edited data flows back **to the script** as `/gui_event <id> "points" <t v shape curve ...>` — flat OSC primitives, ints kept int, new event *payloads* rather than new addresses (bulk data would ride one blob in the `samples_to_blob` layout) — and **to the server** through the binding path, `Host::forward_args`/`Binding::message_args` generalizing the bound widget-value forward to a flat list after the fixed prefix. The host's mapped resources stay **read-only**; editing gestures are native-only today with display + `/gui_set` parity in the browser (the editor-view posture). Python: the `bpf` builder plus `env_to_points`/`points_to_env` mapping to/from `clausters.defs.Env`; `examples/editors/bpf.py` draws an envelope and hears an `EnvGen` play it *(the example is `examples/editors/edit_env.py` since 2026-09-20, and it drives the same widget through `edit()`)*. See `docs/decisions.md`.

- ✅ **G21b - A meter, read as a meter** *(done 2026-09-15, from the user's brief: a general, customizable meter -- the peak that is not lost, a refresh no faster than the window, a held maximum, a clipping indication that stays, and a decibel scale as precise as the resolution the piece is rendered at)*. The `meter` widget was the one element of the catalog that had never been deepened: a linear column over `min..max`, no mark, no scale, no lamp, while the multitrack's own strip had had the decibels and the held peak since `G35.14`. One meter in the host, and the widget is now the general one the strip is a narrow case of.

  **Four rules make a bar a meter, and all four are the shared crate's.** The peak is *the block's* -- the engine walks every sample of every block and publishes it held with a decay, so a reader at a screen's rate (a frame is a dozen blocks) catches the transient; the refresh is the window's, one atomic load per channel per tick and no message at all; a peak is **held** (`measure::Ballistics`, ticked on the element's own `tick` and not on the repaint, so a window that redraws twice does not fall twice); and the scale is **decibels** down to a floor the reader states -- `floor_db`, or the dynamic range of a resolution (`measure::floor_db_for_bits`: 16 bits is -96 dB, 24 is -144, and a 32-bit float takes 24, its significand). `ruler` puts the ladder on either side, `channels` meters adjacent buses one column each, and a control bus carrying something that is not an amplitude keeps its plain `min..max` axis -- which is the same statement the decibel default makes, not an exception to it.

  **The tick grew a `dt`.** A tick runs once per animation frame and an animation frame is not a fixed period, so anything advancing *in time* had to be told how much passed rather than assume a rate. `Live::dt` is the front's measurement (`live::TickClock`, capped so a hidden window catches up instead of falling to silence) and it is wall time, because what it paces is a picture a person is reading.

  **Clipping is a run, and the lamp is latched.** A lone sample at full scale is not clipping -- the engine is floating point, so nothing is destroyed until a conversion or a converter -- and what a red lamp reports is a *flattened* waveform, whose signature is a run of samples at full scale (`measure::CLIP_RUN`, three, the field's number; one is the pessimistic meter). Only something walking samples can see a run, so the count is the server's: the new **`ClipCount`** UGen counts overs onto a control bus and the widget differences it, lighting on a rise and clearing on a fall -- which is what a new transport pass looks like from here. A **click** puts the lamps out and reports nothing, the latch being the reader's state and not the piece's; the number in a lit lamp is how far **past** full scale the signal went, which is the question a lit lamp raises when nothing has been lost yet. With no counting bus the lamp lights on the level alone reaching the top: exact for exceeding full scale, blind to how many samples did, and the reason the UGen exists.

  **Three marks, not two** *(after the first eye pass, 2026-09-15)*. The colour was a single ramp from the alignment level (-18 dBFS) to the hot end (-6), so a column at -12 -- a column **using its headroom**, which is the thing the colour exists to say -- read as green with a cast on it. `measure::METER_AMBER_DB` is the mark that was missing: the amber is reached at -12 and **held** to the hot end, so the bands are bands and only the edges between them are ramps.

  **Three defects one screenshot showed at once** *(the second eye pass, same day)*. The **red was still the end of a ramp**, so a column read amber at -6 and only reached red at full scale -- the opposite of what the mark means -- and it is a band now, leaving the green-to-amber edge as the one gradient on the scale. The **reading and the lamp's number were both drawn at the top**, so on a narrow meter they landed on each other; the reading has a strip of its own at the bottom now, which is also where a meter's number belongs, the loud end of the scale being the end the marks are at. And the live views' **caption was never truncated** (`controls::label_strip` had been since it was written, `meters::label_strip` had not), so two narrow meters side by side ran their labels across each other into one unreadable word -- a general defect of every live view with a label longer than its cell, not the meter's.

  **And the example was showing the scale instead of the meter** *(same pass)*. A gain ramped slowly up and down visits every colour and demonstrates none of the ballistics: an attack, a hold and a fall are only legible against a **transient**. It strikes the tone every two seconds now, from a quiet bed, with every fourth hit past full scale -- so the jump, the mark standing still for a second and a half, the fall at twenty decibels a second and the lamps are all visible in one pass. Both examples, call for call.

  **Two more the third screenshot showed** *(third eye pass, same day)*. The reading at the foot **reserved its height before the ladder took its side**, so it was centred over columns *and* ladder and landed under the ladder's own last label; the height is still reserved first (the ticks and the columns have to end where the reading begins) but the number is written over the **columns**, once the side is settled. And the ladder's last rung before the floor was dropped at a fixed half-step, which at the bottom of a 60 dB strip left `-57` pressed against `-60`: that pair is the only one whose gap is not the step, so it is now dropped when its label would crowd the floor's, measured in pixels.

  **And the example was striking too often to show what it was demonstrating**: at a hit every two seconds the mark had not finished falling when the next one landed, so a hold of a second and a half read as a mark that never falls. The hits are three and a half seconds apart, which is the whole ballistic -- the jump, the column starting down at once, the mark waiting, and the mark walking down after it.

  **And the numbers are one prop** *(same pass)*. `readout` began as the lamp's figure alone and covers **every number the meter writes** -- that figure and the level at its foot -- because the case it serves is one case: with `ruler` off as well it is a **bare column**, which is what the multitrack's own header strip is, and which the user named as the example. A meter with neither is not a degraded meter; it is the meter a narrow band has room for.

  **And the lamp is a proportion** *(same pass)*. It measured one line of caption text however tall the meter was, so a tall meter's mark was a hairline nobody notices and a short one lost a quarter of its scale to it. It takes `LAMP_SHARE` of the height now, floored at a line of text and capped at a quarter, and its number is scaled to the strip it is written across -- quantized the way every scale in this host is, the font being a bitmap. `readout` turns the figure off and keeps the lamps, which is what a mixer's strip wants.

  **And the number left the lamp** *(same pass)*. It was written inside each column's lamp, which a narrow meter has no room for -- so the two requests, *thinner* and *show the number*, were one defect: which channel was flattened is worth a lamp of its own, but **how far past full scale** is a question about the signal, so the worst of them is written once across the whole strip, on the plate the host already uses for a line drawn over a picture. The column's width stopped deciding whether the number is readable.

  **And a meter is thin** *(same pass)*. It had no natural size, so it took whatever cell it was placed in, while a level is read by how far up it goes and a wider column says nothing more. It asks for one narrow column per channel plus its ladder's strip now, and stays elastic on the height; a wider one is asked for with `w`, the placement prop every widget already has.

  **Placement (the G7b rule):** the run rule, the latch and the floor-per-resolution are `clausters-core` (`measure::ClipCount`, `ClipLatch`, `floor_db_for_bits`, `METER_FALL_DB` -- which the engine's own `LEVEL_RELEASE_DB_PER_SEC` now reads, so a bus's published fall and a drawn mark's fall are one number). The lamp's geometry, the ladder's step and the colour roles are the host's. The whole meter is drawn by `graphics::meters::draw_meter_view` over the one `draw_column` this host has, so the widget and the track strip cannot drift.

  **And it found the reading it exists to prevent.** Both measuring UGens walked their **own output**, which at `kr` -- the rate a meter runs at, one number a block being exactly what a control bus carries -- is one sample in sixty-four. So `Meter` on a control bus reported the first sample of each block and called it the peak, and `ClipCount` counted a different thing entirely: measured against theory, a 330 Hz sine at 1.4 reported 90 overs a second where 660 crests are flattened. They walk the input's samples now (an audio-rate input reaches a `kr` UGen as the whole block; only the output is one sample), which is 657. Held by `a_control_rate_meter_reads_the_whole_block` and `a_control_rate_count_sees_the_run`.

  Both clients got the widget's props and the `clip_count`/`clipCount` builder in the same commit, `clausters.defs` gained the `meter` UGen it had never exported, and the example is `views/vumeter.py` with its page twin: the same two meters over the same two buses on two floors -- the mixing strip and a 24-bit render -- with a ramp that pushes the tone past unity so the lamps light by themselves.

### G20e - RMS waveform layer — **superseded by the A track (A1 + A2)**

*Superseded 2026-08-12: the milestone's two halves are A1 (the per-bucket mean square in the pyramid, with its cache bump) and A2 (the drawn body, generalized into the signal element's `measure` prop and into the first layer of the stack). Its decisions were kept verbatim there — energy-exact aggregation, and the crossfade rather than a hard-off below `base_bucket`. The scope below stands as the record of what was designed here.*

The third classic waveform layer: the RMS body drawn inside the min/max envelope, the editor convention that separates dense peaks from perceived level.

**Scope:**

- **Placement analysis (the G7b rule, decided up front):** per-bucket RMS is a **general audio descriptor** - the Python client analyzing arrays/chunks headlessly and a future server analysis UGen/command both plausibly consume it, exactly the user-noted case - so it lands in `clausters-core`: an `rms` measure over slices/chunks (beside `measure::correlation`) and the peak pyramid growing per-bucket energy - CLPK v2 → v3 (v1/v2 caches still parse), the FFI exports bumped so `clausters.gui.peaks_cache_file` keeps building the identical cache the host maps. Only the drawing (the body layer, its color, the regime blend) stays gui-side.
- **Energy-exact aggregation (decision - record it):** store mean-square (or sum-of-squares) per bucket internally and expose RMS, so higher pyramid levels combine exactly (the RMS of a union is the root of the mean of the mean-squares) and the LOD crossfade blends without bias.
- **Rendering:** a second column layer inside the min/max envelope (±rms about zero, the editor convention) with its own color, LOD-crossfaded exactly like min/max; an `rms` prop toggling it live via `/gui_set`. **Decision (record it):** the zoomed-in Line regime (raw samples on screen) - fade the body out with the existing crossfade weight vs. hard-off below `base_bucket`.
- Python: the cache builder and `waveform` builder grow the RMS surface; the descriptor exposed through the `_native` wrappers for headless use; an example (or `editor.py` extended).

**Acceptance:** the waveform draws the RMS body inside the min/max envelope with no pop across LOD switches; a Python-built v3 cache and a host-built pyramid draw identically; core RMS matches brute force on random chunks; v1/v2 caches still load.

### G20f - Constant-Q spectrogram - **experimental**

An exploratory alternative analysis for the `spectrogram` widget: a constant-Q transform (geometrically spaced center frequencies, window length inversely proportional to frequency, so Q = f/Δf is constant) gives uniform per-octave resolution - the musically faithful time-frequency picture the log-scaled STFT only approximates (an STFT's low bins are linearly spaced, so the log display stretches a handful of coarse bins across the bottom octaves). Experimental: it lands behind a prop, is evaluated for quality/cost against the STFT views, and is promoted or dropped on that evidence - the STFT stays the default either way.

**Scope:**

- **The analysis**: a CQT over the same sources, parameterized by `bins_per_octave` (12-48 typical), `fmin` and the source's Nyquist. **Decision (record it):** the algorithm - the Brown/Puckette FFT-based sparse-kernel CQT vs. the multi-rate hybrid (one STFT per octave, window/hop halving per octave, resampled or restacked) - decided by measured compute cost on editor-length files and by wasm friendliness (CPU-only, no threads assumed in the browser). Long-file capping follows the existing posture (`hop_capped`'s role) so the magnitude grid fits the 8192-wide texture.
- **Placement analysis (the G7b rule):** the transform is general audio analysis (Python chunk analysis and any future server spectral feature are plausible consumers, exactly like the forward FFT) - it lands in `clausters-core` beside `fft`/`window`, with the `Stft` pipeline shape (compute → magnitude grid → cache/texture) and an FFI export **deferred until a client actually consumes it** (the tap-reader precedent: record the decision, move on evidence). Display mapping, ruler ticks and props stay gui-side.
- **The widget surface**: a prop on the existing `spectrogram` kind (`analysis: "stft"` default / `"cqt"` plus the CQT params), extension by props - no new widget kind, no protocol change. Rendering reuses the full-screen-quad pipeline unchanged: the magnitude grid uploads as the same 2D texture; the rows are already geometrically spaced, so the shader's display→bin mapping is the identity on the log axis (and `freq_scale` interacts accordingly - **decision:** which scales stay meaningful under CQT rows, likely log/identity only). The frequency ruler inverts the bin→Hz law `f(k) = fmin·2^(k/bpo)`; with bins on the equal-tempered grid a note-name unit (`"note"`: C4, A4...) becomes nearly free - **decision (record it):** whether the note ruler ships with this milestone or waits for promotion.
- Python: the `spectrogram` builder grows the analysis props; an example comparing STFT and CQT views of the same harmonic element.

**Acceptance (experimental bar):** a CQT view of a bass-register arpeggio resolves the individual partials the log-STFT smears, with correct ruler/readout frequencies across zoom (G20c) and linked navigation (G20d); compute cost on an editor-length file is measured and recorded; the promote-or-drop decision is written down with the evidence.

## Multitrack timeline view (G22): the DAW-style track editor

The "DAW / timeline view" future direction, now firming up. It builds on exactly the seat G20d designed and named: the navigation group already navigates in **timeline sample units** with its length aggregated over member extents, and its own doc block declared "a clip item there is a member with a *placement* (offset) on the same shared timeline — G20d ships every member at offset 0; the per-member placement is the one designed extension point." G22 activates that point and draws on top of it, reusing the group model, the `EditorProps` chrome, the second overlay pass of `host::frame::render`, the `host::interact` hit-test, the heavy views as clip bodies, and the G21 edit-back pattern — no protocol change (a widget is added by extension). Staged in incremental, separately-shippable parts, each with its Python leg. **Placement analysis (the G7b rule, decided up front):** clip/track geometry, the offset arithmetic, hit-testing and snapping are protocol/display logic — **gui-side** (host core + `interact`), nothing for `clausters-core`, the same call G20d made for navigation state.

- ✅ **G22a - Per-member placement (the offset)**: the extension point G20d declared, activated as pure host-core plumbing. An `offset` (timeline samples) on `EditorProps` — per-member (unlike the group-wide `link`/`sel_*`/`view_*`), so a view's own data sample 0 sits at timeline position `offset`; a placed member lengthens its group timeline to `offset + extent` (`timeline_total` aggregates the *placed* extents). A change routes through the group model (`set_timeline_offset`: writes the one widget's offset, re-clamps the shared window with the same full-follows-growth rule as `set_timeline_total`, repaints every member) and rides `/gui_set offset` as a timeline key. The GPU body upload draws through the **placed** window (`frame::placed_nav` shifts the group window by `-offset`); the time ruler and the selection/playhead overlay keep the timeline-unit window. At `offset = 0` (the un-placed default) everything is the identity, so no existing view regresses. Tests: the placement lengthens the group and re-clamps a zoomed window; `placed_nav` shifts the body window; the offset parses/clamps.
- ✅ **G22b - `track`/`clip` widgets + the graphic-unit overlay** (the visible milestone): a new `WidgetKind::Track` lane container carrying `clip` children, and `WidgetKind::Clip` — a placed rectangle spanning `[offset, offset + dur]` in timeline sample units (the graphic unit, length = duration) with an optional inline decimated body. Drawn by the new `host::track` module (pure over a `Mesh`, unit-tested without a GPU, the static-view posture): a left header names the track, the lane field frames it, and each clip is a framed rectangle with its body and label. The window's tracks share **one time axis** (aligned lanes) — the frame renderer computes the common span (the longest clip end) and maps every lane through the same `View`, so a clip at a given offset lines up across tracks. `clip_x_range` is the pure placement helper (offset/dur → clamped x range). Scalar vertical (a `track` of events → piano-roll) defers to G22d; shared-group zoom/pan of the axis (vs. today's full-span static view) is a later refinement. Python: `track`/`clip` builders; `examples/editors/multitrack.py` composes three aligned lanes.
- ✅ **G22c - Clip edit-back (drag/resize)**: `host::interact` grows clip hit-testing on the overlay (`clip_hit` reconstructs the same `track::window_nav` the renderer drew, so pixels map identically) — drag the body → move `offset`; drag an edge → resize `dur` (edges before body, as `bpf` prioritizes points over segments), each snapped to the track's `snap` grid (`interact::snap`; 0 = whole samples). The native front grows a `Drag::Clip` (a press-time snapshot so a clamped edge never drifts); `clip_set` writes the placement, `emit_clip` delivers it as `/gui_event <id> "clip" <offset> <dur>` — flat OSC primitives, a new event *payload* not a new address, through `Host::forward_args`, exactly the G21 pattern; the widget tree stays the source of truth. Editing is native-only with display + `/gui_set` parity in the browser (the `bpf` posture); the bound path (forward to the server to re-schedule) and the arrangement re-render are the Fase 3 driver's job. Tests: `clip_hit` finds the clip and body/edge part, `snap`/`clip_part` are pure, `clip_set`/`clip_event_args` move and report. Python: `snap=` on the `track` builder; `examples/editors/multitrack.py` prints the `"clip"` edit-back stream.
- ✅ **G22d - Events track → piano-roll**: the scalar-vertical clip body, reusing the clip placement/edit-back whole. A `clip` with `notes` (a flat `[start, dur, pitch, …]` list, the `bpf`-points convention) draws a **piano-roll** instead of a waveform body: each note a bar placed in time on the same shared axis (`track::draw_piano_roll`, so the whole roll moves when the clip is dragged) and in pitch over the clip's `[min, max]` range. New geometry, deliberately the reused-body sibling of `draw_clip_body` — the clip is the graphic unit either way, so move/resize/edit-back and the aligned time axis come for free. Pitch→y is linear here; a note-name ruler would read `clausters_core::scale` (G20b), deferred until it earns its place. Python: `notes=` on the `clip` builder (an iterable of `(start, dur, pitch)`); `multitrack.py`'s lead track is a piano-roll. Tests: a clip parses its notes (dropping a trailing partial triple); a piano-roll clip draws its note geometry.

- ✅ **G22e - Lane playhead + time ruler**: the lane grows the time chrome the heavy views already had, so a composition can be *watched* playing over its clips. A `track` carries an `EditorProps` of its own — the same struct, parsing and `/gui_set` keys the timeline views use, of which a lane reads the time `ruler` (a strip reserved under the lane by `track::lane_body`, **off** by default so an un-rulered multitrack keeps its layout; the tick math is `frame::draw_time_ruler`, unchanged) and `playhead_at` (the sample-clock value at timeline position 0, so the line lands at `sample_clock - playhead_at` on the shared clip axis — `track::playhead_x`, drawn into the overlay over the clips). A lane joined **no** navigation group then (its axis was the window's clip span), so those keys applied to the widget itself rather than routing through the group model — G22g made every lane a group member, and they route, and the animation demand now sees a lane, so the window animates. The hit-test reconstructs the same ruler-reserved body, so a clip is grabbed by the pixels it is drawn on. Python: `ruler=`/`playhead_at=`/`tempo=`/`sample_rate=` on the `track` builder; `multitrack.py` rules its bottom lane in beats and rolls the playhead from `/clock_query`. Tests: the ruler strip is reserved (and not when off), `playhead_x` maps the clock onto the axis, a lane parses and live-`/gui_set`s its chrome.
- ✅ **G22f - Long takes in a clip (the bulk body)**: a real take is minutes long, so it must never ride the wire as JSON — a `clip` now resolves its waveform body exactly the ways the heavy `waveform` view does, in the same precedence order: `cache` (a prebuilt peak-pyramid file), `path` (raw little-endian `f32`, mapped and its pyramid built/cached on load), `buffer` (a server buffer over the client leg's `/buffer_query` + chunked `/buffer_getRange`), then inline `data`/`blob` for a sketch. **Placement analysis (the G7b rule):** no new decimator — the min/max summary *is* `clausters_core::peaks` (the pyramid every client already builds), read through `WaveformData::column`, the same LOD source and crossfade the waveform view draws from; the host only maps the resource, through the existing `BulkLoader` seam (`MmapLoader` natively, `fetch` in the browser). A loaded take lands in the **tree** (like a plot's samples), not on the GPU: a lane body is flat geometry decimated to the clip's pixel width, so a minutes-long clip costs a screen's worth of columns — the one graphics rule, again. Python: `path=`/`cache=`/`buffer=`/`channels=`/`base_bucket=` on the `clip` builder; `multitrack.py`'s audio lanes map real take files. Tests: a clip parses its bulk sources; a loaded take draws decimated to the clip's width, not to its sample count.

- ✅ **G22g - Zoom/pan of the shared axis**: the multitrack's time axis stops being a static full span and becomes navigable — and it does so by **joining the navigation group model**, not by growing an axis of its own. A `track` is now a timeline widget (`is_timeline`), so it is a group member like any heavy view: the lanes of a window are **linked by default** (keyed by the window root, since aligned lanes are the multitrack's whole promise; an explicit `link` splits them or joins lanes *across* windows), the group's length aggregates each lane's extent — the end of its last clip, registered by `sync_track_totals` on def, on a `/gui_set` of a clip and after a drag, so pulling a clip past the end lengthens the timeline — and the group's window is what the renderer and the hit-test both draw through, so a zoomed axis grabs clips where it looks. The gestures are the ones the editor already had: wheel zooms (anchored at the cursor, over the lane body), Shift+drag pans (plain drag still grabs a clip), `r` resets, and `/gui_set view_start`/`view_len` navigates from a script — all group-wide. Tests: the lanes of a window share one axis spanning the composition, zoom/pan on one lane moves all of them, a clip moved past the end lengthens the shared axis.

- ✅ **G22h - The automation clip (a curve as the body, edited in place)**: the third body of the same graphic unit, and the sibling G22d's piano-roll promised. A `clip` with `points` draws a **break-point curve** instead of a waveform or notes — the `bpf` editor's model and shape math (`clausters_core::envshape`, the very function the server's `EnvGen` plays, so what is drawn is what is heard), placed on a lane: times are timeline units relative to the clip, values live over `[min, max]` (with the `exp` display scale), and the curve maps through the shared axis exactly as the piano-roll's notes do, so it moves with the clip and holds up under zoom. It is **editable in place**: a break-point wins over the clip body (as it wins over a segment in the `bpf` view), dragging it moves it, Ctrl+click adds one or removes the one under the cursor, and the edit leaves as the **same flat `"points"` payload** the `bpf` view sends — one payload, so an `Automation` consumes an edit without caring which view drew it. The reuse is structural, not by copy: `bpf::place_point`/`insert_point` were extracted as the mapping-free core both the widget-local and the clip-placed editors call, and `track::clip_draw` became the one place the typed tree becomes a drawable clip, so renderer and hit-test cannot disagree. Python: `points=`/`exp=` on the `clip` builder; `Editor` renders any element wrapping a `clausters.seq.Automation` as a curve clip and writes a `"points"` edit back onto its `Env` (times converted back to beats) — the curve you draw is the curve the next rendering plays; `composer.py` sweeps a filter from a lane. Tests: the curve draws instead of a body, a point is hit where it is drawn and maps back through the axis, and the edit-back lands on the automation in beats.

- ✅ **G22i - Locate: the multitrack's cursor**: the one gesture a timeline view cannot do without. A press on a lane's **time ruler** — or on its empty space, where no clip is — locates the transport: the position under the cursor becomes the group's **static cursor** (drawn at once on every lane, so the click lands where you see it) and leaves as `/gui_event <id> "locate" <position>`, which the script seeks its playhead to. That needed a second kind of playhead, and the distinction is the point: `playhead_at` **anchors the line to the engine clock** so it sweeps with the audio, while the new `playhead` prop is a **cursor that stands still** — a located, stopped transport has a position, and it must not drift with the clock. Both are group-wide keys (one cursor across the lanes) and both are drawn by the lanes and the heavy views alike. Python: `Editor` grows the transport this implies — `position`, `play`/`pause`/`stop`/`locate` — and consumes the `"locate"` event as a *transport* action, not an edit (it does not mark the arrangement changed). Seeking while playing re-renders from there, so a seek also picks up any pending edit.

- ✅ **G22k - Layered clip bodies**: a clip's bodies stop being exclusive and **stack**, back to front: the take, the events over it, the envelope over both. An automation drawn on top of the element it shapes is *one* clip, not two hiding each other — which is what makes an envelope attachable to its event. They do not share a value axis (a roll's `min`/`max` are pitches, a curve's are its parameter's units), so the curve carries its own `points_min`/`points_max`, and its editing (drag a point, Ctrl+click) maps through that. Python: the `clip` builder grows them; the `Editor` fuses a **simultaneous** group into one layered clip (below).

## G23: the patcher view (the logical group)

- ✅ **G23 - `patch`: a patcher view of a bus-wired node graph**: the arrangement's *logical* grouping — members that relate by **processing**, not by time — finally has the shape it always needed, and it is not a lane: it is a **server patch** (level 1, in the P track's later two-level vocabulary — this milestone built its view). The new `patch` widget (`host::patch`, pure over a `Mesh` and unit-tested without a GPU, the static-view posture) draws a GraphDef as it actually is: a **member box** per node (its def name, and a port per control that names a bus), a **bus node** per internal bus (`OUT`, the hardware, among them), and a **wire** per `(member, control) ↔ bus` pair. **Decision (recorded):** the view is deliberately **bipartite** rather than a directed source→sink patch, because a GraphDef only knows that a control *touches* a bus — which end writes and which reads is the server's own analysis (it sorts the graph topologically), and guessing it from a control's name would be a lie the renderer tells; a bus is therefore a first-class node, not an edge, and several members writing one bus is a sum on that node. **(This bus-as-node decision was reversed 2026-07-22** — it proved unreadable as a signal-flow diagram; the patcher turned directed and typed, see `docs/decisions.md`, "The patcher is a directed, typed graph", and the P track below. This milestone's widget stands as the record of what shipped; it is reworked in the redefined P3.)** Rewiring is therefore well-defined: drag a port onto a bus to point that control at it, onto empty space to unwire it (one wire per control, always), and the edit leaves as the flat `/gui_event <id> "wire" <member> <control> <bus>` — the clip/points edit-back pattern, third time. Python: the `patch` builder; the `Editor` renders a `Aggregate{logical}` as a server patch (the same 1:1 mapping onto a GraphDef that rendering uses, drawn instead of sent) and a `"wire"` edit rewrites the member `Generator`'s controls, so the next rendering sends a GraphDef wired the way the patch is drawn. Tests: the server patch lays out and hit-tests, a wire dropped on a bus rewires and reports, one dropped on empty space unwires; a logical aggregate draws as a server patch and its rewiring reaches the GraphDef spec.

## G24: the editor-grade piano-roll view

The multitrack (G22d) draws notes only as a **clip body** — bars, no keyboard, no velocity, no note editing, no OSC events. G24 is the dedicated, editor-grade `pianoroll`, the note view at the depth the heavy `waveform`/`spectrogram` reached: a keyboard gutter, an editable MIDI-note grid, a velocity lane and an OSC-event lane, with the same rulers, group navigation, selection and playhead. It contemplates the two message families a sequence carries — **MIDI notes** in the grid and **OSC events** in their lane. Staged in incremental parts; the first shippable slice is below.

- ✅ **G24a - The `pianoroll` widget + the shared note core**: a new `WidgetKind::PianoRoll`, architecturally a flat-geometry timeline widget (the `track` posture: `is_timeline`, `EditorProps`, group navigation — **not** a wgpu view; notes/keyboard/lanes are `Mesh` geometry, so it is browser-correct and wasm-clean by construction and unit-testable without a GPU). The note model, pitch↔y / time↔x mapping, drawing, hit-testing and editing live **once** in `host::pianoroll`, the reusable core **shared by two consumers** — the dedicated widget and the multitrack `clip` body (`track::draw_piano_roll` now delegates to `pianoroll::draw_notes`) — the same discipline G22h took extracting `bpf::place_point`. The `Note` gained `velocity`/`channel` (a real MIDI note), unified between the clip and the widget; the wire form is the flat quintuple `start dur pitch velocity channel` (a legacy triple list still parses). **Decision (OSC events, recorded):** notes draw in the pitch×time grid; OSC events (which have no pitch) draw as flags in a **separate lane** below it, not scattered in the grid. Editing is native (the browser keeps display + `/gui_set` parity, the editor-view posture): drag a note to move it in time/pitch, drag an edge to resize, Ctrl+click to add/remove, drag the velocity lane to set velocity, Ctrl+click/drag the OSC lane to add/remove/move an event; every edit rides the **edit-back pattern** — `/gui_event <id> "notes" <start dur pitch velocity channel …>` and `"osc" <time label …>`, flat payloads not new addresses, the fourth use of that pattern. **Placement (the G7b rule):** the one piece of general musical knowledge — the MIDI-note ↔ name / black-key spelling drawn on the keyboard — landed in `clausters_core::scale` (`note_name`/`pitch_class`/`is_black_key`, beside the perceptual scales), its FFI export **deferred** until a client evaluates it (the `envshape`/bus_tap-reader precedent); everything else (grid model, hit-test, keyboard/lane geometry, drag clamps) is display-only, gui-side. Python: the `pianoroll` builder (and `clip(notes=…)` grew velocity/channel); the `Editor` reads a note's velocity from the data (an explicit `velocity`, else `amp`), so a clip roll shows the events' dynamics; `examples/editors/pianoroll.py` draws notes + OSC events and plays them through a `Pbind`. Tests: the note core (hit-test edges-vs-body, insert/remove/resize clamps, pitch↔y, region layout), the widget parse (quintuples, OSC pairs, pitch window), the host edit-back (`pianoroll_hit`, `notes`/`osc` event args), and the note-name mapping in the core.

- ✅ **G24b - Pitch zoom/pan gestures + cursor readout**: the vertical axis becomes navigable by gesture, not only by `/gui_set`. The **keyboard gutter** is the piano-roll's y-axis surface (the counterpart of the heavy views' y-ruler strip): wheel over it zooms the pitch window (`y_start`/`y_len`, anchored at the cursor via `viewport::zoom_span`), drag pans it (`Drag::PanY`, the same absolute-from-snapshot pan), and `r` resets — reusing `zoom_timeline_y`/`set_y_view` verbatim, so nothing new in the group model. Wheel over the grid still zooms the shared time axis. A **cursor readout** (the note name under the cursor via `clausters_core::scale::note_name`, plus the time in the ruler's unit) draws in the grid corner, the piano-roll's answer to the heavy views' time+amplitude/frequency readout. All display-only, gui-side.

- ✅ **G24c - The Editor's dedicated piano-roll view**: `Editor.open_pianoroll(host, element=None)` opens one events element as the editor-grade `pianoroll` — the note view of an element, as opposed to `open`, where the same notes are only a clip body. `render` dispatches on the editor's view mode; the dedicated render draws the element's MIDI notes in the grid and its OSC (and raw MIDI) events in the event lane, frames the pitch window around the notes, snaps to the `quant` grid and rides the beats ruler; the roll registers as a lane too, so playhead/cursor/locate work unchanged. **Decision (recorded): per-note edit-back writes only to random-access element.** The `"notes"` payload rebuilds a `Track`'s editable `seq.Timeline` (times converted to beats, the OSC/MIDI events sharing the timeline preserved); a generator (`Pbind`/`Routine`) is forward-only, so its bounced notes show **read-only** (bounce it to a `Track` to edit) — the generated/generator distinction of the arrangement model doing its job at note granularity. OSC events are display-only for now: the `(time, label)` marker is a lossy view of the message, so writing it back would drop the args. Python-only (`gui/editor.py`); `examples/editors/composer.py` opens the melody `Track` this way beside the multitrack, sharing its timeline. Tests: the dedicated render (notes, OSC lane, pitch window), the timeline rewrite preserving the shared OSC event, the generator no-op.

- ✅ **G24d - Multi-note selection + block edits**: **Decision (recorded): the marquee is the shared time selection restricted in pitch.** A plain drag on the empty grid keeps driving the linked views' time selection exactly as before, and the notes inside the time × pitch rectangle become the widget's **selected set** — one gesture, no conflict with the heavy-view convention (Shift still pans, Ctrl still adds/removes); Alt+click toggles a note in/out (a non-rectangular selection, one note at a time). The set is **native view state** (`WidgetKind::PianoRoll::selected` — never parsed from the wire, cleared when the script replaces `notes` since the indices would dangle; the browser draws it but never builds it, the display-parity posture). Block edits: dragging a **selected** note's body moves the whole selection **rigidly** in time and pitch (the grabbed note snaps to the grid; the core clamps the block's delta as one, so an edge stops the block instead of folding it — grabbing an unselected note drops the selection and moves singly, and an edge drag still resizes just that note); Delete/Backspace over the roll removes the selection; dragging the velocity lane over a selected note nudges every selected velocity **relatively** from a press snapshot (each saturating 0–127 on its own, so reversing restores the spread). All pure block ops live in `host::pianoroll` beside the single-note edits (`notes_in_rect`, `move_notes_from`, `remove_notes`, `nudge_velocities_from`, `selection_after_removal`, `toggle_selected`); the gestures are thin `Drag::SelectNotes`/`NoteBlock`/`VelocityBlock` wrappers over them. **The wire did not grow**: every block edit emits the same flat `"notes"` payload, so the Python `Editor` (and any script) consumes it with zero changes. Tests: the marquee rect (reversed ranges, span edges), the rigid clamps (time zero, pitch top, a block wider than the pitch window), removal order/duplicates, the relative velocity nudge with per-note saturation, and selection index maintenance across a single removal and toggling.

- ✅ **G24e - Quantization, on both surfaces**: **the GUI runs standalone too, so the gesture and the client-side helper are complementary, not alternatives.** In the host, `q` over the roll quantizes the **selected** notes' onsets (all of them when nothing is selected) to the widget's `snap` grid — the same grid a drag snaps to — through the pure `pianoroll::quantize_notes` (durations kept, no-op without a grid), emitting the usual flat `"notes"` payload; that is the whole quantize story for a standalone host wired straight to the server. In the client, `seq.Timeline.quantize(grid)` snaps every placement to the beat grid **in the client** (the same rounding, in beats), for programmatic/NRT use and for elements never opened in a view. No wire change either way. Tests: the core op (selection vs all, durations kept, movement reported, gridless no-op) and the timeline method (snap, re-sort, gridless no-op).

- ✅ **G24f - The clip roll's note-name pitch ruler**: the multitrack `clip`'s roll body has no keyboard gutter, so it gets the **compact** form of the same knowledge — `pianoroll::draw_pitch_labels` names each C at the clip's left edge (via `clausters_core::scale::note_name`, the G7b placement again), drawn **only when a semitone row is tall enough to read** (the LOD discipline: a collapsed lane shows bars, an expanded one gains the names). Display-only, shared-core drawing, no wire change; tested by the row-height guard (a readable window draws, a squeezed one stays silent).

- ✅ **G24g - Live MIDI input painting notes, on both surfaces** (the standalone rule again): **host-side**, a `pianoroll` with the new `midi_in` prop makes the native host open a virtual MIDI input port ("clausters-gui", held open while any such roll is; dropped when the last closes) through the shared `clausters-midi` crate — whose `live::Input` gained a **safe Rust face** (`open`/`poll`, the C ABI now delegating to it) and a shared channel-voice **note parser** (`parse_note`: on / off / zero-velocity-on-is-off) — drained at the frame cadence behind the gui's new default-on `midi` feature (target-gated off wasm; the browser keeps display parity). A note-on paints a **held** note — at the **running playhead** (live recording; note-off then writes the real held duration) or at the **step cursor** when the transport is stopped (step entry: notes land on the snap grid, a chord shares one step, the last key up advances it) — and every paint emits the usual flat `"notes"` payload. **Client-side**, the same loop in script: `examples/editors/pianoroll.py` grew a `record_midi()` cell hanging a `MidiFunc` note-on/off pair off the client's own virtual port and painting via `/gui_set`, timed by the session clock. Python: the `pianoroll(midi_in=…)` builder kwarg. Tests: the shared parser (on/off/velocity-0/ignored types) and the `midi_in` parse/default.

- ✅ **G24h - Cut/copy/paste**: Ctrl+C copies the selected notes to a **host-wide clipboard** (normalized to the block's first onset, pitches absolute — so a block travels between rolls and windows), Ctrl+X also removes them, and Ctrl+V pastes with the block's first onset at the **mouse cursor's time** (snapped to the note grid), original spread and pitches kept. The pasted block lands **selected**, ready to drag into place — paste and adjust is one motion. Pure ops (`pianoroll::copy_notes`/`paste_notes`) beside the block edits; cut and paste emit the usual flat `"notes"` payload (copy is silent — nothing changed). Tests: normalization, paste placement + returned selection, the negative-time clamp, the empty copy.

With G24h the follow-ups G24 opened are all closed; further piano-roll work (e.g. per-channel coloring, a controller lane beyond velocity) gets numbered when a design converges.

## Done outside the numbered tracks

- **Shared config + config-driven standalone.** Server, GUI host and Python
  client read one TOML schema (`clausters-core::config`): a user file under
  `clausters.toml`/`config.toml` overridden by a project `clausters.toml`, then
  CLI flags. The GUI host reads `[gui]`/`[standalone]` as flag defaults, accepts
  `--config <path>`, and `--standalone` with no name falls back to
  `[standalone].gui`. Standalone now lets the embedded server load the whole
  bundle from the data directory (`Clausters::open_with_data_dir` →
  `attach_store`: SynthDefs, Faust defs, GraphDefs, bindings, `boot.json`),
  instead of replaying specs by hand; a `standalone-faust` feature pulls
  `clausters/faust` for Faust bundles. See `docs/configuration.md`.

- **The host's two long files became directories.** `gestures.rs` (4706 lines)
  and `frame.rs` (2514) each split along a seam they already had, so new work
  lands in a named place instead of at the end of a file:

  ```
  host/gestures/mod.rs      the machine: Drag + press/drag_to/release/wheel/tick
  host/gestures/effects.rs  what a gesture delivers (bound -> server, else event)
  host/gestures/nav.rs      what it reads and moves (hit-test, pan/zoom/selection)
  host/gestures/keys.rs     the keyboard half (text editing, block operations)
  host/gestures/tests.rs    the test module
  host/frame/mod.rs         the spine: GPU slots, FrameInputs, render
  host/frame/items.rs       the read half: per-widget snapshots + the tree walk
  host/frame/draw.rs        the write half: the three mesh passes
  ```

  Alongside it, three pieces of repetition got an address: the heavy-view slot
  walk both fronts had written separately (`frame::visit_elements` +
  `inline_slot` — the drift risk, since the browser half is not built on the
  native target), the five edit-back emitters (`gestures::effects::emit_read`),
  and the `window_def(id)?.find(id)?.kind` walk (`Host::widget_kind`/`_mut`,
  plus `Host::layout_window` for the layout call hit-testing shares with the
  renderer).

  **Two cuts deliberately not made**, so they are not re-proposed: splitting
  `drag_to`/`element_press` by widget family (the arms mutate `self.drag`, so
  the coupling would cross a file boundary rather than go away, and `drag_to`'s
  match is exhaustive over every `Drag` variant with no catch-all — that check
  is what guarantees a new drag kind gets driven), and folding the
  `{rect, clip, theme}` header the frame's item structs share into a nested
  struct (18 field triplets deleted, 102 sites made longer).

## G25: the host's TCP front (default command transport)

- ✅ **G25 — TCP server front for the `/gui_*` protocol** *(done 2026-07-12; pairs with server M25 and client C34)*: the host grows the TCP variant its `ClientId` seam anticipated — an acceptor plus one reader thread per connection handing whole length-prefixed frames to the single-threaded front, the audio server's `osc::tcp` pattern reused verbatim (the zero-length wake datagram is already in place in `host::transport`), with the same configurable frame ceiling as M25 (`--max-frame`, default 16 MiB). On **by default** beside UDP on 57210; `--no-tcp` opts out, and a standalone bundle (native or the future Tauri/mobile wrapper) that talks to its host in-process needs no network front at all. Python's `GuiHost` / `Session.gui` connect over TCP by default (`transport=` opts down to UDP), so a `/gui_def` JSON — the largest payload in the system, a whole editor tree with its blobs — is no longer bounded by a datagram, and neither are dense `notes`/`points` edit-backs on long elements. The browser front keeps WebSocket unchanged (it already frames without the datagram cap; it adopts the shared ceiling).

- ✅ **G25b — WebSocket server front (`--ws`), the browser's carrier into a native host** *(done 2026-07-18; pairs with the web client's W2 groundwork)*: the third leg of the script front, controlled exactly like the audio server's — `--ws [port]` (default 57220, clear of the host port since both bind a TCP listener), `ws = true`/a port under `[gui]` in the config, the shared `--max-frame` ceiling. `host/ws.rs` mirrors the server's `osc::ws` (one OSC packet per binary message — WebSocket frames, so no length prefix; tungstenite-enforced size ceiling; per-connection reply channels since a tungstenite socket owns both halves) with G25's sink generalization, so both fronts take it: the headless serve loop through a `WsHub` (mpsc + the zero-length wake datagram), the windowed front through the winit proxy. `ClientId::Ws(id)` joins the seam; replies route back on the originating connection. A page cannot open UDP or raw TCP, so this is what lets the TypeScript client drive a **desktop** host the way it drives a `clausters --ws` audio server; the e2e (`clients/web/tests/gui-host.test.ts`) runs `/gui_def` + `/gui_query` from the web client's `WsConnection` against a spawned `clausters-gui --headless --ws`.

- ✅ **G26 — The measuring plot: views, rulers, multichannel, readout + the free-standing `plot()`** *(done 2026-07-14)*: the G8 `plot` grows from a bare trace into a **measurement** view while staying static (no zoom/pan/edit — the heavy views own navigation). It gains **views** (`view` prop; `PlotView`, an enum on purpose so future forms — a histogram, a phase plot — extend it without touching the wiring): **signal** — value against time/index, every channel drawn (stacked lanes or `overlay` traces), the whole sequence always contributing (polyline or per-column min/max envelope: no visual aliasing), the value axis `[min, max]` with **either side omitted auto-fitting to the data** (the arbitrary-range sequence case; the string `"auto"` releases a side live) — and **spectrum** — the Welch-averaged magnitude spectrum per channel in dB over the four spectrogram scales (linear/log/mel/bark), analyzed **once at the widget's mutation points** (parse / bulk load / `/gui_set`) through `spectrogram::Stft` (the shared-core FFT, so it agrees with the spectrogram bin for bin) and drawn through the identical `display_to_hz` geometry the rulers use. New chrome, all shared math: a generic 1-2-5 **value ruler** (`ruler::value_ticks`, any range — not amplitude-tied), a **horizontal** frequency ruler (`ruler::hz_ticks_h`, width-fitted labels), the x ruler in samples or clock time, and a **hover readout** (hairline + dot + the exact sample's index/time and value, or the bin's frequency-per-scale and dB) in the overlay pass. The bulk paths keep every interleaved channel (native mmap and browser fetch alike). Python: the `guidef.plot` builder grows the props, and the new **free-standing `clausters.plot(obj, …)`** (the visual sibling of `play`) opens **one individual window per call** on the ambient GUI host (the session's, else one it boots lazily, no client leg) — dispatching a **def** (`SynthDef`/`FaustDef`/`GraphDef` + its `defs` members: an ephemeral NRT session renders `dur` seconds of its output), an **`Env`** (rendered through the engine's own `EnvGen`, gate-released at its sustain point), a **`Buffer`** (fetched live with its shape/rate), or any **iterable of numbers** (a `Pseq`/`Pwhite`/list, materialized up to `n`, range auto-fitted). Tests: tick fitting/collisions for the new rulers, the per-channel analysis peaks, decimation bounds, parse/apply round-trips, and the Python dispatch + NRT render paths against a fake host.

- ✅ **G27 — Shared gesture machine: one interaction state machine for both fronts**: retire the two divergent per-front drag machines (the native front's full one; the browser's `host/web/` mini-`Drag` covering only slider/knob/button) in favor of **one** press → drag → release → wheel interpreter in the agnostic core, so every editor gesture the native front has — selection, pan/zoom, BPF editing, clip placement, the whole piano-roll editing set — reaches the browser by rewiring, not by reimplementation. The machine (`host/gestures/`: the `Drag` state + `Gestures::press/drag_to/release/wheel` and the keyboard block operations) mutates the `Host` through the existing `interact`/model doors and returns `GestureEffect`s (emit/redraw) for the front's own sinks; the front supplies a per-call `GestureCtx` (framebuffer, modifiers, the GPU slots' lane counts — the one front-owned datum) and nothing else. Two legs:
  - ✅ **Leg 1 — extract + native rewire** *(done 2026-07-15)*: the machine extracted with **no behavior change**; `host/gui/input.rs` reduced to winit adapters (context snapshot + effect application), the piano-roll press/keyboard ops folded into the machine, MIDI painting kept native (`host/gui/midi.rs` — the virtual port is a device). The gestures became unit-testable without a window or GPU for the first time (press/drag/release/wheel dispatch tests over a JSON-built host).
  - ✅ **Leg 2 — browser rewire** *(done 2026-07-15)*: the `#[cfg(not(wasm32))]` lifted from `gestures` and from the `interact` gesture helpers it drives (they are pure; the cfg only reflected native-only use); `host/web/`'s mini-`Drag` deleted and its pointer/wheel/keyboard events routed through the same machine (`GestureEffect`s to the outbox/binding surface; the piano-roll clipboard is page-wide, modifiers tracked via `ModifiersChanged`), so the browser gained selection, pan/zoom, BPF/clip editing and the whole piano-roll editing set by rewiring, with zero new gesture logic. The browser posture re-stated in the docs (`docs/clients.md`, `docs/architecture.md`, `docs/decisions.md`): from "display + `/gui_set` parity" to full editing gestures. Verification: `check-wasm.sh` gates the lifted cfgs; a browser smoke (headless Chrome over CDP, real input events on the wasm bundle) drove a selection drag, a wheel zoom and a BPF edit end to end, each emitting its `/gui_event` payload.

- ✅ **G28 — Multichannel live scopes: one rule for `bus` + `channels`, instrument rulers, a visible trigger** *(done 2026-07-16)*: the C37 verb generalized so monitoring stereo/multichannel/HOA sets is one coherent rule — *the verb monitors `channels` consecutive buses from `bus`, each view presents them its way* — instead of three per-view special cases. The `scope` and `spectrum` widgets grew a `channels` prop reading that many **adjacent** tap rings from `tap` (the phasescope's existing adjacency convention, now the family's): the oscilloscope draws stacked lanes or color-coded `overlay` traces, searching the trigger in the **first** channel and applying the found alignment to all (true relative phase across channels); the spectrum folds one FFT state per channel and overlays one color-coded curve each. The trigger became **visible**: a faint level line in the trigger channel's lane and a `lock`/`free` corner read-out (`oscil::align` now reports whether it fired). Both widgets gained **axis rulers** as owned strips (the G20b posture — a ruler measures its widget's axis, it is not a free-standing element; recorded in the ruler-as-widget discussion): `ruler` (ms of the window / Hz on the active `freq_scale`, via the shared tick machinery) and `ruler_y` (value / dB), shown by default, `"off"` hides, all live via `/gui_set`. The tick's tap window became the typed `TapWindow` (interleaved channels + lock flag) shared by scope and phasescope on both fronts. The composite "analyzer" stays **client-side composition** (a GuiDef of menu + number + scope; deliberately no monolithic widget). Python: `channels`/`overlay`/`ruler`/`ruler_y` on the builders; the verb takes `channels=` (a `Bus` brings its own count; `view="phase"` is the fixed two-channel case, erroring on any other explicit count) and its docs read as a brief user manual. Tests: multichannel parse/apply and adjacent `taps_read`, the interleaved tick alignment on the first channel, per-channel spectrum states, and the Python channel semantics.

## G29: the virtual piano keyboard (`piano`)

- ✅ **G29 — The `piano` widget: a playable keyboard with real proportions, range navigation, and two instrument-mapping paths** *(done 2026-07-19)*: a new mesh-drawn widget (`host/piano.rs`, pure and window-free) whose keys follow the **physical instrument's ratios expressed only as proportions** — white keys 1 unit, black keys `13.7/23.5 ≈ 0.583` units wide and `0.6` of the white length, and the black keys *not* centered on the white boundaries: the C–E group's rear white segments are equal (`(3−2b)/3`) and F–B's are equal (`(4−3b)/4`), so the keyboard resizes freely and always reads like a piano. The visible `min`/`max` window (min snapped to a white key) pans over the whole MIDI range through the widget's own "ruler": an **overview strip** of all 128 keys with the window boxed — drag pans, wheel zooms anchored at the cursor (span clamped one octave..128), wheel over the keys pans by white keys — every change emitted as `"range" min max` and settable via `/gui_set` (browser parity), all gated off by `pan: 0`. Keys outside `active_min`/`active_max` draw **grayed and inert**, visualizing the mapped range. Playing emits the **MIDI-shaped** payload `"note" pitch velocity state channel` (ints; state 1/0; a later consumer translates 1:1 to note-on/off): velocity from the press height (front of the key = louder; a `velocity` prop fixes it), glissando on drag (off + on crossing keys), `pressed` as native view state (the `selected` posture). **Two instrument-mapping paths, both worked**: unbound, the events reach the script, which programs the voices (`examples/panels/piano.py` — the "programmable like a GuiDef" path; a bound piano instead forwards the flat payload); with `voice: "<def>"` the **host** manages one server voice per held key — `/synth_new <voice> <id> 0 0 freq <hz> amp <vel/127> gate 1 <voice_args…>` on press, `gate 0` on release, the def freeing itself (`FREE_SELF`) — over the existing `ServerLink`, so UDP, embed and the browser's in-page engine all work unchanged; the self-driving bundle `clients/web/examples/piano` plays with zero page JS (the graph-controls posture). Voice node ids come from a dedicated high window (`0x1000_0000`, recorded in `docs/decisions.md`); held voices release on widget free/redefine. Placement analysis: `midi_to_hz`/`hz_to_midi` landed in `clausters_core::scale` (general musical knowledge, beside the note spelling); the key geometry, overview math and velocity map stay gui-side (display-only). Python: the `piano` builder; tests cover the layout ratios and hit priority, pan/zoom clamps, velocity endpoints, parse/apply round trips, press/glissando/release/gray/pan-gating gestures and the voice bookkeeping. Future lines (unnumbered until a design converges): lighting keys from live MIDI-in (the G24g machinery) and a script-settable `pressed` display.

## G30: the editable text field (`text`)

- ✅ **G30 — The `text` widget becomes a real editable entry field**: the catalog's `text` was a display-only string (script-driven via `/gui_set`); G30 makes it *editable* — the general-widget track's overdue counterpart to the numeric `number`. Clicking focuses the field and drops the caret where the press lands; typing inserts, the arrows/Home/End/Ctrl+arrows move the caret (word-wise with Ctrl), Backspace/Delete edit, a drag or Shift+arrows select, Ctrl+A selects all. **The entered string is delivered exactly as a numeric control's value is — on *every* edit, never gated on Enter** (`emit_value` on each keystroke, so the bound path forwards it straight to the audio server and the unbound path emits `/gui_event <id> <string>`, the scalar form `text` already reported): a text field is a single-value control like a slider, so **no wire change** — no new address, no new tag. A `multiline` prop allows embedded newlines (Enter inserts one; single-line ignores it) and a growing field; the caret/selection are **native view state** (never parsed from or sent over the wire — the `PianoRoll::selected` precedent). **The editing logic is a std-only pure module** (`host/textedit.rs`, the sibling of `bpf.rs`/`pianoroll.rs`): the caret and selection are byte offsets kept on `char` boundaries via `str::char_indices` (no `unicode-segmentation` — `char` is the right granularity for the per-cell bitmap font), the ops mutate the widget's `value` in place so it stays the single source of truth, and it unit-tests without a window and compiles to wasm unchanged. Focus is host-wide state (`Host::focused_text`), so a keystroke edits the focused field instead of running the global editor shortcuts; both fronts translate their winit key into a neutral `TextKey` and route it through the shared gesture machine (`Gestures::text_key`), so native and browser edit identically by construction (the G27 posture). **Cut/Copy/Paste (recorded):** an **internal host-wide clipboard** (a `String`, zero new dependency — the G24h note-clipboard precedent; winit exposes no OS clipboard and libwgpu covers none), the same in-page clipboard on the web front for now; binding it to the browser's real OS clipboard (a `writeText` out plus a `paste`-event listener in — free via the DOM, no Rust dep) is the one recorded follow-up. The embedded 5×7 font grew the punctuation an OSC message and free text need (`_ = < > [ ] { } " ' ; ? ! * # & @ | ~ ^ $ \`); a real font atlas and true lowercase stay the deferred refinement L4 already named. **Placement analysis (the G7b rule):** all display/interaction logic — caret/selection model, hit-testing, focus, key routing, rendering — is **gui-side**, nothing for `clausters-core`; the one client-facing piece, tokenizing a typed string into an OSC message, is Python-client ergonomics (the shell layer), not shared core. Python: `multiline=` on the `text` builder, and `examples/panels/oscsend.py` — an OSC-message field whose typed `/node_set <id> freq <value>` sweeps a running synth's pitch live as you edit the number (the ungated string delivery made audible), parsed by a minimal `parse_osc` helper, plus a multiline scratch pad. Tests: the `textedit` model (insert/delete/word & vertical motion/selection/clipboard-free ops/boundary clamp), the `caret_at` hit layout, and the gesture integration (a press focuses, each keystroke emits the whole string ungated, a press elsewhere defocuses, Enter is a newline only when multiline, cut/paste move text through the clipboard).

## G31: music notation (the `score` widget)

The plan carried notation for years as "Verovio's SVG, hosted by the web surface, off the GPU path" — a whole second rendering substrate for one widget, and the strongest single argument for going web-first. G31 landed it the other way and the second substrate turned out to be unnecessary: **the SVG is a serialization, not a runtime**. The client engraves once and parses the result into a *display list* — a glyph-outline table keyed by SMuFL codepoint plus placed glyphs, staff lines, stems, beams, slurs, hairpins and text in page units — and the host tessellates that into the ordinary `paint::Mesh`, knowing nothing about MEI, MusicXML or notation. So notation costs the host **no dependency** (Verovio is an optional *client* dependency), draws in the one batch the rest of the chrome draws in, works in the browser by construction like every other widget, and needs no SVG renderer anywhere. The seam is the same one the heavy views already use — the client analyzes, the host draws — with the display list as the cache.

- ✅ **G31a — The display list and the `score` widget** *(done 2026-07-23)*: the spike (Verovio → SVG → geometry) grew into the widget. The engraving vocabulary is complete (glyphs, staff/stem/beam strokes, slurs and ties as filled curves, hairpins, text), a multi-system page lives in a `scroll` (pans on both axes, zooms anchored), and `vb` fits the page into the widget so the drawing is resolution-independent. `docs/decisions.md` records the finding that made the shape choosable at all: Verovio's `DeviceContext` is a viable extraction point, but the SVG output already carries everything the display list needs, so parsing it is the cheaper seam.
- ✅ **G31b — The playback cursor on the engine clock** *(done 2026-07-23)*: the engraving's third layer is a `cursors` track (the score's own timemap folded into `t` in ms → an `x` and a staff span), and the widget takes `playhead_at` — the sample-clock value score time 0 maps to — so the cursor **sweeps on its own**, one message per pass, the host reading the clock every frame exactly as the timeline views do. Negative means stopped, and the static `playhead` holds the cursor where the music was left, which is what makes pause look like pause.
- ✅ **G31c — The page is clickable: `"element"`** *(done 2026-07-23)*: every primitive carries the MEI `xml:id` it was engraved from, so a press hit-tests to an element and reports it. The id is the *client's own*, so it indexes the client's score directly — nothing but a string crosses the wire, and the host still reads no notation.
- ✅ **G31d — The edit round trip: drag a note, `"transpose"`** *(done 2026-07-23)*: a drag on an engraved element moves it up or down the staff in **whole diatonic steps**, drawn as it goes, and the release emits `"transpose" <xml:id> <steps>`; the client applies it, re-engraves, and replaces the drawing with one `/gui_set <id> display_list` (a new prop — until then a page could only change by redefining its window). Steps rather than a coordinate, because Verovio's coordinate-taking `drag` reads a page y in a frame that does not line up with the display list's, and its editor transposes by steps anyway; the quantum travels **with the page** (`step`, page units per diatonic step, measured from the staff-line spacing) since it follows the engraver's unit size, not the staff scale. Two things the drag revealed about what an "element" is: a sounding element **claims every primitive drawn inside it** (Verovio ids a note's stem and flag separately, which scattered one note across three draggable things), while a chord deliberately stays a container so its notes remain separately transposable. And the staff furniture a pitch implies — **ledger lines**, drawn per staff, so no per-element displacement can move them — is *re-derived* by the host from the staves it reads back out of the engraving, with the engraved ones over the notehead's column suppressed: that is the half that could not be faked, since a note dragged back onto the staff has to *lose* them. The host cannot re-engrave, so the preview stands after release until the new page arrives.
- ✅ **G31e — A reproducible Verovio build** *(done 2026-07-23)*: `third_party/verovio.pin` + `build-verovio.sh`, the arrangement `build-faust.sh` established (source uncommitted, reproducibility in two committed files). It pins a `develop` commit rather than a release tag on purpose: in 6.2.1 — the PyPI wheel included — the score editor is **dead in any build keeping Humdrum support**, a guard typo that makes every `Toolkit::Edit()` return false; upstream inverted it after 6.2.1. Recorded in `docs/decisions.md`; repin to 6.3.0 when it ships. The editing half of the widget therefore needs a Verovio whose editor works, which the docs and the smoke step both say.
- ✅ **G31f — Engrave the client's own data: `from_notes` / `from_timeline`** *(done 2026-07-24)*: the inverse of the score→sound flow. Until now a page could only be *typed* (ABC/PAE/MEI/MusicXML); now `clausters.gui.notation.from_notes` (a monophonic run of `Event`s) and `from_timeline` (a `Timeline`, simultaneous events → a chord, gaps → rests) turn the client's own `clausters.seq` data into **MEI** that flows into the same `engrave`/`Score` path — a third way in, beside typed text and the SVG adapter, with `Score.from_notes`/`from_timeline` as the generate-and-edit shortcut. MEI is the target because it spells pitch and value explicitly, sidestepping ABC's contextual traps, and it needs no ids (Verovio mints them on load, so id stability across editing is unchanged). The substance is the rhythm: durations decompose into tied note values (a dotted value when exact) and a note overrunning a barline splits and ties across it, all in exact 32nd-note ticks. Example `examples/editors/score_from_data.py` (a Timeline of chords + melody, engraved then played from the same timeline); tests cover the pure encoder (spelling, decomposition, cross-bar ties, chords/rests) without the engraver. v1 reads only the written `dur`; the refinements are the next line.
- ✅ **G31g — Engraving refinements** *(shipped 2026-08-29/30 as `N1`-`N6`)* — **promoted out of this section to the `N` track** ("N track — notation: the score model, and what is written on it"). It stopped being one refinement the day it was sized, and stopped being an encoder's refinements the day the layer was read against itself: the return path already existed and was being written by hand, each direction went through a different intermediate, and `Score` turned out to be an action API with no structure under it. So what G31g owed is now owed by a **model** — two durational structures in exact rationals, in the core — and the four refinements are distributed over its emission. The G31 section stays the record of how the widget got here; what it still owes is numbered there. **`N1`-`N6` closed on 2026-08-29/30**, so what this line owed is written: the model, its verbs, the emission of everything it can hold, the interpreter, the reader and the enriched forward path. What is numbered `N7`-`N9` there is not this line's — those are the *editing* questions the G31 open lines left standing, plus one the editor example turned up.
- ✅ **G31h — The notation layer moves to `clausters-notation`/`core`/`ffi`** *(done 2026-07-24)*: the entire notation layer used to live in the Python client (`clausters.gui.notation`) — the `ctypes` binding to libverovio, `svg_to_display_list`, the editable `Score` model (`transpose`, `undo`/`redo` over its own MEI-snapshot stack, the edit→commit→reload cycle) and the `from_notes`/`from_timeline` encoder. The host is deliberately **dumb about the domain** (it draws the display list and reports gestures as requests, holding no score), which keeps it reusable across languages — but it pushes all the notation *logic* onto the client, so a second client (JS/wasm) would reimplement every line. Per the cross-client rule (`clients/python/PLAN.md`, "Build strategy") that logic moves down and each client rebinds it. **The planning pass settled the shape — this is now a build, not a design:**
  - **Where the native half lives.** libverovio is C++; `clausters-core` is deliberately `build.rs`-free and wasm-clean, so it cannot host the binding. A new **`clausters-notation`** crate (its own `build.rs`, the libverovio binding, the `Score` model) owns everything native, behind a `verovio` feature **default-off** for flexibility — the Python reference wheel turns it on (`build_native.py`), exactly as it already stages `libverovio.so`. `clausters-ffi` re-exports its `extern "C"` surface. Linking/staging mirror libfaust: `DT_RPATH` of `$ORIGIN`/`$ORIGIN/../_libs`/prefix, `CLAUSTERS_VEROVIO` to locate, the SMuFL resource dir resolved inside the Rust binding.
  - **Where the pure half lives.** The SVG→display-list walk (moving to Rust XML parsing via `roxmltree`, wasm-safe — no duplicate parser per client) and the MEI encoder go into **`clausters-core`** behind an opt-in `notation` feature, so they compile to wasm and a future JS/wasm client reuses them. The `Event`/`Timeline`→*voice* reduction reads client-native types and **stays in each client**; the core exposes `voice_to_mei(voice, meter, clef, key)` — that is where the agnostic/shell line falls.
  - **The producer stays the SVG walk, not the native `DeviceContext`.** Binding libverovio in Rust makes `RenderToDeviceContext` reachable, but verovio-wasm only emits SVG, so a native `DeviceContext` producer would force the wasm client onto a *different* producer that would have to agree with it bit-for-bit. The SVG walk is **one producer every client shares** (native libverovio→SVG and verovio-wasm→SVG both feed the same Rust walk), so cross-client parity is structural, not asserted. The native `DeviceContext` (no serialize round-trip, in-memory relayout) becomes a later *optional producer-swap*, decoupled here (`docs/decisions.md` updated to say so).
  - **The C-ABI** is a near 1:1 relabel of today's `Score`: an opaque `*mut Score` handle (`score_open`/`_free`/`_display_list`/`_mei`/`_transpose`/`_edit`/`_undo`/`_redo`/`_can_undo`/`_can_redo`), the one-shot `engrave`, and the pure `svg_to_display_list`/`voice_to_mei` — every JSON return *size-then-fill* into a caller buffer (the `clausters_core_patch_compile` pattern), every stateful call carrying the handle (the `Scheduler`/`SampleClockModel` pattern).
  - **Editing ports minimal and extensible.** The move carries the existing minimal edit (transpose = verovio `keyDown`+`commit`, whose `commit` **applies verovio's own autolayout** for the definitive placement — which is why the round trip re-sends a whole display list rather than nudging one primitive), our MEI-snapshot undo stack, and the raw `edit(action, …)` escape hatch that already passes any `EditorToolkitShared` action (`insert`/`delete`/`set`/…). Richer editing rides that same seam **later** — not in G31h.

  The build, in four steps:
  - ✅ **G31h-a — The `clausters-notation` crate + libverovio in Rust** *(done 2026-07-24)*: the new crate binds libverovio through its C wrapper (`vrvToolkit_*`) — a safe RAII `Toolkit` and a one-shot `engrave_svg` — behind the `verovio` feature **default-off** (the crate is empty without it, so `cargo build --workspace` links no libverovio). `build.rs` links it from `VEROVIO_PREFIX`/`~/.local`/`/usr/local` with the relocatable `DT_RPATH` libfaust uses, and bakes the build prefix's `share/verovio` in as the run-time resource fallback (overridable by `CLAUSTERS_VEROVIO`). An `ffi_lock` serializes the library (verovio's resource init is global, and the Rust harness runs tests in parallel). Verified **byte-for-byte geometry parity** against the Python client's engraving of the same phrase: identical once the per-load random `xml:id`s are normalized out. The SVG→display-list walk is still Python's — it moves to `core` in G31h-b.
  - ✅ **G31h-b — The pure half into `core`** *(done 2026-07-24)*: `svg_to_display_list` (over `roxmltree`, with `regex` parsing transforms/glyph refs exactly as the Python original) and `voice_to_mei` now live in `clausters_core::notation` behind the `notation` feature (**off by default** — a plain core pulls no XML/regex/JSON weight), and the module compiles to `wasm32-unknown-unknown`. The `Event`/`Timeline`->*voice* reduction stays per-client (it reads client-native types); the core takes the reduced [`Slot`] voice, which is where the agnostic/shell line falls. The pure-logic tests migrated to Rust — pitch spelling, tick decomposition, cross-bar ties, chords/rests, clef/key — and the SVG walk gained the unit tests Python never had (one primitive of each kind on a hand-built SVG: the note-owns-its-stem rule, the glyph flip, line-vs-fill, the deepest font size, the measured step, the codepoint scan). **Parity verified against the Python original**, both directions: `voice_to_mei` is **byte-identical** to `_mei_document` across five voices (melody, cross-bar tie, chord+rest, empty bar, alternate clef/key), and `svg_to_display_list` reproduces Python's display list **prim-for-prim in draw order** (plus vb/glyphs/step) on the engraved phrase's real verovio SVG. The Python client is untouched — it still runs its own copy; the ctypes rebind onto this core is G31h-d.
  - ✅ **G31h-c — The stateful `Score` + the full C-ABI** *(done 2026-07-24)*: `clausters_notation::Score` is the editable model — `open`/`display_list`/`mei`/`transpose`/`edit`/`undo`/`redo`/`can_undo`/`can_redo`, carrying the edit→commit→reload cycle, the MEI-snapshot undo stack (64 deep) and the never-drawn guard that would otherwise segfault the editor — and `clausters-ffi` exposes it behind two new features, `notation` (pure) and `verovio` (native, pulls `notation`), both **off by default**. `display_list` returns the whole `Page`: the drawing layers flattened together with `cursors` and `notes`, so one call carries what the host draws, where the playhead goes and what sounds — they must come from one engraving, since ids are minted per load. The **timemap→cursor fold moved to `core`** with the rest of the pure half (`cursor_track`, over the display list's own geometry), leaving only the timemap/MIDI reads native. The C ABI is `clausters_core_svg_to_display_list` / `_voice_to_mei` plus the opaque `clausters_score_*` handle, size-then-fill throughout; `CORE_ABI_VERSION` is 12. **The one-shot `engrave` is deliberately absent from the ABI**: size-then-fill needs the payload identical on both calls, and the engraver mints fresh `xml:id`s of varying length per load, so sizing and filling would engrave two different documents — a binding's one-shot is `score_open`→`score_display_list`→`score_free`, where the ids are minted once (recorded in `docs/decisions.md`). A `links = "verovio"` key publishes the build prefix to `clausters-ffi`'s own `build.rs`, so the shipped cdylib carries the same relocatable `DT_RPATH` the rlib does. The Python client is still untouched; the rebind is G31h-d.
  - ✅ **G31h-d — The Python rebind** *(done 2026-07-24)*: `clausters.gui.notation` is now a ctypes shell over the ABI — **public API unchanged** (`Score`, `engrave`, `page_json`, `from_notes`/`from_timeline`, `svg_to_display_list`, `score_view`), roughly 1120 lines down to ~380, with the verovio ctypes binding, the SVG walk, the MEI writer and the cursor/geometry folds all gone into the workspace. What stayed is what reads Python-native types: the `Event`/`Timeline`→*voice* reduction, `page_json` and `score_view`. Python owns the **handle's lifetime** (the score is freed with the object) and nothing else. `_native` gained `has_notation`/`has_engraver` (the symbols are bound only if present, so a cdylib built without the features still loads and every other entry point works) plus the `size_then_fill`/`as_u8` helpers the surface needs; `clausters_score_open` grew an `options` JSON argument so the client's `options={"unit": 6}` still reaches the engraver without the ABI growing a parameter per knob. `build_native.py` builds the FFI with `--features verovio`. **Verified against the pre-move implementation**: engraving the same phrase through both produces identical `vb`/`step`/`glyphs`/`prims`/`cursors`/`notes` (ids normalized), and the client suite passes unchanged (381 tests) — its pitch-spelling and tick-decomposition unit tests were dropped, having migrated to Rust with the encoder. Docs updated: `docs/architecture.md` (the layer is shared, not per-client), `docs/decisions.md` (the `DeviceContext` entry now says the move happened and kept the SVG producer), `docs/clients.md`, `docs/gui-protocol.md` and `BUILD.md` (libverovio is behind a Cargo feature now, not "Python-side only").

Open lines (unnumbered until a design converges):

- ~~**Per-element edit properties.**~~ **Numbered `N8`** on 2026-08-30. A widget-level `editable` flag now gates the whole page — off by default (a `score` is a read-only view; a drag does nothing), on for an editor that applies the `"transpose"` round trip, since the host cannot fulfil an edit the driver will not apply. That is the coarse cut; the finer one remains open: *which element* allows *which* edit is a per-element property (a notehead transposes, a barline does not), deliberately not gated yet since Verovio's editor is still developing and the obvious next wants (lengthening a stem, which autolayout normally does) are edits, not restrictions.
- ~~**More edit verbs.**~~ **Delivered by `N2`/`N5`, and not the way this line predicted.** It expected each verb to be a verovio editor action plus a client method; what shipped is a verb of the **model** — `set_dur`, `set_pitches`, `set_marks`, `move_steps` — applied through one `Score.apply`, so duration, accidentals, articulation and stem direction are edits to a structure rather than actions on a layout engine's document. The prediction about the *protocol* held exactly: the payload is still a tag plus flat primitives, and the three gestures a page reports (`"element"`, `"transpose"`, `"insert"`) grew without the wire changing shape.
- ~~**Notation as a composition surface.**~~ **Numbered `N9`** on 2026-08-30, once the model it needed existed: binding the `score` to the arrangement (`clausters.form`) the way `track`/`clip` are bound is the open question the multitrack and piano-roll views already answered for their own material.

## G32: host gaps the editor example turned up

Three of them, found while porting `editor.py` to the browser. The first
two are **platform-independent** — they live in the shared widget/render code,
so the native front has them too — and are about a view that is *driven* rather
than merely opened; the third is the browser front's. The first is closed; the
other two remain open (unnumbered until their design converges).

- ✅ **G32c — `timeruler`: the axis ruled once, above the lanes** *(done
  2026-07-28)*: a ruler over a multitrack belongs to the **axis**, not to any
  one lane, and a `track`'s `ruler` strip is reserved out of *that lane's*
  height — so ruling a stack meant electing one lane to carry the strip and to
  pay for it, and the strip then sat wherever that lane sat (between two lanes
  unless it was the last). The new `timeruler` widget owns its own box: the
  document places it above the lanes and no lane loses a pixel. It is a timeline
  widget like any other — it joins the group its `link` names, reads that
  group's window, and its ticks are indented by a lane's header width so they
  stand over the samples they label. Gestures are the DAW's: a press **locates**,
  Shift+drag pans, the wheel zooms. Deliberately **no sizing concept of its
  own** — its thickness is the `h` place prop, like every other widget's, with
  the default in the builders (`track`'s vestigial `height` is read by nobody,
  which is the counter-example). Python and TypeScript builders, both catalog
  tables, and `composer.html` now rules from the top. Tests: the ruler and the
  lanes resolve to one group key, it follows the lanes' window across a zoom, and
  it contributes no extent of its own.

- ✅ **G32d — A roll's axis exists before its notes, and follows them**: the
  gap the client-side MIDI painting turned up (`pianoroll_midi.py`), the
  piano-roll's version of G32b. A `pianoroll` opened *empty* registered extent
  0, so its window was the single sample `View::full` floors to and every note
  written into it by `/gui_set` landed outside — the roll stayed blank however
  much was painted. Two halves, both the DAW's: the roll now navigates an
  **empty grid** it has before it has content (`EMPTY_BEATS`, sixteen beats read
  off its own `tempo`/`sample_rate`; a floor on the *axis*, never on the extent,
  so nothing that measures material sees it) and the authoring headroom that
  used to be the lane's alone; and a `/gui_set` of `notes`/`osc` re-registers
  the extent **keeping the window** (a take grows under a still axis instead of
  zooming it out from under the notes just drawn) and **pages the axis forward**
  when the take passes the right edge. Tests: an empty roll spans its grid and
  survives its first note; a take written past the edge pages one window on at
  the same zoom.

  **Open — unify the follow rules.** Three places now decide how a window
  follows content it does not contain: this paging, G32b's edge auto-scroll
  during a clip drag, and the notation cursor's follow. They share one
  principle — *at constant zoom, the window goes where the writing is* — and
  differ only in what triggers them (content arriving, a held cursor, a moving
  playhead). Factor the principle into one place rather than letting three
  copies drift; unnumbered until the shape is clear.

- ✅ **G32b — A clip dragged to the edge pulls the view** *(done 2026-07-28)*:
  the companion gap to G22's "empty time to zoom out into". The headroom let a
  lane be zoomed *out* past its content, but a drag still mapped the cursor
  through the **press-time** window and nothing scrolled while it was in
  flight — so a clip could never travel more than one visible window per
  gesture, and holding it against the lane's edge did nothing, because a
  standing pointer sends no events. Zoomed in to place something precisely,
  that was a sliver. Now the drag maps through the **current** window (which is
  the whole mechanism: panning under a standing cursor moves the sample beneath
  it and the clip follows), and `Gestures::tick` pans while the cursor is held
  within `EDGE_MARGIN` of a lane's edge, at a rate that is a **fraction of the
  visible window per second** rather than a pixel rate, so it behaves the same
  at every zoom (rationale in `docs/decisions.md`). The tick is each front's
  existing frame timer — the one already driving meters, scopes and the
  playhead — so both fronts get it from the one shared method, and an
  otherwise-still window runs the timer only while such a drag is in flight.
  `ClipHit`/`Drag::Clip` carry the **lane** id now: a clip is not itself a
  navigation-group member, so everything reaching the shared axis asks through
  its lane. Tests: the right edge pulls the view and carries the clip (and
  reports both the `"clip"` and the lane's `"view"`), a cursor clear of the
  margins scrolls nothing, the scroll stops with the drag, and the left edge
  walks back and parks at the origin without the clip going negative.
  `clients/web/examples/editors/composer.html` is where it was found and verified.

- ✅ **G32a — The playhead follows a loop** *(done 2026-07-28)*: the swept line
  was one subtraction in `host::frame` (`sample_clock - playhead_at`), which is
  why following a transport costs **no messages** and is exactly right for a
  straight pass — but it had no notion of a loop, so a repeating region (an
  editor's "play selection", a looping clip) ran off the view instead of being
  followed. Shape **(a)** landed, the explicit region: `playhead_loop_start` /
  `playhead_loop_len` on the shared `EditorProps`, the swept position becoming
  `start + ((sample_clock - playhead_at - start) mod len)` and `len <= 0`
  keeping the straight sweep. It beat the `playhead_loop`-flag-over-the-
  selection alternative because it does not tie playback to a selection a
  gesture can change under it, and because a client may loop something that is
  not the selection (rationale in `docs/decisions.md`). The whole sweep rule
  moved into **one** place — `head_at` (and `swept_at`, which keeps the
  playing/stopped distinction the MIDI painting needs), on `EditorProps` then
  and on `GroupState` since E3 — so the four
  sites that each re-derived it (the timeline views, the piano-roll, the track
  lanes, the native MIDI cursor) now agree by construction; the `score` carries
  the same pair in **ms**, its own unit. Group-wide like the anchor, held
  by the group model, so linked views wrap at one place. Python and
  TypeScript builders and `docs/gui-protocol.md` carry the props;
  `clients/web/examples/views/editor.html` uses them — and documents the one real
  trap, that a looped pass anchors at `clock - start`, not `clock`. Tests: the
  wrap model (straight pass, static park, wrapping, a non-positive length) and
  the group-wide routing.

**A heavy view's source is fixed once sent.** `/gui_set` applies the chrome
(selection, playhead, rulers, the spectrogram's contrast) but not `buffer`,
`path`, `cache`, `data` or `blob`: `widget::apply` has no arm for them. So an
editor that opens a second file must close its window and send a new GuiDef,
losing the navigation, the selection and the group the user was working in. The
question to settle is whether a source swap is worth its cost — it drops the
peak pyramid and the STFT cache and re-fetches, which is most of the work of a
new def anyway — or whether the honest answer is that a view *is* its source
and the client should redefine. If it lands, the reset must be explicit about
what survives (the view window? the selection? the link group?), which is the
part that makes it a design and not a patch.

**A canvas starts at the host's size, not the page's.** The browser front
creates every surface with `CANVAS_SIZE` (480x420, `host::web`), overriding
whatever the element already measured — so a page that sizes its canvas before
the def goes out sees it reset. There *is* a way through, and it is the right
one: `GuiBridge::resize`, which is what a `<clausters-bundle>` drives from its
`ResizeObserver`; the web client now offers it to any script as
`guiHost().fit(defId, element)`, so this is no longer a wall. What remains is
the smaller correctness point: when the page supplies a canvas that already has
a size, the front should adopt it instead of stamping the default on top and
waiting to be corrected.

**Acceptance:** ✅ an editor plays a selected region in a loop and the line
sweeps that region, wrapping, with one message sent when playback starts and
none after — the sweep is shared code, so both fronts get it from the one
`head_at` (on `GroupState` since E3); a view fed a second buffer redraws from it without the
window being redefined, if the source swap lands; and a canvas supplied at a
size keeps it, with no `resize` needed to correct the default.

## G33: every live view names a bus and a rate

- ✅ **G33 — Every live view names a bus and a rate; the sample ring is the
  server's** *(done 2026-07-28)*: the four live data views took three different
  kinds of number — a `meter` a control bus, a `scope` either a control bus or a
  *tap* (an index into the segment's eight sample rings), the goniometer and the
  spectroscope only taps — so the surface claimed two kinds of signal where the
  server has one kind at two rates, and the ring index (which the caller had to
  allocate, route and release) leaked through the wire, the host, both clients
  and every example. Now each view takes **`bus`, `rate` and `channels`**
  adjacent buses (the SuperCollider model; audio is the default and bus 0 is the
  first hardware output, so a bare `scope()` is the console scope). `/bus_tap bus
  watch` replaces `/bus_tap tapIndex bus`: the **server** picks the ring and
  publishes the choice in a per-bus directory in the segment (ABI v3 → v4),
  counting watches so views share one ring and the last one to stop frees it;
  the **host** is what turns a widget into that command, diffing the audio buses
  its open documents read on every def/free/set; a `/bus_tapStream` subscription
  *is* a watch, so a browser client never sends it. A `meter` needed a path of
  its own — eight rings cannot back a mixer, and a meter wants one number per
  block, not samples — so the segment also carries a **per-bus level**, the
  block peak held with a 20 dB/s decay (correct for a reader a dozen blocks
  slower than the engine, and for several readers at once, which a
  reader-clears-the-max scheme is not). Ported to both clients in the same
  change; the Python and TypeScript tap allocators are gone. Rationale in
  `docs/decisions.md`.

## G34 — The multitrack editor reuses the widget set that grew past it

Found while reviewing `composer.py`. `clausters.gui.Editor` composes its window the way it did when the lanes first landed (G22) and has not followed the widgets that landed after — so the Python multitrack view builds by hand what the catalog now provides, and the two multitrack views have **drifted apart**: the web's `composer.html` already rules from the top with the `timeruler` G32c added, while the client that is supposed to be the reference for the port does not know the widget exists. That inversion is the milestone's real subject; the individual gaps are small and mostly mechanical.

- **The ruler is elected, not placed.** `Editor.draw` walks the lanes backwards and mutates the last `track` dict (`clients/python/clausters/gui/editor.py:257-260`, `lane["ruler"] = "beats"`), which is exactly the pattern G32c replaced: the strip is reserved out of *that lane's* height, and because a patch lane is skipped when electing, it can land **between two lanes** rather than under the stack — the failure G32c named, still reachable from Python. Place a `timeruler` above the lanes instead.
- **The editor names no navigation group.** No lane carries `link`; the stack shares an axis only because `Widget::link_lanes` keys an un-linked lane by the window root (`crates/clausters-gui/src/host/widget/mod.rs:1327-1337`). A `timeruler` joins the group its `link` names, so it cannot join a group that has no name — which is why the ruler fix drags this one along, and why the web example writes `link: 1` explicitly.
- **The lanes are not told the bar.** `_lane` passes `sample_rate`, `tempo` and `snap` but not `quant` (`editor.py:713-720`), so a beat ruler labels beats with no bar grid; the web composer passes `quant: 4.0` to every lane and to its ruler.
- **Nothing sizes a lane.** The editor gives no height and could not: `track`'s `height` is vestigial (G32c: "read by nobody"). Every lane is therefore elastic weight 1 and any chrome a script hands to `extra` takes a lane's whole share — `gui_composer`'s transport did precisely that until it was given a fixed `h`. Decide it here: a lane sizes with the place props everything else uses (`h`/`weight`), and the dead `height` prop goes.
- **The window sets no `margin`/`gap`** (`editor.py:261`), so the lanes touch the frame and each other; the web composer uses 8 and 6.
- **The transport strip is copied, not built.** `Transport` is the shared *logic*, but the same four buttons are hand-written in `composer.py`, `score.py` and `score_from_data.py`. A builder for the strip (named buttons, a fixed `h`) would end the copy and give the sizing one place to be right.

**Open — the stack does not scroll, and the vertical scroll view cannot simply wrap it.** Lanes are children of the window in a `col` layout, so a composition of many lanes divides the window instead of scrolling. The obvious fix — a `scroll(axis="y", zoom=False)` around them, the constrained form L3 fixes — collides with the gesture the lanes already own: `Gestures::wheel` gives a timeline view the wheel first (`crates/clausters-gui/src/host/gestures/mod.rs:1824`) and only falls through to the workspace when nothing claimed it (`gestures/mod.rs:1859-1862`, "a widget with its own wheel won above"), and a drag inside a lane is the clip's. A scrolled stack of full-width lanes would therefore scroll nowhere but over its own gaps. So the scrolling model is a decision, not an edit: lane heights plus a scrolled viewport, with the wheel arbitrated (a modifier, a header/gutter that yields it, or a visible scrollbar) — and whatever is decided holds for the piano-roll and notation views, which own the wheel the same way.

**Acceptance:** a composition of a dozen lanes is navigable in a window that shows four, with the ruler above the stack labelling bars, the lanes aligned on one named group, and a transport strip that stays a strip — checked by eye through `composer.py`, and ported so the Python and web composers compose the same window.

- ✅ **G34 — The multitrack is one widget, and it owns the arrangement**
  *(designed 2026-09-08 with the user, after two days of the same defect
  arriving as a playback bug)*.

  **The defect first, because the design is its shape.** A clip's placement
  leaves the host under one of three tags and the *gesture* picks which:
  `"clip"` for one clip inside its lane, `"clips"` once a selection exists
  (addressed to the **lane**, naming every held clip by id), `"lane"` when the
  clip crossed the stack (addressed to the **clip**). They replace each other
  rather than accompanying each other, so a reader that takes `"clip"` alone is
  correct until the first marquee and silently wrong after it — with no error
  and no missing pixel, because the host has already moved the picture. Two
  independent readers got this wrong: an example, and **the host's own document
  owner** (`document.rs::read_event` handles `"clip"` and neither of the other
  two, so a block move or a lane change in `--session` never reaches the
  document).

  **The cause is not the tags, it is that a multitrack has no owner.** A
  `pianoroll` is one widget that holds its notes and reports `"notes"` — the
  whole list, the *result* — after any gesture whatever, which is why it has
  never had this bug. A multitrack's structure is spread over N `Track` widgets
  under whatever generic container the script picked (`WidgetKind` is `Window |
  Panel | Stack | Scroll | Track | TimeRuler | Clip | Custom | Unknown`: there
  is no multitrack container, and `build.rs` never asks a `Track` who its parent
  is). With nowhere to report *the arrangement*, a gesture reports *what the
  hand did*, to whichever widget it touched.

  **So: `multitrack` is a heavy widget, on the `pianoroll`'s shape.** It draws
  its own ruler, its own lane headers (name, mute, solo, gain), its own vertical
  scroll and the clips on the shared time axis — and `track` and `clip` stop
  being widgets in the tree and become **data in its props**, exactly as a note
  is data inside `notes`. A lane cannot sit in a void, which is the whole point:
  it is always inside the view that owns it.

  | prop | per entry |
  |---|---|
  | `lanes` | `id name height mute solo gain` |
  | `clips` | `id lane offset dur start source label` |
  | the axis props it already understands | `sample_rate`, `snap`, `view_*`, `playhead*`, `link`, `ruler` (or off) |

  **Two edit-backs, each stating the result**: `"clips"` (the clips as they now
  are) and `"lanes"` (the lanes as they now are, absorbing today's `"mute"`,
  `"solo"` and `"level"`). Two structures, two payloads, one widget — the split
  `notes`/`osc` already makes on the roll, and it keeps a gain drag from
  resending every clip. Each is idempotent and its own inverse is the previous
  list, which is the rule `clausters-document` states for `events`, `points` and
  `samples`.

  **What that does to the defect: it dissolves it rather than fixing it.** An
  earlier proposal — one always-plural payload carrying the lane per clip —
  repaired the symptom. This makes the question not arise: if the report is the
  arrangement, nothing has to say *which* clips moved or *how*. Moving one,
  moving a block, crossing a lane, trimming, quantizing, splitting, deleting are
  all "the clips are now this". **No gesture is left on the wire**, and the
  document owner's bug closes by construction because there is one tag to read.

  **The client objects wire nothing.** A `Multitrack` view holds lanes and
  clips, has verbs to add, move and remove them, and an `on_change` that hands
  back the arrangement. It never sees a widget id and registers no per-clip
  handler. Being a `View` over a structure with a `Domain`, it goes through
  `edit()` and **inherits the history** — which closes "half the editors a hand
  can use have no history" for the multitrack as a side effect rather than as a
  second job.

  **The multitrack places; a clip is entered to edit** *(decided by the user,
  2026-09-08)*. This is the question that shapes everything else, because a clip
  today may carry an editable body (a roll, a curve) with gestures and an
  edit-back of its own. It does not follow the clips into the props: the
  multitrack owns **placement** and draws bodies read-only, and a body that is
  *edited* is entered — opening the clip shows its own editor. That is what a
  DAW does, it keeps the heavy widget from becoming every widget, and it is the
  same line the three applications are drawn on: the multitrack places, the
  audio editor and the score editor edit.

  **What it costs, recorded rather than discovered.** The whole list travels on
  every edit, which is the trade the roll already makes with its notes and which
  a piece of thousands of clips pays more for; the crate chose that shape
  deliberately for this seam, so it is inherited knowingly. It **breaks the
  multitrack surface in both clients** (pre-1.0 tier): `"clip"`, `"clips"` and
  `"lane"` leave the wire, and so do `track` and `clip` as widgets. And the host
  grows a large element — though almost none of it is new code: `graphics/track.rs`,
  `placement.rs`, the gesture machine, the rulers, the layers and the playhead
  all stay, and what changes is **what they take as input**, which is exactly
  what the AP track's reading of the host's multitrack code predicted for `O24`
  (`crates/clausters-document/PLAN.md`).

  **Acceptance:** the multitrack example composes a window by describing lanes
  and clips and never registers a handler on one; every gesture — a drag, a
  block drag, a lane change, a trim, a quantize — arrives as one `"clips"` or
  one `"lanes"`; `Ctrl`+`Z` walks them; and `clausters-gui --session` applies a
  block move and a lane change to its document, which it cannot do today.

  **`Ctrl`+`Z` in the clients landed 2026-09-09**, and it took the mapping being
  lowered first: `clausters.gui.editing.MultitrackEditor` is the piece as one of
  the fundamental structures, so it has the history every other editor has and
  two windows over one piece walk one stack. What made a piece not a structure
  before was that its picture and the reading of its report were written per
  client; both are the crate's now
  (`multitrack::picture`, bound as `multitrack_picture`/`multitrack_read`), so
  what a client adds is the axis its window counts in and which server buffer a
  source was read into. `clausters.gui.Multitrack` stays what it is — the
  view-only object for a script that just wants boxes and no history.

  It is `O24`'s multitrack seen from the host's side, and `O24` waits on it: the
  application cannot be written against a widget tree that has no owner.

  **`track` and `clip` stopped being widgets, 2026-09-09** — the cost the
  milestone declared, paid. `WidgetKind` lost both variants and `field` went
  from three things told apart by what was on it to **one**: the free-standing
  ruler. What went with them, and it is most of the milestone's diff: the lane
  and clip arms of the build, the layout's whole `place_on_time` pass (nothing
  is placed on a time axis by the layout any more, so `AxisSource`,
  `Space::time` and `Placed::time` went too), the clip drag and the lane-level
  drag, the header hit and its press, `LaneClips` and `clips_span` and
  `window_nav`, the frame's `TrackItem`/`ClipItem`/`ClipBodyItem` passes, the
  `clip_verb`/`clip_quantize` keys in both shells, and the `"clip"`, `"lane"`,
  `"clips"`, `"mute"`, `"solo"`, `"level"`, `"split"` and `"join"` tags on the
  wire — the multitrack emits its own `"clips"` and `"lanes"`, which is the
  whole point.

  Two things were **generalized rather than deleted**, because the question
  they answered is still real: `Element::unbounded_axis` is the one a lane used
  to answer by being a `Track` (a piece is composed into the space after its
  last box, so its axis is not bounded by what it holds, and that is where its
  authoring headroom comes from); and the `layers` module is kept whole,
  unreached, because the container it was written for is exactly what an
  **entered** box brings back.

  **What it cost in tests, said plainly:** about 80 tests went with the widgets
  they exercised, and `widget/reconcile`'s suite was **rewritten** rather than
  dropped — the reconcile is `O23`'s and its subject is any tree, so its cases
  came back on `multitrack` and `layout` widgets. The two clients lost their
  `field`/`track`/`clip` builders, the parity vectors were regenerated, and
  `docs/gui-protocol.md`, `docs/gui-props.md`, `docs/architecture.md` and the
  Python book's GUI chapter say what a `field` is now.

  **Found while doing it:** `clausters.gui.multitrack` was the *submodule*, not
  the builder — importing `clausters.gui.multitrack` binds the module over the
  name `guidef` had put there, so a script asking for the widget got something
  it could not call. It only bit now because this is the milestone that makes
  that builder the way to draw a piece. Re-bound after every submodule is in.

  **A box's base view is what its contents are, 2026-09-09.** The picture is no
  longer this element's own: a box of samples draws through the signal element's
  body door and a box of notes through the roll's — the very elements that stand
  on their own elsewhere, handed the box's axis and drawing no chrome. A box is
  a window onto a picture, never a second implementation of one, and that is
  what keeps a take in a piece and a take in a window from drifting.

  Two props carry it. `notes` is the flat `box start dur pitch velocity channel`
  sextuples — one list for the widget, each note naming its box, the shape every
  payload here has — and `view` names the presentation a box of samples takes,
  for every box at once because every box drawn the same way is the normal case.
  A roll body is fitted by the crate's own `pitch_window`, so a box and a window
  over the same notes are the same height.

  **The spectrogram body landed 2026-09-09**, and it took a slot growing a
  second half to its key. A GPU slot was addressed by the widget alone, so a
  widget could hold one picture; it is `(widget, key)` now, and a multitrack
  keeps one per buffer its boxes are windows onto. The samples reach it twice on
  purpose: a time-frequency preset asks for the bulk in pyramid form, so a body
  over a *server buffer* never sees raw frames and its slot is never dirtied —
  the element keeps the frames it was handed and runs the transform itself when
  the slot is filled, which is what a standalone spectral view has its loader
  do.

  **The light views landed 2026-09-09, and there are two of them.** A
  break-point automation is one element in two places, and the place is the
  whole difference — the user's rule, stated 2026-09-09: a **track
  automation** is a row of its own under its lane and runs the whole timeline
  (`curves`, the flat `name lane label min max height` sextuples), a **clip
  envelope** is a layer inside its box and runs as long as the box does
  (`layers`, the flat `name box label min max` quintuples — no height, because a
  layer is as tall as what it is drawn on). `points` carries every curve's
  break-points in one list, each naming its curve the way a note names its box.
  Both are drawn and edited by the `curve` element in its body form, so the
  light views are editable where the base view is read-only, and `layers.rs`'
  one-at-a-time rule finally has the container it was written for: `layer` names
  the curve in hand (a curve has a name, so it is named — the `points:1` ordinal
  is for containers whose layers are anonymous) and `hidden` the ones that are
  not drawn. The vertical axis stopped being the lanes and became the **rows**
  (`graphics::multitrack::Stack`), since an automation row pushes the lane under
  it down.

  **The curves reached the document the same day.** A piece's picture carries
  them — `curves` for a track's automations and `layers` for a region's, the two
  lists a caller draws in two different places — and a report of them is read by
  the crate too (`multitrack_read_points`), as one `SetAutomation` per curve
  whose break-points actually moved. Both clients' editors bind those two, so a
  break-point dragged in a piece is an entry in the piece's own history and an
  undo puts the *shape* back, since a point's `data` is carried and never read.
  The one thing left to the client is the **value domain**: the document says
  what a curve automates and never reads it, so which range that parameter has
  is stated where the parameter is — in the target — and unity is what an
  unlabelled level means.

  **Entering a box landed 2026-09-09**, and it needed a gesture the host did
  not have. A double click is now the machine's — winit reports no click count
  and a browser reports one of its own, so the rule lives beside the other
  gesture rules and each front hands over only its own wall clock, which the
  agnostic core cannot have. A double click on a box reports `"enter"` with its
  name and nothing else; what that box *opens as* is the client's, out of the
  same `Sources` table that already holds the one fact about a piece that is not
  in the piece.

  **One undo order, and it is the piece's** *(decided by the user, 2026-09-09,
  against the alternative of a window with an order of its own)*. An undo that
  needs a window reopened to reach it is a hole in the history that does not
  announce itself, so the entered editor is opened on the piece's editing
  context. What it costs is that the entered structure stays registered in that
  context while the piece is open even if its window closes — which the context
  already did, since it holds what it registered. An editor grew a third kind of
  event for it: beside an edit and screen state, an **interface event** is a tag
  that asks the editor for a *window*, which is not something a vocabulary of
  edits can say.

  **Still open, and each has its home:** assembled samples (a box over several
  segments of several files); a box of notes reaching the editor's picture (the
  widget draws one, and `MultitrackView` states no `notes` yet); and
  the web twin of the Python example — **written 2026-09-10** as
  `examples/editors/edit-multitrack.html`, beside the Python example's own
  rename to `edit_multitrack.py`.

## G35 — The multitrack editor, refined by using it

Written 2026-09-09 from the first session spent **using** the editor rather than
building it: the multitrack example opened, played, cut and joined by hand
(`multitrack_audio.py` then, `edit_multitrack.py` since 2026-09-10). The
fourteen points below are what that session found, and they are one milestone
because they are one subject — a piece being edited — and because several of
them wait on two structures the rest need: a second cursor, and a node chain
that says where a gain is applied.

**The list is in the order they are built, and the numbers are the order they
were found in**, so a point keeps the name it was named by. Where two of them
turned out to be one piece of work they are **one entry** here rather than two:
splitting them would mean writing one rule in two places, and the second half
would be written against a picture the first half had already changed.

- ✅ **G35.12 — The ruler is always there, and it points at what it rules.**
  A multitrack window opened by `edit` has no time ruler at all: the widget's
  own `ruler` is a strip inside its rectangle and the editor never places a
  `timeruler` above the stack, so a piece opens with no numbers on it. A ruler
  is not optional in an editor — it is how a position is read, and after G35.1
  it is the only place a position is *placed* — so the editor places one.

  And a ruler's marks belong on the side its content is: **a ruler above the
  stack draws its ticks and numbers along its bottom edge, one below draws them
  along its top**, so a tick and the pixel it names are adjacent instead of
  separated by the label. A prop of the strip (`dir`, `"down"`/`"up"`), because
  only the document knows which side of the ruler the content is on, and it
  applies to every ruled view rather than to this one.

  **First**, because reading a position is what every other point is checked by
  eye against.

  *Done 2026-09-09.* `dir` is an **x-axis** property (`RulerDir`, `down`/`up`)
  rather than the ruler widget's, because a strip is what every ruled view
  reserves: `draw_ticks_h` takes it and mirrors the tick, the label and a
  marker's arrow together, so the whole row turns over at once. The default is
  `up` everywhere the host reserves the strip itself (under a body, which is
  the drawing that already existed) and `down` for a free-standing `timeruler`
  — `EditorProps::parse_ruler`, since a document that places a ruler places it
  above what it rules. `MultitrackView` now builds the window as ruler over
  piece, both in **one navigation group** (`group()`, the piece's own widget
  id when the caller named none) — without that the ruler is a group of one
  and pans away from the lanes it is ruling. The two `recording` examples,
  whose ruler is under its lanes, say `dir="up"`.

- ✅ **G35.1 — Two cursors, and only one of them is placed.**
  Playing and then clicking somewhere else leaves the sound running from where
  it was while the mark sits where the hand pointed. That is not a bug in the
  seek, it is two cursors wearing one name: the protocol states "a window has
  **one** cursor", placed by a click anywhere on the axis, and that click *is*
  the transport seek. They are two and always were:

  - the **position cursor** — where a playback starts and where a paste lands.
    Placed by a click that landed on **nothing** — the time ruler, the slack
    between boxes, a grid nothing is drawn on — and never by a click *on*
    something, which is that thing's; the content never moves it, playing never
    moves it, and it stays where it was put.
  - the **playhead** — the line that sweeps. Never placed: it *starts* from the
    position cursor, and while the transport runs it is the engine's own
    position.

  So `"locate"` stops meaning "seek here" and starts meaning "the position
  cursor is here"; play seeks the transport to the position cursor; and a click
  on a box, a lane or a roll's grid moves nothing at all. The wire has two props
  for two lines already (`playhead_at`, `playhead`); what it has not got is a
  *position* that is neither, group-wide like the rest of the axis' state.

  It changes a rule the whole protocol states, so it goes early, before anything
  is checked against the picture it corrects.

  *Done 2026-09-09.* The position cursor is `cursor`, an **x-axis** property
  and therefore the group's (`GroupState::cursor`, `TimeSpace::cursor`), drawn
  in its own theme role beside the playhead's — two lines, two colours, two
  questions. Placing it is one rule in one place: the click the machine already
  derived at the press now survives **only where the press landed on a ruler
  strip** (`Frame::ruler`, which the hit-test already answered for the gesture
  table), so a box, bare stack and a roll's grid place nothing. `"locate"` stops
  meaning *seek* and starts meaning *the reader is here*: `set_timeline_cursor`
  moves the mark and touches neither playhead prop, so a click mid-playback
  leaves the music alone. A key gesture anchors on the cursor and falls back to
  the playhead only where none was ever placed (`cursor_of`), which is what
  makes a cut and a paste land where the hand pointed rather than where the
  music got to. The session host cues a **stopped** transport to the mark and
  leaves a rolling one alone; a client hears it as `Editor.on_locate` /
  `onLocate` with `Editor.cursor` kept beside the selection, and
  the multitrack editor cues its own transport from there (`MultitrackEditor.locate`,
  since 2026-09-10; the example did it by hand before that).

  *Amended the same day, from the first session using it.* Three things that
  session found, and they were one gesture between them — point at a box, put
  the mark down, split it. The mark **never reached the editor**: since the
  ruler became a widget of its own the one gesture that places the cursor
  happens on a strip the editor had not named, so `"locate"` went to nobody and
  play started wherever the last pass had left it; the strip is a named widget
  of the picture now, and a piece opens with the cursor stated at its top.
  Pressing the ruler also **took the keyboard away** from the stack, because a
  ruler takes no focus and a press on a widget that takes none drops it — it
  takes none away either, now, when it rules the very axis the focused widget
  is on. And **a click on a clip selects that one**, which is what Alt+click
  alone used to do: selecting is the common act, adding to a selection the rare
  one. Decided on release, because a press is not yet a gesture. Then, asked for
  once the three were in: a click on the **slack** places the mark too — the
  space between boxes, a lane's empty tail — so a mark can be put down without
  reaching for the strip at the top of the window every time. The rule that
  survives is *a click places the mark where it landed on **nothing***: a press
  an element takes is that thing's.

- ✅ **G35.2 + G35.13 — The header is a surface, and the track is what it
  addresses.**
  One entry because they are one hit-test and one visual state: the header
  answers a press for its three controls today and for nothing else, which is
  why the track itself cannot be pointed at. Three verbs land on the space
  beside them.

  - **A click on a track's empty header space selects that track**, with a
    visual indicator saying so *(G35.2)*. Not decoration: a paste needs two
    coordinates and the piece only had one — the position cursor says *when*,
    the selected track says *where* — so without it a paste can only guess the
    lane it came from.
  - **A double click on the empty header space adds a track** *(G35.13)* — the
    gesture a desktop already spends on "open this", spent here on "make one",
    which is what the stack has no other way to ask for.
  - **Delete removes the selected track and everything on it** *(G35.13)*.
    Today Delete is the held boxes'; with a track selected it is the track's,
    which is the ordinary rule — a delete acts on what is in hand, and the
    header is what puts a track there.

  Open, to settle when it is built: one track selected or several, and whether
  the selection is the hand's (like the box selection, which nothing on the wire
  states) or the view's — a paste anchor needs one, a mixer strip may want more.
  And whether the double click that adds a track is the one **below the last
  header** (where there is no track to point at, so nothing else could be meant)
  or anywhere on a header no control was hit: the first is unambiguous, the
  second is what a hand reaches for.

  *Done 2026-09-09, and the two open questions settled as they were built.*
  **One** track, and the selection is the **hand's** — nothing on the wire sets
  or reports it, exactly as nothing reports which boxes are held: a paste needs
  one anchor, and a mixer strip wanting several is a different question than
  this one. **Both** double clicks add a track: anywhere on a header no control
  was hit (which is what a hand reaches for) *and* the band under the last one
  (where there is nothing to point at) — one rule, since the new track goes
  after the row that was pointed at and at the end when there was none.
  `HeaderPart::Body` is the hit-test's new answer and the whole of what the
  three verbs needed.

  The report is `"lanes"` for all three, and that is what made this bigger than
  a hit-test: a row report could only say *mute, solo, fader* because each
  client read it **itself** — the one payload of the three that had no reader in
  the crate. So `picture::read_rows` is that reader
  (`clausters_multitrack_read_rows` / `multitrackReadRows`, `CORE_ABI_VERSION`
  48), and with it a row report says everything a piece's tracks can be: an id
  is that track, a name that is no id is a track a hand made (minted with one
  empty lane), a track the report leaves out is **gone with its boxes**, and the
  order is the report's. One `SetTracks` whatever changed, so adding a track and
  deleting one with twenty boxes on it are each one entry in the history.

  Found on the way: **`fresh_id` did not count the curves**, though a piece looks
  an id up by number without asking what kind of thing it expected — so the first
  track added to the example was minted onto its track automation's id.

- ✅ **G35.9 — The trim grip stops blinking.**
  The grip is drawn only while the pointer is inside the box's rectangle, and
  dragging an edge takes the pointer outside it — so the affordance disappears
  under the hand that is using it. A held edge draws its grip whatever the
  pointer is over; `track::clip_grip_on` exists for exactly this case and nobody
  calls it.

  A line of work, and it stops an affordance from lying, so it goes before the
  window work that is done by dragging those edges.

  *Done 2026-09-09.* Two answers in one place (`Multitrack::lit_grip`), and the
  order is the fix: what the hand is **holding** is asked first, and only then
  where the pointer is. `clip_grip_on` was written for exactly this and had no
  caller.

- ✅ **G35.8 + G35.10 — The window over the source: what a box reads, and from
  where.**
  One entry because it is one arithmetic seen at both ends — a policy at the
  edges of the window, and that window reaching whatever sounds.

  - **A trim knows what its content can do** *(G35.8)*. A box is a **window**
    onto a file — a fragment that can be slid over the same file's contents — so
    pulling an edge past what the file holds has three defensible answers, and a
    multitrack usually offers the choice rather than deciding: leave the silence
    (what happens now), **stop the edge at the content**, or **loop the content**
    under the longer box. A policy of the box, so it is stated on the box.
  - **The halves of a split play what they show** *(G35.10)*. `e` cuts and the
    picture is right; the sound is not. A split moves the second half's window
    over its source (`start`), and whatever sounds has to read from that offset
    — the example's reader ignores it and starts every box at frame zero, so
    both halves play the beginning.

  *Done 2026-09-09, and the three answers turned out to be two.* **Leave the
  silence** is what a box over samples nobody loaded does, and it needs no
  policy: there is no length to stop at. **Stop at the content** is not a
  property of a box at all — it is what an edge drag does when there is
  something to stop at — so it is the default and says nothing. **Loop** is the
  only one of the three that changes what *sounds*, so it is the only one the
  document holds: `Content::Window` gained a `loop` flag, the picture carries it
  (`Box::looping`), and the widget takes it as `loops`, a **name set** like
  `hidden`. Not a field of the `clips` septuple, because nothing in the widget
  changes it and a report carrying it would be reporting a fact it cannot edit —
  a gesture that toggles looping is its own piece of work, and it will report it
  its own way.

  With that, `placement::drag` finally gets the `Contents` it was written to
  take (it was handed `Contents::default()` — unbounded — at the one call site
  in the piece), and the same statement decides the picture: a looping box draws
  its samples wrapped, through the `SourceWindow` the body door already carries.

  And the other end: **a report whose `start` moved now says so**
  (`picture::read`, `content: Some(...)`). A trim of the left edge slides the
  window over the source — that is what makes an edge drag a trim and not a
  squeeze — and dropping it left the document believing every box read from
  where it always had, which is what made both halves of a split play the
  beginning. The example's reader takes that `start`, and a `loop` flag beside
  it.

- ✅ **G35.11 — Join, defined — and the snap that makes it reachable.**
  `j` does nothing: it asks whether two placements are within **one sample** of
  each other (`placement::adjacent(a, b, 1.0)`), and with no quantization a hand
  never lands one there, so it never has two boxes to join. The snap is **inside
  this point** rather than beside it, being what gives `j` two boxes to act on:

  - a drag inside a lane **snaps to the edges of the boxes already on it**,
    within a tolerance, so two boxes can be made to meet at the sample; a hand
    that keeps pulling past the tolerance goes on and **overlaps** them, which
    is a crossfade and legal. A snap to *content* rather than to a grid, so it
    stands beside `snap` and does not replace it.
  - then the rule, one sentence per case: the held boxes of **one track** join
    (a track is what makes two boxes joinable, as a pitch is for two notes); a
    **gap** between two of them becomes silence in the joined box; an
    **overlap** mixes the two signals rather than one winning.

  A join is therefore an edit of *contents* and not only of placements, which is
  the part the document has to answer: a joined box is a window onto more than
  one source — the same shape "assembled samples" asks for, which is why those
  two land together. It follows the entry above because it needs that window
  vocabulary, and it needs the drag to be settled first.

  *The snap done 2026-09-09, and with it `j` on a run that touches.* A drag
  pulls onto the edges of the boxes already on its lane, within `SNAP_PX` device
  pixels crossed to time — a **screen** distance, the same kind of number as the
  hit slop, because a tolerance in samples is unreachable zoomed out and
  enormous zoomed in. The box under the hand is what snaps and the rest of a
  block travels with it; an **edge** drag snaps too, which is how a gap is
  closed by trimming rather than by moving. With two boxes meetable, the `j`
  that was already written does what it says: the run merges, and for the two
  halves of a split — one source, contiguous windows — the result is the box
  that was there before the cut.

  **Still open: a gap and an overlap.** Both are edits of *contents* rather than
  of placements — a joined box would be a window onto more than one source, with
  silence between them or a mix where they meet — and the document has no
  vocabulary for that yet. It is the same shape **assembled samples** asks for
  (`G34`), and the two should land together rather than one inventing a form the
  other has to live with. Until then `j` joins what touches and leaves what does
  not, which is honest: nothing is silently mixed or padded.

  *Closed 2026-09-14 as a milestone of this track.* A join is a window onto
  segments of several sources now (`/buffer_stitch`, the multitrack's join
  vocabulary), so what is left is not the host's: how a join treats a gap and an
  overlap is `crates/clausters-document/PLAN.md`'s "A join takes its clips' fades,
  and invents none", with the clip fades it stands on beside it.

- ✅ **G35.3 + G35.4 — The chain: a clip's gain and a track's gain are two
  controls, and both curves play.** *(closed 2026-09-10)*
  One entry because it is one **node structure** at two levels, and building
  either level alone would decide the other by accident.

  - a **clip group**: the source or playback node, then the clip's own curves
    (its dynamic envelope, its pan, its per-clip effect parameters), then the
    clip's fader *(G35.3)*;
  - a **track group** holding the clip groups, then the track's curves, its
    effect chain, and the track's fader *(G35.4)*.

  A gain at the clip and a gain at the track are two controls in that order, and
  the picture already says so: a clip envelope is drawn inside its box, a track
  automation in a row of its own.

  What each level owes: **the clip's envelope drives the clip's own gain**, and
  the box `white` — which draws no contents and makes no sound while its
  envelope is drawn, though the samples appear when the box is entered — is what
  found it. The sound half is already understood: the example reads the envelope
  once, at the box's first break-point, where it is zero. The picture half is to
  be diagnosed, whether the base view is not drawn or is covered by the layer
  over it. **The track's automation drives its fader**, which is drawn, edited
  and undone today and does nothing.

  Both are the same second demand: a curve is a control that **plays**. Open,
  and to settle here rather than per parameter, because everything the piece
  automates later goes down the same path: whether a curve is played by a node
  reading the transport's position (the shape the readers already have) or
  written into a control bus, and where the curve's samples live — with no
  message per frame either way, the discipline the playhead already keeps.

  *Closed 2026-09-10 by the node system.* A clip and a track are nested
  `GraphDef`s (`mt.clip`, `mt.track`, `mt.piece` in `clausters_core::mixer`), each
  with its own `gain` port in that order, and a curve **plays**: `mt.curve` reads a
  table on the transport's position and writes a control bus, and `/graph_map`
  maps the port to it, so a curve drives a control three levels down with no
  message per frame and survives a locate. The header's knob writes the same
  port the curve maps. What is not built is the effect chain between the curves
  and the fader, which waits on what an effect is (root `PLAN.md`, Future
  directions).

- ✅ **G35.14 — The header's level is a knob, and a track shows what it
  produces.**
  Straight after the chain, because the meter is the first thing that reads what
  the chain produces.

  A header is a narrow band beside a lane, and a horizontal groove long enough
  to be read takes the width the name needs — so the level control is a **knob**,
  which reads and turns in the space a header actually has.

  *The knob done 2026-09-09.* A square cell like the two toggles beside it, and
  the **same dial** a `knob` widget draws (`controls::knob_dial`, factored out
  of it) rather than a second drawing of the same idea — a control that read one
  way here and another way there would be two controls. It turns by a
  **relative** vertical drag, which is not a detail of the drawing: a dial has
  no left and right end to put the pointer between, so an absolute reading would
  jump the value to wherever the press landed. The header also stopped asking
  for the width a groove needed, so a stack of tracks is narrower than it was.

  *The meter done 2026-09-11.* A thin column per channel down the right edge of
  the header, over the amplitude the track is producing **after everything has
  been applied** — its clips' own gains, its curves and its fader. That is the
  number a hand on the knob is watching, and the one place in the piece where
  the picture is of the *sound* rather than of the description.

  **What it reads is a bus, not a value.** The `meters` prop says where to look
  — the lane, the first bus of the level run, the first of the mark run, how
  many channels each is — and the host reads them every frame out of the shared
  segment, so a level that moves every block costs no message at all. A prop of
  its own rather than two more fields on `lanes`, because `lanes` is what a hand
  *edited* and a bus number in it would be a number the host was expected to
  hand back unchanged.

  Three things the drawing had to get right. The strip comes off the right edge
  **before** anything else is laid out, so a meter is beside a name rather than
  over it, and the group's indent holds it (`gutter`) or the band would take it
  out of the name. Its width follows the **channel count and nothing else** — a
  level is not a size, or a moving meter would move the controls under it, and
  the hit test (which never reads a bus) lays the header out where the drawing
  did. And the column stands in **decibels**
  (`clausters_core::measure::meter_fraction`, beside the ballistics for the same
  reason they are there): half of unity is -6 dB, which is a tenth of the way
  down a 60 dB strip and not half of it, and a linear column spends nine tenths
  of its height on the top 20 dB nobody mixes in.

  The other half is the clients': `Playback` allocates `2 * channels` control
  buses per track and adds **two** meters to the track's slot — one with no hold
  for the level, one with the core's hold for the mark that waits — so a piece
  nobody plays holds none, and both clients do it with the same two calls.

  *The scale done 2026-09-11, after the first eye pass.* Three things the first
  cut got wrong, and all three were general rather than the strip's: the column
  stood in a well the colour of the header, so one track's meter ran into the
  next (`meter_field`, darker than any panel); it was one flat colour, where a
  meter is read by **where** it changes colour (`meter_low`/`meter_mid`/
  `meter_high`, ramped over the height with the two levels the core now names —
  `METER_WARN_DB` at -18 dBFS, the field's alignment level, and `METER_HOT_DB`
  at -6); and it was drawn by the strip's own code, so a `meter` widget and a
  mixer would each have grown their own. The column is now
  `graphics::meters::draw_column`, the one column this host draws, and the
  widget renders through it. The pass also turned up why the right channel moved on
  its own over a mono take, which was the def and not the drawing: the meter's
  UGen indices advanced by two where each channel pushes three, so the second
  channel wrote its raw input to the bus and metered its neighbour.

- ✅ **G35.6 — An entered box keeps its own history.**
  After editing a box's samples, closing that window and opening it again,
  `Ctrl`+`Z` in the box's editor steps the **multitrack's** last edit, and the
  sample edits are gone. One order is right — that is what `G34` decided — but a
  leg belonging to the take must still be the take's.

  Diagnose before deciding. The two candidates: the participant the reopened
  editor registers as, and whether the samples domain records an inverse at all
  through this path. (A first probe with a stand-in take could not reproduce it:
  the edit never applied, so the fault is further in than the registration.)

  Independent of every other entry, so it lands whenever the diagnosis is cheap.

  *Diagnosed 2026-09-09, and it is not in the client.* Both candidates are ruled
  out, each by a test that now stands as a regression: an entered box's editor
  is closed, dropped from the table and **built again over the same structure**,
  and an undo in the new one steps the stroke and not the piece's last edit —
  on the samples path and on a nested piece alike, in both clients. So the
  identity a reopened editor registers under is the context's and survives
  (candidate one), and the samples domain does record an inverse through this
  path (candidate two).

  What is left is the round trip the tests do not make: the host addresses undo
  to **the window the key arrived on** (`keys::history` emits on `ctx.def_id`),
  and the client answers it only for the editor whose `_window` that is — so the
  next probe is a live one, watching which window id the keystroke carries after
  a box has been closed and reopened, and which editor answers it. The TS
  stand-in also had to be corrected on the way (`setSamples` takes an options
  object, and the writes are a promise chain): a stand-in with a shape no client
  has is a test that passes against nothing, which is why the first probe found
  "the edit never applied".

  *Closed 2026-09-14, superseded.* The history a box's editor walks is no longer
  a client's: it is the applications crate's editing context (AP track, `AP7`),
  where two editors over one take are one structure under its buffer's key and a
  step is taken by the crate. Entering a box from a piece was taken out of that
  track on purpose, since the multitrack does not edit samples, so the live probe
  this entry wanted has no path left to probe.

**Two of the fourteen are not work**, and are here so a later pass does not
reopen them:

- ✅ **G35.5 — Entering a box works, and the multitrack follows what it edits.**
  A double click opens the box's structure in its own editor and the piece's
  picture updates with what that editor changes. Landed with `G34`.
- ✅ **G35.7 — Selecting and moving boxes works.** A sweep and a click select,
  and a box moves singly and as a block, across the stack.

**Acceptance:** the same session that found these, run again — a piece opened
with a ruler over it whose marks face the stack, located from that ruler, played
from the cursor it placed, a track added, selected and removed from its header,
a box trimmed against its content, two boxes snapped together and joined, a box
split and both halves heard from the right place, a clip envelope and a track
automation heard as two gains on one signal, a level turned on a knob that fits
its header with a meter beside it answering, and every one of them undone in one
order.

## G36 — Key bindings are configuration, not code

- ✅ **G36 — A key names a verb, and which key is set outside the code.** *(Done
  2026-10-04, as designed below.)* *(Added
  2026-09-14 by the user: the widgets' keyboard shortcuts should be definable as
  configuration, for instance from a config file and a GuiDef; configurable
  shortcuts will be very useful for complex applications such as the multitrack,
  audio and MIDI editors, and this way they are not hardcoded — **a milestone
  that needs design** before
  anything is built. It absorbs the future direction "A shortcut is the
  application's, not the widget's", raised by the user 2026-09-08 closing `G34`.)*

  **The design is the standard one** *(the user, 2026-09-14)*: it follows the
  conventions common to desktop and mobile interfaces rather than inventing its
  own, and where desktop and mobile conventions differ, choosing between them is
  part of the design.

  **What is wrong today is where the letters live, not which they are.** Every
  heavy view spells its own keys in its own `key` arm: a roll splits on `e` and
  joins on `j`, a multitrack does the same over clips, `q` quantizes in both,
  `Ctrl`+`C`/`X`/`V` move a block through the host's one clipboard, Delete
  removes, the space bar drives the transport and `A` in a track's header shows
  its automation. The two fronts read keys separately too (`host/gui/app.rs` and
  `host/web/input.rs`), which is how the browser's space bar came not to reach the
  window's transport while the native one did. A key is a **binding** — a name
  for a verb, chosen by whoever uses the program — and as a `match` arm inside the
  element that performs the verb, the same key cannot mean two things in two
  applications over one widget, a user cannot change one, and a verb no key names
  is unreachable though the element implements it. The audio, multitrack and
  notes editors (`crates/clausters-apps/PLAN.md`) are several applications over
  one set of views, which is what makes this stop being hypothetical.

  **Precedents in the host, each a shape the design can reuse or reject:**
  `Element::gesture_map` already did this for the pointer (a container's plan names
  the steps, the element performs them); and the look is set from outside the code
  at two levels — a launch-time table in the config file (`[gui.theme]`,
  `[gui.metrics]`, `docs/configuration.md`) and a protocol message a client sends
  (`/gui_theme`, `/gui_metrics`), with a `theme` prop scoping it to a subtree.

  **Decided with the user, 2026-10-04.** The user asked for the table to come from a file handed to the host at launch or read from the configuration directory, because an application here may have **no chrome at all** — no bar and no menu where a key could be discovered or a command reached by hand — so the keys are the only way in, and a program with none of its own still has to let its user choose them.

  - **A binding names a verb, and the table is `verb = chord(s)`.** One TOML table, `[gui.keys]`, where each entry is a verb's name and the chord (`"Ctrl+Shift+Z"`) or the list of chords that perform it; `[]` or `""` unbinds the verb. A chord means one verb: binding it to a second takes it from the first. It is keyed by the verb rather than by the key because that is what a reader changes ("undo should be this"), and because a verb with two keys (`redo`: Ctrl+Shift+Z and Ctrl+Y) is one line.
  - **One vocabulary, shared by every view.** A verb names the act, not the element: `split` is a clip's and a note's, `copy` is a block of clips', a block of notes' and a span of samples'. Each element performs the verbs it can (`Element::verb`), and declines the rest — exactly as it declined a key it had no arm for.
  - **The host's verbs and the application's.** The host performs `view_all` (R), `play` (Space), `loop` (L), `to_start` (Home), `to_end` (End), `copy`/`cut`/`paste`/`mix` (Ctrl+C/X/V, Ctrl+Shift+V), `quantize` (Q), `split` (E), `join` (J) and `delete` (Delete, Backspace). **Every other name is the application's**: the host performs nothing and reports it to the window's owner, `/gui_event <window> <seq> <version> <verb>` — the shape `undo` (Ctrl+Z), `redo` and `save` (Ctrl+S) already had, which are now simply the application verbs the table ships bound. So a program with no chrome gets a command by naming it in the table and answering the event, with nothing in the GuiDef. A host verb nothing took (a `copy` over no view) is reported the same way, so an application can answer it itself.
  - **A key bound to a verb the window's menu bar names is that entry's pick.** The desktop rule: the accelerator and the entry are one command, so the key reports `"menu" <verb> [<state>]` exactly as a pick would (a check entry toggles), and a disabled entry's key does nothing. A program that has a bar answers one message whichever way the command came; one that has none answers the bare verb.
  - **Levels, each overlaying the one before**: the host's defaults; `[gui.keys]` in the user's `config.toml` (`$XDG_CONFIG_HOME/clausters/`), then in a project `clausters.toml`, merged per verb; a free-standing file named by `--keys <path>` (the same flat table); and `/gui_keys <json>` at run time, which is how a page sets it (a page has no file) and how a script sets its application's keys after launch. This is the theme's ladder (`[gui.theme]`, `--theme`, `/gui_theme`), and the table is the **host's**, like the theme: a GuiDef carries none yet (see "Future directions", "Key bindings per window").
  - **Context is the order the key was already offered in**, unchanged: an open popup takes every key (it is modal); Tab is the focus ring's; Escape is an open dialog's; then the **focused element** reads the raw key — so a field types the letter, a list walks on the arrows and a focused control's Space presses it. A focused element claims only its own keys and lets the rest through, the model a desktop toolkit's shortcut override has: a field keeps its editing chords and passes Ctrl+O to the program. And an open menu, which takes the keys that walk it and every bare letter, lets a **command chord** through — it closes and the command runs, as a key equivalent does during a platform's menu tracking *(the user, 2026-10-04, finding Ctrl+O dead with File open)*. Only what that declined is looked up in the table, and the verb is offered to the focused element, then to the element **under the pointer**, then performed by the window, and finally reported to the owner.
  - **What a binding cannot take**: Tab and Escape are refused (the ring and the dismissal are the platform's), and what the focus reads — a field's typing and editing keys, a focused control's arrows and Space — is the platform's convention and stays fixed, because it is the focused element's before the table is read. A chord is spelled with `Ctrl` (the platform's command key: Command on a Mac, as decided by the user 2026-10-04 — see "The host reads no Command key", Found by use), `Shift` and `Alt` and a key name (a character, `Space`, `Delete`, `Backspace`, `Enter`, `Home`, `End`, `Up`/`Down`/`Left`/`Right`, `F1`–`F12`), case-insensitive. A letter's Shift is part of the chord (`Q` is not `Shift+Q`); a punctuation character already says its Shift (`?`).
  - **One reading for both fronts**: the dispatch is one function of the gesture machine (`Gestures::press_key`), and the native window and the page each only translate their event and call it — the two match blocks they each kept are gone.
  - **Discovery**: an entry of any menu whose verb is bound shows its chord at the right of the row, in the disabled-text role, and the list is as wide as that needs. A tip does not show one yet: a tip belongs to a widget, and a widget's report is its value, not a verb.

  **Acceptance**: the multitrack, the roll and the take keep every key they had, now as rows of the default table; a `[gui.keys]` entry rebinds one, an application verb bound in a file reaches the owner of a window with no chrome, a key bound to a bar entry's verb reports the entry's pick, and the bar's lists show the chords. Both fronts go through the one dispatch; both clients send `/gui_keys`; `examples/panels/chrome` binds one application verb and shows the chords in its bar.

## G37 — Menus: the popup layer, the chooser put right, and a tree of entries

*(Added 2026-09-14 by the user as a milestone that needed design; reformulated
2026-10-04 with the user, out of a review of the host's chrome. `G37`–`G40` are
one set — the basic elements an application's face is composed from — and
`G38` is the tooltip milestone widened to hold the other things every widget
carries.)*

**The design is the standard one** *(the user, 2026-09-14)*: it follows the
conventions common to desktop and mobile interfaces rather than inventing its
own, and where desktop and mobile conventions differ, choosing between them is
part of the design.

**Decided with the user, 2026-10-04:**

- **The chooser's wire type becomes `choice`, and `menu` names the tree of
  entries.** What `menu` is today is a combobox with no typing: a field showing
  the chosen option and a list that opens under it (`E11`). Its builder is
  renamed with it in both clients.
- **The menu bar is a `window` prop, drawn by the host** as a band along the top
  edge, the way `status` is one along the bottom: chrome, not a node of the tree.
- **An icon is a glyph of the font** (`G38.3`), with a small symbol set in the
  embedded bitmap face so the floor draws them too.

**What holds across `G37`–`G40`:**

- **No new `/gui_*` address and no new container kind.** What is added is
  elements, props on the containers there are, and event payloads. A `tabs`, a
  `toolbar` or a `pager` is a builder shortcut onto the model, as `panel` is.
- **Parametric, never structural** (the L track's rule): a look is a prop and a
  theme role, and there is no styling language.
- **A value never sizes anything.** A choice is as wide as its widest option and
  a menu as its widest entry, because both are props.
- **A widget keeps no state the work produced.** A collapsed group, a divider's
  position and a checked entry are props: set live, reported as events, kept by
  whoever owns the session.
- **One host, both fronts, both clients.** Each part closes with its builders in
  the Python and the web client, its row in `docs/gui-protocol.md`, and an
  existing example extended to show it.

**Order:** `G37.1`, `G37.2`, `G38`, `G39`, `G40`. None of them waited for `G36`:
an entry and a tip name a verb, and the key beside an entry appeared when
`G36`'s table landed.

- ✅ **G37.1 — The popup layer, and the chooser on it.** *(Done 2026-10-04.
  `overlay_rect`/`overlay` were replaced: an element asks for a list through
  `Events::and_popup` and is told the pick through `Element::picked`, and a
  `draw_over` keeps the one thing the old pair also served, a stored plot's
  hover readout -- which the old walk never drew, since it asked only an
  element that had declared an outside rectangle. The list marks the chosen
  option. The out-of-line list the user reported is not reproduced; it is under
  "Found by use".)*

  **What exists:** one overlay per element. An element declares `overlay_rect`
  and draws `overlay`; `frame::draw_element_overlays` draws it last and
  `gestures::element::overlay_owner` hands it the press. It is one level deep and
  it routes the press only.

  **What is wrong with the list today**, read out of the code on 2026-10-04:

  - It is placed against the whole window (`controls::menu_popup` takes the
    framebuffer's height), and the status bar is drawn after it, so the bar
    covers its last rows.
  - A list taller than the window runs off it: there is no scroll.
  - It is as wide as the field, not as its widest option, and nothing keeps it
    inside the window's right edge.
  - The row under the pointer does not light up in a static window: pointer
    motion repaints only a window that holds a hover readout.
  - Its rectangle is resolved once, at the press, so a resize or a `plane`
    panning under it leaves the list where it was.
  - It has no keyboard, Escape does not close it, and the wheel reaches the
    tree under it.
  - *(the user, 2026-09-14)* The chosen option's name is shown twice, and the
    list opens out of line with the field. The second was not found by reading —
    the field and the list come out of one rectangle (`Menu::body`) — so it is
    reproduced before it is fixed.

  **What it builds:** a **stack of popups the host owns**, one per window, each
  anchored to a rectangle or a point and filled by the element that opened it.
  **One function places every popup**: it measures the content, places it inside
  the work area (the window minus the host's bands), flips it to the side that
  has room, and scrolls it when it is taller than the area. A popup is placed
  again at every layout, so it follows its anchor. While one is up it takes the
  press (outside closes the stack), the wheel, the walking keys — arrows, Home,
  End, Enter, Escape — and pointer motion repaints the window. Popups draw
  through a `popup` theme role. Whether `overlay_rect`/`overlay` grow or are
  replaced is settled with the code; what is fixed is one placement function and
  a stack that is the host's.

  **The chooser on it:** `choice`, with the props it has (`options`, `index`,
  `label`, `text_size`) and the value it has, the index. A node of the old
  spelling is an unknown type, as every retired name is (`docs/gui-protocol.md`,
  "The builders keep their names; the wire does not"). **Open:** whether the
  list marks the chosen option or leaves it out, settled by eye.

  **Acceptance:** the placement function tested without a GPU — below, flipped,
  held at the right edge, scrolled, clear of the status band — and the wheel,
  the keys and the motion repaint tested through the gesture machine.

- ✅ **G37.2 — A menu is a tree of entries, shown in three places.** *(Done
  2026-10-04. Freeing a widget inside an open window now takes it out of the
  tree the front draws -- it was left on screen and under the pointer, which a
  dialog closed by freeing it is how it showed. A context menu and a tip per
  part of a heavy view stay open below.)*

  **The tree** is a prop value, `menu`, not a node type: a list of entries, each
  an **action**, a **check**, **one of several**, a **separator** or a
  **submenu** holding entries of its own. An entry carries a label, the **verb**
  it reports, `enabled`, an `icon`, and its state where it has one. A pick
  reports the verb as a `/gui_event` payload of the widget holding the menu, and
  a check reports its new state with it.

  **The three places:**

  - **`menu` on a `window`** — the menu bar: a row of titles in a band along the
    top. The tree is laid out in the window minus both bands. Titles that do not
    fit gather under a last one.
  - **`context` on any widget or container** — the context menu, opened at the
    pointer by the secondary button, and answered by the nearest ancestor that
    carries one.
  - **`menu` on a `button`** — a button that opens its menu under itself.

  **The secondary button and the long press.** Both fronts read the primary
  button only (`host/gui/app.rs`, `host/web/input.rs`). The gesture machine
  learns one request, *context at this point*, with two spellings: the secondary
  press, and a press held still — the timer "The double click and the long
  press" (Future directions) says the machine lacks. The browser's own context
  menu is suppressed where the host answers.

  **Submenus** are the popup stack more than once. The keys that walk an open
  menu are the standard ones and are not bindings; the keys that *open* a menu,
  and the key shown beside an entry, are `G36`'s.

  **What waits on it:** the applications' window chrome in standalone
  (`crates/clausters-apps/PLAN.md`, Future directions).

  **Open:** a context menu per *part* of a heavy view — a clip, a note, a box —
  rather than one for the whole widget. *(2026-10-06: an element brings one of
  its own, asked at the point the button landed (`Element::context_menu`); the
  multitrack's tells a clip from a track. A note's and a box of a patcher's are
  still open.)*

  **Acceptance:** the tree parsed and walked in tests; a pick, a check and a
  disabled entry through the gesture machine on both fronts; the bar's band
  taken out of the layout as the status band is.

## G38 — What every widget carries: `enabled`, focus, a tip and an icon

*(Added 2026-09-14 by the user as the tooltip milestone; widened 2026-10-04.)*

- ✅ **G38.1 — `enabled`, and every control takes the focus.** *(Done
  2026-10-04. The light controls take the focus and do not report it: their
  event stream is their value, read by callbacks of one argument. A disabled
  widget draws through a theme derived from its own, `Theme::disabled`.)* `enabled` is a
  prop of every widget: a disabled one draws through a `text_disabled` role,
  declines the press and leaves the tab ring, and a disabled container disables
  its subtree. A control under the pointer draws through a `hover` role, and the
  window repaints when the widget under the pointer *changes*, not at every
  move. Of the light controls only `text` accepts the focus today; all of them
  do after this, operated by the standard keys — Space or Enter on a `button`
  and a `toggle`, the arrows on a `slider`, a `knob`, a `number` and a `choice`.
  `G36` kept them out of its table: they are the focused element's, read before
  the table is.

- ✅ **G38.2 — `tip`: a widget says what it is when the pointer rests on it.** *(Done 2026-10-04.)*
  A prop, not a widget. The tip shows in the popup layer once the pointer has
  rested — the timer `G37.2` gave the machine — beside the pointer and inside
  the work area; it goes when the pointer leaves, on a press and on a key; and
  moving onto another widget with a tip while one is up shows the next at once.
  On touch a long press shows it where no `context` answers. It says its text
  and not yet a key: `G36` landed without one there, since a widget's report is
  its value and not a verb. It is not a line of the
  status bar, which reports what a hand did. **Open:** a tip per part of a
  heavy view, the same question `G37.2` leaves.

- ✅ **G38.3 — `icon`: a glyph of the font.** *(Done 2026-10-04; seventeen symbols.)* There are no icons in the host
  and there are fonts, so an icon is a character. `icon` is a prop of a
  `button`, a `toggle` and a menu entry, drawn beside the label or in its place.
  The host gains a **small symbol set** — arrows, check, close, menu, play,
  stop, record, pause, loop — each at the Unicode codepoint that means it,
  drawn as **shapes** in a glyph's cell rather than from either face: seen with
  a system typeface, a symbol scaled from the bitmap was a staircase beside
  smooth text, and the typeface had no glyph of its own for it. The chooser's marker and a check entry's mark are drawn
  from the same set. A source file writes the character as an escape.

## G39 — One choice, several presentations, and the small elements beside it

- ✅ **G39 — Tabs, a pager, a radio group and a segmented control are one
  element.** *(Added 2026-10-04 with the user: tabs and a pager already exist as
  composition — a `stack` whose `index` is bound to a chooser — and what is
  missing is the look. Done 2026-10-04. Two additions the composition needed:
  a binding may name its target **by the node**, resolved when the tree is
  opened (`tabs` and `pager` wire a chooser to a stack before either has an
  id), and a strip takes **`pack`**, which sizes each child by its content --
  without it a toolbar's buttons shared the row evenly.)*

  - **`choice` gains `view`**: `combo` (the default, `G37.1`), `radio`,
    `segmented`, `tabs`, `pager`, `list`. The data, the value and the event are
    the same in all of them — `options` and `index` — so a `bind` onto a
    `stack`'s `index` is tabs or a pager with nothing new on the wire. A row of
    tabs that does not fit gathers the remainder in a list on the popup layer.
  - **`toggle` gains `view`**: `check` (today's), `switch`, `button` (a button
    that stays pressed).
  - **`button` gains `flat`**: no fill until the pointer is over it, the look a
    row of tools wants.
  - **`number` gains `stepper`**: a pair of arrows that move it one `step`.
  - **`separator`**: a line between groups; with a `weight` it is the spring
    that pushes what follows to the far edge.
  - **`progress`**: a bar over `value` in 0..1, and with no value the
    indeterminate form — which is the only time it asks for the window's tick.
  - **Builder shortcuts in both clients**: `tabs`, `pager`, `toolbar`.

  **Acceptance:** each presentation's hit test reads what it draws (`E22`'s
  rule); the natural size of each is a function of the options and the metrics;
  an existing example shows a toolbar and a tabbed `stack`.

## G40 — Container chrome: groups, split panes, scroll bars and a dialog

- ✅ **G40 — What a container shows of itself.** *(Added 2026-10-04 with the
  user. Done 2026-10-04; a texture view inside a dialog is under "Found by
  use". The example is `panels/chrome`, which holds every element of the set in
  one window.)* Four props on the containers there are:

  - **`title`, `frame` and `collapsed` on a `layout`**: a titled group, and a
    section that folds to its title strip. A press on the strip reports the
    change.
  - **`split` on a `row` or a `col`**: a divider between children that a drag
    moves, changing the `weight` of its two neighbours — or the fixed `w`/`h` of
    one that has it — and reporting the result.
  - **`bars` on a `plane`**: scroll bars along the axes it pans, shown when the
    content is larger than the view, with a thumb a drag moves.
  - **`modal` on a `layout`**: a dialog. It is placed by the popup layer rather
    than by its parent's flow, over a dimmed window, and it takes every press
    while it exists — it is up from the moment it is defined until it is freed,
    in a window and in a page alike.

  **Left for `G41`:** a table, a tree and choosing a file.

  **Acceptance:** each gesture through the machine on both fronts; a collapsed
  group and a moved divider read back by `/gui_query` as the props they are.

## G41 — An option: rows of data, and choosing a file

*(Added 2026-10-04 with the user, as an **option**: it is not in the sequence
`G37`–`G40` and nothing there waits on it. It holds what that set left out, and
the record of why, because neither reason survived being asked about.)*

**Why they were left out, and why that did not hold:**

- **A table and a tree** were left out as "views of data, not chrome": they
  need a data shape of their own — rows, columns, a hierarchy, a selection of
  several, an order, drawing only the rows on screen — where everything in
  `G37`–`G40` is a prop on a control there already is. That is a reason for a
  milestone of their own, not for leaving them out. The host has `nodes`, which
  is the server's node tree and nothing else, and `G39`'s one-column `list`.
- **Choosing a file** was left out as "the platform's". A native dialog in a
  window and something else in a page is the divergence the project calls a
  defect, since the host is one; and a page has a filesystem (OPFS), so the
  platform lacking one is not a constraint. Nothing in any plan names a file
  chooser, and the host has no way to list a directory — `DefStore::list` lists
  the saved GuiDefs only. A standalone host needs one to open and save a
  session.

The two are chained: a file chooser is composed from rows, `G40`'s modal
`layout` and a `text` field.

- ✅ **G41.1 — One element over rows: a list, a table and a tree.** *(Done
  2026-10-04. Decided with the code: the element is `table`; the rows ride the
  def; a cell is not edited in place; a column's width is named or shared, not
  dragged; `G39`'s `list` view stays a presentation of `choice`, the small case
  a chooser wants; `nodes` is not rebuilt on it. Which branches are open is a
  prop written where it is drawn and reported, as a menu's check is; ordering
  the rows is an intent.)* One
  element, configured by what it holds: rows of cells, `columns` where there is
  more than one, and a depth per row where the rows nest — so a list is the
  one-column case and a tree is rows with a depth, not three types. It draws
  only the rows on screen and scrolls the others, and its natural size never
  follows the row count. It selects one row or several and reports the
  selection; opening or folding a branch, and a press on a column's header, are
  reported as **intents** — the owner orders the rows and sends them back,
  since the host holds no document.

  **Open, none of it decided:** the element's name; how many rows ride the def
  before they need the bulk path; whether a cell is edited in place, and how
  that edit travels back; whether a column's width is `G40`'s divider; whether
  `G39`'s `list` view is this element's one-column case rather than a
  presentation of `choice`; whether `nodes` is rebuilt on it.

- ✅ **G41.2 — Choosing a file, drawn by the host.** *(Done 2026-10-04. It
  is an element a def composes, `files`, and `file_dialog` puts it in a dialog
  -- a request to the host would have been a new address. The listing is asked
  by the element and answered by the front: the disk at once natively, OPFS
  awaited in a page. A path is the platform's, as for every verb that takes
  one; a file reaching a page's storage from the user's disk stays the port's
  question.)* The same chooser in a
  window and in a page: the rows of a directory, a `text` field for the name
  and the buttons, in a modal `layout`. The listing is the host's, behind one
  more small I/O trait beside `Transport`, `DefStore`, `BulkLoader` and
  `BusSource` — the disk natively, OPFS in the page — so neither client
  implements it. A pick reports the path as an event payload.

  **Open, none of it decided:** whether it is an element a def composes or a
  request a client makes of the host; what a path means across the two fronts
  when a session moves between them; OPFS being asynchronous where the host's
  frame is not; and how a file on the user's own disk reaches a page's storage
  and leaves it, which is the port's question rather than the chooser's.

## G42 — The clipboard is the platform's, and a field edits like every field

- ✅ **G42 — The host's clipboard reaches the system's, and a text field has the standard edit gestures.** *(Done 2026-10-04, as decided below; the desktop's clipboard through `arboard`, text only.)* *(Added 2026-10-04 by the user, after a text field learned the double and triple click: cut, copy, paste and delete from a right-click menu, and the middle button pasting, for text fields -- which first needs a clipboard other programs see.)*

  **What is there**: one typed clipboard per host (`host::clipboard::Clip`, the document crate's format), shared by every window of the host and holding text, notes, clips or samples. It reaches **nothing outside**: text copied in the host does not paste in another program, natively or in a page, and the reverse.

  **Decided with the user, 2026-10-04:**

  - **The clipboard is the platform's**, behind one more small I/O trait beside `FontSource` (`SystemClipboard`: read and write text, and the primary selection where there is one), implemented by the native front. The core keeps the typed `Clip`; the trait only moves text across. The `Clip` lives on the `Host`, like the theme and the keys, so every gesture reaches it.
  - **Text crosses, the rest stays.** A copy of text (a field's selection, a notes or clips block, which ride as JSON text) is written to the system's clipboard too. A paste reads the system's first: text another program put there after the host's last copy is what is pasted; otherwise the host's own, so a range of samples -- which no other program reads -- still pastes. The read happens at a paste, never per key.
  - **A text field has the standard edit menu** -- Cut, Copy, Paste, Delete, Select all -- on the secondary button (or a held finger), the host's own when the field carries no `context` of its own; an entry with nothing to act on is disabled.
  - **The middle button pastes the primary selection** where the platform has one (X11 and Wayland): selecting text in a field writes it, and a middle click in a field puts it at the pointer.

  - **A page keeps the host's own clipboard** *(the user, 2026-10-04)*: the host draws inside a canvas, so reaching the browser's clipboard needs code on the page's side, and it waits under "Future directions" ("The page's clipboard reaches the browser's"). Everything else is the same in both fronts -- the edit menu, and the middle button pasting the last selection made inside the same host, which is the primary selection a page has.

  **Acceptance**: natively, text copied in a field pastes in another program and back, and the middle button pastes what another program selected; a block of samples copied and pasted inside the host is unchanged; the edit menu and the middle button work on a field in `examples/panels/chrome`, in a window and in a page.

## G43 — Text is shaped: every script a field accepts is drawn right

*(Added 2026-10-08, from the review of what building the host without a GUI toolkit has cost: of the commodity text and input layers a toolkit ships, this is the one the host does not have yet.)*

**What is there.** K10 rasterizes a real face, but character by character: the atlas is keyed by `(character, pixel size)`, a string's width is the sum of per-character advances, and nothing between the string and the glyphs is a shaper. That is right for Latin, Greek and Cyrillic without marks and wrong for the rest: no ligatures, no contextual forms (Arabic joins every letter to its neighbours), no mark positioning (a combining accent sits where the advance left it, not over its base), no reordering (Devanagari's vowel signs), and no right-to-left runs. Since composed input closed (2026-08-27) a field *accepts* all of it, so the host takes text it then draws wrong. The bundled face makes the same limit twice over: `assets/fonts/subset.sh` keeps Latin, Greek and Cyrillic and drops the layout tables (`--layout-features=''`), because the rasterizer read neither.

**Decided before opening it:**

- **A shaper, behind `font-atlas`, and nothing else moves.** `rustybuzz` (a pure-Rust port of the standard shaping engine, wasm-safe, no system dependency -- the posture `fontdue` was chosen for) turns a run into glyph ids and positions; the atlas is keyed by `(glyph id, pixel size)` and rasterizes through `fontdue`'s indexed path. The bitmap floor is untouched: a build without the feature, or a host with no face loaded, draws exactly what it draws today.
- **What is measured is what is drawn, still.** K10's rule carries over unchanged: the ellipsis cut, the word wrap, the caret, the selection band and the click-to-column hit-test read the shaped advances, never a per-character sum, so a wrap and the drawing cannot disagree.
- **Direction is a paragraph's, resolved once.** A label is reordered by the Unicode bidirectional algorithm (`unicode-bidi`) into visual runs before shaping; a label's `align` keeps its meaning (start/end follow the paragraph's direction only if a prop asks for it -- decided in G43a, with the default the current left/right).
- **Placement (the G7b rule): display-only.** Shaping is drawing; nothing for `clausters-core`, and nothing on the wire -- a label's text is already a string.
- **The face covers what the host claims to draw.** The bundled subset keeps its layout tables and grows the scripts the acceptance names, or the host draws those from a system face natively and a fetched one in a page through the `FontSource` seam K10 already has. Which of the two -- or a size budget for the bundled file -- is decided in G43a with the measured sizes, since the face is compiled into every default build.

**The milestones:**

- ✅ **G43a — Labels are shaped.** The shaper, the glyph-keyed atlas, the bidi reorder and the face's coverage, over every widget that draws text it does not edit. **Acceptance:** an example's labels in Arabic, Hebrew, Devanagari, and Latin with combining marks drawn correctly by eye in a window and in a page; tests shape a ligature, a joining sequence and a base-plus-mark against the face the host ships and check the glyph ids and offsets; the wasm build's size before and after is measured and recorded. *Done 2026-11-08.* `font::shape` splits a line by `unicode-bidi` into visual runs, a run into pieces by the face that draws each grapheme cluster (a symbol or a window's outline character stays the shape it was), and shapes each piece with `rustybuzz`; the atlas is keyed by `(face, glyph id, pixel size)` and rasterizes through `fontdue`'s indexed path, and `width`, `text`, the ellipsis (cut at a grapheme by halving over the shaped width) and the wrap read the shaped run. **The face, decided on measured sizes:** both bundled, since the acceptance's tests shape against the face the host ships and a page has nothing to fetch it from that a window would not -- `DejaVuSans.ttf` keeps its layout tables and adds Arabic and Hebrew (238 404 bytes before, 298 412 after), and `NotoSansDevanagari.ttf` (145 916 bytes, OFL) is the face the host falls back to; a face a client loads replaces only the first. The wasm host grew from 9 279 679 to 10 268 416 bytes (the applications' build from 24 108 015 to 25 065 580), some 200 KB of it the faces and the rest the shaper and the bidi tables. **`align`:** start stays the left edge whatever the direction, and no prop asks for the paragraph's; a right-to-left label that wants the right edge says `end`. Tests: `font::shape::tests` (a ligature, Arabic joining read from the right, a mark over its base, Devanagari's vowel sign from the fallback face, a mixed line in visual order, a symbol kept a shape). The example is `panels/text` in both clients, with a panel of the scripts; checked in a page by screenshot in headless Chrome.
- ✅ **G43b — A field edits shaped text.** The caret moves by **grapheme cluster**, not by character (today it moves by characters); in mixed-direction text it keeps a logical position and draws at the visual one, and a selection over a direction change paints the runs it covers. The field's scroll window and its click-to-column hit-test read clusters. **Acceptance:** a caret walks a mixed Hebrew/Latin string cluster by cluster with the arrow keys and lands where it was clicked; a selection across the direction change copies the logical substring; the same in a page. *Done 2026-11-08.* `textedit` moves, deletes and counts columns by grapheme cluster (`unicode-segmentation`); positions stay logical. The field shows a window of whole clusters measured as drawn, draws the caret at `font::caret_x` (a cluster's leading edge in its run's direction, a share of a ligature inside one), paints a selection over `font::spans` (one span per run it covers) and lands a click with `font::byte_at`, all read off the one shaped run. Natively, a commit of several characters from an input method is typed whole, as the page types what its field composed. Tests: `font::shape::caret_tests` (a caret walked through Latin, a Hebrew word and Latin, leftward inside the word, a click landing on the caret drawn there; a selection across the change of direction copying the logical bytes and painting two spans; an arrow and a backspace taking a letter with its mark whole). The host is the same code in a page; `panels/text` in both clients has the mixed line in a field.

## G44 — A take's picture is read at the level it is drawn, out of summaries kept once

*(Added 2026-10-22 by the user, after the audio editor's acceptance: with every take drawn from its summary in a host that cannot map it (`docs/decisions.md`, "A host that cannot map draws every take from its summary"), a page redraws a take slowly from its start after every edit, and slower the longer the take. Taken next.)*

**What is there.** A host that cannot map a server buffer (a page, or a host attached to a remote server) asks `/buffer_peaks` for the take's summary at the finest grid (`DEFAULT_BASE_BUCKET`, 256 frames) from frame 0 to the end, one reply of at most 8 KiB at a time (`fetch::PeaksWalk`), whatever the view shows: a ten-minute stereo take is about 350 round trips for a window 760 pixels wide. The server answers out of a kept pyramid only for a buffer it shares as a region file (`osc::server::overviews`, native with a segment); everywhere else, and for every join, it reads the samples to answer. An edit stitches a new join (`/buffer_stitch`), so the host forgets the picture and walks the take again from its start: the slow redraw from the left, growing with the length. A join is a list of parts over source buffers, which is the edit list an audio editor draws from -- and nothing uses that.

**How the field does it.** A summary is kept per source, multi-resolution, computed once when the audio arrives; an edit rearranges regions over the sources, and the picture is composed from the sources' summaries, so a cut or a paste recomputes nothing and only new audio is summarized. A view asks for the level nearest its resolution, over what it shows, and the next finer one as it zooms; samples only at sample level. Scrolling and zooming ask for bounded pieces, never the whole.

**Decided before opening it:**

- **The criterion is the decision's**: answer fast with the least CPU an operation needs, and do the work at the pace of the interaction so none arrives as a spike. Every step below is measured against it, in a page and natively.
- **A summary is kept once per buffer, on the server, for every buffer.** The overview the server keeps beside a region file is kept for every buffer it holds, in memory where there is no segment (a page's engine, a server booted without `--shm`): built when samples arrive (`/buffer_allocRead`, a write, a recording) and refreshed by the span written, never rebuilt, with the arithmetic `overviews` already has. Natively the region file's overview is what a peer maps, unchanged. *(Revised 2026-10-22 while building it: measured as it is asked and kept, not built when the samples arrive, because a page's engine answers between two audio blocks; `docs/decisions.md`, "A take's overview is answered from a summary kept on the buffer".)*
- **A join's summary is composed from its parts'.** `/buffer_stitch` builds the join's pyramid out of its sources' pyramids: whole buckets copied, and only what no source bucket describes is read from the samples: the buckets a part's edge cuts in two, and those under a fade. The cost of an edit is the number of parts, not the length of the take. *(Revised 2026-10-22 while building it: a paste moves everything after it off the source's grid, so no bucket could be copied; the join keeps no pyramid and each question is composed from the sources' summaries instead, exact, reading only a fade and the edges a cut leaves off the grid.)*
- **`/buffer_peaks` answers at any level of the pyramid.** A bucket of `256 * 2^k` is answered from level `k` rather than refused off the base grid, so a view asks for the coarsest level that still resolves its pixels (`never resolve finer than the screen`), over the span it shows, in one or a few replies. The command keeps its arguments; what changes is that a coarse bucket is answered from what is kept. Its reference in `docs/schemas.md` says so, and both clients' builders already pass a bucket.
- **The host keeps what it was answered, by level, and asks for what it lacks.** A view's picture is drawn from the finest level it holds over each column; what is missing for the view's level and span is asked for, within the reply ring's budget, and nothing else. The detail grids a zoom already asks for (`fetch::want_detail`) and the spans read back under it are the finer end of the same ladder, not a second mechanism.
- **An edit keeps the picture and re-reads only what is in view.** A join replaced by an edit is asked again at the view's level over the view's span; the rest is asked as the view reaches it. The picture before the edit stays drawn until its replacement lands, so nothing redraws from the left.
- **Placement (the G7b rule).** The pyramid arithmetic and the composition of a join are `clausters-core`'s (`peaks`), bound by the server; what to ask for at which level is the host's (`fetch`, `frame::owed`). Nothing for the clients: a script still says what the view is *of*.

**The milestones:**

- ✅ **G44a — Every buffer keeps its summary, and a join composes it.** The server keeps a pyramid for every buffer, in memory without a segment, and `/buffer_stitch` composes the join's from its parts'. **Acceptance:** after a paste into a join of ten minutes of stereo, the server's work is proportional to the number of parts (measured, not read from samples), in a page and natively; `/buffer_peaks` over a join answers from the composed pyramid and matches a summary of the stitched samples bucket for bucket, a part's cut edges and its fades included (a test over a join with fades and a channel map). *Done 2026-10-22:* `dsp::summary` over `clausters_core::peaks::Kept`; a paste's join over ten minutes of stereo answers its 760-column overview in 0.8 ms natively and 3 ms in the wasm engine, from 199 and 215 ms. Natively with a segment the join is still copied into a region, the entry under "Found by use".
- ✅ **G44b — `/buffer_peaks` at any level.** A bucket of `256 * 2^k` answered from level `k`, documented. **Acceptance:** a request at a coarse bucket matches the summary of the samples at that bucket; a ten-minute take's whole overview at a 760-column view is one or two replies. *Done 2026-10-22:* any bucket is answered exactly out of the kept summary, one of 256 * 2^k on its grid from the levels with no sample read, and a span to the end answers its last, short bucket too (the walk in the host now runs to the end). A ten-minute stereo take at a 760-column view is 879 buckets in three replies, against 330 at the base bucket: three and not two, because a reply is bounded at an eighth of the 64 KiB ring, which several replies share.
- ✅ **G44c — The host reads at the view's level, and an edit re-reads what is in view.** The level ladder in the host, the asks bounded by the view and the ring, and the picture kept across an edit. **Acceptance:** in `editors/audio-lifecycle.html` and its script against a remote-style host, a paste into a two-second and into a ten-minute stereo take shows in the same time, a few replies, with nothing redrawn from the left; opening an hour-long take asks for bytes proportional to the window's width, not to its length; zooming to samples reads only the span shown; `/gui_activity` shows no frame or request a still window does not need; the native mapped path draws as before. By eye in a page and natively, with two windows over one take. *Built 2026-10-22:* a view is owed its take's summary and asks it after a draw, at the coarsest level its width resolves; the replies are collected and the grid replaces the picture whole, so an edit's `reload` keeps the old picture until then; detail grids are walked the same way (a grid longer than one reply used to be re-asked every frame, since it never covered its view); a plural asker (a multitrack's clips, a preview) waits for its widest drawn picture. Measured in the fetch machine and a front-level test: ten minutes of mono at 760 columns is two replies, and the paste's new take replaces the old at once. **Open:** the eye pass, in a page and natively with two windows, which closes it. A native paste into a long take still pays the region copy `S23` removes, so that half of the pass waits for the audio editor on lists (`X28`, after `G45`, done). *Native half looked at 2026-10-28 by the user*, over a ten-minute and a two-second stereo take: a paste shows at once in both, nothing is redrawn from the left, the other window does not move, and zoom and pan to samples draw clean; the picture blinks as it is replaced ("A paste blinks the audio editor's picture", Found by use). Two windows over **one** take are not something a client can open: `edit` hands back the editor already open on a file, so the second window was over a copy. The page's half could not be judged until "The audio editor in a page: the pointer lands off the picture" (Found by use) was fixed. *Page half looked at 2026-10-29 by the user* in Firefox, with the waveform and the spectrogram zoomed in time and frequency: it draws clean. That pass found a regression of the server buffer's spectrogram fix, which had stopped a page's file from asking its list for columns and drew a read-back run in their place (a dark half window at the start, blank under a zoom); fixed the same day, so a list's texture is always its columns. **Closed 2026-10-29.**

## G45 — A take is drawn from its files

*(Opened 2026-10-22 by the user, with the turn it serves: `crates/clausters-apps/PLAN.md`, "The turn: an application's takes live on disk".)* A take is a list of segments over files, and the host draws it without a server: the picture is composed from the parts' peaks files (the composition `G44a` built in the server moves to `clausters-core`, beside `peaks::Kept`, and the server keeps its own for pool buffers), and a view zoomed to samples reads the span it shows from the file. Natively the host maps or reads the files; in a page it reads OPFS through the platform seam, and the same fetch machine asks for both (`G44c`'s asking at the drawn level and placing whole carries over unchanged). **Acceptance:** a take of hours opens in the time it takes to read the peaks its window resolves; an edit's new list is drawn from the peaks it already holds, with only the buckets under a seam's fade read from the samples; zooming to samples reads only the span shown; two windows over one file share nothing but the files; the same in a page and natively, by eye.

**The milestones:**

- ✅ **G45 — The host draws a list of segments from its files.** The acceptance above. *Done 2026-10-22.* A `signal`, `waveform` or `spectrogram` names a list with `segments` (the clipboard's segment parts plus each file's `peaks`), in both clients. The host gives it a take key below every server buffer and its views ask about it through the fetch machine as about a buffer, answered by the host's own answerer (`host::disk`) in the server's reply formats: natively on a thread that reads the disk at offsets, in a page over OPFS; the answerer does no I/O and is driven by both. The composition is the core's (`clausters_core::peaks::compose`, which the server's join now uses too), over the container parser and part arithmetic that moved to the core (`clausters_core::pcm`, `clausters_core::segments`). A coarse column is folded from the peaks alone, its edges rounded to a level a thirty-second of its width; a part's own edges, its fades and every edge of a finer column are exact (`docs/decisions.md`, "A host draws a list of segments from its files, through the fetch machine it already has"). Measured over an hour of stereo W64 and its peaks (`tests/long_list.rs`, release): opening at 760 columns is 6 replies, 179 kB read and no sample, in 1.8 ms; ten successive pastes of half an hour each are drawn in 57 ms reading 5.1 MB, the sample blocks under the new seams; a zoom to 4096 samples reads 32 kB in 0.9 ms. A page draws a list recorded into its storage under the same key across an edit and asks the engine nothing (`clients/web/tests/segments-host.html`). Not covered: a spectrogram of a list (this plan's Found by use, for `X28`). The eye pass over `views/segments`, natively and in a page, is the ROADMAP's.

## G46 — A complex element is described again only where it changed

*(Added 2026-10-29 by the user, after reading how the multitrack and the score editor are drawn.)*

**What is there.** A window keeps its description between frames as one piece (`frame::Kept`, a single `valid` flag): a tick that moves only a live layer redraws that layer, and **every other change describes the window again whole**. Inside an element nothing finer is kept:

- **The multitrack** (`elements::multitrack`) emits every visible track, box, curve and header into the window's mesh on every whole frame. Moving one box, a zoom of one row, a `/gui_set` on another widget of the window: all of them describe every visible box again. It stays cheap because a box is drawn at the screen's resolution from kept summaries, so the cost is the visible boxes and not the takes' lengths; a dense multitrack (many tracks, many short boxes, curves on every row) pays it on every frame of a drag.
- **The score editor** (`elements::score`) keeps its page's triangles (`KeptPage`, now `KeptParts`, see below), so a change elsewhere in the window is a copy. But any change of its own (a note edited, a selection, a press, a drag) lets the whole page go and tessellates it again, every system of it, for one note.

**What the milestone asks.** A complex element keeps its picture **in parts** and describes again only the parts a change reached: the multitrack per track row (or per box) and per header, the score editor per system (or per measure), with the selection's wash and the hand's feedback drawn as a layer over the kept parts rather than into them, so selecting does not invalidate what is selected. The window's own `Kept` learns the same at its scale: a change confined to one element describes that element again and copies the rest.

**Constraints:**

- **The single pass is the reference.** A frame drawn from kept parts is the frame drawn whole, pixel for pixel; a test compares the two, as `a_kept_picture_frame_is_the_single_pass_frame` does for the live layers.
- **The key is what the part is drawn from**, as `KeptPage`'s is: placement, clip, theme, ink, the text's generation and the part's own state. A part whose key moved is drawn again; one the axis scrolled is moved, not drawn again, where its key allows.
- **One mechanism, not one per element.** The parts cache is a facility of the element API that the multitrack, the score editor and the roll use, never three caches written beside each other (the rule "what is built is adapted").
- **Measured before and after**, on the heaviest window of each: a drag of one box in a multitrack of many tracks, a note edited on a score of several pages, natively and in a page, under `/gui_activity` and the host's profiler.

**The milestones:**

- ✅ **G46 — Partial invalidation inside a complex element.** The acceptance above, with the multitrack and the score editor as its two first users.
  *Native half done 2026-11-08.* The mechanism is `paint::KeptParts`: a part is a mesh built under a key and copied while the key stands. The score keeps its ink a staff to a part, keyed on where it is, its ink and colors, the text's generation and -- for the staves a drag or a text being typed over reaches (`ScoreData::moved_parts`) -- that state; the selection and the edit cursor are drawn fresh under the ink (`ScoreData::render_marks`), so a press that selects keeps every staff and a drag draws its own staff again. The multitrack keeps each box -- its frame and its body, a roll or a page -- keyed on its rect, the stack's, the clip, ink, theme, sizes, whether it is held, its axis, how notes are drawn and a count of its contents. Tests: `a_kept_page_is_the_page_until_what_it_shows_changes`, `a_drag_draws_only_its_own_staff_again`, `a_moved_box_is_the_one_box_drawn_again`, each against the single pass. Measured natively in release (`frame::tests::what_a_change_costs`, a 1280x900 window): a score of four staves of 512 notes, 1.62 ms a frame after its selection changed before and 0.35 ms after, as with nothing changed; a multitrack of 32 tracks of 24 boxes of notes drawn as pages, 1.45 ms a whole frame before and 0.68 ms after, and 0.50 ms drawn as rolls either way, which is the stack's headers, curves and overlays and not its boxes. *Closed 2026-11-08 with the page's numbers.* In a page (headless Chrome, the wasm host, a GuiDef of the same multitrack in a 1280x900 canvas, a box dragged by real mouse events over CDP and every animation-frame callback timed, against a build of the commit before): drawn as pages 4.3 ms a frame of the drag before and 3.2 ms after, drawn as rolls 1.8 ms either way. **What was not built, and why:** the roll as a third user -- a roll of 4000 notes costs 0.11 ms a whole frame natively, so a part would keep nothing worth keeping; and the window's own `Kept` describing one element again -- with the elements' parts kept, what a whole frame still costs is the window's chrome (0.3 to 0.5 ms natively) and, in a page, uploading and encoding the whole batch, which describing less would not reduce. Both are reopened by a measurement that says otherwise.

## L track — the look: layout, sizing and themes for the light widgets

Section added 2026-07-19; ordered before the P track because the patcher's surfaces build on exactly these primitives — a positioned child, a themed accent, a sized label. This track delivers what the layout engine deliberately deferred ("children are evenly sized at this milestone") plus the customization the protocol has none of today: no color prop on any widget, no per-child size, no text size or wrap, margin/gap as compiled constants. The goal is versatility of *composition* — a script must be able to build a real application face (a menu bar, a working area, a status bar) from the same light elements — while the elements themselves stay simple and cheap to draw: flexibility lives in the layout and the theme, never in the widgets' drawing cost. Five design rules bound it, all in the direction of keeping the host light:

- **Parametric, never structural.** *How* a widget is drawn stays monolithic and compiled — one `Mesh` pass, the embedded font, fixed shapes — because that is what keeps a whole window one draw batch. Customization is a *parameter* of that fixed drawing (a color, a size, a wrap width), never a template, box-model or cascade system. There is no styling language; there are a few props and a theme.
- **The basics stay light, the particular stays particular.** The new props are the small general set every widget can share (`w`/`h`/`weight`, `x`/`y`, `color`, `text_size`, `wrap`, `align`, container `margin`/`gap`). Anything specific to one visualization remains that widget's own Rust prop, exactly as today — the spectrogram's `colormap` is the model, not the exception.
- **Color by function, not by part.** Every chrome color becomes a named **role** in one `Theme` (background, panel, field, text, dim text, accent, selection, …); every paint site names a role, no paint site names an RGBA literal. The roles themselves are **programmatically overridable at every level** — host-wide from the style file, per subtree through a container's `theme` prop (the theme group, L5), per widget through the single `color` prop (the shorthand for the accent role, the color that carries a widget's function: a meter's bar, a slider's handle, a trace). What stays closed is the *part*: a widget's pieces are only ever named by roles, never addressed individually — no selectors, no per-part styling.
- **Size by role, not by literal** *(added 2026-07-29, with L6–L8)*. The twin of the color rule, one step behind it: sizes become named roles in one **metrics** table — `pad`, `gap`, `control_h`, `track_thick`, `hit_slop`, `text_scale` — and no layout or paint site names a number, exactly as no paint site names an RGBA. The roles' defaults come from a single quantized modular scale over the font cell rather than from per-site invention, and the table is **constant**: the scale generates the defaults once, it is never arithmetic on a frame. A widget's own extent is a pure function of that table (its **natural size**), so density and HiDPI are one multiplier applied to one table, never a pass over the tree.
- **Monolithic per host, portable per def.** A host has one look, loaded from a **theme file** — a TOML role table through the shared config — and that uniformity is a feature. A GuiDef that needs its own look carries partial theme overlays in its tree, so a standalone bundle ships its appearance the way it ships its defs.

Out of scope, recorded so they are not relitigated per milestone: per-widget borders/radius/shadows and a real font atlas — each folds into a milestone only if a need firms up (the font is the likeliest candidate, see L4). The recursive theme overlay of L5 is deliberately the *only* cascade-like mechanism, and it cascades role tables, never per-part rules.

- ✅ **L1 — the `Theme`: color roles centralized + the host style file** *(done 2026-07-19)*: a pure refactor with zero visual change that creates the seam everything else styles through. Every RGBA literal scattered across `controls.rs`, `frame.rs`, `meters.rs` and the view painters moves into one `Theme` struct in the agnostic core (`host/theme.rs`), its fields named by **function**: `background`, `panel`, `field`, `track`, `text`, `text_dim`, `accent`, `accent_dim`, `hilite`, `border`, `ruler`, `selection`, `playhead`, `warn` (the clip/error red), and the **series palette** (the per-channel cycle the overlay traces and multichannel lanes already share informally). The default `Theme` reproduces today's exact values. The host loads a theme from `[gui.theme]` in the shared config (`role = "#rrggbb[aa]"`; a partial table overlays the default, so a style file names only what it changes) and from `--theme <file>` for a free-standing style file; the web front takes the same table through the `GuiBridge`, so both fronts render one look from one source. Tests: hex parse round trips, partial-overlay semantics, and the default-equals-today guard (the frame snapshot tests, where they exist, unchanged).

- ✅ **L2 — the layout props: per-child size, weight, flow and free positioning** *(done 2026-07-19; sizes are device pixels, like every other pixel in the host — the roadmap's "logical px, HiDPI-scaled by the front" is deferred until the frame as a whole scales, text and margins included)*: today's even split becomes the *default case* of a richer rule that stays **one pass, no measurement, no constraint solver** — a deliberate boundary: when a layout needs negotiation, the answer is explicit sizes, not a solver. Along a `row`/`col` main axis a child with `w`/`h` is **fixed** (logical px, HiDPI-scaled by the front); the remaining children share the remainder by `weight` (default 1, so every existing GuiDef lays out pixel-identically); the cross axis keeps filling. `grid` gains `cols` (absent, today's near-square). Containers gain `margin` and `gap` props (defaults the current 6). `free` becomes real free placement: a child with `x`/`y` (+ optional `w`/`h`) positions absolutely inside the parent, and a child without them keeps the full-area overlay — the current behavior, preserved. All pure geometry in `layout.rs`, unit-tested without a window; the Python builders grow the props with the same defaults. The acceptance shape is the **application shell**: a `col` window holding a fixed-height menu-bar row (`menu`/`button` children), a `weight`-ed content area and a fixed-height status-bar row (`label`s a script live-updates via `/gui_set`) — versatility from *composition and flow*, with every element staying a handful of flat-colored quads in the one mesh batch, so a full application chrome costs what a panel of sliders costs.

- ✅ **L3 — the 2D workspace: a scrolling, zooming container, general first, constrained by configuration** *(done 2026-07-19; two departures from the sketch below, both recorded here: the view is **one uniform scale plus a pan offset per axis** rather than a `viewport::View` per axis — the two axes share a scale so the plane never distorts, and an independent per-axis clamp would de-couple them, so the anchor-preserving pivot math is reused on the scale factor (`host/scroll.rs`) instead of on two `View`s; and the clip is **geometric** (a Sutherland-Hodgman clip inside `paint::Mesh`) for the flat chrome, with a pass scissor only for the heavy views' own pipelines — which keeps the whole window one mesh upload and one draw call, the L-track cost rule, where a per-widget scissor would have split the batch)*: a new light container, `scroll`, whose children live in a **virtual content area** (its size from the free-placement extents of L2, or explicit `content_w`/`content_h`) seen through a 2D window — one `viewport::View` per axis, so the anchor-preserving zoom/pan math, already pure and unit-tested, is reused rather than rewritten. Deliberately **general first**: the default is the full 2D workspace (pan both axes, wheel zoom anchored at the cursor), and the *constrained* scroll views degrade from it by configuration — `axis: "y"` with `zoom: 0` *is* a plain vertical scroll view, `axis: "x"` a horizontal strip — one widget, one gesture path, the restricted forms costing nothing extra; if a constrained variant later earns an optimization, it specializes from the general one, not beside it. View state (`view_x`/`view_y`/`view_zoom`) is settable via `/gui_set` and emitted as the existing `"view"`-family payloads; gestures ride the shared `gestures` machine (browser parity by rewiring, the G27 posture); drawing stays the one mesh batch, clipped by the widget's scissor rect exactly as every widget already clips to its viewport. This is also the seat the P track's patch canvas (P1) sits on: the patcher's pan/zoom becomes this container's, not its own machinery. A further horizon this generality deliberately keeps open (recorded under Future directions, not opened here): the workspace's x `View` joining the window's **navigation group**, which turns the plane into a time-axed surface — the free arrangement plane. Pure tests: content extents, per-axis clamp/zoom, hit-testing through the transform, the constrained degenerations.

- ✅ **L4 — text: size, wrap and alignment on the light widgets** *(done 2026-07-19; `text_size` is clamped to `[1, 16]`, and the ellipsis is a dedicated single-cell glyph so a clipped string costs one cell, not three)*: `text_size` (a scale over the embedded 5×7 font; today's fixed 2.0 becomes its default) on every text-bearing light widget — `label`, `button`, `toggle`, `text`, `number`, `menu` and the control labels; `label` additionally gains `wrap` (word wrap on the font's fixed advance — a cheap width computation, no shaping) and `align` (`start`/`center`/`end`). Single-line text that overflows its rect clips with an ellipsis instead of bleeding into the neighbor. The font itself deliberately stays the embedded bitmap: weight, zero dependencies and wasm parity all argue for it until a real need is demonstrated — a crisper font (an atlas) is the future direction this track explicitly does not open.

- ✅ **L5 — theme groups: the recursive role overlay + the `color` prop** *(done 2026-07-19; the `color` prop re-seeds the whole function family — accent/dim/lit by the default theme's own ratios, the trace pair, `series_1` and the placed-object pair — so one hex behaves like the default green does)*: the customization surface reaches the wire, in one recursive mechanism. Any **container** (window, panel, scroll, track) may carry a `theme` prop — a partial role table (JSON object, role → `"#rrggbb[aa]"`), the same shape as the TOML file — that overlays its parent's theme for its whole subtree: a **theme group**, a style scoped to the *function of a set of widgets* (the transport bar dimmed, the recording strip warm, the analysis pane cold) rather than to any individual part. The concept is recursive by construction and cheap by construction: overlays **resolve at mutation points** (define / `/gui_set`), each widget ending up with one resolved theme reference, so the per-frame drawing path reads exactly one theme per widget and pays nothing — the less-overhead alternative to selector/cascade systems, recorded as such. The leaf case is the single `color` prop: a per-widget overlay of just the **accent** role — the meter's bar, the slider/knob handle and fill, the button/toggle face, a scope/spectrum/plot trace (re-seeding the series cycle for its multichannel siblings), the bpf curve, a clip body. A `theme` on the GuiDef root persists with a named def, so a standalone bundle keeps its look with zero host configuration; everything is live via `/gui_set`. Python: `color=` on every builder, `theme=` on the container builders; example `examples/panels/style.py` (one panel under the default theme, a file theme and a nested theme group, plus per-widget accents); docs: the props join the catalog in `docs/gui-protocol.md`, the theme file lands in `docs/configuration.md`.

**L6–L8 added 2026-07-29 — the half of the look L1–L5 did not cover: how big things are.** L1 centralized color and L2–L5 gave composition its props, but *size* stayed where it always was: every widget fills the cell the layout hands it, and the numbers around it are per-module constants invented at their own sites — `PAD` three times over (`controls.rs`, `meters.rs`, `track.rs`), `MARGIN`/`GAP` in `layout.rs`, `TRACK_THICK 4`/`HANDLE_THICK 8`/`HANDLE_GRIP 18`, `HEADER_W 96`, `RULER_H 18`, `LABEL_GAP_PX 14`, three separate text scales. Two consequences, both visible. **Nothing has a size of its own**, so a `button` alone in a `col` is as tall as its share of the window — a slab; the shipped workaround (`controls::button` filling its whole cell) fixed the opposite failure, a box shrunk to the text height floating in dead space, and the two failures are the same missing concept seen from either side. And **4, 6, 8, 14, 18, 24, 96 belong to no series**, so nesting one widget in another lands proportions nobody chose. The three milestones follow L1's order — the pure refactor that creates the seam first, then what it makes possible — and none of them adds a dependency, a measurement pass, a solver or a second draw call: the whole track's cost rule (one mesh upload, one draw call per window) is unchanged, and the per-frame path gets *cheaper*, since resolved metrics replace arithmetic now done at paint sites.

- ✅ **L6 — the metrics table: one sizing scale, no literals** *(done 2026-07-29; four departures from the sketch below, all recorded here. The **grid is 2 px, not 4**: the shipped spacing pair is 6 and every shipped chrome extent is even, so a 4-px grid would have moved them and cost the refactor its zero-visual-change guarantee — the generator quantizes extents onto 2 px, hairlines (`divider_w`, `tick_gap`) onto whole pixels. **Text scales quantize to half-steps**, not integers: `caption_scale` ships at 1.5 (the ruler and clip captions) and the bitmap cell has no rung between 1 and 2, so the integer rule holds for the whole-text roles while the reduced caption keeps its half-step — revisit when the font atlas L4 named lands. **The table holds the shared sizing vocabulary, not a widget's own structural geometry**: the patcher's box/port series (`patch.rs`'s own `PAD 8`/`HEAD_H`/`ROW_H`/`STRIP_H`, which interlock in its bounds math), the piano roll's key gutter and velocity/OSC lane heights, the score's staff step and the waveform's amp margin stay in their modules — a role earns its place when more than one widget means the same thing by it, which is what keeps the table readable. Sub-pad optical nudges (a 1–3 px lift on a label baseline, a knob's inner insets) likewise stay literal. And the **density multiplier covers this table, not the wire's own `text_size` prop**, which stays the number the script sent — the wire becoming scalable units is L8's business. Plus the roles the sketch did not name but the constants needed: `ruler_w` (the vertical-ruler strip, 46), `box_side` (a square marker, 24), `indent` (a tree's nesting step, 14), `label_gap`/`tick_gap` (the ruler ladders' collision floors, 14/7), `trace_w` (a drawn trace's weight, 1.5) and `micro_scale` (1.0, the densest in-view mark: a key's octave label).)*: `host/metrics.rs`, the peer of `host/theme.rs` in the agnostic core, same macro shape, same partial-overlay semantics, same config path (`[gui.metrics]` in the shared config beside `[gui.theme]`). Roles named by **function**, not by widget: the spacing family (`pad`, `gap`, `margin`), the control family (`control_h`, `row_h`, `track_thick`, `handle_thick`, `handle_grip`, `knob_d` — its own role, 5–10% over the square nominal, because a disc reads smaller than a box of the same bounding rect), the chrome family (`ruler_h`, `header_w`, `divider_w`, `point_radius`, `hit_slop`) and the text family (`text_scale`, `label_scale`, `caption_scale`). Every constant listed above moves into it and every site reads a role. The **defaults come from one quantized modular scale**: the base is the font cell (`font::GLYPH_H` at scale 2 = 14 px, not a round decimal, because every readable widget is text plus padding and the cell is what makes a button, a number field and a menu line up unaided); spacings step on a 4-unit grid; **text scales are integers only** — the embedded font is a bitmap, so a fractional scale makes a glyph's own pixels unequal, which reads as ragged rather than soft, and that is why the ratio governs spacing and control heights but never the font. The scale is a **generator for the table's defaults, not a runtime computation** — the resolved table is constant data. One `scale` multiplier in the config gives compact/comfortable density for free, which is deliberately the *whole* density surface: no `xs`/`sm`/`md` token props on the wire, no per-widget density, since a host has one density the way it has one look. Tests: role parse and partial overlay (mirroring L1's), the grid/integer quantization invariants, and a default-equals-today guard so the refactor is provably invisible. Wasm-clean by construction (`check-wasm.sh`), so both fronts inherit it with no web-side work. **How it is threaded** (the shape L7/L8 build on): the table lives on the `Host` beside the theme, reaches the render through `FrameInputs::metrics` (so every draw pass that already had the inputs has it), reaches `layout::layout`/`scroll_content` and the per-widget draw/hit functions as one `&Metrics` parameter beside their `&Theme`, and reaches the hit-test/gesture side as `host.metrics` — passed, never global, because L8 resolves one table per window. The browser gets the sibling setter `GuiBridge::metrics(json)` next to `theme(json)`, so a page restyles sizes the way the config does.

- ✅ **L7 — intrinsic size: a widget knows how big it wants to be** *(done 2026-07-29; the visible change, and it redrew every existing GuiDef as planned. Three notes on what shipped. The natural size lives in its own wire pass, `host/widget/size.rs`, beside `build` and `apply` — the third arm-per-kind pass, in the other direction. The eye pass at the window found L6's `knob_d` too small — L7 is its **first reader**, so the role's value only became visible now: at 26 px (a toggle box plus 8%) the disc drew as a 22-px marker floating in a sidebar column. A dial is read by its angle, so the role is now **two lines of control** plus the same 8% (48 px), and the natural height follows it. A **knob is intrinsic in height only**: its disc sizes itself to the shorter side of its body and centres there, so extra width is slack it absorbs, while a natural width would pack a row of knobs to the left and hand the leftover to whatever elastic sibling sat next to them; height was the axis that actually needed fixing. And the eye pass over the examples found the break in exactly one shape — a **single control alone in a window** (`bind.py`, `standalone.py`), which now takes `weight=1.0`, the escape hatch used for what it is for; a row panel of menus that a comment already called compact (`rulers.py`) took the `h` it had always needed, since a container still does not hug its content. Every other example improved untouched (a caption over a view stopped taking half the pane) and the web examples needed nothing: their controls already sit in rows and strips with explicit heights. The `<clausters-bundle>` component builds no widgets, so nothing there to size.)*: each `WidgetKind` gains a **natural size** — a pure function of the L6 table returning an optional extent per axis, `None` meaning elastic — and the main axis of a `row`/`col` resolves in one order: explicit `w`/`h` → **explicit `weight`** → natural size → a share of the leftover. Explicit `weight` beating the natural size is the escape hatch that keeps "stretch this button" expressible, so L2's vocabulary is extended, not replaced; the cross axis keeps filling, and the pass stays **one pass with no measurement and no constraint solver** — the boundary L2 fixed, which is exactly why a natural size must be a function of the metrics and never of the widget's *data* (a scope's height must not follow its sample count, or a `/gui_set` would relayout the window on every message). The split is content vs surface: content whose extent the widget itself knows is intrinsic (`label`, `button`, `toggle`, `number`, `menu`, single-line `text`, a control's label strip), a surface whose extent is the caller's stays elastic (`panel`, `scroll`, `patch`, `track`, the heavy views, `plot`, `nodetree`, `canvas`), and mixed is normal rather than exceptional (a `slider` has a natural thickness across its track and is elastic along it). Hit-testing follows for free — `interact.rs` reads the rects the layout produced — but `hit_slop` is what keeps a small target clickable once it stops being a slab. **This is the one-time break:** every `clients/python/examples/gui_*.py`, every `clients/web/examples/*.html` and the `<clausters-bundle>` component get an eye pass, and the commit says so plainly. Tests: pure layout cases per resolution branch (fixed, weighted, natural, mixed, all-natural with leftover), and the natural sizes themselves against the metrics table.

- ✅ **L8 — logical units and HiDPI: one scale per window, resolved once** *(done 2026-07-30; three notes on what shipped. **Resolution at scale 1 is the identity**, quantizers included: an ordinary display paints the number the config declared (a role set by hand to an odd 5 px stays 5), which is what let the whole track keep its zero-visual-change guarantee and let every existing layout test stand unchanged. **Text is scaled, not re-quantized**: the resolved table's own text roles quantize to half-steps as L6 fixed, while a widget's `text_size` prop times the window's scale stays the number the script sent — the same line L6 drew for the density, and the reason the integer-scale rule in the sketch below applies to the table rather than to the wire. And the **snapping the sketch called "integral rects" is snapping at resolution**, not a pass over the placed tree: the resolved roles and every declared length land on whole pixels (`metrics::snap_px`), while an *elastic* share of a fractional leftover stays exact — rounding those independently would open one-pixel seams between neighbours, and the chrome that must be crisp is metric-driven, not share-driven. Plus what the units seam turned out to need beyond the sketch: the placement `Space` in `layout.rs`, which is what makes "chrome is logical, a navigable plane is physical" a rule the pass carries rather than a convention — a `scroll` workspace's content plane keeps its own units (its `content_w`/`content_h`, its `view_x`/`view_y` and its children's placements), because it has a zoom of its own and its pan is written in the pixels the pointer moved. It follows, and is recorded rather than fixed: a widget's module-local structural geometry (the patcher's box/port series, the roll's key gutter and lanes, the score's staff step) lives in that physical space too, so a plane's interior grows with its zoom and not with the display's density.)*: the wire's `w`/`h`/`x`/`y`/`margin`/`gap`/`text_size` become **logical** units and the host resolves them, paying the deferral L2 wrote down ("logical px, HiDPI-scaled by the front… deferred until the frame as a whole scales, text and margins included") now that the frame as a whole *can* scale. The mechanism is one `ui_scale` per window — **written by the shell, never read by the core**, which is both the platform-seam rule and what `check-wasm.sh` enforces: natively from winit's `scale_factor()`, re-armed on `ScaleFactorChanged` (whose `inner_size_writer` is answered, not ignored) with a re-resolve, a re-layout and a redraw; in the browser from the page. The efficiency shape is the point: the metrics table is resolved to **whole physical pixels once per scale change**, so layout and painting are byte-for-byte the code L6/L7 left and the per-frame cost of HiDPI is zero — which matters most on a page holding forty canvases. Snapping is part of resolution (integral rects, integer text scale ≥ 1) because the chrome is hairlines: dividers, track edges and font pixels are all one unit, and a fractional position turns a crisp line into a two-pixel grey smear. **The heavy views do not move**: `render_width_px` stays physical and "never resolve the signal finer than the screen" is untouched — the two pixel spaces coexist, and that is the invariant to state rather than a detail to remember.
  - **The web half is a real change, not a port.** `GuiBridge::resize(defId, w, h)` takes device pixels *only*: the page multiplies its box by `devicePixelRatio` in `devicePixelBox()` (`clients/web/src/gui/page.ts`) and hands over the product, so **the ratio is destroyed at the boundary** and the browser host has no way to recover it — by design, since "the host never reads the DOM: the element owns its box and reports the pixels". So the binding grows the scale (`resize` carrying it, or a `set_scale` beside it), `devicePixelBox` returns the logical box and the ratio separately instead of collapsing them, and `fit()` and the `<clausters-bundle>` component follow. That reshapes the wasm-bindgen surface rather than extending it; it ships in this milestone's commit and is named there.
  - **Docs:** all three books — `docs/gui-protocol.md` (the units the wire declares), the Python book's builders, the web book's regenerated TypeDoc — plus a `docs/decisions.md` entry covering both halves at once: why the wire is logical while the heavy views stay physical, and why the scale is an input the shell writes rather than something the host detects.

- ✅ **L9 — a plane starts at the density it is drawn on** *(done 2026-07-30; found by eye on `patch1.py` at 2x, right after L8: the chrome scaled and the patcher's boxes did not, so they read half-size against everything around them.)*: a `scroll`'s `view_zoom` becomes `Option<f64>` and resolves through `ScrollView::zoom(&Metrics)` to the window's `ui_scale` when nothing names one — physical pixels per content unit is unchanged as the *meaning*, so the pan, the clamp, the gestures and the hit math are untouched; only the default moves. A named zoom (the tree's, or the wheel's first turn) is literal from then on. The rule this settles, and the reason it is not a patch on the patcher: **a navigable view's default zoom belongs to its content unit** — a display unit (the plane's) takes the density, a data unit (a `waveform`'s sample, a roll's semitone, a `score`'s staff step) keeps its own window and must never see the scale, since a denser screen is more detail over the same span. Zoom-to-fit was rejected as the plane's default (a box's apparent size would follow how many boxes there are, and every edit would re-zoom) and stays a command. Scaling the patcher's own box/port series was rejected too: same result, 35 call sites, and two multipliers competing inside one space. **Two bugs the eye pass caught, both older than the default that exposed them.** `layout::scroll_content` compared a content extent against the *pane's pixels*, which at zoom 1 was the same number and at any other zoom put a graph that should centre itself into the corner by exactly that factor — every extent there is a content unit now, the viewport included. And a **natural size was measured at `text_size` while the drawing draws at `text_size * scale`** (the placement's zoom): the box promised room for a 14-pixel line and got a 28-pixel one, so text spilled into the next row and a knob — which reserves strips for its label and its read-out — had nothing left for its disc, which collapsed to a dot. `natural_size` takes the placement scale now, so a box is measured where its own text lands. Both were reachable before by wheel-zooming a workspace; nothing but the default made them the first thing you see. A third, same shape, cost the wheel its anchor over a **graph-sized** plane: its content extent is "the graph, never smaller than the viewport", and converting that viewport with the *live* zoom made the extent shrink as the zoom grew — the plane the pivot math holds a point in was moving, so the graph slid under the cursor (invisible on a plane with an explicit `content_w`, which is why the older anchor test passed). The conversion uses the plane's natural scale, so the extent is constant under zooming. **And the deepest one, which the eye pass named exactly**: only *text* scaled with a plane's zoom, while every metric role (padding, a track, a knob's diameter, the gap between rows) stayed at the window's table — the old "patcher posture", tolerable while the default zoom was 1 and plainly broken proportions at any other. A placement now carries the table it is drawn with (`Metrics::at`, resolved at the placement's scale) and the layout measures with that same table, so a zoomed box is an enlargement of itself. `layout::Space` grew from two spaces to **three** to make it exact — the window, a plane's content units, the inside of a scrolled child at the accumulated zoom — which also stopped a plane's default margin being counted twice. The default also needed a way *back*: a wire that can only name numbers cannot ask for a default that has none, so `/gui_set view_zoom 0` (or any non-number, the shape an empty `theme` already uses) **clears** the zoom and returns the plane to its density — found through `workspace.py`'s reset button, which pinned `1.0` and so halved a 2x plane.

## E track — the element library: coordinate-system containers and composed elements

Section added 2026-08-07. The tracks above grew the catalog widget by widget, and it worked: each one shipped with its model, its tests and its books. What they did not grow is a **model of what a widget is**, so the catalog now carries the same idea in several hardcoded spellings — three implementations of min/max-per-column, two implementations of a visible window, five ad-hoc value axes, three clip bodies selected by precedence because a clip cannot contain anything. This track replaces the accumulated shapes with **one small model**, and it is a **refactor track**: almost nothing here is a new capability, and the capabilities that do fall out (a navigable plot, a live waterfall, envelopes over a clip, tabs) arrive because the model makes them free, not because they were built.

**The model.** An **axis** is a mapping from a domain to screen extent: a visible window, a unit, and a scale policy. A **container** owns 0, 1 or 2 axes and therefore a *coordinate system* — layout (0 axes, children flow by `row`/`col`/`grid`/`free`), plane (2 axes locked to a uniform scale, the patcher's canvas), or time/value (2 **independent** axes, x time and y a value). An **element** draws against the axes of the container holding it; it owns no navigation. Each child declares **how** it is placed: in axis coordinates, or pinned in layout coordinates. Coordinate systems **nest** — a clip inside a track is a time/value container whose local x is `[0, dur]`, placed at `offset` in its parent, one affine transform per level.

**Why this is the right decomposition** — it is what the existing widgets already are, once the accidents are removed. The heavy timeline views already have two independent axes (`View` in x through the group, `y_start`/`y_len` in y); the `track` already places children by time; the `scroll` already owns a plane. The model does not invent a mechanism, it **moves where the mechanism lives**: out of five per-widget implementations into one container property.

**Generality is the constraint, not the multitrack.** The DAW is one configuration of this library, never its shape. `track` and `clip` are not core concepts — they are time/value containers with particular chrome. A laboratory oscilloscope, a curve editor, a patcher and a multitrack must come out of the same pieces, or the pieces are wrong.

### The two efficiency facts this track is built on (measured 2026-08-07)

Both were measured before planning, on a 10M-frame × 2-channel buffer (`base_bucket` 256) at 1200/2400/3840 device pixels, release build. They are the reason two of the milestones exist and the reason a third one does not.

- **Geometry backend is not the cost; the pipeline count is.** Building the waveform's per-frame columns into the shared `Mesh` instead of its own vertex buffer costs **1.05–1.06×** in the realistic zoom regimes (0.370 ms vs 0.351 ms at 3840 px — 19 µs against a 16.6 ms frame), because **80–96% of that per-frame cost is the column computation itself** (the pyramid reads), which both paths share identically. The per-frame `Mesh::new()` costs 1.05–1.06×, i.e. nothing. **So the waveform is not moved to the mesh**: the real waste is that every `waveform` widget builds its own shader module + 2 render pipelines, and every `spectrogram` builds a shader + pipeline + texture **per channel**, so an eight-channel spectrogram compiles eight of each. Pipelines depend only on device and format; they belong to the window. This is E2. *(Corrected 2026-08-07, while opening E2: this bullet first read "a multitrack of 40 waveform clips is 80 pipelines", which is false — a `clip` body is CPU-side `WaveformData` drawn into the mesh by `track.rs` and allocates no GPU slot at all, so today's duplication is real but modest, bounded by the number of actual `waveform`/`spectrogram` widgets and a spectrogram's channel count. The claim describes where the track is **going**: E7 makes a clip body a signal element and E6 makes the signal element one thing, and from there the slot count scales with the composition. That is what makes E2 a prerequisite of those two rather than an optimization of today, and it is why it is ordered here.)*
- **Geometric clipping is the one real cost.** `Mesh::tri` runs Sutherland-Hodgman per triangle whenever a clip rect is active: **2.6–6.9×**. A column, a lane, a clip body and a note are axis-aligned rects, for which clipping is a clamp — the fast path measures **1.10–1.17×**. Today only `scroll` subtrees set a clip, so nothing pays it; the moment a timeline container goes inside a scroll (which is the point of this track) everything pays it. This is E1, and it is a prerequisite, not an optimization.

### Decisions taken before opening the track

- **Total migration of the wire, not presets** *(decided 2026-08-07)*. The alternative — keeping `waveform`/`plot`/`track`/`scroll`/`clip` as wire names that build the general form underneath — was rejected despite costing no migration. It buys compatibility and pays with two levels of naming: the reference would document 28 types over a model of six concepts, and a reader would never discover that a plot can navigate, because "plot" reads as a different thing. Since the model's whole value is that the general form is what a user learns, hiding it behind the old names defeats the refactor. The migration is not smeared through the track: it is **one milestone (E9)** that moves the wire, both builders, the three books, every example and the persisted bundles together, and carries the versioning consequence.
- **Navigability is a property of the axis, not of the source** *(decided 2026-08-07)*. The live views were nearly filed as "the ones without a time axis", which is wrong: a `scope`'s x *is* time (a triggered window in milliseconds), a `spectrum`'s x is frequency, a `phasescope` has no time at all. What actually separates them is that a tap is **forward-only** — the ring keeps the newest window and there is no addressable past — against the random-access sources (buffer, file, inline). That is the arrangement layer's own generated/generator distinction, and it settles the question cleanly: an axis is navigable when its domain is addressable. A live spectrum's **frequency** axis is addressable now, so zooming into part of the spectrum needs no retention and is available immediately; a live scope's **time** axis becomes addressable only if the host retains a history ring, so retention is a **policy on the axis**. The "missing waterfall" found while inventorying the catalog is therefore not a missing widget but a missing retention policy, which is why there is no milestone for it.
- **The container governs gestures; the element supplies defaults** *(decided 2026-08-07)*. Today `interact::hit` returns the deepest interactive widget and `press` matches on its kind across 15 arms, so that Shift+drag pans and plain drag selects is written inside the waveform's arm rather than belonging to the axis every timeline view shares. Under the model a hit produces a **chain** (container → … → element) and each level may claim a gesture or pass it down, which is what makes gestures configurable. **The chain is not new machinery**: it already exists implicitly, as 25 call sites of by-id lookups that each mean "find my container's state" (`nav` 12, `scroll_view` 4, `set_scroll_view` 3, `nav_of` 2, `note_at` 2, `clip_has_curve` 2), and `hit` already treats one container as a gesture surface — a `scroll`'s empty area is the pan target. So the split across E4/E5 is not mechanism-versus-policy with two mechanisms coexisting; it is **additive first, selective second**: E4 makes the chain explicit while every arm keeps its behaviour, and E5 lifts the six drags that are container-level (`Pan`, `PanY`, `Select`, `ScrollPan`, `Marquee`, `Clip`) out of the ~24 that exist, leaving the other eighteen where they belong.

### What actually constrains the track

Not the size of `gestures.rs` (3995 lines) and `interact.rs` (1414). Only 15 of their arms are press decisions, only six drags are container-level, and the container lookups the chain replaces already exist — so the conversion is additive rather than a rewrite (see the gesture decision above). The real constraint belongs to the project rather than to this track: **nothing in CI runs any example**, so a gesture regression is invisible to the build and is caught only by hand. That fixes the *shape* of the milestones rather than their size — E1 through E4 are behaviour-preserving and verifiable by running the examples each change touches and watching nothing move; E5 onward is where behaviour deliberately changes, so the manual checking concentrates there and each lifted gesture family is checked on its own.

- ✅ **E1 — Axis-aligned clipping is a clamp** *(done 2026-08-07)*: the fast path went into `Mesh::quad` rather than into `rect`, because `rect`, `line` and `border` all funnel through it — so a panel, a lane, a note body, a waveform column and every horizontal or vertical hairline take it, while a disc's fan, a slanted line and a glyph outline keep the general `tri` clipper untouched. Recognition is by **edge** (all four axis-parallel) rather than by corner order, so it holds whichever corner the quad starts from and whichever way it winds — `rect` and an axis-parallel `line` build different orders. It engages only when a clip is actually set; unclipped drawing is the code it always was. Equivalence is tested as **covered area** against the general clipper over ten placements (inside, over each edge, over a corner, flush, outside, touching one edge), which is the only comparison that means anything: the general path emits a fan of up to seven vertices where the clamp emits six. Two things the test surfaced and now documents: a sample landing exactly on the quad's interior **diagonal** is inside both, neither or one depending on rounding — the one place the paths may legitimately disagree, and a place the rasterizer never sees, since the two triangles share exact vertices and the clamp has no interior diagonal at all — and the general clipper **emits degenerate triangles** where the clamp emits nothing (a quad flush against a clip edge collapses to three equal vertices), which paint no pixel either way.

- ✅ **E2 — One pipeline per window, buffers per element** *(done 2026-08-07)*: `WaveformRenderer` and `SpectrogramRenderer` kept their names and became **window-owned**, holding only what a pipeline is a function of (the device and the target format) plus the waveform's build scratch, which is transient frame space rather than element state. What was left behind is genuinely per element and moved into new types beside the views: `WaveformGeometry` (vertex buffer, per-channel ranges, the regime they were built for, the trace palette) and `SpectrogramTexture` (the magnitude texture, its sampler and bind group, the uniforms). Both fronts, the `native` harness and the two prototype binaries hold a `view::Renderers` — the pair, the mirror of `Painter`, which has always had this shape — created once per window before the slots, since a spectrogram texture binds against the renderer's layout. Counted in `tests/gpu_slot_cost.rs`: two waveforms plus an eight-channel spectrogram allocated **22** shader/pipeline objects and now allocate **5**, and the number no longer follows the element count — which, at a single element, saves exactly nothing, and is the whole point at forty. The same test re-checks the measurement behind E1's other half, that 92-96% of a heavy view's per-frame cost is the peak-pyramid column computation both destinations share, so where the vertices are written stays not worth re-deciding. The heavy views stay GPU views and stay structurally identical to each other, which is the property the whole track needs — a waveform and a spectrogram must be placeable in a clip, a panel or a separate edit window by the same mechanism. **The point is not today's saving** (see the corrected measurement above: a `clip` allocates no slot, so the duplication is bounded by the widget count and a spectrogram's channels) **but the cost of a slot**, which E6 and E7 turn into the per-element cost of a composition. A bench in the crate records the before/after, since this is also the evidence for why the heavy views were not moved to the mesh.

- ✅ **E3 — The `Axis` primitive** *(done 2026-08-07)*: one type carrying a visible window (`viewport::View`'s `f64` sample-precise window, generalized past samples) plus the **bound it moves inside** — a domain with a unit and a policy — so clamping, panning, zooming and slicing are the axis's own arithmetic instead of four re-derivations of it. It took the vertical axes of both heavy views, `EditorProps::y_view` and both vertical gesture paths (outside `viewport.rs` nothing calls `clamp_span`/`zoom_span` any more), and then the **value ranges**: `meters::fraction`, `Range` and the linear branch of `bpf::value_fraction` were three copies of one mapping, and `interact::pitch_window` a fourth shape of it — a value domain sliced by a display window. `bpf` keeps only what is genuinely its own, the geometric branch. That corrects the count this milestone was written with: **three implementations plus a slice**, not "five ad-hoc value ranges", which counted widgets — `meters::fraction` was already shared by four of them. It also cost a regression on the way in: `Axis::clamp` bounded the window's start against zero rather than against the domain's origin, invisible for a counted or normalized axis and wrong for every value range (a roll over pitches 48..72 reported 0..24, so no note was ever under the cursor); a test pins it now.

  The third part is the one the milestone was for: **the navigation group is read, never copied**. Selection and playhead used to be mirrored group→members' `EditorProps` on every change, so that the frame path, the animation demand and the MIDI cursor could keep reading the widget tree they had always read. Every one of those readers now resolves the member's group key and reads the group — the same lookup `nav_for` already did per timeline item per frame, so it costs nothing new — and the mirror is gone: `mirror_timeline_group` and its five call sites, and with them one invariant that could only ever hold by discipline. The sweep rule (`head_at`/`swept_at`/`playhead_loop`) and the selection accessor moved off `EditorProps` onto `GroupState`, which is where the values live, so the type system no longer offers a stale copy to read. A member's own `sel_*`/`playhead*` props stay exactly one thing — the **def-time seed** a fresh group takes its values from — and `has_playhead` became a group-aware walk in `host::live`, since whether a window must animate is a fact about the group, not about the props a widget was parsed from. Behaviour-preserving throughout: the wire is unchanged (`docs/gui-protocol.md` never promised a mirror), and the widgets navigate exactly as before.

- ✅ **E4 — Containers own coordinate systems** *(done 2026-08-07)*: a hit-test used to throw away the containment it had just resolved — `interact::hit` walked the layout, returned the deepest widget under the cursor, and whoever needed the container around it laid the **whole window out again** to search for it. A wheel cost three passes, a press on a lane four (`hit`, then `clip_hit` twice, each rebuilding the lane's body and window from scratch), a press on a roll two. Now `Placed` keeps the parent index the recursion already had, and a hit carries the **chain**: the containers over the point, outermost first, the widget itself included when it is one, each naming the coordinate system it gives its contents (`Coords` — layout for `window`/`panel`, a plane for `scroll`, time for a `track` lane, which carries the body its samples map onto and the window they are seen through). `scroll_at` is gone, `clip_hit` and `pianoroll_hit` take the element the hit already found, and `interact::hit` is the only place a window is laid out to answer a pointer question. What deliberately did **not** move onto the chain is what a *live drag* reads: the chain is a snapshot of the press, and by the second step of a plane pan or a note drag the thing being dragged has moved, so those stay lookups of the current state and say so in place.

  Two departures from the sketch. The **placement mode is the container's `coords`, not a second enum beside it**: "flow" and "axis" would have been a second name for what layout-vs-time already says, and the third mode, `pin`, has no widget asking for it yet — it lands with the clip that carries state buttons (E7/E8), not before, so the variant is not built dead. And the **hit's tuple became a struct** rather than growing a fifth element: five positional fields at every call site is where a tuple stops reading, and the pattern matches got clearer, not longer. Two corners behave better rather than identically, both stated in their commits: a press falling through onto a widget that merely *overlaps* a `scroll` (a free layout, no containment) no longer pans that plane, and a lane whose navigation group is not registered yet pans the window's full span instead of refusing. Verified by hand on the workspace, patcher, multitrack, composer, piano and piano-roll examples — which is also how the piano-roll's vertical clamps surfaced (fixed in its own commit: a row leaving the view is cut, not slid back in).

- ✅ **E5 — The container's gestures move to the container, then become a table** *(done 2026-08-07)*: the six drags were not six lifts but **one**, because the chain first had to admit what a container is. `Coords::Time` stopped being the lane's and became every timeline view's — a `TimeAxis` carrying the body the renderer drew through, the navigation window it is seen at and the vertical `YAxis` beside it (its strip, its display window, the lane height one window's worth spans, and the value window where the axis measures something) — and `patch` became a `Canvas`. With the axis on the chain the five re-implementations collapsed on their own: `gestures.rs` no longer imports `track` or `frame` at all, because nothing in it rebuilds a body any more. A press now runs the containers' **plans** innermost-first — an ordered list of steps (`element`, `pan`, `select`, `locate`), each free to decline and pass the press outward — which is one mechanism where there were two: today's "nothing consumed it, so pan the enclosing plane" fallback *is* a plan's last step, and Shift on a patcher's empty canvas reaching the workspace around it is the same walk rather than a special case. The wheel went the same way, and `Select` absorbed the roll's marquee: one sweep, a time span that the linked views follow plus — on an axis that measures a value — the rectangle its elements are picked by.

  Then the plan table became the wire's (`gestures`, an object keyed by modifier, live via `/gui_set`, re-read from the kind's defaults each time so a table names only what it changes), with its two builders, `docs/gui-protocol.md` and an example that shows the same three views under two tables side by side. Two gestures deliberately stayed out of it: a press on a view's vertical strip always pans that axis, and the wheel always zooms — neither is ambiguous, so neither is a policy. Checked by hand on the gesture example (both tables, all four chords, the vertical strips and the wheel), on `gui_rulers` and on `gui_multitrack`; the remaining gesture examples — the patcher's marquee and its Shift fall-through, the workspace, the piano-roll's block edits, the score and the composer — are covered by the suite but were **not** re-run by eye, so that is the first thing to do if one of them looks wrong. Three corners behave better rather than identically, named here rather than left to be found: a press on a lane's **header** (beside the axis, naming no position) no longer locates to sample 0, a press that merely defocuses a text field no longer swallows the plane pan behind it, and the elements a container places are now looked up through the axis the hit resolved rather than through the kind that was matched.

- ✅ **E6 — The signal element** *(done 2026-08-07)*: `waveform`, `plot`, `scope`, `spectrum`, `spectrogram` and `phasescope` differ along exactly three axes and nothing else, so `host::signal` carries them as one element — a **presentation** (the trace, a magnitude spectrum, the time-frequency texture, the phase of a stereo pair) of a **source** (random-access samples: buffer/file/inline; or forward-only: a bus at a rate with its window/trigger/hold policy) with the **capabilities** the view offers over it — and `WidgetKind::Signal` is one arm where six were. The wire still names the six, each a **preset** naming a point of that product, which is the whole compatibility layer and the reason this is a refactor rather than the break: moving the wire onto the model is E9's, as decided when the track opened. What that bought immediately is that `build` and `apply` are one arm apiece, the frame's dispatch is *one* place that sorts an element to the renderer its presentation picks, and every collector that walks a tree — live buses, GPU slots, bulk sources, the gesture table, the lane count — asks the model instead of matching six variants.

  **The three min/max implementations collapsed first, in their own commit**, because that is where the milestone's other half lives: `signal::trace::Trace` is the one column source (an arm for raw interleaved samples, an arm for a peak pyramid) and `draw_channel` the mesh half of the renderer, so a clip body, a plot and the GPU waveform read their columns from one place and differ only in where the vertices land. That made the **renderer split explicit rather than implied**: an element costs a GPU slot only when it is a *navigable heavy* view, and a plot, a live curve and a clip's body all draw into the window's shared mesh — which is what E2's corrected measurement says should happen, and what E7 needs in order to put a signal element inside a clip without allocating per clip. Two corners of the trace behave better rather than identically: a clip body zoomed past the polyline threshold steps by sample instead of by pixel (nothing visible is dropped between one and two samples per pixel), and the plot's polyline and column branches now agree on one span mapping where they used to differ by a sample.

  Three seams the collapse settled rather than carried, each named because a reader will otherwise find them as surprises. A spectrogram had **two `sample_rate` fields** parsed from the same prop — its own and the editor's — and only the editor's took a `/gui_set`, so a rate set before a fetch landed was silently ignored by the analysis; there is one now. The **ruler strips are `EditorProps` for every presentation**, not only the navigable ones, so the live views' boolean spelling of `ruler`/`ruler_y` became `Ruler`/`RulerY` reading a boolean as off/on, and the strip's *unit* now follows the presentation, which is what it always did in practice. And a live view **accepts the editor keys it used to refuse** (`sample_rate`, the selection and playhead props): they mean nothing to it yet, but refusing them was an accident of six separate matches rather than a rule.

  The gaps the matrix exposes — a navigable live spectrum, a retained waterfall — are now expressible in the model (`caps.navigable` over a forward-only source; a retention policy on the axis) but are **not built**: they need the retention half of E3's policy wired to a live source, which no milestone has opened. Checked by hand on `gui_rulers` (both heavy views, all four axis units, the vertical zoom), `gui_scope`, `gui_analyzer`, `gui_plot`, `gui_bulk` (the raw/cache/buffer paths), `gui_multitrack`, `gui_linked` and `gui_standalone` (the persisted bundle still parses, since the wire did not move), plus a headless def holding all six names at once with a live `/gui_set` on each part of the model.

- ✅ **E7 — `clip` and `track` are containers** *(done 2026-08-07)*: a clip's three bodies were fourteen fields on `WidgetKind::Clip` that the *lane* knew how to draw; they are **children** now — a signal element, a piano-roll element, a break-point curve — placed by the layout on the clip's own axis and drawn by the elements themselves. What that bought is visible in what disappeared: `ClipDraw` and the copy-out behind it, the `Clip` arms of both bulk loaders and of both buffer-fetch legs (they fell into the `Signal` arms that already existed), and the take's own loader, since a clip's take is an ordinary `Data` source marked `bulk` — the flag that says "resolve as a peak pyramid, not as an array", which is a property of the source's size and not of the widget holding it. The bodies **layer** because that is the order the layout placed them in, so a Reaper-style envelope over a take is composition rather than a case in a match.

  **The clip's own axis came first**, in its own commit, because nothing else was possible until it did. Every body mapped its pixels through the *lane's* geometry — the body rect, the group's window, and the clip's offset subtracted back out at each of five call sites — so a body was not liftable anywhere. `track::clip_local_view` turns the lane's window into the slice of `[0, dur]` the clip's clamped rectangle actually shows, `Space::on_time` carries it down the subtree and `Placed::time` states it; from the clip inwards everything maps through `(rect, time)` and nothing else, which is exactly what makes a body an element. In the hit chain a clip contributes `Coords::Local` rather than `Coords::Time` — deliberately: a clip is not a navigation-group member, so a pan or a locate started over one still measures against the lane under it, and `time_of` keeps meaning the axis the groups move.

  Two things the split settled rather than carried. Each body now keeps **its own** value axis in fact and not only in the docs: `min`/`max` reach the bodies that measure with them (a take's amplitude, a roll's pitches) and the curve keeps `points_min`/`points_max`, where before one clip-wide pair fed both and a clip carrying notes drew its take against a pitch range. And a body prop arriving for a body the clip does not have **grows** that body (in layering order), which is how a script draws an envelope over a take without rebuilding the def — the wire has not moved, so a `/gui_set` still addresses the clip and `apply_clip_body` routes it to the child that owns it.

  **The header width was part of it** and landed as its own piece: what a container reserves before its axis started as three unrelated numbers (a lane's `header_w`, a roll's `KEYBOARD_W`, a heavy view's `ruler_w`), so stacking a lane, a roll and a ruler on one shared axis drew that axis at three different x. `timeline::group_indents` resolves one gutter per navigation group — the widest any member asks for — and the narrower chrome simply draws into the wider band. That is what made the lane header **sizeable**: `track::Header` carries a name, a mute, a solo and a level fader, presence-driven so a lane that asks for nothing is the name strip it always was, and a width that follows what it carries — and because the width is the *axis'*, one wide lane header moves the roll and the ruler with it. The controls edit back like everything else (`"mute"`/`"solo"`/`"level"`, the names of the props a driver would set).

  Found while checking by eye and fixed here: the multitrack, the composer and the `Editor` still ruled their axis with the bottom lane's own `ruler` strip, which is reserved out of *that lane's* height — the problem the free-standing `timeruler` was added to solve; they place one under the stack now. And a `timeruler` with no `link` joins the window's lanes, as an un-linked `track` already did, since a free-standing ruler exists to rule the lanes beside it. Every other timeline view stays out: a `waveform` among lanes is showing its own buffer, and joining it would be a guess.

  One piece of collateral, in its own commit: `test_gui_props.py` — the only thing standing between a prop and the three edits it costs — had been **red since E6**, because the six wire names stopped being six named schema arms and became one guarded arm over the preset table, so it reported ninety-six phantom divergences and hid whatever real one came next. It reads the guarded arm now, and the clip's bodies (whose props are read outside the clip's own arm, since they are children). With it reading again it found one real divergence, and the manifest was the stale side: the web builder's `plot` had always offered `buffer`/`cache` while the host read neither, and the signal element closed that without anyone noticing.

- ✅ **E8 — `stack`, and widgets that talk to each other** *(done 2026-08-07)*: a `stack` container showing one child at a time (an `index` prop), plus a `"widget"` target for `/gui_bind` alongside the existing `"server"` one. Together they cover alternating a waveform and a spectrogram from a control, tabs and a pager — with no script round-trip, so a persisted GuiDef becomes an autonomous application. Hidden pages skip layout and drawing, and because a heavy element's GPU slot and its bus watch are collected from the **tree** and not from the placements, a hidden one retains both: a tab switch re-uploads nothing, which is the property that made the "skip" a layout decision and nothing else. Tabs stayed composition (a control bound to a stack), so no `tabbar` was promoted.

  Three things the milestone settled rather than assumed. A binding is now an **enum of destinations**, not an address with a keyword in front of it — the wire's `"server"`/`"widget"` keyword was already there for exactly this, and what it bought is that a widget binding has no address, no prefix and no server to fail on. The **apply is the `/gui_set` mutation extracted as a method** (`Host::set_props`), so the two paths cannot drift and the no-cascade rule holds by construction rather than by a check: what a binding fires is that method, which never re-enters delivery. And the delivery seam (`gestures::deliver`) took a `&mut Host` and an effect sink, because an apply can ask for a repaint where a forward could only ask for a datagram — the one place the two destinations are genuinely different, and the reason it is a translation (`HostEffect::Redraw` → `GestureEffect::Redraw`) instead of a shared vector.

  Two smaller calls, both stated where a reader will look for them. A `stack` carries a `margin` rather than a whole `Flow`: it arranges nothing, so the `gap` between children and a `grid`'s column count would have been props the host reads and nothing means — the prop-parity check is what turned that from an opinion into a diff. And an `index` outside the children shows **nothing**, deliberately: a blank page rather than a clamped one, so a pager that runs off its end cannot silently show the wrong child. A multi-value edit-back payload aimed at a widget rides as the JSON string the array-valued props already take, so one curve drives another curve's `points` with no second carrier invented.

  Exercised through the new `stack.py` — a waveform and a spectrogram of one take behind a bound picker, with a slider bound to `view_start` — and headless through `/gui_query` (the `stack` type and its `index` round-trip), plus a gesture-level test that presses a bound toggle and asserts the page moved, the window was asked to repaint and **nothing** left for the script. What that leaves for a human is the one thing none of it covers: clicking the picker and watching a page flip, and flipping back to see that the spectrogram does not rebuild.

  The example is also where the **shape of a second binding** got decided, after the first draft bound a slider to one page's value axis and the reviewer put it plainly: a control that only works on one of the pages has no business being under all of them. The y axis is per view on purpose — a waveform's is amplitude, a spectrogram's is frequency — so nothing vertical is shared; what the pages *do* share is time, through the navigation group. So the slider drives `view_start` on a linked pair, and moves the page nobody is looking at as well as the one on screen. Which is the same tree-not-placements property the GPU slot has, and now has a test of its own: a hidden page is still a member of its group.

### E9's target vocabulary, decided before the code moves

Written 2026-08-08, ahead of the milestone: E9 is a rename of everything at once, so what it renames *to* is the one thing that cannot be decided arm by arm while converting. This section is what the conversion is checked against; where a choice is genuinely open it says so rather than picking quietly.

**The shape of a node stays exactly what it is** — `{id, type, props, children}`, JSON inside one OSC argument, ids client-allocated, unknown types laid out but not painted. E9 changes the **vocabulary of `type` and the ownership of the props**, not the wire's form, and no `/gui_*` address moves.

#### The three kinds of node, and what each one is for

| Kind | What it is | `type` values |
|---|---|---|
| **Container** | owns 0, 1 or 2 **axes**, and therefore a coordinate system its children are placed in | `window`, `layout`, `plane`, `field` |
| **Element** | draws against the axes of the container holding it; owns no navigation | `signal`, `notes`, `curve`, `score`, `keys`, `nodes`, `meter`, `canvas`, `label`, and the controls |
| **Control** | an element with no axis and a value: already at the right altitude, so **it does not move** | `slider`, `knob`, `number`, `button`, `toggle`, `text`, `menu` |

The controls are called out because the temptation is to rename everything. A `knob` is not an accident of the catalog — it names what it is, its props are its own, and nothing about it duplicates another node. The migration's target is where the catalog says one thing several ways.

#### The container table

| New | Axes | Replaces | Notes |
|---|---|---|---|
| `window` | 0 | `window` | unchanged: a window is a root, not a coordinate system, and it is a `layout` that happens to be one |
| `layout` | 0 | `panel`, `box`, `stack` | children flow by `flow` (`row`/`col`/`grid`/`free`/**`stack`**). `stack` stops being a type and becomes the flow that shows one child — which is what it always was: a layout container with a selection instead of an arrangement. Its `index` rides along |
| `plane` | 2, **locked** to one scale | `scroll`, `patch` | a pannable/zoomable plane in content units. The patcher stops being a container type and becomes a `plane` holding `box` elements — the boxes and wires are what `patch` actually adds, and they are elements |
| `field` | 2, **independent** (x and y each their own domain) | `track`, `clip`, `timeruler`, and the container implicit in every heavy view | the time/value container. A lane is a `field` with lane chrome, a clip is a `field` nested in one, a free-standing ruler is a `field` with no elements in it |

**`field` is settled** *(2026-08-08)*, having been the name most open to a better one: it has to cover a lane, a clip and the inside of a plot without reading as any of them, and every alternative leaned toward one use — `region` *is* the clip in DAW vocabulary (Pro Tools, Logic), `chart` reads as the plot, `frame` is already taken twice in the crate (the renderer, an STFT column), and `axes` is the prop key the container carries. `field` is the one that names none of the three, which is what a container covering all three needs.

#### The element table

| New | Replaces | How the old name is reconstructed |
|---|---|---|
| `signal` | `waveform`, `spectrogram`, `plot`, `scope`, `spectrum`, `phasescope` | `view` (`trace`/`spectrum`/`spectrogram`/`phase`) × the source (`bus` = forward-only, `buffer`/`path`/`cache`/`data` = addressable) × `navigable`/`selectable`/`editable`. The table already exists in code as `signal::preset` — E9 deletes it by making the wire say the point directly |
| `notes` | `pianoroll`, a `clip`'s note body | one element, already one model (`host::pianoroll`) |
| `curve` | `bpf`, a `clip`'s automation body | idem (`host::bpf`) |
| `nodes` | `nodetree` | renamed only to stop reading as a *widget* named after a tree |
| `keys` | `piano` | a keyboard is an element; `piano` reads as an instrument |
| `box` | the patcher's boxes (today inside `patch`) | a `plane` places them; the wires stay a prop of the plane, since a wire is between two children and belongs to neither |
| `score`, `meter`, `canvas`, `label` | themselves | already one thing each |

A `clip`'s three bodies stop being props routed to children (E7's `apply_clip_body`) and become **children a script writes**, which is the migration's biggest simplification and the reason E7 shipped the containment first.

#### Where the props go

The migration moves props to whoever owns the thing they describe. This is the half that breaks scripts, so it is enumerated:

| Props | Today | Under the model |
|---|---|---|
| `ruler`, `tempo`, `beat_at`, `quant`, `sample_rate`, `link`, `view_start`, `view_len`, `sel_start`, `sel_len`, `playhead*` | on every timeline view (`EditorProps`) | the **x axis** of the container |
| `ruler_y`, `y_start`, `y_len`, `min`, `max`, `bit_depth` | idem, plus `min`/`max` on five widgets | the **y axis** of the container |
| `offset`, `dur` | on a `clip` | the child's **placement on the parent's x axis** |
| `w`, `h`, `weight`, `x`, `y` | any widget | unchanged — placement in a layout container |
| `data`, `buffer`, `path`, `cache`, `channels`, `base_bucket`, `bus`, `rate` | on each view | unchanged — the **source** is the element's |
| `fft_size`/`window_size`, `hop`, `db_floor`, `db_ceil`, `freq_scale`, `colormap` | on the spectral views | unchanged — the presentation's own parameters |
| `layout`, `margin`, `gap`, `cols`, `index` | containers | unchanged, on the container |

**The name collision this creates is `x`/`y`**, which are already the free-placement props. So the axes are declared under one key — `"axes": {"x": {…}, "y": {…}}` — rather than as bare `x`/`y` objects. That also gives an axis somewhere to grow a `unit` and a retention policy (the waterfall the model already allows and nothing builds).

#### How it lands without a half-migrated wire

E9 is written as one milestone because the wire, both builders, the three books, every example and the persisted bundles are ends of one cable. It still has to survive being interrupted, so it lands **expand → migrate → contract**, each stage green on its own:

1. ✅ **Expand** *(done 2026-08-08)*. The host learns the new vocabulary and keeps accepting all 29 current names as an internal alias into the same constructions. Nothing else moves; every example still runs. `docs/gui-protocol.md` documents the new reference and marks the old names as leaving.

   It landed as **one door rather than a second parse** (`host/widget/vocabulary.rs`): a node is rewritten into the catalog's type and flat props *before* `build` sees it, so there is one construction path, the alias layer is the only thing that knows two vocabularies exist, and stage 3 is a deletion. The rewrite is skipped entirely for a node the catalog already names, which is every node any current script writes.

   Three things the writing settled that the table had left implicit. **A `field` is told apart by what is on it** — a placement (`offset`/`dur`) makes it a clip, children make it a lane, neither makes it the bare ruler — which is the model's claim ("one container, three uses") turned into the only rule that could implement it. **A `plane` is a patcher when it has boxes**, for the same reason: what `patch` adds to a plane is its boxes and cords, so their presence is the distinction rather than a second type name. And the **capabilities became props of the signal element** (`navigable`/`selectable`/`editable` over the preset's), which is the part of the six-into-one collapse that actually buys something new: a trace that does not navigate used to require choosing a different widget.

   The one thing expand cannot do is `box`: the catalog spends it on a synonym of `panel` while the model wants it for a patcher's box, and both cannot hold while both spellings parse. It is deferred to stage 3, and a plane's boxes stay the `boxes` prop until then.
2. ✅ **Migrate** *(done 2026-08-08)*. Both builders emit the new names; with them the examples of the three directories, the Python `Editor`, the web composer, and the bundle path (`host::bundle`, `/gui_load`) — reading either spelling, writing the new one. At the end of this stage everything speaks the model and the old spelling still parses.

   **The builders keep their names.** `panel`, `waveform`, `track`, `clip`, `scope` and the rest stay, as *shortcuts* that build a model node with the props of one common case, and `layout`, `plane`, `field` and `signal` sit beside them for what no shortcut names. That is the one place the "total migration, not presets" decision does not reach, and deliberately: the decision is about the **wire** — the reference documenting 28 types over a model of six concepts — and the wire is now total. A client-side shortcut re-documents nothing: the protocol reference names the model, and the shortcut's entry says which point of it that name is. What it bought is the whole reason the stage landed in one pass: **no example changed**, in any of the three directories, because every call site was already writing a shortcut.

   The cost is that the *Python and TypeScript* surfaces are not total, and stage 3 has to decide whether the shortcuts stay. The argument for keeping them is the same one that keeps `Env.adsr` beside the general envelope; the argument against is that a reader who only ever types `waveform` never discovers that a plot is a trace that does not navigate.

   Four things the migration settled that only writing it could:

   - **A plot is `navigable: 0`, and that is not only a capability.** A trace that does not navigate also resolves its source as the sequence itself rather than as a take, and auto-fits a value axis nobody named — so the vocabulary layer resolves it to the whole `plot` construction rather than to a `waveform` with a flag off. The capability *is* the difference; it just implies more than a flag.
   - **An empty lane is a lane.** `field` was to be told apart by "children, or nothing at all" — but a multitrack opens empty lanes constantly, and reading one as a ruler is worse than any rule. So the ruler is the case that must say what it is: a bare strip of a given thickness `h`, nothing placed on it, no lane chrome.
   - **`flow` is every container's, not `layout`'s.** The model spends the word `layout` on the container type, so the arrangement needed the other word — on a `window` and a `plane` too, not only on the container named after it.
   - **The axis chrome is recorded flat.** `/gui_info` carries flat OSC arguments, so a structural prop cannot be answered — and moving the chrome into `axes` would have made a `/gui_query` stop reporting the ruler, the window and the value range, which is a real loss for a widget whose state a script reads back. So a def's axis pair is flattened at the door, before the registry records it, while the node's *type* is kept as written: a query answers in the vocabulary it was asked in, with the chrome the host itself reads.

   The parity instrument moved with them. `test_gui_props.py` reads all three surfaces by **model** type now, which meant fixing two readers that had been quietly wrong: both keyed a kind by assignment, so with several builders sharing one type (`waveform`, `plot` and `scope` all build a `signal`) the last one read won and the others vanished — and the web reader was attributing the generic `node` escape hatch's `type` parameter to whichever builder followed it in the file. Both now union per kind. The manifest's twelve `idiom` rows on `track` and `timeruler` became four on `field`, since the two are one container.
3. ✅ **Contract** *(done 2026-08-08)*. The alias table is deleted, `test_gui_props.py` is extended to the new surface, the `docs/decisions.md` entry for the axis/container model lands, and the versioning consequence is taken (the `release-versioning` skill; the persisted bundle format is what makes it breaking).

   The old names **stop parsing**: a `type: "waveform"` is an unknown type now, laid out and unpainted like any other. The alias module did not simply vanish, though — half of it was never scaffolding. `vocabulary::resolve` dissolved into the schema, where the discriminations became **guarded arms** of the model's own names (`"field" if it carries a placement => Clip`), and `signal::preset(name)` — the six-name table the whole track called the compatibility layer — became `signal::point(view, live, navigable)`, a function of the product's coordinates with no names in it. What remained is the axis pair and its flattening, which is a *prop* relocation and not an alias, so the file is now `widget/axes.rs` and says only that.

   The **client** surfaces keep their builder names, as decided: `waveform()`, `track()`, `panel()` and the rest are shortcuts onto the model, and `layout=` survives as the old spelling of `flow=` on the three containers that take one. Those are the only three rows the prop manifest gained; the host never sees either.

   The version moved to **0.9.0** — pre-1.0, so the minor is the breaking tier. Neither ABI counter moved: the GuiDef vocabulary is not the shm/embed boundary nor the core FFI, and a saved bundle in the old spelling fails as *content*, not as a version mismatch. Re-save it from a current client.

The transitional aliases are **not** the rejected "presets on the wire" decision reappearing: that decision is about the end state, and stage 3 is what makes the end state total. If stage 3 is not reached, the migration is not done — the alias table is scaffolding, and a release must not ship standing on it.

- ✅ **E9 — The migration** *(done 2026-08-08)*: the wire stops naming widgets and starts naming the model. One milestone moving together, per the packages-move-together rule: `docs/gui-protocol.md` rewritten around containers/axes/elements, `docs/architecture.md`'s widget map and module split, both builders (`clients/python/clausters/gui/guidef.py`, `clients/web/src/gui/guidef.ts`), every example in the three example directories, the Python `Editor` and the web composer, and the persisted bundles (`host::bundle`, `/gui_load`) — which is the piece that makes this a breaking change and drags the SemVer tier along (see the `release-versioning` skill). `clients/python/tests/test_gui_props.py` is extended to the new surface, since it is the only thing standing between a prop and the three edits it costs. The `docs/decisions.md` entry for the axis/container model lands here, when the model is proven rather than proposed. **The target vocabulary is written above** (the section before this one) rather than decided while converting, and the milestone landed expand → migrate → contract so an interruption never left a half-migrated wire.

  What it cost, in the end, was **not** the examples: the two clients' builder names survive as shortcuts onto the model, so every call site in the three example directories kept working untouched. What it did cost is the wire, the persisted bundles and the version (0.9.0). One name is left unclaimed on purpose — `box`, which the model wants for a patcher's box: a plane's boxes are still its `boxes` prop, because making them child elements changes behavior (ids, layout, per-box hit-testing and edit-back) rather than spelling. That is its own milestone, not a rename.

- ✅ **E10 — A clip's take is a presentation too** *(done 2026-08-08)*: `view` on a `clip`, so a take is drawn as the trace (the default) or as the time-frequency texture — the same signal element the `signal` view is, placed in time. The point is what it *stops* being: a spectral lane used to be a heavy view dropped into a lane, spanning the visible axis forever, with no offset, no duration and no drag; now it is a clip, and everything a clip does it does. Three seams moved. The **texture** is not geometry, so `frame` collects a time-frequency body apart from the mesh bodies and draws it in the GPU pass, uploaded against the clip's **own** axis (`Placed::time`) rather than the group's window — that upload is the whole difference between a spectral view of a file and a spectral clip of it. A slot is keyed by whatever *addresses* the widget, so `SignalElement::needs_gpu_slot` replaced `is_gpu_view` at the four collection points (the native window, the buffer-fetch leg, the browser's inline and bulk paths), a clip's body resolving under the clip's id the way its buffer fetch already did. And the analysis props stopped being read twice: `build::spectral_props` is one function for the element and for a take, so a spectral clip is tuned exactly like a spectral view, with the **display** half (dB window, frequency scale, colormap) live on the clip through `apply_clip_body` and the analysis half read when the texture is built. Ported in the same change: `docs/gui-protocol.md`, both builders, and `examples/editors/multitrack.py`, whose third lane is a spectral clip.

  A gesture bug the same example surfaced, open since E7 made a clip a container: the chain's innermost time container over a press on a clip **is the clip**, and its default table answered `element` for every chord — so Shift+drag grabbed the clip instead of panning the lane, and a busy arrangement could only be panned in the gaps between its clips. A clip now takes the plain drag and declines the rest (an **empty** plan, which walks outward, as against `none`, which consumes), because a clip is the container of its own local `[0, dur]` axis and a pan there would move the wrong window.

  Three more the example found, once it was a window someone actually used. **The wheel and Shift+drag off the lanes** did nothing: the gap between two lanes, the slack under the last one and a container's margin belong to the plane, which ate the gesture and — pinned, with nothing to scroll — did nothing with it. A plane that cannot move now passes it on — its wheel and its **pan step** alike (`interact::plane_can_pan`), since a press eaten by a pinned plane is what left Shift+drag dead everywhere but over a lane — and a window with exactly one navigation group answers for those pixels (`interact::sole_time_axis`): the wheel zooms the axis, Ctrl+wheel resizes its lanes, Shift+drag pans it, including on the bare window outside every container. **A free-standing `timeruler` had no `apply` arm at all**, so a `/gui_set` of its unit was recorded in the registry and never reached the drawing — the strip could not be switched between bars, seconds and samples after it was built. And a **`quant` in samples** (the example's own bug, but one the docs invited): `quant` is *beats per bar*, and given a sample count the ruler decides a bar is 24000 beats, finds no step whose labels fit as soon as you zoom out, and draws none — the wire reference and both builders now say the unit.

  Two drawing bugs the same example surfaced. A clip's **name** was drawn with its box, so the take's trace went over it and a spectral clip's texture — a GPU pass after every mesh — hid it outright; the box stays the base mesh's and the name moved to the **overlay**, where the playhead and the selection already are. And the group's gutter was applied **twice** to anything drawn inside a time container — the lane takes it out of its rect to make the body, and a member drawn in there took it out again. A clip's bodies never noticed (they carry no id, so they are in no group); a heavy view used as a lane's body landed a gutter's width right of the clips it shares an axis with, which reads as a desynchronized playhead. `layout::place` now stamps `indent = 0` inside a time container.

- ✅ **E11 — The menu opens** *(done 2026-08-08)*: a press opens the option list over the window and a press on a row picks it — what everyone who saw a menu bar already believed. What it took was not a widget but a **popup**: a thing on screen that is not a placement. It lives with the rest of the interaction state (`Gestures::menu_open`, beside the marquee), reaches the renderer through `FrameInputs` and draws into the **overlay**, last of everything, so it covers the heavy views the GPU pass paints between the two meshes. It is **modal at the door**: `Gestures::press` tests it before the tree and swallows the press either way — on a row it picks, elsewhere it only dismisses — which is one branch at the top of the press rather than a priority rule threaded through the hit-test. The geometry is one function used by the drawing and by the hit (`controls::menu_popup`/`menu_row_at`), so a click lands on the row it highlighted, and the list flips above its field when there is no room below. The hover highlight is read straight off the frame's cursor: a hover is not a gesture and needs no state. The wire did not move — same `options`, same `index`, same event, and a bound menu still forwards without a script — so neither client changed beyond its docstring. Out of scope, deliberately: keyboard navigation and submenus.

  What the milestone replaced was its own interim: the cycling press shipped with E10's marker pointing sideways, which now points **down**.

### Finishing the track (E12–E16)

*Added 2026-08-09, from the refactor audit written after E11 and from the generality audit that followed it.* E1–E11 replaced the catalog with a model, and the model is sound; what is left is **the files the model did not reach** and **the verification the track never got**. Nothing here is a capability (E16 excepted, and it is a capability the model already expresses), so the whole segment is checked the way E1–E4 were: the tests stay green and nothing moves on screen.

- ✅ **E12 — The eye pass and the web pass E1–E11 never got**: the track moved every drawing path in the host and every gesture in the machine, and **not one example was run**. Two verification holes, both named in the refactor audit rather than in a commit. Nothing in CI runs an example (the project rule: they *are* the manual test surface), and a `Draw::new` argument of the same type swapped for another compiles clean — so a pure move is low risk and not zero risk. And `clients/web` was never built or tested, which is the one thing that proves the web package works, and the split of `web/` is precisely what would break it. Order, because the second is expensive: `scripts/refresh-bin.sh gui_shell` first (the bundled binaries beat `target/`, so a stale copy would test the pre-refactor host), then the examples E5 recorded as *not* re-run by eye — `gui_patch1`, `gui_patch2`, `gui_workspace`, `gui_pianoroll`, `gui_score`, `gui_composer` — then `cd clients/web && ./build.sh && ./test.sh` (build first; `dist/` is git-ignored). Acceptance: the list above opened and watched, each divergence fixed in its own commit, and this entry recording what was actually looked at — so the next refactor inherits a list instead of a hole.

  **What was looked at (2026-08-09).** Binaries refreshed first, then the six
  opened and watched: `gui_patch1` (both patcher levels, auto-layout and box
  dragging), `gui_patch2` (the SynthDef and the FaustDef views, the spec's round
  trip), `gui_workspace` (plane pan/zoom, the view reported each frame),
  `gui_pianoroll` (scale edit and playback), `gui_score` (engraving, click,
  transpose drag, transport), `gui_composer` (clip drag and resize, the cursor
  moved from a lane's ruler). Every drawing path and every gesture the track
  moved came back unchanged on screen — **one** divergence, and it was not a
  drawing one: clicking a note in `gui_score` raised `no server to play on`,
  because a handler runs on the client's reply thread and the ambient session
  `play` resolves against is per-thread. Fixed in its own commit by naming the
  server; `pyright` clean over the package, the tests and both example
  directories. Then the web half: `./build.sh` staged, `./test.sh` green — 229
  unit tests and all 19 pages, the split of `web/` intact.

- ✅ **E13 — `interact.rs` splits by the question it answers** *(2036 lines, no submodules)*: the file has the same shape `gestures/mod.rs` had at 2149 before E5 cut it, and `web.rs` had at 2325 — hit-testing, the coordinate-system types, per-element value mutation and per-element data readers (`score_data`, `piano_state`, the `clip_*` and `graph_*` families) in one flat file, carrying the three surviving `allow(too_many_arguments)`. Into `interact/`: `coords.rs` (`Coords`/`TimeAxis`/`YAxis`/`Link`/`Hit` — the chain's vocabulary), `hit.rs` (the one layout pass that answers a pointer question, `chain_of`, `time_axis`, `view_of`), `edit.rs` (the write doors — `set_fraction`, the text field, `bpf_edit`, the scroll-view door) and `read.rs` (the per-element readers the frame and the gestures ask for). A pure move; the `allow`s go with the split rather than surviving it, since each one is a function that was carrying two files' worth of parameters.

  **What shipped.** The four children, and one rule that turned out to be worth
  more than the file sizes: **`coords.rs` never mentions the `Host`**. That is
  what makes it the vocabulary rather than a fourth door — types and arithmetic,
  testable on their own — and it decided every borderline case (the axis math,
  the clip-part split and the drag placement went there; anything that had to
  ask the tree did not). The other three follow the moment of the gesture:
  `hit.rs` answers *what is under the point*, `edit.rs` writes, `read.rs` says
  what an element holds and builds the payload that reports it. `mod.rs` is 68
  lines — the map, plus the re-exports the fronts use, which is deliberately
  *less* than the file held: a name used only inside the module is not
  re-exported, so the list is an inventory of the door rather than of the file.
  The three `allow(too_many_arguments)` retired the way the entry predicted, by
  naming what the scalars were: `CanvasAt` (the area, the workspace scale and
  the cursor — a patch canvas question) and `ClipAt` (the clip's rectangle, its
  own window and the cursor), each one value where four positional floats could
  be paired wrongly. Zero `allow`s left in the module. Green on 501 tests,
  clippy, rustdoc and `check-wasm.sh`; `docs/architecture.md` and the
  `gui-widgets` skill now name the children.

- ✅ **E14 — The schema file keeps the schema** *(`widget/mod.rs`, 3222 lines)*: `build`/`apply`/`size`/`parse`/`axes` are already siblings, so what is left is the enum plus roughly 600 lines that are not the enum — the prop bundles (`EditorProps`, `GestureMap`/`GesturePlan`, `ScrollView`, `Flow`, `Range`, `Place`) to `widget/props.rs`, and the per-kind predicates that are really a query pass (`is_timeline`, `event_value`, `live_bus`, `editor`, `link_lanes`) to `widget/query.rs` — the same third-arm-per-kind shape `size.rs` already is, in the same direction. What stays is `WidgetKind` and its documentation, which is where a reader looking for the model should land. Ordered after E13 because it is the smaller of the two and the one the K track's registry rewrites anyway.

  **What shipped, and the one thing the entry did not foresee.** `props.rs` and
  `query.rs` landed as described — the bundles and their vocabularies (the
  `Ruler`/`RulerY`/`Axis`/`GestureStep`/`Layout`/`Align`/`Rate` words go with the
  bundles they are built from, since they are the same detail), and the query
  pass took both halves: the per-kind readers *and* the **clip routing**
  (`signal_target`, `kind_or_body`, `clip_body`), which is the same question
  asked through a container and had no better home. But the file was 3222 lines
  and the enum plus the bundles plus the queries only account for half of it:
  **1466 lines were the test suite**, which no reading of the entry would have
  guessed and which is why the file did not shrink the way the arithmetic
  promised. It moved whole to `widget/tests.rs` — it is one suite with one
  `node()` helper, testing the schema through `from_node`, and splitting it
  three ways would have duplicated the helper without separating anything.
  `mod.rs` is 661 lines: the enum, the `Widget` node and the tree walk.
  One trap worth recording for the next move of this kind: **`pub(super)`
  changes meaning when an item moves down a level.** Three doors that `host` had
  reached (`EditorProps::parse` from `host::signal`) became invisible the moment
  they were a level deeper, and the compiler said "private" about code nobody
  had edited. They are `pub(crate)` now, which says what they always meant.

- ✅ **E15 — Notation gets a submodule, on both sides**: `host/score.rs` is 1664 flat lines, and unlike the other elements its growth is **semantic, not graphic** — the timemap, the identity of the element under the cursor, transposition in diatonic steps, the edit-back payloads, and everything G31g still owes (tuplets, voices, spelling, articulations). That is the argument that made `signal` a submodule, applied one element later. Host: `host/score/` — `list.rs` (display-list decode), `glyphs.rs` (the outline cache), `tess.rs` (the lyon fill and the painter's strokes), `cursor.rs` (timemap, playback cursor, element hit). Client: `clausters/gui/notation.py` (460 lines) becomes `clausters/gui/notation/` — `engrave.py` (verovio), `mei.py` (the encoder G31g extends, kept as the seam it already is), `timemap.py`, `edit.py`. The web client has **no** notation module at all, so per the packages-move-together rule this milestone leaves `clients/web/PLAN.md` naming the shape the port must follow rather than letting the two re-derive it. No wire change, no behavioral change; the host still never depends on verovio.

  **What shipped, and where the entry guessed wrong.** The host split landed as
  written: `score/list.rs`, `score/glyphs.rs`, `score/tess.rs`,
  `score/cursor.rs`, plus the 500-line suite in `score/tests.rs` and `mod.rs`
  left as the page itself — the geometry types, the primitive, and the
  `ScoreData` every one of those files is a method of. The client half is a
  package too, but with **three modules, not four**: `engraver` (the loaded
  document, the one-shot engrave, the SVG adapter, the page-replacement
  payload), `mei` (the encoder, kept as the seam it already is) and `view`
  (`score_view` + `transport`), beside a private `_abi` for the two shapes both
  native callers share. There is no `edit.py`, and the reason is worth
  recording: **editing is not a body of code here** — `transpose`, `edit`,
  `undo` and `redo` are four methods on the verovio handle, and lifting them
  off `Score` would invent a seam rather than find one. `timemap.py` did not
  survive contact either: the timemap is decoded in the core and consumed by
  `transport`, so it is a line of `view`, not a module. One name changed on the
  way: the submodule is `engraver`, because `notation.engrave` is already the
  function and a module of the same name would have shadowed it at runtime.
  530 Python tests green, `pyright` at zero, both score examples watched by eye
  (a click, a transpose drag, undo/redo, and a page engraved from a timeline).
  `clients/web/PLAN.md` now carries the shape the port must follow.

- ✅ **E16 — Retention is a policy on the axis** *(the one gap the model expresses and nobody built)*: E3 decided that an axis is navigable when its domain is addressable and that a forward-only source becomes addressable only if the host **retains** a history — and then built the clamp/zoom half and not the retention half. E6 confirmed it from the other side: a navigable live spectrum and a waterfall are reachable in the model and absent from the catalog. This wires the missing half: a retention ring per watched bus, sized by the axis's declared span, filled by the same live path that already reads the segment each frame, so `caps.navigable` over a forward-only source stops being a combination that parses and does nothing. Acceptance: a retained waterfall and a zoomable live spectrum in `gui_analyzer`, and the retention span live via `/gui_set`.

  **What shipped.** `retention`, in **seconds**, on the forward-only source —
  the unit the axis is declared in, so the same seconds are the same seconds
  whatever the frame rate, the `fft_size` or the `hop`. Two stores, keyed
  differently on purpose: a **history per watched bus** (`live::BusHistory` —
  one history however many views read it) and a **rolling transform per widget**
  (`waterfall::Waterfall` — two views of one bus may analyze it differently).
  The transform is rolling rather than recomputed: at 30 fps over an 8 s span
  a full re-analysis would be ~750 FFTs a tick to learn what two of them say, so
  each whole hop is analyzed once, through the very function the stored path
  uses (`spectrogram::column_into`, factored out for exactly this) — which is
  what makes a retained waterfall and an offline spectrogram of the same audio
  the same picture, and lets the renderer, the frequency ruler and the cursor
  readout stay one implementation.

  **Three things the entry did not foresee, all found by eye and all worth
  keeping.** *(1)* The append must be by **stream position**, not by window:
  two ticks read overlapping windows and the overlap follows the frame rate, so
  appending windows would stretch the history. `BusSource::read_bus_at` carries
  the position; a stretch the reader never saw is retained as **silence**, so a
  drop-out is visible instead of being compressed out of the time axis. *(2)* A
  retaining read must size itself by **what the source will serve**
  (`BusSource::window_limit` — half the tap ring): asking for more is refused,
  and a refusal is indistinguishable from a bus nobody writes, so the first
  build retained nothing and looked exactly like silence. *(3)* The one that
  actually took two rounds at the window: **a live axis has to register its
  length, and follow it.** Without the length the navigation window falls back
  to a span the size of the body in *samples* and the whole history draws as one
  stretched column; with the length but a kept view, the view walks off a
  history that slides underneath it and the picture stalls. The rule is
  `Host::set_live_timeline_total`: **follow the newest until you navigate it,
  then stay where you were put** — "untouched" being the view still spanning the
  whole axis, which a zoom or a pan stops being true of.

  **Not done, and it is a separate design rather than a leftover: the zoomable
  live `spectrum`.** The waterfall is a navigable live view and it is what the
  entry's body describes; the *curve*'s frequency-axis zoom is a different
  mechanism. A `spectrum` is not a timeline container — it has no `Coords::Time`,
  no navigation group, no x window — and its x domain is **frequency**, not
  time, so making `navigable` mean something over one means deciding what a
  navigation group is when its axis is not the window's time (today
  `signal::point` deliberately drops `navigable` for a live spectrum). The draw
  side is nearly free — `draw_spectrum` already maps every column through
  `ruler::display_to_hz`, so a normalized `[start, len]` sub-range is one
  remapping there and one in `hz_ticks_h`. It is the gesture and grouping half
  that needs a decision, and inventing one here would land it in the same voice
  as a staged design. It is **E17**, opened below.

**Not a milestone here — `box`.** The catalog spent the name on a synonym for `panel` and E9's model wants it for the patcher's box, but a plane's boxes are still its `boxes` prop. Turning them into child elements changes ids, layout, per-box hit-testing and edit-back — it is behavior, not spelling — so it belongs to **P5**, the patcher's editing surface, where it is now written down as that milestone's first item rather than only pointed at from here.

### The axis E16 left open, and what the GPU path still owes (E17–E19)

*Added 2026-08-09, from what closing E16 found; E19 added 2026-08-10, from the
by-eye pass that closed E17.* Retention made a forward-only source addressable
and proved the model end to end, and it surfaced two things that are genuinely
missing rather than merely unbuilt. Neither is a refactor: one is a capability
the acceptance line asked for and did not get, the other is a cost the first
landing knowingly paid. The third is older than both — a defect the examples
walked past for as long as there have been GPU windows, and that only a pass
with an eye on the drawing was ever going to catch.

- ✅ **E17 — A navigable axis whose domain is not time**: E16 shipped the
  retained waterfall and **not** the zoomable live `spectrum` its acceptance
  line also named, because the two are not the same mechanism and the second has
  no staged design. Everything navigable in the host today navigates *time*: a
  navigation group is keyed by `link` over a shared time axis, `Coords::Time`
  carries a `TimeAxis`, `is_timeline` is what joins a widget to a group, and the
  span is measured in samples. A `spectrum`'s x is **frequency** — a domain that
  is addressable without any retention at all (E3's own observation, and E6's) —
  so `navigable` over one is a combination the model expresses and the code has
  nowhere to put: `signal::point` deliberately drops the capability for a live
  spectrum rather than pretending.

  What the milestone has to decide is exactly one thing, and it is not a
  drawing question: **what a navigation group is when its axis is not the
  window's time.** Two shapes are open and the choice belongs here rather than
  in an implementation — a *second axis kind* beside `Coords::Time` (a
  `Coords::Domain` carrying its unit, so a frequency axis groups with other
  frequency axes and never with a lane), or the **element's own window**, no
  group at all, in the shape `EditorProps::y_view` already has for the vertical
  axis (a normalized `[start, len]` the element carries alone). The second is
  smaller and is probably right for a spectrum, which nothing else would ever
  share an axis with; the first is what a *pair* of spectra comparing two buses
  would want, and it is the one that generalizes to the arbitrary-domain plot.
  Decide it, record it in `docs/decisions.md`, then build.

  The draw side is nearly free once that is settled and is worth stating so the
  cost is not overestimated: `spectrum::draw_spectrum` already maps every screen
  column through `ruler::display_to_hz`, so a normalized sub-range is one
  remapping there and one in `ruler::hz_ticks_h` — the same inversion the
  spectrogram's frequency ruler and cursor readout already share. The work is
  the gesture and grouping half. Acceptance: a live spectrum in `gui_analyzer`
  zoomed and panned on its frequency axis, the window round-tripping through
  `/gui_set` and a `"view_x"`-shaped event, and the ruler revealing finer rungs
  as it zooms — the property L8 already fixed for every other axis.

  **The decision, and the argument that settled it.** It is the **element's own
  window**, and not because the diff is smaller: *frequency is already navigated
  that way in this host*. A spectrogram's frequency axis is its vertical one,
  and that axis is a normalized per-element window with no group behind it —
  because a display axis over `[0, Nyquist]` on a log/mel/bark scale is a
  display coordinate inverted through one shared mapping, and nothing else in a
  window measures in hertz to share it with. A grouped x would have given one
  domain two mechanisms depending on which way it happened to be drawn. So
  `EditorProps` grew `x_start`/`x_len` beside `y_start`/`y_len`, read through
  `x_view()` with the same clamp and the same read-at-use validation, reported
  as `"view_x"` beside `"view_y"`. The general case is not foreclosed: the
  per-element window stays where the answer is stored, so a future group would
  *write* it, exactly as a timeline group writes its members' view today.
  `docs/decisions.md` carries the record.

  **What shipped, and the three things the entry did not foresee.** The draw
  side was as cheap as promised — one remapping in `draw_spectrum`'s `bin_at`,
  one in `plot`'s (the stored spectrum navigates on the same window, for free)
  and a windowed `ruler::hz_ticks_h`. *(1)* That ruler needed more than a
  remapping to keep L8's property: the horizontal twin only walked decades, so
  zoomed past one it had nothing finer to show. It now picks between the two
  schemes its **vertical** twin already picked between — decades over a wide
  window, a 1-2-5 ladder in hertz fitted at the actual (nonlinear) positions
  over a narrow one — which is what makes zooming reveal finer round
  frequencies. *(2)* `navigable` had to become **opt-in for this one view**:
  the prop defaults to on everywhere else, and letting it default on here would
  have made every shipped `spectrum` start panning under a drag, unasked. *(3)*
  `is_nav_signal` had to stop meaning "navigable signal" and start meaning
  "navigates *time*" — otherwise a navigable spectrum joins `is_timeline`, and
  a curve over frequency lands in a navigation group measured in samples. That
  one line is the whole seam between the two axes. Green on 515 tests (the
  ruler property, the props round trip, and press/wheel/`R` over the new axis),
  clippy, `check-wasm.sh`, `pyright` at zero and the web suite; both builders,
  `docs/gui-protocol.md`, both books and `gui_analyzer` carry it.

- ✅ **E18 — The waterfall writes its new columns instead of re-uploading the
  texture** *(done 2026-08-10)*: E16's rolling analysis is incremental — a hop's worth of samples
  costs one FFT, not a re-analysis of the span — but the *upload* is not: every
  tick that lands a column rebuilds a `SpectrogramView`, which allocates a
  texture and writes the whole magnitude image. **The number, so this is a
  measured cost and not a worry**: the texture is `R8Unorm`, one byte a texel, so
  an 8-second span at hop 512 and `fft_size` 1024 is 750 columns × 512 bins =
  **384 KB per tick, ~11.5 MB/s at 30 fps**, plus one texture allocation per
  tick per waterfall. That is affordable — it is why the milestone landed this
  way — and it is also the wrong shape: it scales with the *span* where the work
  scales with the *hop*, so a minute of retention costs eight times as much to
  show the same two new columns.

  The fix is the classic one and it is what a ring texture is for: keep the
  texture allocated, `write_texture` the **new columns only**, and let the shader
  unwrap a write cursor (one more uniform beside the `time`/`freq`/`db` triple
  the pipeline already carries) so the oldest column reads as the leftmost.
  Its cost is a change to `spectrogram.wgsl`, which **every** spectrogram shares
  — the stored path included — so the invariant to hold is that a stored
  transform is the degenerate case (cursor 0, never advanced) and draws
  byte-identically to what it draws today. Ordered after E17 because it changes
  nothing a user can see and E17 changes something they asked for.

  **What shipped, and the one thing it did differently.** The ring is there and
  the numbers are the ones above (`tests/gpu_slot_cost.rs` states them: 384 000
  bytes a tick becomes 3 072, and eight times the retention leaves that
  unchanged) — but **`spectrogram.wgsl` was not touched**. The entry assumed the
  shader would unwrap a write cursor, and unwrapping is exactly what the shader
  must not do: the sampler filters linearly, so a coordinate that wraps blends
  the newest column into the oldest and sends a visible seam travelling through
  the picture. Storing the ring **twice**, back to back, makes the visible window
  one contiguous run of texels however far the cursor has advanced — and a
  contiguous sub-range is *precisely* what the `time` uniform already expressed.
  So the whole change is CPU-side: the invariant the entry asked for (a stored
  transform draws byte-identically) is not a property the diff has to preserve
  but one it cannot break, since a stored transform reaches the same shader
  through the same uniform. The doubling costs the texture twice the memory and
  halves the retained-column ceiling (`MAX_ROLLING_FRAMES`, 4096 — the pair still
  has to fit the 8192-texel dimension a plain device guarantees).

  Where the state landed follows from that: the **ring is the transform**, an
  `Stft` in rolling form (`capacity` + `head`, columns read through `column`),
  owned by the GPU view that draws it — so the renderer, the frequency ruler and
  the cursor readout still see one type and never learn which form they have.
  `host/waterfall.rs` correspondingly stops owning the retention: it analyzes and
  hands over the columns that landed (`take_pending`), and both fronts push them
  through one shared `frame::roll_into_slot`. A live `retention` change resizes
  the ring around its newest columns and rebuilds the texture once — one full
  upload per change, against one per tick before. Green on 521 tests (the ring's
  wrap, its contiguity in the texture, the stored degenerate case, the resize,
  and the property that a rolling transform *is* the stored one column for
  column), clippy, `check-wasm.sh`, the doc build, and `gui_analyzer` by eye
  including its live halving of the span.

- ✅ **E19 — A heavy view hanging off the window is cut, not squashed**
  *(done 2026-08-10; found by eye the same day, checking the trace-weight fix in
  `gui_multitrack`: pan a spectral clip until its lane pokes past the bottom of
  the window and the picture **flattens into what is left** instead of being
  cropped — the content behaves as if it were sized by the visible area rather
  than by the lane.)*

  The cause is one function. Every heavy view builds its geometry in NDC
  `[-1, 1]` and lets `set_viewport` place it, so the viewport is not a crop but
  a **scale**; and since wgpu refuses a viewport that leaves the attachment,
  `frame::clamp_viewport` shrinks the rectangle against the framebuffer. A view
  overhanging the bottom therefore gets its whole picture mapped into the
  remaining height, and one overhanging the top gets its origin clamped to 0 and
  slides instead of being cut. The scissor beside it (`frame::apply_scissor`) is
  right and its comment already says the viewport "positions but does not cut" —
  the sentence that is missing is that it also *scales*, which is where the bug
  lives. It dates from `277f79f`, the track's first GPU window.

  **Why only a spectral clip shows it today**, which is also why the milestone
  is small: every other clip body is drawn into the shared mesh and cut
  geometrically, and the one exception is a take whose `view` is the
  time-frequency texture — `frame/items.rs` collects it apart precisely because
  it is a texture rather than geometry. The navigable waveform and the `canvas`
  have the same defect and no example currently parks one against a window edge.

  The fix is to stop asking the viewport to do the framing: keep it legal
  (clamped, as now) and hand each pipeline the **visible sub-rectangle** as a
  scale + offset in NDC, one more uniform beside the ones each already carries,
  so the geometry is placed for the full rect and only the visible slice is
  rasterized. It touches the three heavy pipelines — waveform, spectrogram,
  `canvas` — and the `canvas` one is user-authored WGSL, so whatever the uniform
  is called becomes part of that contract and belongs in `docs/gui-protocol.md`.
  Acceptance: a lane dragged past each of the four window edges keeps its
  content at a fixed size and simply loses what is outside, in all three
  pipelines; the same view fully inside the window draws byte-identically to
  what it draws today.

  **What shipped, and the half of the bug the entry did not name.** The framing
  is a `Framing` in `src/view.rs` — a `scale * ndc + offset` a view applies to
  its own geometry — and the viewport keeps taking the visible part of the rect,
  as planned. But `frame::clamp_viewport` had to stop being a *clamp*: clamping
  the origin is what made an overhanging **top** slide, since the rect kept its
  full height and simply moved down into the window. It is the **intersection**
  now, and the framing is built from that pair.

  Where each pipeline takes it differs, and follows where its geometry is built.
  A **waveform** builds its columns on the CPU every frame, so the framing goes
  into the vertices, one per *lane* — each channel is drawn with its own
  viewport, so a stack can have its top lane whole and its bottom one cut. The
  **spectrogram** and the **canvas** draw one full-screen triangle from a
  uniform block, so each grew a `rect: vec4` and one line in its vertex stage;
  `uv` is taken from the *untransformed* corner, so the framing moves the picture
  without moving what each of its pixels samples.

  **The `canvas` contract did not change**, against what the entry anticipated:
  the vertex stage is the host's prelude, not the user's, so `shade(uv, frag)`
  sees exactly what it always saw and `docs/gui-protocol.md` is untouched. The
  added field is host-owned (`_rect`).

  One thing no test could have caught: the spectrogram's uniform was declared
  visible to the fragment stage only, so the first run died creating the
  pipeline. Nothing in the suite builds a GPU device — the example is the only
  thing that runs a shader, which is exactly why it is the manual test surface.
  Green on 526 tests (the identity, both vertical edges, the horizontal one, and
  the viewport's intersection), clippy, `check-wasm.sh`, the doc build, and
  `gui_multitrack` by eye at all four edges.

### What a pass with an eye on the drawing found (E20–E21)

*Added 2026-08-10, from the same session that opened E19.* Closing E17 and the
trace-weight fix meant opening four examples and watching them, and the pass
turned up two more things neither change caused. Both are older than both
changes, both are one rule away from right, and both are the kind of thing no
test was ever going to report — a label that runs out of room, and a gesture
that lands somewhere the reader did not point.

- ✅ **E20 — A ruler strip is as wide as its own labels** *(done 2026-08-10)*: the vertical strip's
  width is a fixed size role — `metrics.rs`'s `ruler_w`, `grid(5.0 *
  advance(caption_scale))`, five caption characters, sized for `-32768`, `20K`
  and `-INF`. But a label's width is a property of the **data**: a value axis
  over small numbers formats `0.0625` and `-0.0625`, six and seven characters,
  and the label is then clamped against the strip's edge and runs out of room.
  (Found on a spectrum's dB axis zoomed in; any auto-fitted `plot` over a small
  range does it too.)

  **Not a bigger constant.** That charges every view in the system the widest
  case a few axes have, and it would be a number picked to survive the examples
  rather than a rule. The rule is that the strip asks for what its own ticks
  need — and the negotiation for that already exists one level up: a navigation
  group's gutter is *the widest any member asks for* (`layout.rs`'s `indents`
  table, `Ctx::indent`), which is exactly this question asked across a group.
  So: the tick formatter reports the widest label it will draw, a view's
  vertical strip asks for that instead of the role, and the group table keeps
  reconciling members as it already does. The role stays as the **floor**, so
  an axis whose labels are narrow looks exactly as it looks today.

  Deliberately **not** in scope: the caption font and its advance. This is about
  how much room the strip asks for, not about the glyphs it asks for it in.
  Acceptance: a `plot` auto-fitted over `[-0.1, 0.1]` and a zoomed dB axis both
  label every tick in full, no clamping; a waveform's `-1.0 … 1.0` axis and a
  spectrogram's hertz axis are pixel-identical to today; two linked views with
  different label widths still share one gutter, the wider one's.

  **What shipped, and the one thing the entry underestimated.** `ruler` gained
  the measure (`ticks_width`, and the two floored forms `value_strip_w` /
  `amp_strip_w`), and it is asked in the two places a vertical strip is
  reserved. In the **local** ones — a `plot`'s, a `meter`'s, a `spectrum`'s —
  the body is right there, so the reservation just had to be reordered: the x
  strip takes height and the y strip takes width, so the *height* is settled
  first, because it is what decides how finely the axis steps and therefore how
  wide its labels are.

  The **group gutter** is where the entry was optimistic. `own_gutter` answers
  from the kind alone, and it has to: that is what lets the layout place a
  lane's clips before a single rectangle exists. But a label's width depends on
  the member's *height*, which is one pass too late — so `layout_on` measures
  the placed members and lays out **again**, and only when the measure asks for
  more than the roles reserved, which an ordinary window never does. The
  circularity is only apparent: the gutter moves a body's x and never its
  height, so the second pass reads a height the first one already settled.

  Two things the measure deliberately does not do. A **hertz** axis stays on the
  role — its labels are short and bounded (`20K`, `1.5k`, `440`) and the
  frequency they run to belongs to the analysis, not to the tree. And a member
  is measured as **one lane**: a stacked view's lanes are shorter and step more
  coarsely, so this asks for at most what a multichannel member needs and never
  for less. Green on 533 tests (the strip measure and its floor, a zoomed
  amplitude gutter, linked members sharing the wider one, the hertz axis
  unchanged, and the local dB and auto-fitted-plot strips each growing by
  exactly what their widest label needs), clippy, `check-wasm.sh` and the doc
  build; `docs/architecture.md` carries the second pass.

- ✅ **E21 — The wheel's fall-through is for pixels with nothing on them** *(done 2026-08-10)*:
  when nothing under the pointer claims the wheel and the window has exactly one
  navigation group, `gestures::wheel` treats those pixels as that axis and zooms
  it. The justification written beside it names *"a gap between lanes, the slack
  under the last one, a container's margin"* — pixels with nothing drawn on
  them, which is right. What the code actually tests is only whether the wheel
  was **claimed**, so it fires over an element that draws its own picture and
  simply has no wheel gesture: in `gui_analyzer`, turning the wheel over the
  goniometer zooms the waterfall underneath it. The reader pointed at one thing
  and another moved.

  The missing rule is the one already written in prose: the fall-through is for
  *empty* pixels. What makes it more than a one-line gate is that **the press
  path shares the mechanism and means it differently** — `pan_sole_axis` is what
  makes Shift+drag pan the axis from anywhere at all, over any element, and that
  is intended and documented. So the milestone has to separate the two: either
  the wheel's fall-through alone requires empty pixels (the press keeps its
  reach), or the distinction becomes a property a widget answers for — an
  element with a picture of its own against a label, a container, a gap — and
  each gesture says which it wants. Prefer the second if it stays small: it is
  the same question the gesture map answers everywhere else.
  Acceptance: the wheel over the goniometer (and over any element that has no
  wheel of its own) leaves the sole axis alone; over a gap, a margin and the
  slack under the last lane it still zooms it; Shift+drag still pans from every
  one of those places, including over an element. (E17 removed one instance of
  this by giving a navigable spectrum a wheel of its own — that is a coincidence
  of that milestone, not a fix of this one.)

  **What shipped**: the second shape, and it did stay small. `WidgetKind`
  answers `is_bare_surface` — a container's own surface, a label, a lane's empty
  space (and the strip that rules it, whose empty pixels *are* the axis), an
  unknown type this host does not paint — and the **wheel** asks it before
  falling through while the **press** does not, which is the whole separation.
  A one-line gate in `gestures::wheel`, a predicate beside the other kind
  queries, and the prose in `docs/gui-protocol.md` now says both halves out
  loud: the wheel needs the pixels to be empty, Shift+drag reaches from
  anywhere over any element. The regression test asserts both from the same
  pixel and fails without the gate.

### The hit-test pass: a control answers on what it draws (E22-E23)

*Added 2026-08-12, from a by-eye pass over the standard controls.* Every
pointer question in the host was answered by containment in the **placement
rectangle**, which is right for what fills its cell and wrong for everything
drawn smaller or rounder than the cell the layout handed it. A knob's dial is a
disc under a label strip and over a read-out; a slider is a thin groove in a
track area several times its thickness; a checkbox is a small box with a word
beside it in however wide a row it landed in. Each of those was acting on
presses that landed on blank space around the drawing, and the air around a
control belongs to the window, not to the control.

- ✅ **E22 — the seam, and the two dials on it** *(done 2026-08-12)*: `Element::hit_area`
  returns the shape the element answers the pointer on (`HitArea::Rect`, the
  default and the whole placement; `Disc`; `Ellipse`), and
  `gestures::element::press` is the **one** place it is applied, adding the
  metrics' `hit_slop` so no leaf decides how much air a small target deserves.
  A point off the shape reads exactly as a `Claim::Decline`, so the press falls
  back to the chain like any other; a leaf that declares nothing keeps the
  rectangle it always had, so nothing else in the catalog moved. The round
  geometry is `graphics::shape` (`in_disc`, `in_ellipse`, `dist2`), **squared
  throughout** — a hit-test compares a distance and never reports one, so the
  square root has no reader. Each shape is read off the same function the
  drawing places it with (`controls::knob_disc`, `controls::slider_groove`,
  `controls::toggle_hit`), so the two cannot disagree. The **overlay** route
  deliberately skips the filter: a modal is offered the press because it is
  outside as often as because it is inside (a click on the window closes a
  menu's list). What landed with it: the break-point grab dropped its `sqrt`,
  and the score's hit index carries a `HitShape` per entry so a **notehead** is
  the oval inside its box instead of the box.
  **Verified by eye** on `gui_panel`: the knob, the slider and the toggle now
  answer only where they are drawn. The toggle took a second pass and the miss
  is worth keeping: the first bound its width and returned the cell's whole
  **height**, and in a `panel` with `layout="row"` that cell is as tall as the
  tallest control in the row — so the air over and under the box still flipped
  it, and a half-bounded shape reads exactly like an unbounded one. A shape is
  two axes; bounding one of them is not a partial fix, it is none.

- ✅ **E23 — the rest of the chrome on the same rule** *(done 2026-08-18)*. Every remaining light widget states its drawn shape through `hit_area` — a pass over the catalog, as expected, with the mechanism unchanged.

  **What changed**: `number`, `menu` and `text` are a strip under a label, and each now answers on that body (read off the same `controls::body_rect_at` that draws it) instead of on the cell a stretched row handed them — a press on the label strip or on the run beside it used to turn a value, open a list or land a caret in text nobody pointed at.

  **What was already right, recorded as checked rather than changed**: a `button` *is* its cell and fills it, so the default rectangle is its shape; a lane header's controls are hit on the rects `header_parts` lays them out at; the patcher's boxes and ports are hit on `obj_rect` and `port_cell`, which is what they are drawn as (confirmed, not assumed). A `label` answers on **nothing at all** — it takes no press, so it has no shape to state, and the pixels it marks stay the container's.

  **The two questions, answered.** *Hover and the wheel*: the wheel now reads the shape, and it was the worse of the two cases — a press is aimed and a wheel is wherever the hand left the pointer, so an element reaching past its own drawing takes turns meant for the axis behind it. Hover needed nothing: the only passes that read the cursor read their own body already. *A hugging container whose child answers on less than it was given*: nothing to do — a hug gives the child its natural size, and a hit shape is not a size; the air a shape leaves inside a cell is the container's, which is the rule everywhere else.

  **Not verified by eye yet**, unlike E22: the changed widgets are `number`, `menu` and `text`, which `gui_panel` shows, and the check wants a window and a hand. Written down here rather than claimed — the by-eye pass over that example is the one part of this entry still open.

## A track — what a signal measures, and the layers that show it

*Section added 2026-08-12, from the user's brief: true peak drawn at sample zoom, the loudness descriptors as a general library (so a file can be analyzed headlessly, not only looked at), and a system of **layers** — synchronized visualizations drawn over one another, a waveform over a spectrogram, an RMS envelope over a waveform with a configurable alpha, the way Sonic Visualiser stacks layers in a pane.* Two subjects, one track, because they are one sentence: a measurement that cannot be *drawn over the thing it measures* is a number in a log, and a layer stack with nothing to stack is a container that already exists. The track ships the measurements as a **library first** and the drawing as its consumer, which is also the order that keeps the API honest — nothing in `clausters-core` may be shaped by what a pixel needs.

**The whole track is bound by one constraint the user stated: the implementation is general where it can be, behind a clear API that is easy to maintain and to use.** Concretely, three rules every milestone below is checked against. **One algorithm, two faces** — every descriptor is a one-shot function over a slice *and* a streaming meter fed block by block, with the streaming one the implementation and the one-shot a loop over it, so an offline analysis of a file and a live meter on a bus can never disagree and there is no second code path to maintain. **The face is uniform** — the descriptors take the same shape (samples, channel count, sample rate) and return the same shape, so learning one is learning all of them, and a new descriptor is a new type in one table rather than a new spelling. **Nothing enters the API for the drawing's sake** — a display concern (a per-pixel column, a colour, an alpha) stays gui-side, and the test of a placement is still the E-track's: would anything outside a window ever call it.

### The descriptors, and which ones ship

Named here so the track is not re-derived from memory, and because the user asked which measurements matter. All of them are over interleaved samples with a channel count and a rate; all of them are `clausters-core`.

| Descriptor | What it is | Where it is defined |
|---|---|---|
| **Sample peak** | the largest absolute sample over the span, in dBFS | already the pyramid's min/max |
| **True peak** | the peak of the *reconstructed* continuous signal, which a sample peak underestimates by up to ~3 dB — attenuate 12.04 dB, oversample ≥4× through the specified polyphase FIR, take the maximum, restore the gain; reported in **dBTP** | ITU-R BS.1770-4, Annex 2 |
| **RMS** | root mean square over a span or a bucket; the perceived-level body an editor draws inside the peak envelope | classic; A1 stores its **mean square** |
| **Loudness (LUFS/LKFS)** | K-weighted mean square summed over channels with their weights: `-0.691 + 10·log₁₀(Σ Gᵢ·zᵢ)`. **Momentary** = a 400 ms window, **short-term** = 3 s, **integrated** = the gated mean of 400 ms blocks (75 % overlap) with an absolute gate at −70 LUFS and a relative gate 10 LU below the ungated mean | ITU-R BS.1770-4 + EBU R128 |
| **Loudness range (LRA)** | the spread of the short-term loudness: 3 s blocks, absolute gate −70 LUFS, relative gate −20 LU, the 10th-to-95th percentile in LU | EBU Tech 3342 |
| **Crest factor / PLR** | peak-to-RMS and true-peak-to-integrated-loudness; one division over descriptors already computed, and the number a reader actually asks for when they say "how compressed is this" | derived |
| **DC offset** | the mean; a defect detector, free beside the mean square | derived |

What deliberately stays out: DR14 and the other publisher-specific scores (they are a policy over these numbers, not a measurement), and psychoacoustic sharpness/roughness (a different literature, and nothing in the system asks for them). Correlation is already `measure::correlation` and joins the table by reference, not by reimplementation.

### Decisions taken before opening the track

- **A measurement is a derivation of a *source*, not a new element.** The signal element is a presentation × a source × capabilities (E6/E9), and a measured curve is that product with one more factor: a `measure` prop naming a descriptor. That is what makes the feature general for free — the same prop over a bus is a live loudness meter, over a file an offline analysis, inside a clip a body, and under a `field`'s axes a layer; the catalog does not grow a name, which is the `signal` rule the K track made structural. It also settles where the measure's own parameters live: with it, on the element, beside `fft_size` and the rest.
- **A layer is a child of a container, and the container is already there.** A `field` places several elements on its two axes (E7 layered a clip's bodies by doing nothing but placing them), and `opacity` is already a style (K). So the layer system is **not new machinery** — what is missing is what a *stack* has to say and today cannot: the order it is drawn in, which layer the y ruler and the cursor read-out follow, and whether a layer maps through the container's y axis or normalizes into its box. A6 is those three questions and nothing else; if it turns into a new container type, it is wrong.
- **A layer that normalizes is stated, not inferred.** A waveform over a spectrogram share a time axis and share **no** value axis — one is amplitude, the other frequency — which is exactly the case Sonic Visualiser answers by letting one layer own the pane's vertical scale. So a layer declares `y: "axis"` (it measures in the container's y domain) or `y: "box"` (it fills the box it is given, its own domain normalized), and the ruler follows the one layer that says `axis`. Two layers claiming the axis is a script error the host reports rather than resolves.
- **Precision at sample zoom is a resampler, and a resampler needs guard samples.** The user named the artifact: an interpolation filter reading up to the edge of a window sees zeros beyond it and rings. Every reconstruction in this track therefore takes a span *plus* the filter's context on both sides (`guard = (taps-1)/2` at the working rate) and the core API takes the guard **explicitly** rather than hiding it — a caller that cannot supply context (the first samples of a file) gets the documented edge policy, not a silent lie.
- **Placement, once for the whole track** (the G7b rule): every descriptor and the reconstruction filter are `clausters-core`, FFI-exported, because the Python client analyzing a rendered file headlessly is a real consumer today and a server-side analysis command is a plausible one tomorrow. What stays gui-side: the column layer's geometry, the colour and alpha roles, the crossfade weights, the over-0 dBTP marking, and the layer stack's ordering rules.

### The milestones

- ✅ **A1 — Mean square in the pyramid** *(done 2026-08-14; this is G20e's analysis half, that section superseded by A1+A2)*. `clausters_core::peaks` grows a per-bucket **mean square** beside min/max — stored as energy, not as RMS, so higher levels combine exactly (the mean of the means) and the LOD crossfade blends without bias, which is the decision G20e recorded and A1 keeps. CLPK v2 → v3, with v1/v2 still parsing; `cache_size`/`multi_cache_size` follow; the FFI exports and `CORE_ABI_VERSION` bump so `clausters.gui.peaks_cache_file` builds the identical cache the host maps. **Acceptance:** core RMS matches brute force on random chunks; a level-2 bucket equals the direct RMS of its samples; a Python-built v3 cache and a host-built pyramid are byte-identical; v1/v2 caches still load.

  **What shipped, and the three calls it took** (the rationale is in `docs/decisions.md`, "The pyramid stores energy, and a cache that never measured it says so"). The statistic is a **mean square**, as planned, and the acceptance is stronger than the one written above: the test asserts every bucket of **every** level against the direct mean square of its samples, on a buffer whose length is deliberately not a multiple of the bucket — because the exactness is not the mean of the means but a mean **weighted by the samples each bucket holds**, and the counts are *derived* from the length, the bucket size and the index rather than stored, so the cache grows no fourth plane. The one place it bites is the odd sibling, where min/max may read the last entry twice and an energy may not.
  A cache written before v3 has min/max and **no** energy, so the measure is an option that rides with the data (`has_mean_square`, `column_ms` answering `None`): zeros would be a measurement — silence — and A2's layer draws nothing rather than a flat line across material that is not flat. And a **mono cache is now one channel** instead of a second format: both writers emit v3, `cache_size` is `multi_cache_size` at one channel, and the mono reader refuses a multichannel cache rather than narrowing it to its first channel. v1 and v2 still load, asserted against hand-written legacy bytes so the claim is about the *reader*.
  No FFI symbol moved — the builders' signatures are unchanged and only the bytes they emit are longer — so `CORE_ABI_VERSION` **18 → 19 is a format bump**, which is what the version handshake is for; `docs/bindings.md` needed no row. The web parity vectors moved with the format (`clients/web/tests/data-vectors.json`, regenerated) and `clients/python/clausters/_native.py` follows the version.
- ✅ **A2 — The RMS layer, and the `measure` prop** *(done 2026-08-14; G20e's drawing half, generalized)*. The signal element takes `measure` (`peak` default, `rms` here), and an `rms` trace draws the symmetric body inside the min/max envelope with its own colour role and the element's `opacity`, LOD-crossfaded exactly like min/max. Below `base_bucket` the body follows the crossfade weight out rather than switching off (G20e's call, kept). **This is the first layer**, so it is also the proof that a measure needs no container work: the same element with `measure:"rms"` placed over a plain one in a `field` is the overlay, and A6 only makes the stack's rules explicit. Python: `measure=` on the builder plus the descriptor through `_native` for headless use; the web builder ports in the same commit. Example: `bulk.py` grows the layer.

  **What shipped, and the one thing the milestone had assumed wrongly.** `measure` is a factor of the element (`peak` default, `rms`), live on `/gui_set`, read by every presentation that draws a signal against time — the navigable view, a clip's take, the plot and the live scope, since all four are the one renderer and a prop that half of them ignored would be the divergence this element exists to close. The body is `±sqrt(mean square)` about zero in the new `trace_body` colour role, translucent *by role* rather than by the layer's opacity, because a body is read against the envelope it sits in. It keeps the column regime at every zoom and **fades out** of the line one on the crossfade's own weight, which is G20e's call kept. A source that cannot measure draws nothing at all: A1's absent measure gets its consumer here.
  **The assumption**: A2 said "the same element with `measure:"rms"` placed over a plain one in a `field` is the overlay", and the field that layers is the **lane**, not the clip — a clip's bodies are built from its own props and nodes nested under one are ignored, deliberately. So the stack is `field(signal(...), signal(..., measure="rms"))` with no placement on it, which is placement work the host already did and container work nobody had to do; the claim holds, the shape is not the one the sentence implied. Pinned by a layout test asserting the two layers land on one rectangle, so A6 can change the *rules* of a stack without re-deriving what makes one.
  **Column by column is also how the body's containment is true.** The RMS of a span never exceeds the largest magnitude in it, so no column's body leaves its own column's envelope — but a signal riding a DC offset has an envelope offset with it while the body stays centred on zero, so the body reaches *below* the lowest sample of an all-positive signal. That is the measurement being right (RMS is about zero and includes the offset), and the test says so rather than asserting a union that only holds for zero-mean audio.
- ✅ **A3 — Band-limited reconstruction and true peak** *(done 2026-09-15)*. `clausters_core::resample`: one polyphase mechanism (`Interpolator`) over **two** tables, and which table a caller asks for is the whole design — the record is in `docs/decisions.md`, "Two reconstruction filters: the standard's for the number, a designed one for the picture".

  **`BS1770` is transcribed, not designed.** It is ITU-R BS.1770-4 Annex 2's own order-48, 4-phase table, because a number called dBTP has to be the number the recommendation defines — and it is *not* a filter anybody would design today (its branches sum to 1.0016/0.9730 rather than to one apiece, it ripples a fifth of a decibel, its images are down 40 dB), which the Appendix explains and the module's tests assert as measured figures rather than as properties a good interpolator would have. Worth recording how it got here: the table was first written **from memory**, and the check that established it was right was reading the recommendation itself. Three of the first tests failed against it, and every one of the three was the *test* asserting something the standard's filter does not do.

  **`FINE` is designed, for a property the standard's table does not have.** A waveform zoomed past its samples draws the reconstruction between them, and BS.1770's centre falls half a sub-sample off the grid — none of its phases is a passthrough, so the curve misses its own dots by up to 0.09. `FINE` is the fractional-delay form (`sinc(k − d)` under a Kaiser, β = 7), where `d = 0` leaves every term but `x[i]` a sinc at an integer: **phase 0 is the sample, exactly**. Eight phases rather than four for the Appendix's other table — the residual error of a true-peak reading is the *grid*, 0.554 dB of worst-case under-read at 4× against 0.136 at 8×.

  **The guard is a parameter, as the milestone asked.** `oversample_into` takes the span plus `GUARD` samples of context each side and `oversample_edge` states the policy where there is none (silence outside), and the test between them says what the difference at an edge is worth rather than asserting they match.

  **The drawing is a layer, found by use the same day.** The first cut drew the curve *instead of* the straight segments past the polyline threshold, and the user saw a waveform's amplitude jump at that zoom. Measured rather than guessed: the samples' envelope sits under the signal's by a **fixed** amount that is the signal's own (0.21 dB for a 440 Hz sawtooth, 3.01 dB for the fs/4 tone, the same at every samples-per-pixel), so swapping one picture for the other is a jump by construction and no threshold fixes it. The user asked what the field does, and the answer was researched rather than decided: editors draw samples at every zoom (Audacity), oscilloscopes draw the reconstruction as a selectable mode (Keysight), and iZotope RX — the one editor that draws both — plots "the analog waveform in red under digital sample values (blue)". So the reconstruction is the **`signal` measure**, a layer in the same stack as `peak` and `rms` with an ink of its own (`trace_signal`), on by default (`"peak signal"`) as it is in RX; where it draws, the sample layer keeps its dots and drops only the straight segment the curve stands in for. The user also asked whether the line between dots should be cubic: it should not, since a spline is smooth and still not the signal, and where the reconstruction is available it is the line. The record is in `docs/decisions.md`.

  **And the picture changes character once, where the dots appear** *(the second eye pass, same day)*. With the layer in, the user saw the waveform still turn into another drawing before the zoom reached the samples. It had three crossings where there should be one: columns gave way to the line at two samples a pixel, the curve began at four pixels between samples, and the dots only at three radii. Worse, between the first and the last a column read the min/max of the two samples *around* it, so every pixel between them drew the whole step — a band where a line belongs. The regime now crosses where the dots do (`samples_are_drawn`), so the dots, the segments and the reconstruction arrive together, and above that zoom a column under two samples wide measures the **line** between samples (`line_column`) rather than the band, so it is the same thin line on both sides of the old crossing. A style with no dots has no such moment and keeps `LINE_THRESHOLD`; the level body keeps it too, being a reading of an envelope wide enough to have one.

  **And the meter reads true peak, as an option** *(asked by the user after `A3` closed, 2026-09-15)*. The `TruePeak` UGen is `Meter` with the reconstruction in front of it — the same three inputs and ballistics over `TruePeakMeter`, the filter's context kept across blocks, walking the input's samples at either rate — and the `meter` widget grew `peak: "sample" | "true"`, **sample by default**. A true peak is read against -1 dBTP, EBU R128's ceiling, rather than full scale. It takes effect only with `rate: "control"`: the level an audio bus publishes is the block's largest sample, and a widget told to call that a true peak would read its lamp against a ceiling the number never meant, so an audio meter stays a sample meter whatever it is told.

  **Both consumers landed with it.** *The drawing*: the `signal` layer draws the reconstructed curve through the dots where they are separate points, thinned so it is never resolved finer than the screen (`curve_step`), and **marks the peaks between two samples that leave full scale** — in the meter's clip ink, with how far past full scale the loudest went written once beside them. Neither sample left full scale in that picture, which is exactly the thing a sample-peak view structurally cannot show. *The number*: `true_peak`/`TruePeakMeter` in the core, streaming and one-shot as one algorithm, exported as `clausters_core_true_peak` (ABI v65) and bound in both clients — `clausters.ipc.true_peak` / `data.truePeak` — with the parity vectors carrying the standard's worst case so the two clients cannot drift on it. The pass also closed a divergence the doc build caught: `channelStats` was public in Python and internal in the web client.

  **Acceptance, as written and as met**: the classic worst case (a full-scale tone at fs/4, phase-shifted, whose samples read at full scale) measures **+3.09 dBTP** through the whole chain — Rust, the C ABI and the page all agreeing to a millionth — and draws a curve that leaves the ±1 band; the reconstruction is flat to its stated ripple across the passband, checked against sines of known amplitude rather than against another implementation; and a span read with guard and without differs only at its edges, by a number the test states.

- ✅ **A4 — K-weighting and the loudness family** *(done 2026-09-15)*. `clausters_core::loudness`: the two K-weighting biquads (the high shelf and the RLB high-pass), **rate-generic by bilinear redesign** from the 48 kHz reference rather than by resampling, with the redesign tested against the published coefficients at 48 kHz; the channel weights (L/R/C 1.0, surrounds 1.41, LFE excluded); momentary, short-term, gated integrated, and LRA. The API is the track's rule made concrete: a `LoudnessMeter` fed blocks (no allocation in `feed`, so the same type is usable from a live path) and thin one-shot functions over a slice that drive it. FFI + the Python wrappers, since analyzing a rendered file is the immediate use. **Acceptance:** the EBU Tech 3341 compliance cases that do not need a decoder — the −23 LUFS and −33 LUFS tones, the gating cases, the short-term/momentary step responses — pass within the tolerance the document states, and the test names each case.

  **What shipped.** `clausters_core::loudness`: `KWeighting`, `LoudnessMeter` (momentary, short-term, their maxima, gated integrated, LRA) and the one-shot `loudness`, a loop over the meter. Every call the documents leave to the implementation was looked up rather than decided — ITU-R BS.1770-5 (whose Annex 1 is BS.1770-4's word for word), EBU Tech 3341 V4, Tech 3342 V4 and its MATLAB reference, and libebur128 as the field's reference implementation. The record is in `docs/decisions.md`, "A loudness meter keeps every block without growing, and redesigns the standard's filter".

  **The redesign inverts the table instead of carrying constants.** The published 48 kHz coefficients are undone to the analogue section they are the bilinear transform of (a centre, a Q, three numerator gains), and that section is transformed again at the rate asked for. The recovered sections are libebur128's literals to nine digits (1681.974 Hz / Q 0.70718; 38.135 Hz / Q 0.50033), and 48 kHz gives the table back to 1e-12. One deliberate difference: libebur128 keeps the high-pass numerator at `[1, -2, 1]` at every rate, so its passband gain drifts with the rate; here it is redesigned with the rest. Measured, the gain at 997 Hz is 0.6910 dB at 48 kHz and 0.6838–0.6964 from 32 to 192 kHz — the bilinear warp — so a full-scale 997 Hz sine reads −3.01 LUFS within 0.008 LU at every rate.

  **Two conventions the implementations do not share, settled by the documents.** The range samples the short-term loudness **10 times a second**, which Tech 3342 has required since V3 (2016); libebur128 still takes one value a second. And the momentary and short-term maxima are taken **at every sample**, because Tech 3341's case 13 exists to fail a meter that reads its 400 ms window every 100 ms (it misses up to 40 ms of the tone, −0.46 LU). **No allocation in `feed`** is kept by storing the gating blocks the way libebur128's histogram mode does — fixed bins, here 0.01 LU from −70 to +30 LUFS — except that each bin holds the exact sum of its blocks' energies, so every mean is exact; the windows are a ring of 3 s of per-frame energy whose running sums are replaced by fresh ones once per window, so subtraction never drifts. About 0.9 MB a meter at 48 kHz.

  **Acceptance, as met:** Tech 3341 cases 1–6 and 9–14 (cases 7 and 8 are the EBU's recorded programmes) and Tech 3342 cases 1–4 (5 and 6 likewise), each a test named after its case, within the document's tolerance. Tech 3341's true-peak cases 15–19 joined `resample`'s tests on the way; 20–23 need an anti-aliasing filter the document does not give. Bound as `clausters_core_loudness` (ABI v66) and `loudness` in wasm, and in both clients as `ipc.loudness` / `data.loudness` with parity vectors; the streaming meter has no door, since a live meter runs in the host, a Rust caller. Example: `buffers/loudness.py` and its page, an offline render measured against EBU R 128. The drawing and the read-out are `A5`.
- ✅ **A5 — The loudness layer and its read-out** *(done 2026-09-15)*. `measure:"momentary"`/`"short"` draws the loudness curve over the waveform on a **LU axis** (its own domain, so it is the first layer that must declare `y:"box"` or bring its own ruler — the A6 question, met concretely before it is generalized), with the R128 target and the −18/−9 LU guides as optional lines. Beside it the numbers: integrated LUFS, LRA, true peak and PLR, over a span or over the current **selection**, which is the first place the A and D tracks meet. Live acceptance: the same `measure` over a bus draws a moving momentary curve, from the streaming meter, at no extra API.

  **What an edit under it invalidates, decided 2026-08-16 rather than left for A5 to discover.** It arrives here from the Future-directions entry *an edit invalidates the measures drawn over it, and nothing says so*, named 2026-08-14 for exactly this and now absorbed — the D track shipped the edits, so the question stopped being hypothetical. The rule is one sentence and it generalizes past loudness: **a measure's affected span is the edit's span widened by the measure's memory.** `peaks::update_range` is that rule with a memory of zero — a bucket depends on its own samples and nothing else — which is why peak and RMS follow an edit already, exactly and cheaply. Momentary (400 ms) and short-term (3 s) are the same rule with a real memory: an edit at `t` invalidates the curve from `t − window` to the end of the span, and recomputing exactly that is bounded, correct and needs nothing new. So they **recompute**, and neither greys nor goes stale.
  **The aggregates are not layers and must not pretend to be.** Integrated LUFS and LRA are gated over *all* the material, so their memory is the take: any edit invalidates them entirely, and recomputing them per gesture would cost the take per gesture. They are **numbers**, and a stale number says so — the read-out marks them stale and recomputes on demand (or when the session settles), which is the honest half of the third choice the entry offered, applied only where it is true. Drawing a stale figure with no mark is the one outcome ruled out, since that is the failure the entry was written about.

  **What shipped, and where the measuring lives.** The curve and the numbers come off one measurement, `clausters_core::loudness::Profile` — the take's weighted energy in 10 ms blocks with their running sums, so a reading anywhere is two subtractions and the aggregates over any span are a walk over blocks rather than a pass over samples. It is **core** by the placement rule and not gui-side: a Python client analyzing a rendered file wants the same curve, and the profile runs the same `Weighting` stage `LoudnessMeter` does, so a drawn curve and a live meter cannot disagree. The element measures it at its **mutation points** (a measure turned on, the rate, the samples landing, an edit) and never in a frame; a view measuring only amplitude holds no profile at all.

  **The LU axis, met concretely.** The layer brings its own scale (EBU Tech 3341's, named by its top: `loudness_scale` 9 by default, 18 the other), its own ruler on the **right** of the body in LU relative to the target, and the target line itself (`loudness_target`, −23 LUFS). The milestone's phrase "the −18/−9 LU guides" was read as that scale's own bounds — Tech 3341's `+9` scale *is* −18 LU to +9 LU — so what is drawn is the target and the strip it sits in rather than three separate guide lines. `A6` generalizes the axis claim; this is the case it is generalized from.

  **The read-out, and the prediction the milestone got wrong.** The numbers are the integrated loudness, the range, the true peak and the PLR between them, over the **selection** where there is one (marked `SEL`) and the whole take where there is not. What does *not* ship is the staleness this milestone specified: the entry assumed recomputing the aggregates per gesture would cost the take, which is true of a pass over samples and false of a walk over the profile's blocks — an edit re-measures only the blocks it touched (widened by the filters' memory) and the aggregates follow in microseconds. So nothing is ever drawn stale and nothing has to be marked so, which is a better outcome than the rule the D-track question was answered with; the rule itself still holds and is what the incremental update implements.

  **Two things the eye pass found, and both were the picture rather than the measurement** *(2026-09-15)*. The layer's ruler was drawn in a strip beside the body that nothing had reserved, so its numbers fell outside and were clipped: it draws **inside** the body's right edge now, on the text plate every caption over a picture already uses — a strip would have to be reserved by the container for a layer that is optional and live, and turning a measure on would then relayout the view. And the curve did not follow the **vertical zoom**: it mapped its scale onto the body directly, where the picture under it maps through the view's vertical window. It maps through that window too now, so the LU scale magnifies with the samples and the layer's ruler reports the *visible* part of it.

  **And the amplitude axis got headroom, which is the user's ask that came with it**: the vertical window could never open past the domain, so a signal that leaves full scale was drawn flat against the top of the box. `viewport::Axis` takes a headroom factor (`AMP_HEADROOM`, four times the domain — twelve decibels of air), an **amplitude** axis asks for it and a **frequency** axis does not, since there is nothing above Nyquist to open onto. The element declares which it has, so the gesture, the `/gui_set` and the read all clamp the same way.

  **Live, at no extra API**: the same `measure` over a bus draws the meter's own curve. It is fed from the bus's **retained history** rather than from the oscilloscope's display window — the window is re-read every tick and overlaps itself, and a meter fed that would count the same audio several times — so the measure declares the retention it needs. Both clients carry the four props; `docs/gui-protocol.md` states the axis, the scale and the read-out. Example: `views/loudness.py` and its page twin.
- ✅ **A6 — The layer stack becomes explicit** *(done 2026-09-15)*. The three questions, and nothing more: **order** (children order, back to front, stated on the wire and queryable, with a `visible` and a `solo` per layer so a stack is readable while it is being built), **the vertical scale** (`y: "axis" | "box"`, one layer owning the container's y ruler and the cursor read-out, a second claim reported as an error), and **the alpha rule** (a layer's `opacity` is the style that already exists; what A6 adds is that the *stack* has a defined compositing order and that a layer over a texture gets the plate treatment the captions already have where it draws text). **The acceptance is the user's case**: a spectrogram with a waveform drawn over it and an RMS band over that, one time axis, one selection, one playhead, each layer's alpha live via `/gui_set`, and the y ruler following the layer that claims it.

  **What shipped, and the one thing the milestone had to decide on the way.** The stack is a **declared list** (`clausters_gui::host::graphics::signal::layers`): a `Stack` of `Layer`s back to front, each naming what it paints — a measure, or the time-frequency texture — with its own `visible`, `solo`, `alpha` and `y`. It is the element's field where `Measures` was, so the picture is still one body, one axis, one ruler, one selection, one playhead. The prop is `layers` and `measure` is the same field under its older name (the `fft_size`/`window_size` precedent, on this very element), in either of two shapes: the space-separated word list, which is a stack at its defaults and what every existing def already writes, or the array whose entries may be objects. A `/gui_set` carries the array as a JSON string, the way the `bpf`'s points do, and `/gui_query` answers `layers` with the form it was given.

  **The decision the acceptance forced**: a presentation had to become a layer. A spectrogram with a wave over it is one element (A2's rule: a second view on one rectangle is a lid, not a layer), so `Paint` is `Measure(_) | Spectrogram` and the *presentation* only states the **default** stack — a trace's is `peak signal`, a time-frequency view's is `spectrogram`. Everything that used to ask the presentation what it was drawing now asks the stack: `is_texture_view`, what a live tick accumulates, how wide a tap read is, which bulk form is loaded, whether a y zoom centres, the y strip's default unit and the axis' headroom.

  **The axis claim, made concrete.** A layer maps through the body's vertical (`y: "axis"`) or normalizes into its box (`y: "box"`), defaulting to what it measures — amplitudes and the texture on the axis, a loudness reading in its box, which is A5's case arrived at from the rule rather than the other way round. Two drawn layers claiming the axis for **different domains** is refused at both doors: the def fails to build with a message naming both, and a `/gui_set` leaves the picture as it was and says why. Several amplitude measures on one axis is the ordinary picture and no claim at all.

  **Compositing is the order, and it crosses the two meshes.** The body's field is painted once for the whole stack; the layers before the texture go into the window's base mesh, the texture is sampled by the frame's own pass, and the layers after it into the overlay mesh drawn over that pass — so "the wave is over the spectrogram" is one boolean in the walk rather than a second pipeline. A trace layer over a texture writes its figures on the **text plate** every caption over a picture already stands on. The texture takes a layer alpha of its own: the spectrogram pipeline is alpha-blended now and the weight rides the uniform it already uploads, so a translucent texture costs a uniform write and nothing else.

  **And a layered view reads the samples, not a summary.** An STFT is made from samples and a pyramid is not, so a stack naming both asks for the audio (inline, a mapped file, a server buffer) and derives *both* pictures from it, filling two slots — one geometry, one texture — under one key, since the frame holds each kind in a map of its own. A source that is a summary and nothing else (a peaks cache, a streamed overview) has no spectrum to show, and its texture layer draws nothing rather than a picture of something else.

- ✅ **A7 — The layer stack's rules from the clients, and the books** *(narrowed 2026-08-14, on the roadmap pass that read A7 against A2 and D8; done 2026-09-15)*. Python: a layer-stack builder over the `field` (`layers=[…]` composing signal elements, not a new object model); the web builder ports in the same commit — the packages move together. Docs: `docs/gui-protocol.md` for the layer keys and the axis-claim rule; the client book's composition/analysis pages for the descriptors; `docs/decisions.md` for the two calls worth recording (a measure is a factor of the signal element, not a widget; a layer declares its vertical scale instead of the host inferring it). Example: `gui_layers.py`, the stack the A6 acceptance describes, held open until it is closed.

  **What shipped, and where the milestone's own words had gone stale.** The builder is not "over the `field`" and composes no elements: `G34` left `field` as the free-standing ruler, and `A6` put the stack **inside** the one element, so the port is a `layers=` prop on every signal builder — `signal`, `waveform`, `scope`, `plot` and `spectrogram`, which needed it most, since a texture with a wave over it is written there. It takes either wire shape: the space-separated string (`measure`'s older spelling of the same field) or the list whose entries may be dicts, and both clients already serialize a structure to the JSON string a `/gui_set` carries, so re-stating a stack live needed no wire work in either. TypeScript grew `SignalLayer`/`SignalLayers` for the entry, which is the shape the option type shows a reader.

  The books: the Python book's `gui.md` chapter is "The layer stack" now rather than "What a picture measures", with the dict form, the three per-layer words and the axis claim; the web book delegates the props to its generated reference, so the TSDoc carries them there. `docs/gui-protocol.md` states both shapes, the compositing order and the refusal; `docs/decisions.md` records the two calls A6 actually made — a presentation is one layer of a stack, and a layer declares its vertical.

  The example is `views/layers.py` and its page twin, one program in two languages: the same sweep, the same stack, the same four controls re-stating it, and the same last cell asking for two layers on one axis so a reader can see what comes back (the host refuses and the query answers the stack unchanged).

  **What it no longer carries, and where each half went.** The `measure` prop's builders and its wire reference are **A2's** — that milestone already states them ("Python: `measure=` on the builder … the web builder ports in the same commit"), and a duplicate here would have had two milestones claiming one surface. The `Editor` opening an element with a chosen stack is **D8's**, because it is a view of the arrangement before it is a view of a measurement: it is `open_pianoroll`'s sibling, it needs A2 and not A6, and holding it here would have put the door to the whole sample editor behind the layer stack's rules. What is left is the half that genuinely rides with A6 — the composition of layers and the publication of the rules that make a stack readable.

**What stays out of the track**, recorded so no milestone reopens it: a layer *library* of arbitrary annotations (Sonic Visualiser's time-instants and time-values layers are `notes` and `curve` elements placed on the same `field` — if they need anything, it is A6's rules and not a new type); loudness normalization or gain application (a measurement track measures; applying gain is an edit, and edits are the D track); and per-layer blend *modes* beyond alpha, which nothing in the brief asks for and which would put a compositing model into a mesh renderer that has none.

## H track — the gesture is undoable, and the host is the one that saw it

*Section added 2026-08-13, out of a discussion that began at the D track's exclusion of an undo history "because it belongs to whoever owns the data". **Reformulated 2026-08-14, when that premise turned out to be right in a way the track had not followed**: the exclusion was protecting against two histories of the same data, and the track answered it by giving the host a log of its own gestures — which works only while nobody else edits, and which no real editor does. The log belongs with the document, and the document is now a crate (`crates/clausters-document/PLAN.md`, O5). What is left here is the half that is genuinely a window's.*

**The rule, restated: the host emits an edit that can be inverted, and draws what came back.** It does not keep the history. That is not a demotion — it is what makes the property the track was built on actually hold: *the host can invert exactly what it can draw* was true of the picture and not of the document, because the owner transforms an edit as it applies it (a snap, a clamp, a refusal) and the host cannot see that. With the acknowledgement carrying the **effective** value (O2/O3), the log records what the document became rather than what the hand did, and the ambiguity disappears.

**The order, as in the D track, is one-way**: H1 needs O2 and feeds O5; H2 needs O5 and O11, **both now closed** (the log crosses as a handle, and `clausters._native.Log` is what the `Editor` drives); H3 needs O5 and O8, both of which are now closed. Under `standalone` there is no language client, and the answer is no longer an exception to the rule: the host **links the crate** and hosts a document like anyone else. It is a client of the embedded server for its sources, exactly as the Python client is of a live one.

### Decisions taken before opening the track

*(The decisions about the log itself — the gesture as transaction, the label, what does and does not enter it, the prune, the budget, the spill store — moved with it to `crates/clausters-document/PLAN.md`, O5. What follows is what remains a decision about the view.)*

- **Undo and redo are gesture-plan steps, not addresses.** A key or a toolbar drives them through the same table D2's pencil joins, and the log's position is read and written as ordinary state (K5: what a query gives back is what a set takes). No new `/gui_*` verb: the one this whole discussion added is `/gui_ack`, and it is the reply `/gui_event` never had.
- **~~Every intent carries its previous value.~~ Superseded 2026-08-14, on starting H1** — kept with the record of why, because the reasoning was sound and the premise underneath it moved. It read: *that is the host's whole contribution to invertibility, and it is cheap precisely because the host had to hold the previous value in order to draw it.* True while the log was going to be the host's. Once the log moved to the crate (O5), the owner reads the inverse out of the **document** (`current`/`inverse_of`), which is both cheaper and more truthful than anything the hand can report — the owner knows what it stored, the host knows what it drew, and only the first is the model. The host's contribution to invertibility is therefore **emitting an absolute intent**, nothing more; H1 is what is left of this decision. The one inverse no document holds is a destructive write's samples, and its caller reads them (O5, and D1 where it lands).
- **The pending overlay is not the log.** D1's marked-pending drawing is what the host shows between a gesture and its acknowledgement; the log is what can be replayed afterwards. They are adjacent and must never be merged — one is about a round trip in flight, the other about one that closed.
- **A free or a redefine drops what named the freed ids**, exactly as it already drops bindings (G6). **The blast radius is the thing to size before implementing**: `Editor._reset_ids()` re-mints every id on a whole-tree redefine, so a rule written for one freed widget would empty the history on any structural edit. Whatever identity the log keys on has to survive a redefine, which is the same id→object map O5 needs anyway.
- **Logical material is edited by gesture today, and the rule has to face it.** "A generator is code and has no undo" is right (it is O5's first decision), but the host already carries gestures over structure that is *not* a generator: `Editor._apply_patch` rewrites two members' controls onto a shared bus when a cord is drawn, and `_apply_points` writes onto an `Automation`'s `Env`. Those are concrete configuration, not code, so they are undoable and the rule needs to say so in those words rather than by naming "logical".

### The milestones

- ✅ **H1 — The last relative payload becomes absolute.** *(Rewritten 2026-08-14, on starting it. What it said before: "every edit-back payload the host emits gains the previous value", over `"clip"`, `"notes"`, `"points"`, `"wire"` and `"move"`. That half is **redundant and is dropped** — see below. What survives is the other half, which nothing else can do.)*

  **Why the previous value is not the host's to send.** The track's decision above — *every intent carries its previous value* — was written when the log was going to be the host's, and O5 answered it from the other side without this entry noticing: `apply_logged` reads the inverse **out of the document** before the edit lands (`current`, which O4 had already built for the staleness check), and O11 made `inverse_of` public. So for `"clip"` (a `Place`), `"notes"` (a `SetMembers`) and `"points"` (a `Configure`), the owner already holds what was there before and needs nothing from the hand. Sending it anyway would be a second source for a value the owner can read, which is the two-owner problem in miniature: when they disagree, the one that is wrong is the one that travelled. The **one** edit the document cannot invert by itself is named in O5 and is not this milestone's — `WriteSamples`, whose overwritten samples are not in the document, and whose caller reads the span before writing. That belongs with **D1**, where the destructive path is built, and it is recorded there rather than here.

  **What is left, and it is real.** `"transpose" <xml:id> <steps>` (`src/host/elements/score.rs`) is the only **relative** payload the host emits, and relative is the one thing the owner cannot fix after the fact: an absolute intent is idempotent and needs no rebase, which is what lets a pending edit simply stay drawn over whatever authoritative state arrives. A relative one made against a picture that moved has to be rebased, and rebasing is the replay the host has no model to perform. So the release names **the diatonic staff position the note reaches**, measured from the staff it is on — which the host already derives from the engraving in order to place ledger lines (`ledger_ys` measures outward from `staff.y0`/`y1` in `step` units), so this is a reading it performs today for another purpose rather than new geometry.

  **The applier stays relative, and that is not a contradiction.** `Engraver.transpose` takes steps deliberately and its docstring says why — verovio's coordinate-taking `drag` reads a page y in a frame that does not line up with the display list's, so an absolute call there would need an unexplained offset. That is a fact about *verovio's editor API*, not about the wire: the client converts the position it received into the delta verovio wants by reading the note's current position out of the display list **it engraved and sent**, which is the same artifact the host measured. Both sides compute the position from one drawing, so they cannot disagree about it, and the relative call stays an implementation detail of the applier.

  **Acceptance:** the same drag emitted twice in a row leaves the note where it is rather than moving it twice (the idempotence that makes an intent absolute); a transpose applied against a page the client has since re-engraved lands on the position named, not on a displacement from wherever the note now is; the round trip in `score.py` moves the note by ear and by eye exactly as before. Four ends in one commit: the host, `docs/gui-protocol.md`, the Python builder/applier and the TypeScript one.

  **What it took, beyond the payload.** The applier needed an absolute door — `Score::transpose_to(id, position, page)` in `clausters-notation`, which subtracts the note's current position and calls verovio's relative editor underneath — and the reading both sides measure with went to `clausters-core` (`DisplayList::staves`/`staff_position`), since the host and the engraver each hold their own copy of the same drawing. `CORE_ABI_VERSION` 16 → 17 for `clausters_score_transpose_to`. Already-there returns **success** rather than a refusal: the requested state holds, which is what makes a resend harmless, and it is the same outcome shape the document crate uses.

  **The bug the milestone found is under "Found by use" below** — the staff-line rule measured against the page instead of against the page's own strokes, so a short system on a wide sheet had no staff at all and the position was unreadable. It was latent in the host before this and is now shared code with a shared vector.
- ✅ **H2 — Undo and redo from the hand.** *(Closed 2026-08-14. The host half is restated below, because reading the gesture-plan table found the milestone's own premise wrong about where it goes.)* The log's position as state the root reports, the redraw that follows an inverse being applied like any other edit — because it is one — and a route from the hand to it. Python: the `Editor` routes the intent onto the objects it indexes by widget id, and **an editable widget is not closed until its id→object route is covered**, which is the requirement this milestone adds to every later one.

  **Done: the client drives the crate's log, and the editor stopped deciding.** `Editor.undo`/`redo`/`can_undo`/`can_redo`/`undo_label` over `clausters._native.Log` and a `Document` held in step with the arrangement. Three things fell out of it that are worth more than the buttons.

  **The editor is to the document what the host is to the editor.** It emits an intent and adopts the value that comes back; `_project` writes that value onto the object the id names, and *decides nothing*. So the **grid moved into the crate** — `Editor` no longer snaps a placement, it states where the hand put it and `Rules { quant }` snaps — which is the "consistency by duplication is what this replaces" decision finally paid off rather than restated. Staleness went the same way: the crate answers it, and the Python check is gone.

  **An inverse needs no second path.** `undo` projects the intents the log hands back, exactly as an ordinary edit projects its effective value, and answers the host with the corrected widgets — a **push with a stamp of zero**, which retires no pending edit because an undo answers no gesture. That is why the window is not redefined: a redefine re-mints every id and would drop whatever gesture was in flight.

  **The id→object route is the milestone's real content**, and it is `_index`, built from the same walk `clausters.form.document` makes — which is what keeps the two agreeing about what has an id. `_widget_of` reads it backwards, which is how an undo knows what to redraw.

  **Found by doing it: routing edits through the document makes the document's coverage a hard requirement.** A bare `Element` had no body in the conversion and raised, so one unfamiliar leaf made the whole composition uneditable. It is now an opaque leaf, which is the format's own rule (*what a writer does not understand, it preserves*) read from the writing side.

  **The host half, and the correction it needed.** The milestone said "the two steps in the gesture-plan table", and that is wrong: a plan step consumes a **press at a location** (`Element`, `Pan`, `Select`, `Locate`), and undo is aimed at no place under the cursor. It is a **window shortcut**, which the host already had a precedent for — `R` resets every navigable view in a window, dispatched by each front and answered by `Gestures`. So `Gestures::history` joins it: Ctrl+Z, Ctrl+Shift+Z and Ctrl+Y on both fronts, emitting `/gui_event <window_id> <seq> <version> "undo"|"redo"`.

  **It names the window, the way `/gui_closed` already does.** A window's root widget carries no id, and inventing one for this would have been a widget where there is none; the def id *is* the window's name on the wire, and the script already knows it. Stamped like any other edit-back, so an acknowledgement can name it and the two need no second rule between them.

  **The log's position as a root prop is deliberately not built.** It was in the milestone as the other half, and it has no consumer: a script greys its own undo button out of `Editor.can_undo`, and a standalone host (H3) owns the log directly. Shipping a prop with no caller is a declaration rather than a feature — the same call O6 and O10 already made — so it lands with whoever needs it.

  **Acceptance, met:** a run of gestures undoes back to the starting placement exactly; a redo lands on the snapped value rather than snapping twice; an undo answers the host with a value and no redefine; Ctrl+Z over the window reaches the same history as the button, and another window's shortcut is not this editor's to answer.
- ✅ **H3 — The standalone editor is its own owner.** *(Closed 2026-08-16, on its acceptance and on the split recorded at the end of the entry.)* Under `standalone` the host links `clausters-document`, holds a document, and applies its own inverses through the embedded server's buffer commands, with `peaks::update_range` redrawing only the touched span. This is what makes the audio-editor case real — a host with predefined operations, a session and an undo stack, no language anywhere — and it is one milestone rather than a track because the crate already built all of it but the wiring. **Acceptance:** a session written by the Python client opens in a standalone host, is edited by hand, undone, redone and saved, and reopens in Python unchanged.

  **What has landed** *(2026-08-16)*, and it is the wiring rather than the window. `host::document::Owner` holds the document, the crate's log and the session, and **owns nothing new**: the vocabulary, the inverses and the format are all the crate's, which is the whole reason a session written by one writer opens in another. What a host adds is two things only it can know — which node a widget's gesture addresses, recorded by whatever built the tree, and the translation from a flat `/gui_event` payload to an `Intent` (`"clip"` → `Place`; `"sample"` and `"draw"` → `WriteSamples`, one intent at two lengths). A payload it cannot read, or a widget bound to no node, is declined rather than guessed at.

  **`host::document::tree` is the `Editor`'s port**: a document drawn as lanes of clips over one shared axis with a beat ruler, every clip bound to the node it draws, and ids allocated as it goes. It builds an **ordinary GuiDef** — no new widget, no new prop — so a standalone editor is this host driven by itself, and anything it cannot draw is missing for a script too.

  **`Host::answer_own` is the seam**, and it is what keeps the owner from being a type nobody calls: a host that owns what it draws applies its own gesture and acknowledges itself through the same `settle` a `/gui_ack` goes through, so a standalone editor cannot drift from a scripted one. A host without a document, or a payload that is not an edit, declines and the event goes out on the wire exactly as it always has.

  **The door is `--session <file> [--save-to <file>]`**, and with it the mode runs: a session written by the Python client opens here and draws — checked against a real file the client wrote, not a fixture. **Undo, redo and save are the window's verbs**, addressed to it rather than to whatever is under the cursor, which is the shape undo already had and the reason a save takes it too: what is saved is the document behind the window. A host that owns one answers them; every other one emits them and a script may. **Saving is Ctrl+S rather than an exit's side effect**, and writes only where `--save-to` named a file — overwriting what you opened is a decision.

  **Two defects the first run found, and neither was in the code the tests covered.** A GuiDef's id **is** its root widget's, so a tree numbered from 1 handed to `/gui_def 1` collided with itself and the registry dropped the whole subtree — an empty window and one line in the log. And a lane, a clip and a time ruler are all **`field`** on the wire, told apart by their props; the tree said `"track"`, `"clip"`, `"timeruler"` after the Python builders' *names*, which build nothing. Both are now pinned by a test that feeds a drawn tree to a real `Host` and asserts every clip is in the registry **as a clip** — presence alone would have passed the first bug, since a collided id still resolves, to the window.

  **Three defects the by-hand run found, and only the first was in the drawing.** The tree walked **two levels** and a composition is nested as deeply as its author nested it — a piece of groups of tracks of events is three sets deep before a note — so it drew the containers and called it a picture: two lanes with one empty clip each, where four notes were. Then the two that made the keys look dead. Nothing **adopted** what an edit left: a drag looks right because the gesture already moved the clip, but an undo moved the document and not the widget, so the picture kept the position the hand left and `z` appeared to do nothing at all. And the payload is in **timeline units** while a placement is in **beats**, and nothing converted: a clip dropped two beats along was written to the document at beat *ninety-six thousand*, which no test asked about because none of them had a scale. All three are pinned, the last two by one test that draws the tree the session mode draws, applies a drag's own payload and undoes it, asserting the **widget** comes back.

  **Verified by hand** once they were fixed: a session written by the Python client opens, a clip drags, `z` and `a` move the picture, `s` writes the file — and the saved document holds the edit in beats (2.204) with the version moved from 1 to 159. That is the milestone's acceptance, run.

  **A track is one clip with a roll in it**, which is the picture a clip was missing and the correction the first look demanded: drawing each note as its own clip gave a row of empty rectangles where the music was. A set becomes a roll only when **every** member carries a pitch — half a roll would leave material out without saying so — and the clip draws the *set*, so a drag on it moves the track while a note inside edits through the roll's own payload. The window also says where a save goes, in its title (`in → out`, or *read-only* where no `--save-to` was given), because nothing else on screen could and pressing a key to watch nothing happen is not an answer.

  **The acceptance ran, by hand, end to end** *(2026-08-16)*: `session.py` wrote a session, the host opened and drew it, two clips were dragged, `Ctrl+Z` and `Ctrl+Shift+Z` moved the picture, `Ctrl+S` wrote the file, and the example read it back — printing the tree with the tracks at 2.055 and 0.102 beats, which is where the hand put them. Three writers of one format, and the file means the same thing to each.

  **What is left is a milestone and not a remainder, which is why this one closes.** A clip's body is **notes only**: a take draws nothing, because that needs the session's sources resolved to files and buffers, which belongs with the session and not with the tree. And the material is not *played* — this mode boots no server, which is what still separates it from `--standalone`, and what this entry's own first paragraph means by applying inverses *through the embedded server's buffer commands*. Both of those are one piece of work with one acceptance, they need a leg the wiring never had (a server), and neither is reachable by finishing anything above. So they leave as **H4** rather than sitting under a tick as caveats or holding an entry open whose acceptance ran: what a document **says** round-trips through three writers today, and what it **sounds like** is the next milestone.
- ✅ **H4 — The standalone editor sounds what it edits.** H3's other half, split off when H3's acceptance ran without it. The host boots the **embedded server** (which is what `standalone` links it for), resolves the session's `Source` table to buffers and files, draws a take's clip body as the waveform it already knows how to draw, and applies a destructive edit through the server's own buffer commands — `peaks::update_range` redrawing only the touched span, which is the whole reason S12's editing verbs are frame-addressed. Nothing here is a new capability: the verbs shipped (S12), the interactive NRT session shipped (S13), the pending overlay and the cache update shipped (D1/D2), and `WriteSamples` carries its own inverse because the caller reads the span before writing (H1's one exception, built in D1). What is missing is the **leg**: a session-mode host has no server, so a source resolves to nothing and an edit has nowhere to land.

  **The one decision to take before writing it** is which server a session-mode host boots — the embedded RT one it already links, or an offline `NrtSession`, which is what S13 built the interactive mode *for* and which needs no device. Editing a file is by definition NRT (the argument S12 rests on), so an editor that opens a session on a machine with no audio device should still edit it; playing it back needs the RT one. That points at both, chosen by what is asked of it, and it is the milestone's first question rather than a settled call here.

  **Acceptance:** a session whose track holds a take opens in a standalone host and the clip shows its waveform; a span of it is edited by hand, the picture updates over that span alone, `Ctrl+Z` restores the samples; the material plays; and the file reopens in Python with the edit in it.

  **The first decision, settled by building it: the embedded RT server.** Buffer commands already run on the server's own NRT thread, so an "offline session" would have bought nothing this needed — reading a file into a buffer and writing a span of it are asynchronous jobs either way — while the host's whole client leg (the fetch machine, bound widgets, the voices) is built against `ServerLink`. So a session-mode host boots `EmbedServer`, and **a failed boot is not a failed session**: the document still opens, edits, undoes and saves, with the takes drawn as what they are. That also keeps the machine with no audio device working, which was the argument for the offline one.

  **What shipped** *(2026-08-16)*. The source table resolves to buffers (`host::document::sources`) and the clip draws the buffer it names — one material, not a picture of one. A take also **opens as an editor** under the tracks, on an axis of its own: a lane measures beats and a pencil measures samples, and one axis cannot be both, so the arrangement is where material is placed and the editor is where it is drawn over. Both are bound to the same node, and a write reaches every view of that buffer — which is the milestone's one real finding and is in `docs/decisions.md`: the two views hold the material in **different forms** (the navigable one a pyramid, the clip's body the samples), so the element writes its own (`Element::write_samples`) and the host decides which elements by **buffer number**. The undo restores the samples because the payload carries them (D1's exception, which is why `apply_with_inverse` exists), and a replayed write goes back through the same door. Sounding it is `host::play`: the smallest def that could read a buffer (`BufRateScale → PlayBuf → Out`), loaded with the material, one node, the space bar over whatever the cursor is on.

  **What it refuses says why.** A stroke that leaves the view is clamped to the body it started on and to the material's last frame, the same rule the pencil is already refused under (`docs/decisions.md`, "A stroke writes what the reader can see") — found by use, in the window, by dragging out of it. A **multichannel** take was refused too, and that refusal named the command it needed: the server's **S16** landed beside this (`/buffer_setRangeChannel`), so a stereo take now edits, plays and draws channel by channel, and `Intent::WriteSamples` carries the channel that makes an undo put back the one it took.

  **What it does not have, stated so it is not read as done:** there is **no playhead** while a take plays. The host draws one from the engine's sample clock through the shared segment, and a session-mode host maps none — so the sound moves and the picture does not. It is the next thing this mode wants and it is a milestone of its own, not a caveat under this tick.

- ✅ **H5 — The session's monitor follows the transport, and the head is drawn from it** *(done 2026-08-16; named by H4, which plays a take and cannot show where it is; opened 2026-08-16, and **rewritten before any code** when the design turned out to be mostly the server's.)* Depends on the server's **T5**, which is where the piece's position lives; the two are complementary and are read together — T5 states what playback time *is*, this states what a window does with it.

  **What was cut out of this milestone, and why it matters here.** The first draft of H5 had the host computing the monitor's duration from the take's sample rate, driving a self-freeing timer, and reconstructing a position by adding an offset to a clock. All of that is a **client managing playback time**, which is the server's alone — and it was not a lapse of taste but the shape of a real gap showing through the client: the transport's position never reaches the engine, so anything built on top of it has to be assembled by whoever wants it. That is T5's subject, and once it exists this milestone is small. The rule it now follows: **the host reads where the piece is; it never computes it.**

  **What is left, and it is genuinely a window's.**

  - **The monitor becomes a follower.** `host::play` loads the smallest def that could read a buffer (`BufRateScale → PlayBuf → Out`, from frame 0, one reader per channel). It becomes the smallest def that could *follow* — `BufRd` whose phase is the transport's position, offset to where the take sits — so seek, loop and stop are things the transport does and not things the def has. Still one reader per channel, which is the server's own convention and what H4's fix established.
  - **The monitor's nodes move into a group of their own.** They go to the head of the **root group** today, and `/transport_group` has to bind something that is not the root — binding the root would freeze the whole tree on a stop, which in a session is every sound there is.
  - **The embedded server's segment gets a door.** `Clausters` holds an `Arc<Segment>` and exposes none of it (`src/embed.rs`), `host::shm::SharedSegment` only knows how to `mmap` a path and `munmap` it on drop, and `host::gui::run` only takes a `shm_path`. Three small openings: an accessor on the server, a **non-owning** view over a segment whose lifetime the caller holds, and a `run` that takes an `Arc<dyn BusSource>` already built. The accessor is server code and travels in the same commit, per the rule that the packages move together.
  - **Which clock a window reads is a property of its source, not of a widget.** Several counters are published and a widget reads one number, so the choice is made once where the `BusSource` is built: a **session-mode** host draws the head from the piece's position, a host attached to a live server keeps the **device** clock, since its meters, scopes and taps are on that axis and always were. No widget grows a clock prop.
  - **The gestures**: play from the cursor, play the selection, loop it, stop, and the ruler locates. Each is a `/transport_*` call and a redraw — which is the test of whether T5 landed correctly, since anything here that needs arithmetic instead of a call is a sign the position stayed on the wrong side.

  **The sweep needs no new drawing.** `Timeline::swept_at(sample_clock) = sample_clock - playhead_at` is the one definition of the head and every view already shares it. Pointing a session window's source at the piece's position is the whole of the picture half — including the pause, since a transport that stops stops the position too and the line holds itself.

  **The position is in samples**, which is what an editor addresses: cursor, loop region and head are frame counts of the material. Showing them as samples, seconds or beats is the temporal ruler's job and it already does it, so nothing here chooses a unit for the reader.

  **What shipped** *(2026-08-16)*. The monitor is a follower: `TransportPos → BufRd → Out`, one reader per channel, in a group of its own bound with `/transport_group`. The embedded server hands the host its segment (`Clausters::segment`, `Segment::base`/`size`, and a **borrowed** `SharedSegment` that holds the owner's handle so the memory cannot go out from under it), `gui::run` takes an `Arc<dyn BusSource>` already built, and `HeadClock` settles which counter a head reads where the source is made. The sweep anchor is 0 and the rest falls out: stopped holds, a locate jumps, a loop wraps in the engine.

  **What using it corrected, and it is the milestone's real content.** The code was right and the *gestures* were written against the wrong question, four times over, and a hand found each one in a minute:

  - **Nothing moved the head.** This entry said "the ruler locates" and the take editor's plan had no `Locate` step, so the only sound was the first press of the space bar, from frame 0, with no way back. The answer needed no new gesture: a **sweep already carries a click** (a press sets a zero-length selection), so its start *is* the cursor and its span *is* the loop. One door, and clicking, selecting and looping become the same gesture read at different lengths.
  - **The locate rode every frame of the drag.** Correct-looking and audibly wrong: a rolling transport hears a head chasing the pointer as a retrigger per frame. The fix is *not* to drop the live half — the user's own correction — because it is the **locate** that jumps and the loop that does not (`/transport_loop` never moves the piece). So the span follows the hand live and the head is placed once, **on release**, which is the convention for a click anyway and which also fixes a sweep running leftwards: the head lands on the selection's start rather than on wherever the hand began.
  - **The guard asked the wrong question.** Gating the transport on "a monitor is loaded" meant the cursor did not work until you had already played once — a cursor that fails exactly when you first reach for it. What it should ask is whether **this host bound the governed group** (`Host::owns_transport`), true from the moment a session opens. Behind it sat a second one it was hiding: the space bar read only selections *with a span*, so a click's zero-length selection sent every play back to frame 0.
  - **A click that trembled left a two-sample selection** — harmless for a copy, and a two-sample loop once the transport follows it. A sweep that never leaves `hit_slop` is a click, the same tolerance every other gesture already allows the hand.

  **The one thing left as a decision rather than a defect**: past the end of the material the transport keeps rolling and the reader goes quiet, which is what a DAW does. With the head placeable by a click that is a resting state and not a dead end — and a transport that stopped itself at the end would be the host timing playback, which is the one thing this milestone exists to stop doing.

  **Acceptance:** a take plays from the cursor and the line moves over the picture on the sample it sounds; stopping freezes sound and line together and playing again continues from there rather than restarting; a selection plays as a region and, with loop on, repeats seamlessly with no seam a listener can hear; the head stands still while the transport is stopped, with no pause logic in the host; and `clients/python/examples/editors/session.py` shows all of it by ear and by eye over a **stereo** take, since the readers are per channel — that example being the manual test, as ever. `docs/architecture.md` gains the session's clock choice; anything that reaches the wire lands in `docs/gui-protocol.md` and in both clients' `gui` builders in the same commit.

- ✅ **H6 — The standalone editor maps what it edits, and the RT server is a separate process** *(done 2026-08-17; opened the same day by the user, stating the target the H track has been walking toward: the GUI client as an autonomous application, with NRT processing on demand and RT as separate processes)*. Depends on the server's **S19** (the material in the shared segment) and, through it, on **S18**; this is that milestone's other end, and the two are read together the way H5 and T5 were.

  **What the host does today and where the copies are.** In `standalone` the host boots the **embedded** server in its own process (H4), resolves a session's sources to buffers, and gets the samples one of two ways: a mapped *file*, or `/buffer_get` replies polled off the ring. An edit goes back as a `/buffer_setRange` blob and the host then reconciles its own picture with what it just sent — which is why *"a take's clip and its editor disagreed after an edit"* was a defect this plan already records: two widgets holding one material in two forms, and a third copy on the server.

  **What this milestone makes true.** The take is **one** copy, in the segment. The host maps it, builds its peak pyramid over the mapped region (no fetch), and a stroke stores into the cells — no message, no blob, no reply, no reconciliation, and `peaks::update_range` over the span the hand touched, which is what D1 built it for. The engine reads those very cells, so the edit is audible on the next block whether the engine is **in this process** (embed) or in another one attached to the same `--shm` path. That last property is the point of the milestone: the RT server stops being something the editor contains and becomes something it *shares memory with*, so it can crash, be restarted, or be replaced by an NRT run without the material moving.

  **It starts by giving the embed a door to a *session***, which is the gap the user found on 2026-08-17 and which the plans had been assuming for a while: S12 records that the performer of an edit is the on-demand NRT server, and this host boots a full RT server with an audio device instead — because that is the only embed there was (root `PLAN.md`, "The editor's server is supposed to be the on-demand one"). `SessionConfig::shm` exists since S19; what is missing is `Clausters` opening one.

  **The three processes and what each is for**, since "autonomous application" is the requirement and not an implementation: the **editor** (this host, owner of the document and of the material), the **RT server** (playback and monitoring, attached to the segment, killable), and **NRT on demand** (S13's interactive mode, invoked when the editor needs material rendered — a bounce, a `/buffer_render`, a generator made concrete — writing its result into the same region rather than shipping it back). What crosses between them stays OSC and stays small: allocation, lifetime, render requests, transport.

  **Who creates the segment, and who attaches — the question S19 turned from a detail into the deciding one** *(named 2026-08-17 by the user, asking how the processes ended up arranged)*. The material now lives in regions the **segment** indexes, so whoever owns the segment owns the material, and the three processes cannot all create one. Today only one shape exists: `clausters --shm <path>` **creates and truncates** it (`Segment::create_full`), so the RT server is always the owner — which is backwards for an editor, since the process that may be killed and restarted would be the one holding what the others are editing.

  **"Killable" is a resilience property and not an operating mode, which this entry had blurred** *(corrected 2026-08-17 by the user: "an audio editor has to keep its input/output devices to record and play, otherwise it cannot be routed with JACK")*. The RT server is the process that **holds the devices** — `cpal`'s default output *and input* — so it is not "the player" but the only one that can **record** at all, and under JACK or PipeWire its ports and the connections a person made to them are state that lives with the process. Attaching to a segment restores the *material*; it restores no port, no name and no patch. So a restart is **recovery**, not a routine: the design must let the editor survive an RT server dying, and must not be built as though restarting one were free.

  Two things follow and neither is decided here. A recovery is only cheap if the ports come back **with the same names**, so a patchbay or a session manager can reconnect — today the device is `cpal`'s default and nothing names anything, which is fine for one process and not for one that is expected to return. And **capture** belongs to whoever has the input device, so a session that renders and edits cannot record: the split is not "the RT server plays and the on-demand one works", it is *the RT server owns the devices* and the on-demand one owns nothing but computation.

  So H6 has to settle it, and the answer the rest of this milestone implies is: the **editor** creates the segment (or the session it opens does), and **every server attaches** — long-lived, not disposable. That needs a door the server does not have — `--shm` that *opens* an existing segment instead of creating one, refusing a mismatch the way a peer already does — and it is what makes the acceptance below (kill the RT server, start another) mean anything at all: an RT server that truncates the segment on the way in would take the material with it.

  **What this does not change, deliberately.** The direct write is available exactly when the host **is** the owner (H3's case). With a Python client owning the document, the edit still travels as an intent and comes back acknowledged — O3's machinery, the pending overlay and the refusal are untouched, because the question they answer is *may I* and not *how do the samples get there*.

  **Acceptance:** the standalone host opens a five-minute session with no samples crossing the ring; a stroke is drawn, heard and undone with no `/buffer_*` message sent for the samples themselves; and killing the RT server and starting another against the same segment leaves the editor drawing and editing the same material, which is the test that says the processes are genuinely separate.

  **What shipped, and what it is honest to say about each half.**

  `host::material` opens what the segment's directory names: a take's region, mapped read/write. Drawing one is a **local read** instead of a `/buffer_query` and a chunked `/buffer_getRange` conversation, and a stroke is a **store** instead of a blob out, a job on the server, a reply and a reconciliation. Both halves were run by hand — the session example opens and draws its take through the mapping, and `crates/clausters-gui/tests/material.rs` pins the write against the server's own reader.

  **The copy this did not remove, said plainly**: the widget still holds its own array and its pyramid is built over that, so opening a take is one read of it rather than zero. What went away is the *round trip* and the reconciliation, which is where the per-gesture cost was; making the pyramid read the mapped region directly is a `WaveformData` change and is not this milestone's.

  **The arrangement.** `--session` opens an **on-demand session in this process** (`embed::ClaustersSession`, the door root `PLAN.md`'s "the editor's server is supposed to be the on-demand one" asked for) which owns the segment and the material, and a **player** — another process holding the devices — plays it. The host learned the difference between its two legs: allocation, the editing verbs and the renders go to the server that holds the material (`Host::server`), while playing, sounding a key, the transport and the bus taps go to the one that makes sound (`Host::player`). With no player attached the two are one server and every existing deployment sends what it always sent.

  **The editor starts its own player**, because the order is forced: the session creates the segment and claims the command plane, so a player started first would be the owner and the session would refuse to open. `--server` names one to use instead, and one the user started is never killed. Takes reach the player as `/buffer_attach` and never as samples.

  **And the mirror went away rather than being fixed twice.** The defect below was the second failure of one design — three readers restating one `#[repr(C)]` layout, tied together by a version number that cannot see a disagreement about what the layout *is*. So on the same day the layout moved to `clausters_core::shm` (`docs/decisions.md`): `host/shm.rs` is half its size and holds only what is the host's — getting the memory, and choosing which counter a playhead draws — and the region file's name comes from the core too, since three processes name that file.

  **What it left standing**, and neither is hidden: a session that created its segment removes it on the way out, but a session **killed** (rather than closed) leaves the segment and its regions behind — 230 MB for a ten-minute stereo take — because `Drop` does not run on a signal. And the first defect the mapping turned up was the host mirror's own: it had been moved to ABI v9 by its number while its size check still demanded a segment with no buffer directory, so it refused every real v9 segment. That is what the mirror test now exists to catch.

- ✅ **H7 — The picture reads the material, and a write costs its span** *(opened 2026-08-18 by the user, on the entry H6 left behind: "it is important, it has to be done". H6's own record says what it did not do — "the widget still holds its own array and its pyramid is built over that" — and this is that half.)* Depends on nothing that is not already shipped: S18, S19 and H6 are what make it a change to `WaveformData` rather than a new data path.

  **What is true today, measured against the code rather than remembered.** `place_mapped_buffer` maps the take and then hands the picture `take.read_all().into()` (`src/host/gui/serverleg.rs:61`), a `Vec<f32>` over every cell (`src/host/material.rs:182`); `WaveformData` keeps that as `Arc<[f32]>` per channel and builds the pyramid over it (`src/waveform.rs:34`). So the mapping is used as a *transport* and the widget owns a second copy of the material — 230 MB of it for a ten-minute stereo take. The cost the entry named is the opening read, and reading the write path turns up a second one it did not: a stroke copies the material **again**, twice. `SignalElement::write_samples` clones the body and calls `WaveformData::write_range`, which does `channel.samples.to_vec()` before patching the span (`src/waveform.rs:161`), and then the same function copies the element's own interleaved `samples` whole to write one strided run (`src/host/elements/signal/bulk.rs:213`). Both are O(material) per gesture, on the frame's thread, over samples that are **already written** — the store into the cells happened before any of this (`Host::write_material`, `src/host/mod.rs:1690`). That is the reverse of what S18 established on the server: a write costs the samples it names.

  **What this milestone makes true.** A channel's raw samples are a **source**, not a buffer: an owned `Arc<[f32]>` when the material arrived as bytes (a page, an inline blob, a fetched buffer) and the **mapped region itself** when the host has it, with the pyramid built by streaming through a scratch window rather than by materializing the take. Opening a mapped take then allocates the pyramid and nothing else, and a stroke re-summarizes its span and nothing else — the samples having gone straight to the cells, there is nothing left to copy. The three fine-regime readers (`column`, `column_ms`, `samples_at`) and the clipboard's `block` go through the source, which is the whole of the surface that has to change.

  **Where it goes, and this is the decision the milestone turns on** *(the placement rule: would anything outside a window ever call it)*. **The core**, and specifically `clausters_core::peaks`, gains the ability to summarize a **reader** instead of a slice — `Pyramid::build` and `update_range` over a source that answers "how many frames" and "read this span into this scratch", with a blanket implementation for `&[f32]` so every existing caller is unchanged. That is where the pyramid already lives, and there are three callers rather than one, which is what settles it against keeping the abstraction gui-side. **The host** implements the source over the mapping (atomic loads, because the engine may be writing the very cells being drawn) — and the atomics stay on this side, so the core keeps holding no platform and no concurrency. **The Python client is the second caller and already shows the same defect from the other end**: `peaks_cache_update_file(path, samples, start, frames)` (`clients/python/clausters/gui/guidef.py:1744`) takes *"the whole interleaved buffer as it now stands"* in order to rewrite the part of a cache one span touched, so a client that edits a take hands the core the take to update a kilobyte of summary. **And the renderer is the third** *(named 2026-08-18 by the user, asking what a render returns and whether it shares anything with this)*: `RenderStats` carries `samples` interleaved — the same currency as a buffer, a mapped region and a `/buffer_getRange` blob — beside per-channel peak and RMS that the core measured **as the render streamed**, rather than in a second pass over the result (`clients/python/clausters/render.py:60`, `ipc.channel_stats`). A peak pyramid is that same statistic at higher resolution, and today it is the one summary that cannot be taken while the samples go past: it demands them all at once, in memory. So the reader is not a host convenience — it is what would let a render write its overview as it writes its audio, and what lets a bounce into a session buffer (S19's regions) be drawable without a pass that reads back what was just written. **What is *not* in this milestone**: making the renderer or the Python cache actually use it. This adds the door in the core and walks the host through it; the other two are named so the shape is right for them, and each is its own work.

  **The FFI export is deferred, with its reason.** A trait does not cross a C ABI; what a non-Rust caller would need is a *concrete* source (a soundfile, a region path) exported as its own entry point, which is a different decision from this one and has no caller yet — the same deferral already recorded for the tap rings. So `CORE_ABI_VERSION` does not move here.

  **The freshness half, which is the other thing the entry named and is the reason it is not only about cost.** A picture reading the mapping is live *by construction* for a write that goes through it, and still blind to one that does not — but the wire for that already exists and nobody consumes it. A peer editing shared material announces the span with `/buffer_touch`, and the server broadcasts `/buffer_touched bufnum channel start frames` to every `/server_notify` client but the writer (`docs/schemas.md`). The host **sends** that message (`Host::announce_write`, `src/host/mod.rs:2140`) and does not listen for it, so two editors of one take still disagree. H7 makes the host a consumer: a `/buffer_touched` over material it draws re-summarizes that span — one read of the span for a mapped source, a `/buffer_getRange` of the span for a page, which is the same split every bulk path has and the reason the browser keeps its owned copy unchanged.

  **What stays out, deliberately.** The **engine's own** in-place writes announce nothing — a `RecordBuf` fills cells block by block and no message is due for it — so a view of material being recorded is not what this makes live, and watching a live signal remains the scope's and the tap's job. A recording view is its own design and this milestone must not smuggle one in by polling. Nor does H7 touch the mapping's *read-only* rule where it belongs: `host::bulk` maps a **user's file** and stays a read path; the writable mapping is the segment's region, which is the server's own memory, and H6 is where that was settled.

  **The LOD crossfade is not touched, which the entry this milestone came from had listed as a risk.** It blends the two pyramid levels adjacent to the zoom by `log2(spp / bucket)` — mip-mapping's trilinear filter, applied to a min/max envelope and to the mean square with the same weight — and that is the right algorithm and stays exactly as it is: what H7 changes is where the *fine* regime's samples are read from, not how any two summaries are mixed. What reading it did turn up is a seam one level below, and it is recorded as its own entry rather than folded in here.

  **Two invariants the change must not lose**, both of them things the current copies were incidentally providing. The pyramid is shared with whatever GPU slot is drawing it, so it is still **replaced rather than patched in place** (`src/host/elements/signal/bulk.rs:184`) — cheap, since a pyramid is the summary and not the material. And a redraw that crosses a write sees some old samples and some new, never half of one: that is the buffer model's promise since S14 and it is exactly what a picture can live with, so the reader takes it rather than locking against it. *(The first of the two was amended 2026-08-19, for the refresh path only: a view that is following a recording gives the material back before the write, so the element is the sole owner and mutates in place — a slot that is still holding it is still copied first, which is this invariant unchanged. `docs/decisions.md`, "The buffer is the material".)*

  **Done 2026-08-18. What shipped, and what it is honest to say about each half.**

  `peaks::Source` is the core's door: a length and a span copied into a caller-sized window, with a blanket implementation for a slice so `build`/`update_range` are wrappers and no existing caller moved. `build_from` fills level 0 through one scratch window, so a summary over a mapped region allocates the summary and a kilobyte. `Interleaved` is the strided adapter, and it replaced `update_strided` — `MultiPyramid::build_interleaved` stops copying each channel out before summarizing it, which is the Python cache builder's path too.

  Host-side, a channel's samples are `Samples::Owned` or `Samples::Shared`, and the mapped take is one `MappedChannel` per channel over one mapping. `place_mapped_buffer` builds **one** `WaveformData` for every want of that buffer and hands it over as `Loaded::Shared` — the clip's take body and the navigable view share it, which is the same `Arc` the geometry slot draws from. The two whole-copies a stroke used to make are gone: a shared channel's write re-summarizes its span (`resummarize`) and the element's interleaved array is never filled for mapped material.

  **The freshness half is native only, and that is a real half rather than a rounding.** The host registers `/server_notify` whenever a window draws a server buffer, and `/buffer_touched` re-summarizes the announced span in every view of that buffer, in any window. A **page cannot follow it**: it holds its own samples, so there is no summary to re-read, and following would mean fetching the span back — which the fetch machine does not do. It says so in the console rather than redrawing the same stale picture, and the gap is recorded below.

  **The one copy still made per stroke, said plainly:** the pyramid is cloned before it is patched, because it is shared with whatever slot is drawing it (the invariant this milestone kept). That is the summary and not the material — about 0.8% of it at the default bucket — so the cost stopped following the take and started following its overview.

  **The analysis path is the one exception and it is not an oversight:** a spectrogram reads every sample by definition, so a mapped take is read once into the transform. Nothing else reads it whole.

  **Acceptance**, and it is measurable rather than by eye. Opening a ten-minute stereo take in the standalone session allocates the pyramid and no copy of the material (the read is streamed through a bounded scratch; the resident cost is the summary). A stroke re-summarizes its span in time flat in the material, measured the way S18 measured its own table — the 1 ms run on takes from ten seconds to ten minutes, all four rows the same. A second peer's write over the same segment appears in this host's picture within a frame of the `/buffer_touched` that announced it, which is the test that says the picture reads the material rather than a memory of it. The browser path is unchanged and `check-wasm.sh`, `check-features.sh` and the suite stay clean. And the example is the one the milestone is for: the session example, opened on a long take, drawn, edited and heard.

- ✅ **H8 — The take fills while it records** *(opened 2026-08-18 by the user, asking how a live recording view can be implemented across platforms, and answering the how with what multitrack editors do: a file on disk, volatile if need be)*. The host half of the server's **S20**; H7 first, because this is that source made live and building it the other way round would mean two designs.

  **What H7 leaves and this finishes.** After H7 a picture *reads* the material instead of copying it, and it follows any write that goes through the host or that a peer announced. It still does not follow the **engine**, which writes a recording block by block and announces nothing — so the one material that changes continuously is the one the picture cannot show, and it is the material a multitrack draws most often. H7 also leaves one allocation standing: opening a mapped take still builds its pyramid, which is a pass over the whole take. Both fall out of the same change, which is why they are one milestone.

  **The overview is a resource, not a computation** — S20 writes a CLPK file beside the region, so the host **maps** it exactly as it already maps a `cache=` source. Opening a take then costs two mappings and no pass at all, and this is the case that becomes ordinary rather than exceptional: the crate has drawn cache-only sources since the editor-grade views landed (`WaveformData::with_pyramid` over an empty sample buffer), and what is new is that the samples are mapped *too*, so the fine regime keeps working under the overview instead of the picture flattening on zoom-in. A source is therefore **two mappings and either may be absent**: material with no overview (build it, as today), an overview with no material (a browser's fetched cache, unchanged), or both (a session's take, which is the editor's own case).

  **Following the frontier.** The row's write frontier (S20) is a number in memory the host already has open, so a recording view is a per-frame read of one integer: the frontier moved, so the columns between the old edge and the new one are re-read from the overview and the take is redrawn that much longer. Nothing is polled over the wire, nothing is copied, and the cost is the columns that changed. The **browser** gets the same picture from `/buffer_stream` (S20's wire half): buckets, not samples, appended to the pyramid it holds — which is the same split as every other data path here, and the reason the page keeps an owned copy.

  **What the view has to say that it does not say today.** A take being recorded has a **length that is not its capacity**: the buffer is allocated at its full size and the material reaches only as far as the frontier. Drawing the rest as silence is a measurement nobody made, so the view needs to draw *nothing* past the frontier — the same distinction the level body already makes between a measured zero and an absent statistic. And the axis must not jump: the time ruler is over the buffer's whole span, not over what has been recorded so far, or the picture moves under the hand while it fills.

  **Placement**: nothing new goes to the core. The pyramid, its update-by-span and the CLPK format are already there; the reader trait is H7's; the frontier is a field of a layout the core already owns. What is the host's is what it always is — deciding which columns are stale, and drawing a take that is shorter than its buffer.

  **Done 2026-08-18, and it is two of its three halves. What it dropped and why:**

  **The overview file is not part of it** — S20 dropped it the same day, for the reason recorded there: after H7 a mapped view re-summarizes a span cheaply and incrementally, so what it lacked was only *how far the material goes*, and the file is an optimization of the opening pass with a second writer of derived state attached. It is named in the root plan's Future directions with what it would take. So a view still builds its pyramid when it opens one; what it no longer does is rebuild it to follow a recording.

  **And drawing nothing past the frontier is not part of it either, because the host cannot tell.** A take read from a file is material everywhere and has no frontier at all; a take being recorded is material only up to one. But a `BufWr` writing into the middle of a *loaded* take also raises a frontier, and nothing in the picture distinguishes the two — so a host that stopped drawing past it would erase the second half of a file somebody wrote one sample into. The distinction is the **client's**: it is the one that allocated an empty buffer to record into. That makes it a prop of the signal element (the wire, both builders, `docs/gui-props.md`) rather than an inference, and it is recorded below as its own entry. Until then an unwritten remainder draws as what it is in memory — silence — which is what an empty buffer has always drawn as.

  **What shipped is the live picture.** `SharedMaterial::frontier` reads the row; `App::follow_recordings` runs on the frame tick, compares each drawn buffer's frontier against the one that view was last summarized at, and re-reads **only the frames between them** (`Element::refresh_material`, H7's door for a write this host did not make). A window redraws only if something actually grew. The wake condition is deliberately narrow — `window_follows_a_recording` is true only while a frontier is past zero and short of the buffer's end — so a session window holding loaded takes still sleeps, and one holding a take being filled ticks.

  **And it is its own tick, not the animated set** *(corrected 2026-08-18, measuring where the CPU went)*. Folding the recording into `window_is_animated` had it repainting **every** frame while a take filled, which is what that set is for — a meter's value may have changed and nothing says so — and exactly what a recording does not need, since the frontier says when it changed and by how much. Separated, with its own cadence and a redraw only for what grew, one take's cost while recording went from **12.6% of a core to 3.0%**; after the recording ends both sleep at 0%, because the frontier stops moving at the buffer's end.

  **Then the user asked the question that matters — how it scales when several channels record at once — and the answer was badly.** Two things, both measured against `recording.py` driving one take per track:

  - **A refresh copied the summary once per channel.** The frontier is the *buffer's*, so every channel advances together and one copy serves them all; asking per channel copied the whole view's pyramid N times per tick. At sixteen channels that was 5.9 ms per tick against 0.56 ms — the difference between quadratic and linear in the channel count. `Element::refresh_material` takes `Option<usize>` now: `None` is every channel, one copy.
  - **The picture grows in blocks.** Re-reading the summary at all costs that copy, so following every frame is a copy per frame per view. The host waits for a block of material instead — **`--follow-block`, one second by default**, `[gui] follow_block` in the config, `0` to follow every frame.

    **Corrected 2026-08-19: the copy was the defect and the block was hiding it.** A refresh could not write the pyramid in place because the GPU slot held it, so it cloned the whole take's summary first — a cost proportional to the take, which is why only a bigger block made it affordable. With the holder letting go the step costs the step, `--follow-block` defaults to `0` (the frame) and the numbers below are the old regime's. The record of the fix is "A refresh copied the take's summary because the slot that draws it was holding it", in Found by use.
  - **And the block is one tick for every view, not a wait each view keeps.** Per-view blocks were measured first and they cost the *redraw*: thirty-two takes accumulate their blocks at thirty-two different moments, so the window repaints thirty-two times a second instead of once, and the cost grows with the square of the track count again — 0.04% per track at eight against 0.20% at thirty-two, which is what the numbers said before anyone looked at why. On a shared tick every take that grew is caught up together and the window is repainted once, whatever the count.

  ```
  clausters-gui while recording   every frame   1 s, per view   1 s, shared tick
   8 tracks                            12.6%            0.30%              0.60%
  32 tracks                            57.4%            6.30%              1.20%
  per track                          ~1.7%             ~0.1%              ~0.05%
  ```

  Read down the last column: the cost per track stops growing with the count, which is the property that was missing. (Eight tracks read *higher* per track on the shared tick than on per-view blocks because a fixed per-tick cost — one wake, one repaint — is spread over fewer of them; what matters is that thirty-two no longer pay five times what eight pay.)

  The block is a **setting rather than a constant** because how choppy is too choppy is not ours to decide, and because a session recording thirty-two tracks wants the machine quiet for everything else. Neither the sound nor a playhead over the take reads it: what it changes is how often the *picture* catches up with the material.

  **The wire half is the server's** (S20's `/buffer_stream`), and both clients can now ask for it: `Server.stream_buffers` in Python, `Server.streamBuffers` in TypeScript. **What no page does yet is consume it** — folding streamed buckets into a pyramid it holds needs a way to write buckets into one, which the core does not expose. Recorded below rather than half-built.

  **The example is `clients/python/examples/views/recording.py`**: one server owning a segment, one host mapping it, and **N empty takes with a glissando recorded into each** — they start on one pitch and fan out to a random one, on internal buses, so nothing needs an input device and the count is the point. It takes the track count and the block as arguments (`recording.py 32 0` against `recording.py 32 1`), which is how the table above was measured, and it is by-eye like every other `gui_*`.

  **What it turned up about the lanes**, found by the user looking at it: a stack of lanes ruled by *one lane's* `ruler` prop leaves that lane shorter than the rest, because the strip comes out of its own height. The free-standing `timeruler` is what a stack wants — a box of its own, no lane pays for it — and the lanes share one navigation group, so they zoom and pan together and one ruler labels them all.

  **Acceptance:** the standalone session records a take and draws it filling, with the cost per frame flat in the take's length; the view draws nothing past the frontier and its axis does not move while it fills; the page draws the same recording over `/buffer_stream` and agrees with the native one; and an example records, draws, stops and edits the take it just made — which is also the first time the editor's own capture is exercised by eye.

**What stays out of the track**: the log itself and everything about its contents (the crate's O5); undo of node-tree commands (playing a synth is not an edit) and of navigation (a view is not data); a history that outlives its session; and any merge or conflict machinery, which only a persisted, two-writer history would need — this one has neither.

## ✅ In-browser audio engine (Web Audio / AudioWorklet) — shipped as the server's B track

**This track converged and is numbered on the server side: `PLAN.md` (root), "B track — the engine in the browser" (B0–B4).** The engine port, the AudioWorklet backend and the wasm-`Embed` link landed there because the engine is the server's; the GUI-host leg landed as B3 (`ServerLink::Page` + `GuiBridge.connect_page`/`server_reply`, the `host::bundle` boot replay, the standalone page), and the web-component packaging landed as B4 (the `clausters` package seeded in `clients/web/`). **Consolidation (with W0):** this crate no longer carries a `web/` directory — the harness/parity/standalone pages and the bundle tooling that G12/G16/B3 grew here live in the web package (`clients/web/examples`, `tests`, `tools`), which stages this crate's wasm-bindgen bundle into its `dist/gui-host/`; the milestone entries keep the original paths as the record of what shipped where. The design notes below are the original record that shaped it — note the SharedArrayBuffer route they sketch was *not* taken initially (no COOP/COEP on arbitrary pages; the MessagePort ring landed instead, with SAB kept open as a later optimization — see `docs/decisions.md`).

Not part of G11-G16; recorded here because the G11 seam was deliberately shaped to accept it, and the G11 decision ("no in-process engine in the browser") is a scope boundary that this track relaxes. Through G16 the browser GUI host drives a *separate* audio server over WebSocket. The parallel, larger piece of work is to compile `clausters-server` itself to `wasm32` with a **Web Audio backend**, so the engine runs **in the browser** - the wasm analogue of the native `standalone`/`embed` mode.

- **A new audio backend behind the existing engine seam, not a DSP rewrite.** The engine core (`Engine::process_block`) is already decoupled from the device: it feeds the real-time cpal callback and the offline `render()`/NRT path from the same block function (FTZ armed in both, NRT sample-identical to RT). A browser backend is a *third* driver - an **AudioWorklet** output whose process callback pulls blocks from the engine. cpal does not target Web Audio, so this is a genuine backend addition, which is exactly why it is its own track rather than a step inside G11-G16.
- **An in-process browser link, the wasm `Embed`.** With the engine in the page, the GUI host reaches it through a new `ServerLink` variant (the wasm counterpart of `Embed`): OSC over an in-process channel, **not** WebSocket. This is the same second link kind the native host already has (`Udp` vs `Embed`); the `Transport` trait and the cfg-gated `ServerLink` from G11 take the variant without a protocol change. WebSocket stays the carrier for a *remote* server, so it never disappears - it stops being the browser's only option.
- **The shared-memory paths return inside the browser.** An AudioWorklet runs on its own thread and shares state with the main thread through `SharedArrayBuffer` (which needs cross-origin isolation, COOP/COEP). That is the browser's shared-memory primitive: it can carry the zero-message `BusSource` (control buses read each frame) and the bulk audio path **inside** the page - the same roles `host::shm`/`mapfile` play natively - instead of the WS/`fetch` fallback G14/G15 build for the remote case. So a browser host paired with an in-page engine looks more like the native host than like the remote-server browser.
- **RT-safety carries to the worklet.** The audio thread's no-alloc/no-lock discipline applies to the AudioWorklet thread, and the lock-free command/garbage FIFOs map onto `SharedArrayBuffer` ring buffers. A standalone-style bundle (a GuiDef + GraphDefs) could then boot entirely in a browser tab with no server process at all - the browser twin of `--standalone`.

This intersected the server track and not only the GUI one, which is why it was numbered there. **The one piece of the sketch above that did not ship is the third bullet** — the in-page shared-memory paths — and it is recorded as a track to open under Future directions rather than left here reading as done. The product TypeScript client (`clients/web`, see the G13 note) is still a separate concern from both.

## AP track — the application scope: the window set, the named id, the reconcile, one undo order

*Opened 2026-09-06 with the user as a plan of its own, `APPLICATION-SCOPE.md`,
which `AP8` deleted once nothing in it was the only copy: the decisions are in
`docs/decisions.md`, the reconcile in `docs/gui-protocol.md` and
`docs/architecture.md`, the design reference and the reading of the host's
multitrack code in `crates/clausters-document/PLAN.md`, and the track and what
it found by use here. What shipped is the git history.*

**The inversion.** The editor stops being the unit and the application becomes
it: an application owns the host, the widget-id space and the loop; an editor is
one structure bound to one `Domain` and one `View` and owns nothing global. The
acknowledgement stays each view's, screen state stays the host's and each
window's, and the undo order is the data's editing context — the applications
crate's since `AP7`.

**Non-goals it kept.** No second owner of a document (no operational transform,
no CRDT); shared memory stays a native fast path and never a model the browser
lacks; the opaque leaf stays opaque; no new widget, gesture or drawing rule.

**What would have made it wrong**, checked rather than felt: a seam that needs a
second implementor invented to exist (`AP4` refused one); a move into Rust that
makes the document read an opaque leaf; a second application that needs surface
added to a client (`AP7` added none); a verb one client has and the other lacks
at the end (there is none).

- ✅ **AP0 - The seam, in Python only, with nothing moved.** *(2026-09-06)*
  `Application` took the host and its resolution, the id space, the drain and
  the walk from `Editor`, which kept the structure, the domain, the view and the
  unit bridge. Moved in the cheapest language first, because moving it after the
  core meant moving it in Rust and TypeScript too. Registering a structure at
  construction was dropped: `structure=None` is legal, and `Editing.of` caches on
  the object.
- ✅ **AP1 - A widget id is named, not leased.** *(2026-09-06)*
  `clausters_core::widgetids::WidgetIds`: one occupancy map, the anonymous lease a
  hand-built tree takes and the id a view asks for by `(structure, role, key)`.
  Its four rules — a map and not a hash, a draw names its drawer, an anonymous
  free cannot take back a named id, a draw with no host is private — are in
  `docs/decisions.md`.
- ✅ **AP2 - A redraw is a diff.** *(mechanism 2026-09-06, retired 2026-09-07)*
  The difference landed and then proved uncomputable outside the host (`AP5`).
  Its acceptance — editing one clip emits no definition, a scroll and a
  selection survive — moved to `O23`, where the reconcile holds it.
- ✅ **AP3 - Screen state is about a thing, and a thing is not its address.**
  *(2026-09-06)* Rewritten because it contradicted a decision written three
  times: selection, zoom and layer stay each window's, and `adopt_selection` is a
  composed view handing its range up, which stays. What was wrong was four tables
  keyed by `id(object)`, which CPython reuses after a free; they key by the
  object, weakly. `O23` refined it: screen state reaches neither the document nor
  the history, and does reach the session file as a view.
- ✅ **AP4 - A payload is by value or by reference.** *(closed 2026-09-06 by
  finding it already true)* The seam is `Domain`, and no module of the
  application core names a vocabulary; by value (`points`, `events`) and by
  reference (`samples`) are implementations of one interface, and a marker saying
  which would be machinery nothing reads. How bulk travels is the data-path work,
  not this track's.
- ✅ **AP5 - The application core moves to Rust.** *(2026-09-06 to 2026-09-09)*
  The finding was that **the picture has one owner**: a client-side difference is
  correct only against an equal copy of what the host draws, and that copy is
  unreachable, so no client keeps a picture and the host reconciles a def
  (`widget::reconcile`, with `O23`). `guidiff` was retired from the core and both
  clients (core ABI v42), `"data": "keep"` carries the bulk, and which widget to
  publish is the caller's (`docs/decisions.md`, with the measurement). Also moved:
  the routing table's tag list (`clausters_document::view::NOT_AN_EDIT`, v44),
  the catalogue views (`view::catalogue`, `clausters_view_props`, v45 — the
  document crate rather than the host's `tree.rs`, since a client cannot call into
  the host), and the data half of the undo walk (`History::walk`, v46). The unit
  bridge and the echo's staleness test were audited: already one rule called
  twice. The standing reason, said by the user: the editing logic is in Rust so
  that it is in one place and consistent across clients.
- ✅ **AP6 - `FormEditor` converges.** *(closed by removal, 2026-09-06)* The
  target was wrong: a multitrack's own state is authored, durable and undoable,
  and a projection has nowhere to keep it but the widget tree. `FormEditor` was
  deleted and the session in `crates/clausters-document` replaced it
  (`O21`-`O24`); what survived is the measurement that proved the named ids.
- ✅ **AP7 - A second application, to prove the abstraction.** *(2026-09-14)* The Python client's `SamplesEditor`,
  moved as it is into `crates/clausters-apps` to work out one undo order over two
  applications — not the audio editor, which is a track of its own no plan holds
  yet. (1) ✅ the window, `clausters_apps::samples::{window, props}`; (2) ✅ the
  conversation, the inverse taken from the one reading and the words of a turn in
  `clausters_apps::turn`; (3) ✅ the write, `clausters_editing::samples::write_steps`
  (`/buffer_setRangeChannel` for one channel of a wider take, the last chunk's
  `/done` awaited); (4) ✅ the undo order, `clausters_apps::editing::Editing` —
  members under keys, external members, the context's turn and step — bound as
  `clausters_apps_editing_*` (v62-v64, the separate editor handles retired), with
  both clients' `Editing` and the standalone host's `Owner` over it. Not in it:
  entering a box from a piece, the audio editor, the catalogue views.
  **Acceptance:** written with the supported surface, one program in two
  languages, and an undo that walks edits made in two applications as one order —
  held by the crate's and both clients' tests.
- ✅ **AP8 - The pass over the packages.** *(2026-09-14)* The durable half of
  `APPLICATION-SCOPE.md` went where it belongs (above) and the file was deleted.
  The open question it raised about a version across a load is in
  `crates/clausters-document/PLAN.md`'s "Open decisions".

## V track — development: the host seen and driven by a script

*(Added 2026-10-08 by the user, once every verb a window answers had become reachable from a script (`/gui_verb`): so that a change can be seen working without a person at the screen. Until now every screenshot in this plan was taken by hand. A development track: what it builds is for checking the host, and it ships in the host and both clients like everything else.)*

**What it is for.** A script opens a window, drives it — verbs, edits, a drag, a pass of the transport — and asks for a picture after each step; whoever reads the pictures checks what the eye pass checks today. It is the by-eye check made repeatable: the examples stay the manual test surface, and a capture is how one is looked at without somebody sitting in front of it. The track has two halves: a window that is captured and played to (V1, V2), and the same with no window on the screen at all (V3–V5), which is independent of the first half's fronts and can be taken on its own.

**Decided before opening it:**

- **One more verb on the wire, not a second renderer.** `/gui_capture <window> <path>` draws the window's frame with the one `frame::render` the screen is drawn with, into an offscreen texture of the window's size, reads it back and writes a PNG; the host answers `/gui_captured <window> <path>` when the file is whole, so a script waits on the answer and not on a sleep. What is captured is therefore what the screen shows, by construction — a capture path of its own would be a second picture to keep in agreement with the first.
- **A capture follows the turns before it.** A verb's edit goes to the window's owner and comes back as corrections, so a picture asked for right after a verb could show the window before the owner answered. The clients' `capture` waits until every turn it has outstanding is answered, then asks; the host captures after everything it has received.
- **Both fronts, both clients.** The page front draws the same frame offscreen and writes the PNG into the page's own storage (OPFS, `docs/decisions.md`, "A path in a page is its own storage"); both clients have `editor.capture(path)` and `GuiHost.capture(id, path)`. A capture only the desktop has is the divergence the non-divergence rule forbids.
- **No dependency for the file.** A PNG of stored (uncompressed) deflate blocks is a few dozen lines and needs no crate; a capture is read once and thrown away, so its size does not matter.
- **Placement (the G7b rule): display-only.** The readback and the file are the host's; nothing for `clausters-core`.

**The milestones — a window captured and played to:**

- ✅ **V1 — `/gui_capture`.** The verb, the offscreen draw and readback on both fronts, the PNG writer, the answer, and the two clients' `capture` with the wait. **Acceptance:** a script opens `editors/window_verbs`, quantizes, captures, pastes, captures, and the two pictures show the notes on the beats and then repeated after themselves; a capture of a window with a live view (a meter, a playhead) shows the frame of the moment it was asked for; the same from a page into its storage; the wire and the PNG writer are unit-tested, and the protocol page names the verb. *(Done 2026-10-16. `frame::render` was split at its pass (`encode_pass`), so the screen's frame and a capture's are one pass into two textures; `frame::capture` draws a whole frame into the window's batches and copies it into a mapped buffer (`Readback`), which a native front waits on and a page polls on its loop before writing into its storage. The answer carries a reason when no file was written, a window the host does not have included, so a client never waits on nothing. The clients' wait for the turns before it is a `/gui_query` round trip: its answer comes after every event sent before it. Checked on `editors/window_verbs` (opened, quantized, pasted; two captures 0.8 s apart while it played show the playhead moved), and `tests/editor.html` captures a waveform and a spectrogram into the page's storage and reads the PNG back. The example pair gained `picture(name)`.)*
- ✅ **V2 — A hand played to a window.** A drag is a gesture, not a verb, so V1 cannot check one. `/gui_pointer <window> <kind> <x> <y> [mods]` hands the window's gesture machine a press, a move or a release at a point, as the front's own input does — the same `Gestures::press`/`drag`/`release` a mouse reaches, so a scripted drag is the drag. **Acceptance:** a script drags a note with snap to grid on and off and captures each, and the pictures show it on a beat and then where it was let go; the same in a page. *(Done 2026-10-16. The point is in the window's device pixels, which are a capture's, so a script points where it found something in the picture; `mods` holds for the one event and the keyboard's modifiers come back after it. Both fronts set the pointer and hand the event to the window's own machine through the adapters their input uses, so a move drags only while a drag is held. Both clients have `host.pointer` and, on an editor, `pointer`, `click` and `drag`. Checked on `editors/window_verbs`: a note dragged 0.73 beat with snap on lands on the next beat, and one dragged with it off stays where it was let go, each captured; in a page `tests/gui.html` drags its bound knob from the script and reads the engine's control move.)*

**The milestones — the same with no window on the screen** (independent of V1–V2's fronts; they need V1's verb):

The native front's capture puts a window in front of whoever is at the machine while a script drives it, and needs a display. The headless front (`--headless`) has neither — and today no GPU either: it runs the protocol and draws nothing. A capture from it is a host that opens **no window at all**, which is what a check run while somebody works, or on a machine with no display, needs. What it changes is the front, not the verb: the headless front has to own windows it never shows. `frame::render` is the per-window render both fronts already share, and a third caller that draws into a texture is the whole of what keeps the picture the screen's. wgpu can request an adapter with no compatible surface, and on Linux a software one exists where there is no card; whether a capture is still pixel-for-pixel the screen's on a software adapter (text, line smoothing) is the first thing to measure, and the answer decides whether a reference picture is ever compared byte for byte or only read.

- ✅ **V3 — The headless front draws.** It requests an adapter with no surface, keeps per-window GPU resources the way the windowed front does, and draws a window's frame offscreen at its declared `w`/`h`; with no adapter at all it says so and goes on as it does today. *(Done 2026-10-16. The headless front is now the windowed one's `App` driven by a loop of its own: `Loop` stands where winit's event loop did -- it opens a window on the screen or offscreen and ends the host -- and the fronts' threads reach it through a channel instead of winit's proxy, so every window has its GPU, its gesture machine and its owner, and a verb, a hand and a capture do what they do on the screen. `Gpu::offscreen` asks for an adapter with no surface, a software one when nothing else answers, and logs which; a window is drawn at its declared size at a scale of one. The binary asks once whether an adapter answers and, when none does, runs the protocol-only front as before. A headless host keeps its own clipboard. The web suite's `gui-host.test.ts`, which drives a headless host, passes on the new front.)*
- ✅ **V4 — It captures, and takes a hand.** V1's `/gui_capture` and V2's `/gui_pointer` on the headless front, the window never shown. **Acceptance:** `editors/window_verbs` driven by a script against a headless host captures the pictures V1 and V2 name, with no window on the screen; the difference between a hardware and a software adapter's picture is measured and written down. *(Done 2026-10-16. Nothing of the verbs had to change: the headless front is the windowed one's host since V3, so `/gui_capture` and `/gui_pointer` reach the same doors. `editors/window_verbs` driven against a headless host, with no window shown, captured the quantize, a note dragged 0.75 beat with snap on landing on the next beat, one dragged with it off staying at 3.69, and the paste. The measurement: one adapter is byte for byte the same run to run; an integrated card and llvmpipe differ on about a third of the pixels, by at most two levels of 255 (one with the bitmap face) -- recorded in `docs/decisions.md`, "A capture is the same picture on any adapter, within two levels", and kept as the ignored test `software_against_hardware`.)*
- ✅ **V5 — A check that runs where nobody looks.** What V3 and V4 make possible: a script that drives an example through a headless host and leaves its pictures in the example tree's `out/`, so a change can be looked at after the fact — and, if V4's measurement allows it, a reference picture compared in a test. *(Done 2026-10-16. `scripts/look.py EXAMPLE [STEP ...]` runs a Python example's interactive branch against a host made headless through the configuration's project layer (a `clausters.toml` of `[gui] headless = true` in a directory of its own, so no new surface), calls the functions it names or waits the seconds it is given, and captures every open window after each step into the example tree's `out/look/<example>/`; both clients gained `GuiHost.windows` for that walk. V4's measurement allowed the reference: `tests/references/window.png` is compared within two levels a channel, a second test checks a software adapter holds to it where one is present, and `CLAUSTERS_GUI_BLESS=1` writes it again. A page is not driven by the script -- its host is in the page, and the page acceptances in `clients/web/tests` already run headless. `docs/contributing.md`, "Looking at a window with nobody at the screen", is how it is used.)*

**The milestones: the keyboard, and the procedure written down** *(added 2026-11-09 by the user, to make graphical development faster: a script could point and drag but not type, and the skills that guide work on widgets and applications named neither the captures nor the scripted hand, so a check by eye was still the default)*:

- ✅ **V6: A keyboard played to a window.** `/gui_keypress <window> <key> [mods]` presses one key, spelled as a key table spells it (a character, or a named key: `Enter`, `Backspace`, `Left`, `Tab`, `F5`), with the modifiers held for that press as `/gui_pointer` holds them; `/gui_text <window> <text>` types a text, a key per character with no modifier, as a front types what an input method committed (a newline is Enter, a tab is Tab, any other control character refuses the text). Both reach the window's one keyboard dispatch (`Gestures::press_key`: the popup layer, the focus ring, the focused element, then the key table), so a scripted key is the key. **Acceptance:** a script focuses a text field with Tab, types into it, edits it with chords and captures it, on a headless host and in a page; the wire is unit-tested. *(Done 2026-11-09. The key names are the key table's, read by one function both use (`keymap::key_named`, which reads Tab and leaves refusing it to the table). A letter is what a keyboard gives under the modifiers: upper case under Shift alone, lower case under Ctrl or Alt, the letter a chord was pressed on, as both fronts read their own keyboards. Escape with nothing open does not close a native window when a script presses it, since a page has no such key and a script's keys do the same on both fronts. Both clients: `press_key`/`type_text` (`pressKey`/`typeText`) on `GuiHost` and on an editor. Checked against a headless host: Tab, an accented text, Backspace, Shift+a, Ctrl+Shift+Left and Ctrl+X reported the field's strings in order and the capture showed what was left; `tests/gui.html` types into a field from the script.)*
- ✅ **V7: The check by script is how graphical work is checked.** The procedure, cheapest first (the unit tests, then a headless host driven by a scratch script against the debug host binary, an example's steps through `scripts/look.py`, a before-and-after pair, the page, and the eye last), is written once in the `clausters-gui` skill; the `gui-widgets` skill says what a widget asks of it (each size, each state, what it reports, what it costs at rest, two windows when it shares anything); and an application's views are sent through `gui-widgets`, since they are widgets. *(Done 2026-11-09. The same day the applications got a skill of their own, `gui-apps`, complementary to `gui-widgets`: the modular components an application is composed of, who owns its state and history, the global questions a change answers and an optimized development process, from a turn tested in Rust to the running application checked headless.)*
