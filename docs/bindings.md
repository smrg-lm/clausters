# The shared core's binding surface

`clausters-core` is the single source of truth for the value- and time-level
computations the server and every client must agree on. Three bindings carry it
outward, and **only the first is checked by a compiler**:

| Binding | What it is | How it is kept honest |
|---|---|---|
| `clausters-ffi` | the C ABI (`extern "C"`, flat data) | cargo — a core signature that moves breaks the build |
| `clausters-core-web` | the wasm/JS surface (`wasm_bindgen`) | cargo, against core — but nothing ties it to the C ABI |
| `clausters/_native.py` | the ctypes declarations over the C ABI | `clients/python/tests/test_native_parity.py` |

The Python leg needs no table: it must cover the C ABI **entirely**, so its test
compares the two sets directly and fails on anything the binding never declared.

The wasm leg is different, and this file exists because of that difference. It is
**not** obliged to mirror the C ABI, and pretending otherwise would make the
check unreadable: a browser already has WebSocket, `libverovio` is not built for
wasm, JavaScript has no `u64`, and wasm frees by `Drop` where C needs an explicit
`_free`. So the two surfaces legitimately differ — which means the interesting
question is never "are they equal" but **"is each difference one somebody
decided?"**

That is what the table below records, and what `tests/bindings.rs` enforces:
every symbol either appears in a row, or the test fails. Adding a function to
one binding and not the other is allowed; adding it and *not saying why* is not.

## How to read a row

The note column carries one of three verdicts, and they mean different things:

- **`idiom`** — the same capability, shaped for the language. Nothing to do.
- **`n/a`** — deliberately absent, with the reason. Nothing to do.
- **`gap`** — present on one side, missing on the other, and **nobody has
  decided**. These are the rows worth reading: each is either work waiting or a
  decision waiting to be written down. They are not failures — the test passes
  with gaps in it — because a manifest that forbade them would just be lied to.

When you add a core function and bind it, add its row. When you bind it on one
side only, say which verdict applies. If the honest answer is "I have not
thought about the other side", write `gap` — that is what it is for.

## Numbers, signals and measurement

| C ABI (`clausters-ffi`) | wasm (`clausters-core-web`) | Note |
|---|---|---|
| `clausters_core_unary` | `unary` | |
| `clausters_core_binary` | `binary` | |
| `clausters_core_whitenoise` | — | **gap** — `JsRng` steps values one at a time; there is no buffer fill, so a browser cannot reproduce a server noise stream in one call |
| `clausters_core_window` | — | **gap** — the FFT smoothing windows. The web spectrogram takes magnitudes from `spectrumDb`, which applies its own |
| `clausters_core_stats` | `channel_stats` | `idiom` — the peak and RMS of one channel of an interleaved buffer; C writes the pair through an out pointer, wasm returns it as a two-element array |
| `clausters_core_correlation` | `correlation` | |
| `clausters_core_lissajous` | `lissajous` | |
| `clausters_core_hz_to_mel` | — | **gap** — the perceptual axes a spectrogram labels |
| `clausters_core_mel_to_hz` | — | **gap** — as above |
| `clausters_core_hz_to_bark` | — | **gap** — as above |
| `clausters_core_bark_to_hz` | — | **gap** — as above |
| — | `spectrum_db` | **gap** — the magnitude-to-decibel column the web spectrogram draws |
| — | `oscil_raw_frames` | **gap** — oscilloscope framing |
| — | `oscil_display_frames` | **gap** — as above |
| — | `oscil_align` | **gap** — as above |

## Time and the beat grid

| C ABI | wasm | Note |
|---|---|---|
| `clausters_core_beats_to_secs` | `beats_to_secs` | |
| `clausters_core_secs_to_beats` | `secs_to_beats` | |
| `clausters_core_secs_to_samples` | `secs_to_samples` | |
| `clausters_core_samples_to_secs` | `samples_to_secs` | |
| `clausters_core_unix_to_sample` | `unix_to_sample` | |
| `clausters_core_quant_delay` | `quant_delay` | |
| `clausters_core_bar` | `bar` | |
| `clausters_core_beat_in_bar` | `beat_in_bar` | |
| `clausters_core_unix_to_ntp` | `unix_to_ntp` | |
| `clausters_core_ntp_timetag` | — | `n/a` — wasm packs the timetag inside the bundle encoders (`osc_encode_bundle` from Unix seconds, `osc_encode_score_bundle` from render seconds); nothing on that side holds a raw NTP value |
| `clausters_core_degree_to_midinote` | `degree_to_midinote` | |

## Randomness

| C ABI | wasm | Note |
|---|---|---|
| `clausters_rng_seed` | `JsRng.new` | `idiom` — C mixes a seed into a state the caller then steps; wasm wraps the state in an object |
| `clausters_rng_next_f64` | `JsRng.next_f64` | |
| `clausters_rng_next_below` | `JsRng.next_below` | |
| `clausters_rng_next_u64` | — | `n/a` — JavaScript has no `u64` to hand back; the float reads are the surface there |
| — | `JsRng.uniform` | **gap** — a range read the C ABI does not offer |
| — | `JsRng.spawn` | **gap** — a child stream, the reproducible-substream primitive |

## The beat queue

| C ABI | wasm | Note |
|---|---|---|
| `clausters_sched_new` | `JsScheduler.new` | |
| `clausters_sched_free` | — | `idiom` — wasm frees by `Drop` |
| `clausters_sched_push` | `JsScheduler.push` | |
| `clausters_sched_peek_time` | `JsScheduler.peek_time` | |
| `clausters_sched_pop_due` | `JsScheduler.pop_due` | |
| `clausters_sched_remove` | `JsScheduler.remove` | |
| `clausters_sched_len` | `JsScheduler.len` | |
| `clausters_sched_clear` | `JsScheduler.clear` | |
| — | `JsScheduler.is_empty` | `idiom` — `len == 0`, spelled the way a JS collection is read |

## The sample-clock model

| C ABI | wasm | Note |
|---|---|---|
| `clausters_clocksync_new` | `JsSampleClockModel.new` | |
| `clausters_clocksync_free` | — | `idiom` — wasm frees by `Drop` |
| `clausters_clocksync_add_anchor` | `JsSampleClockModel.add_anchor` | |
| `clausters_clocksync_sample_at` | `JsSampleClockModel.sample_at` | |
| `clausters_clocksync_local_time_of` | `JsSampleClockModel.local_time_of` | |
| `clausters_clocksync_drift_ppm` | `JsSampleClockModel.drift_ppm` | |
| `clausters_clocksync_rate` | `JsSampleClockModel.rate` | |
| `clausters_clocksync_span` | `JsSampleClockModel.span` | |
| `clausters_clocksync_slope` | — | **gap** — the raw fit coefficient; wasm exposes only the derived reads |
| `clausters_clocksync_intercept` | — | **gap** — as above |
| — | `JsSampleClockModel.len` | **gap** — the anchor count |
| — | `JsSampleClockModel.is_empty` | `idiom` — `len == 0` |

## Id registries

| C ABI | wasm | Note |
|---|---|---|
| `clausters_registry_new` | `JsRegistry.new` | |
| `clausters_registry_free` | — | `idiom` — wasm frees by `Drop` |
| `clausters_registry_alloc` | `JsRegistry.alloc` | |
| `clausters_registry_release` | `JsRegistry.release` | |
| `clausters_registry_contains` | `JsRegistry.contains` | |
| `clausters_registry_in_use` | `JsRegistry.in_use` | |
| `clausters_registry_capacity` | `JsRegistry.capacity` | |
| `clausters_registry_clear` | `JsRegistry.clear` | |
| `clausters_registry_node_partition` | `node_id_partition` | |
| `clausters_registry_graph_audio_reserved` | `graph_bus_reserved` | `idiom` — C answers one number per call, wasm returns the pair |
| `clausters_registry_graph_control_reserved` | `graph_bus_reserved` | `idiom` — as above |
| — | `JsRegistry.unbounded` | **gap** — the capacity-less constructor |
| — | `JsRegistry.is_allocated` | **gap** — per-id occupancy read |
| — | `JsRegistry.base` | **gap** — the range's first id |

## Peak pyramids

| C ABI | wasm | Note |
|---|---|---|
| `clausters_core_peaks_build` | `JsPyramid.build` | `idiom` — wasm has one builder, which takes a channel count |
| `clausters_core_peaks_multi_build` | `JsPyramid.build` | `idiom` — as above |
| `clausters_core_peaks_cache_size` | — | `idiom` — C sizes the buffer, then fills it; wasm returns the bytes |
| `clausters_core_peaks_multi_cache_size` | — | `idiom` — as above |
| — | `JsPyramid.from_bytes` | `idiom` — the two sides consume a cache differently: through the C ABI the pyramid is a byte blob whoever mapped it reads, while wasm keeps it as an object and answers about it. The readers below are that object's face |
| — | `JsPyramid.to_bytes` | `idiom` — as above |
| — | `JsPyramid.frames` | `idiom` — as above |
| — | `JsPyramid.channels` | `idiom` — as above |
| — | `JsPyramid.base_bucket` | `idiom` — as above |
| — | `JsPyramid.num_levels` | `idiom` — as above |
| — | `JsPyramid.level_bucket` | `idiom` — as above |
| — | `JsPyramid.level_for` | `idiom` — as above |
| — | `JsPyramid.column` | `idiom` — as above |
| — | `JsPyramid.columns` | `idiom` — as above |

## Bundles and patches

| C ABI | wasm | Note |
|---|---|---|
| `clausters_core_bundle_requirements` | `bundle_requirements` | |
| `clausters_core_bundle_resolve` | `bundle_resolve` | |
| `clausters_core_bundle_validate` | `bundle_validate` | |
| `clausters_core_patch_compile` | — | **gap** — the patcher's graph compilation |

## The document

One implementation of what an edit *means*, bound by every client rather than
re-derived per language. The shape is deliberate and is the crate's own
decision, not a convenience: the document and the intent cross **by value** and
the new document comes back, instead of a client holding a handle into a Rust
object graph. A handle would make every accessor a document has — and a tree has
dozens — a call to design, bind and keep in step; the round trip costs a
serialization per edit and buys a binding that is one function, plus the
property that a client's document *is* the crate's document rather than a
parallel structure synchronizing with it.

| C ABI | wasm | Note |
|---|---|---|
| `clausters_document_apply` | `document_apply` | |
| `clausters_document_resolve` | `document_resolve` | |

## OSC

| C ABI | wasm | Note |
|---|---|---|
| — | `osc_encode_message` | `n/a` — the Python client encodes OSC in Python and the server in Rust; a browser has no such library, so the wasm module carries the core's OSC door |
| — | `osc_decode_packet` | `n/a` — as above |
| — | `osc_decode_packet_timed` | `n/a` — as above, plus each message's containing-bundle time in Unix seconds, which is what the responder layer hands a callback; the Python client reads the same field out of its own decoding door |
| — | `osc_encode_bundle` | `n/a` — as above |
| — | `osc_encode_immediate_bundle` | `n/a` — as above |
| — | `osc_encode_score_bundle` | `n/a` — as above; the NRT score's epoch (seconds from the render's start), where Python packs the timetag with `clausters_core_ntp_timetag` and assembles the bundle itself |

## Notation

The whole layer is behind the crate's `notation` and `verovio` features, and
`verovio` links `libverovio`, which is not built for wasm. A browser that wants
engraving asks a server for it.

| C ABI | wasm | Note |
|---|---|---|
| `clausters_core_svg_to_display_list` | — | `n/a` — notation layer, not built for wasm |
| `clausters_core_voice_to_mei` | — | `n/a` — as above |
| `clausters_score_open` | — | `n/a` — engraver, links libverovio |
| `clausters_score_free` | — | `n/a` — as above |
| `clausters_score_display_list` | — | `n/a` — as above |
| `clausters_score_mei` | — | `n/a` — as above |
| `clausters_score_transpose` | — | `n/a` — as above |
| `clausters_score_edit` | — | `n/a` — as above |
| `clausters_score_undo` | — | `n/a` — as above |
| `clausters_score_redo` | — | `n/a` — as above |
| `clausters_score_can_undo` | — | `n/a` — as above |
| `clausters_score_can_redo` | — | `n/a` — as above |

## Transport and versioning

| C ABI | wasm | Note |
|---|---|---|
| `clausters_ws_connect` | — | `n/a` — the browser provides WebSocket; this exists so a browser-less binding does not re-implement the framing per language |
| `clausters_ws_send` | — | `n/a` — as above |
| `clausters_ws_recv` | — | `n/a` — as above |
| `clausters_ws_close` | — | `n/a` — as above |
| `clausters_ws_last_error` | — | `n/a` — as above |
| `clausters_core_abi_version` | — | `n/a` — the cdylib is loaded by a client built separately, so it answers a version handshake (`CORE_ABI_VERSION`); the wasm module ships inside the npm package and is rebuilt with it, so there are never two versions to reconcile |
