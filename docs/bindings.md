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

The `plugin_shm` audio plane is native host infrastructure, with no C/wasm client
exports. Both clients discover plugins through the same OSC command:
`Server.scan_plugins(*dirs, timeout=...)` / `server.scanPlugins(dirs, timeout)`.
The variadic Python directories versus a TypeScript array are `idiom`; both
return the JSON rows specified in [native plugin discovery](schemas.md#native-plugin-discovery).

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

## How a C call hands bytes back

Two conventions, one per library, and every call follows its library's.

**`clausters-ffi` sizes and fills, in one function.** The caller passes a buffer
and its capacity; the call returns the byte count the answer needs and writes
only when the answer fits, so a null or short buffer is a sizing pass and the
caller repeats the call with a buffer that fits. Most calls simply compute the
answer twice. Two keep it between the passes, for different reasons, and so keep
different things:

- **`clausters_document_snapshot`** is a pure read that is the size of the
  document, so it keeps the serialized bytes rather than serializing twice. It
  needs no key: any edit drops them.
- **`clausters_apps_editing_call`** runs a verb that changes a history, which
  must not run twice, so it keeps the answer *with the request* and hands it
  over only to the same request again.

A mutating call that keeps nothing commits only when the bytes are written, so
its sizing pass changes nothing (see [the document](#the-document)).

**`clausters-midi` allocates.** A writer returns a buffer Rust allocated and its
length through an out pointer, and the caller frees it with
`clausters_midi_free`. It is its own library with its own cdylib, and a file
writer's answer is produced once and kept by the caller, which is the ordinary C
shape for that.

In wasm neither applies: a call returns the bytes or the string.

## Numbers, signals and measurement

| C ABI (`clausters-ffi`) | wasm (`clausters-core-web`) | Note |
|---|---|---|
| `clausters_core_unary` | `unary` | |
| `clausters_core_binary` | `binary` | |
| `clausters_core_list` | `list` | `idiom` — the list functions (`normalize`, `integrate`, `resamp1`, `asrandomtable`, `mirror`, `stutter`, `pyramid`, …): C writes into a buffer the caller sized from the op's output length, wasm returns the list |
| `clausters_core_list_len` | — | `n/a` — how long a list function's result is, which C needs to size the buffer it hands `clausters_core_list`; wasm returns the list, whose length is its own |
| `clausters_core_blendat` | `blend_at` | `idiom` — the read method is a `BoundOp` number in C and its name in wasm |
| `clausters_core_calc` | `calc` | `idiom` — the numeric functions that are not operators (`degrad`, `erf`, `snap`, `bilin`, …), in `f64`; C takes the function by number, wasm by name |
| `clausters_core_scale_calc` | `scale_calc` | `idiom` — the same for the four that read a scale or a sorted list (`degkey`, `keydeg`, `nearestinscale`, `nearestinlist`) |
| `clausters_core_bound` | `bound` | `idiom` — `clip`/`wrap`/`fold` between two bounds; the same sequence-against-one-value split as `unary`/`binary` |
| `clausters_core_map` | `map` | `idiom` — the range maps (`linlin`, `linexp`, `lincurve`, …). C takes a whole sequence and writes it through an out pointer, as it does for the op tables above; wasm maps one value, and the client loops — the same split `unary`/`binary` already have, for the same reason (a ctypes call is worth batching, a wasm-bindgen call is not) |
| `clausters_core_whitenoise` | — | **gap** — `JsRng` steps values one at a time; there is no buffer fill, so a browser cannot reproduce a server noise stream in one call |
| `clausters_core_window` | — | **gap** — the FFT smoothing windows. Nothing in the browser transforms anything: a spectrum is a view and the GUI host draws it, reaching `spectrum` and `window` as a Rust caller of the core |
| `clausters_core_true_peak` | `true_peak` | both — the **reconstructed** peak of one channel of an interleaved buffer, where the row below reports the largest sample. The ITU-R BS.1770-4 Annex 2 filter at 4×, which is what makes a reading dBTP; C and wasm both return the amplitude, there being one number to return |
| `clausters_core_loudness` | `loudness` | `idiom` — the **loudness** of an interleaved buffer as ITU-R BS.1770 and EBU R 128 measure it: integrated, range, and the maximum momentary and short-term readings. C writes the four through an out pointer and takes the weights as a nullable pointer; wasm returns them as an array and takes the weights as an optional one — the split `clausters_core_stats` makes. The streaming `LoudnessMeter` has no door on either side: a live meter runs in the host, a Rust caller of the core |
| `clausters_core_stats` | `channel_stats` | `idiom` — the peak and RMS of one channel of an interleaved buffer; C writes the pair through an out pointer, wasm returns it as a two-element array |
| `clausters_core_correlation` | `correlation` | |
| `clausters_core_shape_value` | `shape_value` | `idiom` — one envelope segment's level at a position, SuperCollider's shape numbers, what `EnvGen` plays and `pseg` reads; the same five scalars on both sides |
| `clausters_core_curve_axis` | `curve_axis` | `idiom` — the axis a break-point curve is *drawn* against: its range with a tenth of headroom, held and only widened once a view has one. C reads the kept axis out of the same `out` pointer it writes (a flag says whether to), wasm takes it as two optional arguments and returns the pair — the same split `clausters_core_stats` makes, and for the same reason. It is a **drawing** rule in the core because it was already written twice, and a range recomputed per redraw makes an edit rescale the picture |
| `clausters_core_lissajous` | `lissajous` | |
| `clausters_core_hz_to_mel` | `hz_to_mel` | the perceptual axes a spectrogram labels. Both clients spell the pair `cpsmel`/`melcps`, following SuperCollider's `<from><to>` conversions (`midicps`, `cpsoct`) — the core keeps the `hz_to_*` names because that is what the GUI host's ruler calls |
| `clausters_core_mel_to_hz` | `mel_to_hz` | |
| `clausters_core_hz_to_bark` | `hz_to_bark` | as above, spelled `cpsbark`/`barkcps` |
| `clausters_core_bark_to_hz` | `bark_to_hz` | |

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

## An event's keys

| C ABI | wasm | Note |
|---|---|---|
| `clausters_core_midinote_to_degree` | `midinote_to_degree` | `idiom` — C writes `[degree, alter]` into an out array; wasm returns it |
| `clausters_core_split_degree` | `split_degree` | `idiom` — the same out array against a returned pair |
| `clausters_core_pitch_resolve` | `pitch_resolve` | `idiom` — the same out array against a returned pair |
| `clausters_core_pitch_set` | `pitch_set` | `idiom` — C rewrites the six keys in place; wasm returns them rewritten |
| `clausters_core_level_resolve` | `level_resolve` | `idiom` — the same out array against a returned pair |
| `clausters_core_level_set` | `level_set` | `idiom` — C rewrites the three keys in place; wasm returns them rewritten |
| `clausters_core_amp_of_velocity` | `amp_of_velocity` | |
| `clausters_core_velocity_of_amp` | `velocity_of_amp` | |
| `clausters_core_event_delta` | `event_delta` | |
| `clausters_core_event_sustain` | `event_sustain` | |
| `clausters_core_articulation` | `articulation` | `idiom` — what a voice's joins write on a page (ties and slurs): C reads `{"pitches", "joins"}` as one JSON request and fills a buffer; wasm takes the pitches and the joins flat, as typed arrays, and returns the JSON |
| `clausters_core_event_synth` | `event_synth` | `idiom` — C reads `{"event", "node"}` as one JSON request and fills a buffer; wasm takes the event's JSON and the node as two arguments and returns the string |
| `clausters_core_event_node_note` | `event_node_note` | `idiom` — C reads `{"event", "note"}` as one JSON request and fills a buffer; wasm takes the event's JSON and the note id as two arguments and returns the string |
| `clausters_core_event_midi` | `event_midi` | `idiom` — the same request/argument split, with the channel |
| `clausters_core_event_of_midi` | `event_of_midi` | `idiom` — C reads `{"bytes"}` as JSON; wasm takes the bytes as a `Uint8Array` |
| `clausters_core_event_notation_keys` | `event_notation_keys` | |
| `clausters_core_event_written_midinote` | `event_written_midinote` | |

## Randomness

| C ABI | wasm | Note |
|---|---|---|
| `clausters_rng_seed` | `JsRng.new` | `idiom` — C mixes a seed into a state the caller then steps; wasm wraps the state in an object |
| `clausters_rng_next_f64` | `JsRng.next_f64` | |
| `clausters_rng_next_below` | `JsRng.next_below` | |
| `clausters_rng_next_u64` | — | `n/a` — JavaScript has no `u64` to hand back; the float reads are the surface there |
| `clausters_rng_draw` | `JsRng.draw` | `idiom` — the distributions (`rand`, `rrand`, `exprand`, `linrand`, `coin`, `irand`, `gauss`, …): C takes the distribution by number, wasm by name |
| `clausters_rng_windex` | `JsRng.windex` | |
| `clausters_rng_tablerand` | `JsRng.tablerand` | |
| `clausters_rng_scramble` | `JsRng.scramble` | `idiom` — C writes the permutation through an out pointer, wasm returns it |
| `clausters_rng_sputter` | `JsRng.sputter` | `idiom` — C writes the indices into a buffer of `maxlen` and returns how many, wasm returns them |
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

## The tempo map

The beat↔second map under a tempo that changes as it goes — the integral
of `1/tempo`, which is what makes a length in beats meaningless without a
position. One implementation, every client bound to it.

| C ABI | wasm | Note |
|---|---|---|
| `clausters_tempomap_new` | `JsTempoMap.new` | |
| `clausters_tempomap_anchored` | `JsTempoMap.anchored` | |
| `clausters_tempomap_free` | — | `idiom` — wasm frees by `Drop` |
| `clausters_tempomap_clone` | `JsTempoMap.copy` | `idiom` — `clone` is taken by JS object semantics, so the method says `copy` |
| `clausters_tempomap_secs_at` | `JsTempoMap.secs_at` | |
| `clausters_tempomap_beats_at` | `JsTempoMap.beats_at` | |
| `clausters_tempomap_tempo_at` | `JsTempoMap.tempo_at` | |
| `clausters_tempomap_span_secs` | `JsTempoMap.span_secs` | |
| `clausters_tempomap_span_beats` | `JsTempoMap.span_beats` | |
| `clausters_tempomap_push` | `JsTempoMap.push` | |
| `clausters_tempomap_ramp` | `JsTempoMap.ramp` | |
| `clausters_tempomap_shaped` | `JsTempoMap.shaped` | |
| `clausters_tempomap_env` | `JsTempoMap.env` | `idiom` — C takes four parallel arrays and a length, wasm takes four `Vec`s |
| `clausters_tempomap_truncate_from` | `JsTempoMap.truncate_from` | |
| `clausters_tempomap_len` | `JsTempoMap.len` | |
| `clausters_tempomap_segment` | `JsTempoMap.segment` | `idiom` — C writes seven `f64`s into a caller buffer, wasm returns the array |
| `clausters_tempomap_last` | `JsTempoMap.last` | `idiom` — as above, three `f64`s |
| `clausters_tempomap_version` | `JsTempoMap.version` | `idiom` — a getter in JS; `f64` there rather than `u64`, so a caller compares with `!==` instead of carrying a BigInt across the boundary |
| `clausters_tempomap_dump` | `JsTempoMap.dump` | `idiom` — the map's breakpoints as JSON. C writes into a caller buffer and returns the size it needs (the `clausters_core_bundle_*` convention); wasm returns the string |
| `clausters_tempomap_load` | `JsTempoMap.load` | `idiom` — C takes pointer and length and answers null; wasm takes the string and answers `undefined` |
| `clausters_tempomap_from_changes` | `JsTempoMap.from_changes` | a map from a document's **authored** tempo entries plus the tempo a document that never said one leaves to its reader. Three decisions a reader of a document would otherwise take again per language: a ramp reaches the *next* entry (a stored breakpoint carries its far end so a segment answers without looking forward, and an entry a person edits cannot), the default is prepended when the first entry is past beat 0 (a map anchored there puts beat 0 at a negative second), and no entries at all is the default alone. The document crate cannot hold this — it depends on serde and nothing else, so it states what a document *says* about tempo and never what that means in time |
| — | `JsTempoMap.is_empty` | `idiom` — always false (a map holds a segment), spelled as `JsScheduler` does |

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
| `clausters_ids_new` | `JsIdSpaces.new` | `idiom` — **a client's id spaces**, sized from the server and sliced by a share: the node table's client range, the audio buses above the server's own outputs, both bus spaces clear of their GraphDef windows, the buffers, and the transports above the one addressed by number. One constructor with a `score` flag in C; two in wasm, because a constructor that cannot fail and one that can are two JS shapes. The policy every endpoint used to restate over its own registries, and had drifted on (two output buses by default against the server's output count) |
| `clausters_ids_new` | `JsIdSpaces.score` | `idiom` — the **offline score**'s spaces, whose node space never runs out: C passes `score` non-zero to the one constructor, wasm has a second one |
| `clausters_ids_free` | — | `idiom` — wasm frees by `Drop` |
| `clausters_ids_alloc` | `JsIdSpaces.alloc` | `idiom` — the space is a small integer in C and its name in wasm; exhaustion is `-1` in C and a thrown error in wasm |
| `clausters_ids_release` | `JsIdSpaces.release` | `idiom` — a double free is `-1` in C and a thrown error in wasm |
| `clausters_ids_narrow` | `JsIdSpaces.narrow` | `idiom` — **a narrower share, keeping what is held**: what a client does when a second client arrives on its server after it has allocated (a script opening a GUI host once its takes are loaded). Refused whole when a held id lies in the other half; `-1` in C, a thrown error in wasm |
| `clausters_ids_node_ended` | `JsIdSpaces.node_ended` | a node the server reports gone, taken back if it was this client's |
| `clausters_ids_contains` | `JsIdSpaces.contains` | `idiom` — as `alloc` for the space |
| `clausters_ids_in_use` | `JsIdSpaces.in_use` | `idiom` — as `alloc` for the space |
| `clausters_ids_share_of` | `share_of` | `idiom` — C writes the pair into an out-array, wasm returns it |
| — | `JsRegistry.unbounded` | **gap** — the capacity-less constructor |
| — | `JsRegistry.is_allocated` | **gap** — per-id occupancy read |
| — | `JsRegistry.base` | **gap** — the range's first id |

## The widget-id table

The GUI namespace's two doors over one occupancy map: the anonymous lease a
hand-built tree takes, and the **keyed** id a view asks for by naming what it
draws — the structure's identity in the history, the role the widget plays and
which one it is. A keyed name keeps its number for as long as it keeps being
drawn, which is what stops an edit-back in flight across a redraw from landing
on the wrong widget. It is one table rather than one per door because two id
spaces kept apart by arithmetic are two spaces that eventually overlap.

| C ABI | wasm | Note |
|---|---|---|
| `clausters_widgetids_new` | `JsWidgetIds.new` | |
| `clausters_widgetids_free` | — | `idiom` — wasm frees by `Drop` |
| — | `JsWidgetIds.unbounded` | **gap** — the capacity-less constructor, as `JsRegistry.unbounded` |
| `clausters_widgetids_owner` | `JsWidgetIds.owner` | a drawer's identity, handed out by the table — one table serves a whole host, and a drawer each client invented for itself would eventually collide |
| `clausters_widgetids_alloc` | `JsWidgetIds.alloc` | |
| `clausters_widgetids_release` | `JsWidgetIds.release` | |
| — | `JsWidgetIds.alloc_block` | `n/a` — a contiguous run of anonymous ids, for the widgets of a bundle a **page** mounts, so that they come from the same table the page's `GuiHost` names its windows from. No C caller mounts a bundle: natively the host mounts it itself, in Rust, from its own table |
| `clausters_widgetids_id_for` | `JsWidgetIds.id_for` | `idiom` — C takes each string as pointer + length and answers -1 for "no id"; wasm takes `&str` and answers `undefined` |
| `clausters_widgetids_id_of` | `JsWidgetIds.id_of` | `idiom` — as above |
| `clausters_widgetids_forget` | `JsWidgetIds.forget` | `idiom` — as above |
| `clausters_widgetids_begin` | `JsWidgetIds.begin` | |
| `clausters_widgetids_retire` | `JsWidgetIds.retire` | `idiom` — C writes the ids through an out pointer and refuses (-1, retiring nothing) when the buffer is too small, since retiring is destructive and an under-sized buffer must be caught before the ids are gone; wasm returns the array |
| `clausters_widgetids_named` | `JsWidgetIds.named` | the bound a C caller sizes its `retire` buffer with |
| `clausters_widgetids_in_use` | `JsWidgetIds.in_use` | |
| `clausters_widgetids_contains` | `JsWidgetIds.contains` | |
| `clausters_widgetids_clear` | `JsWidgetIds.clear` | |

## Peak pyramids

| C ABI | wasm | Note |
|---|---|---|
| `clausters_core_peaks_build` | `JsPyramid.build` | `idiom` — wasm has one builder, which takes a channel count |
| `clausters_core_peaks_multi_build` | `JsPyramid.build` | `idiom` — as above |
| `clausters_core_peaks_cache_size` | — | `idiom` — C sizes the buffer, then fills it; wasm returns the bytes |
| `clausters_core_peaks_multi_cache_size` | — | `idiom` — as above |
| `clausters_core_peaks_multi_update` | `JsPyramid.update_range` | both — the span an edit touched, re-summarized; C rewrites the caller's cache bytes in place, wasm mutates the handle it already holds |
| `clausters_core_peaks_multi_empty` | `JsPyramid.empty` | both — the summary of a take allocated and not yet recorded into, so a client that will fill it from the stream does not allocate the take in silence to summarize it |
| `clausters_core_peaks_multi_write_buckets` | `JsPyramid.write_buckets` | both — the receiving half of `/buffer_stream`: buckets somebody else measured, folded in at an offset. Same split as the row above, and it is on both sides for the same reason the command is — a page cannot map the memory a recording fills, and a headless client that only listens cannot either |
| `clausters_core_stft_build` | `stft_cache` | both — the spectrogram cache of a mono take: the transform the GUI host draws from, as the bytes it maps. On both sides because the host reads the format on both — a mapped file natively, fetched bytes in a page — and until this nothing but the host's own crate could write it |
| `clausters_core_stft_cache_size` | — | `idiom` — C sizes the buffer, then fills it; wasm returns the bytes |
| — | `JsPyramid.from_bytes` | `idiom` — the two sides consume a cache differently: through the C ABI the pyramid is a byte blob whoever mapped it reads, while wasm keeps it as an object. The readers below are that object's face, and they answer **what the cache is** — never what it says: reading a cell, picking a level for a zoom or laying a row of columns is drawing, which is the GUI host's on both sides |
| — | `JsPyramid.to_bytes` | `idiom` — as above |
| — | `JsPyramid.frames` | `idiom` — as above |
| — | `JsPyramid.channels` | `idiom` — as above |
| — | `JsPyramid.base_bucket` | `idiom` — as above |
| — | `JsPyramid.num_levels` | `idiom` — as above |

## Bundles and patches

| C ABI | wasm | Note |
|---|---|---|
| `clausters_core_bundle_requirements` | `bundle_requirements` | |
| `clausters_core_bundle_resolve` | `bundle_resolve` | |
| `clausters_core_bundle_validate` | `bundle_validate` | |
| `clausters_core_patch_compile` | `patch_compile` | the cord→bus pass, one bus per connected net. Reached from a page since the multitrack editor was ported: a logical aggregate draws as a directed patch, and a cord drawn there is compiled by this rather than by a second answer written in TypeScript |

## Native plugins

What a plugin is as data, its catalog and a plugin def's reference
(`clausters_core::plugin_catalog`). Each door is one JSON request and one JSON
answer in both bindings, the same documents: a client reads and searches a
catalog and builds a reference through these, so the vocabulary, the search
and the port rule are the core's.

| C ABI | wasm | Note |
|---|---|---|
| `clausters_core_plugin_catalog` | `plugin_catalog` | `idiom` — C fills a buffer from a JSON request; wasm takes the request's JSON string and returns the answer's |
| `clausters_core_plugin_search` | `plugin_search` | `idiom` — the same |
| `clausters_core_plugin_reference` | `plugin_reference` | `idiom` — the same |
| `clausters_core_node_editor` | `node_editor` | `idiom` — the same |

## The document

One implementation of what an edit *means*, bound by every client rather than
re-derived per language. **The tree stays in Rust and only the intent and the
outcome cross**: a caller opens a document, applies intents to it, and asks for
the JSON when it actually wants the JSON.

That is not the shape this started with, and the change was forced by a
measurement rather than by taste. The first binding passed the whole document in
and took the whole new one back, on the reasoning that a handle would make every
accessor a tree has — and a tree has dozens — a call to design, bind and keep in
step. What that reasoning did not do was price "a serialization per edit": **205
ms** for one placement on a 10240-event multitrack (3.3 MB of JSON), against 6
ms on the 320-event one an example builds — linear in the whole document and
independent of the edit, so a destructive stroke touching fifty samples paid the
same as a clip drag.

The objection was to *accessor* handles, not to pointers, and this is not one:
it is the same three verbs the by-value binding had. What is preserved is the
property the decision was protecting — a client's document **is** the crate's
document rather than a parallel structure synchronizing with it — and it is
preserved more strictly than before, since there is now only one copy. A client
that wants the by-value convenience builds it in its own language out of open →
apply → snapshot → free, and pays the serialization where it asked for it.

| C ABI | wasm | Note |
|---|---|---|
| `clausters_document_open` | `JsDocument.new` | `idiom` — a constructor where C mints a handle |
| `clausters_document_free` | — | `n/a` — wasm frees by `Drop` |
| `clausters_document_sequence_new` | `JsEventSequence.new` | `idiom` — C returns a handle; wasm wraps the sequence in an object |
| `clausters_document_sequence_free` | — | `n/a` — wasm frees by `Drop` |
| `clausters_document_sequence_call` | `JsEventSequence.call` | `idiom` — C fills a buffer size-then-fill; wasm returns the string |
| `clausters_apps_editing_open_notes` | `JsEditing.open_notes` | `idiom` — C takes the two handles and fills a buffer size-then-fill; wasm takes the sequence object and returns the string |
| `clausters_apps_editing_open_score` | `JsEditing.open_score` | `idiom` — C takes the two handles and fills a buffer size-then-fill; wasm takes the score object and returns the string. The score is the one the caller opened (`clausters_score_open`, `JsScore.new`), shared with the editor, so a script reads every edit through it |
| `clausters_apps_editing_open_score_over` | `JsEditing.open_score_over` | `idiom` — C takes the three handles and fills a buffer size-then-fill; wasm takes the score and the sequence objects and returns the string. The score is loaded with the sequence read, and an edit on its page is an edit of the sequence, in the order a roll over it records in |
| `clausters_apps_editing_bind_engraver` | `JsEditing.bind_engraver` | `idiom` — C takes the two handles and fills a buffer size-then-fill; wasm takes the score object and returns the string. The score is one the caller makes for the multitrack editor to engrave its boxes' pages with: the engraver is a port a client has and the crate does not |
| `clausters_document_multitrack_new` | `JsMultitrack.new` | `idiom` — C returns a handle; wasm wraps the multitrack in an object |
| `clausters_document_multitrack_free` | — | `n/a` — wasm frees by `Drop` |
| `clausters_document_multitrack_call` | `JsMultitrack.call` | `idiom` — C fills a buffer size-then-fill; wasm returns the string |
| `clausters_apps_editing_open_multitrack` | `JsEditing.open_multitrack` | `idiom` — C takes the two handles and fills a buffer size-then-fill; wasm takes the multitrack object and returns the string |
| `clausters_apps_editing_bind_sequence` | `JsEditing.bind_sequence` | `idiom` — C takes the two handles and fills a buffer size-then-fill; wasm takes the sequence object and returns the string |
| `clausters_document_apply` | `JsDocument.apply` | |
| `clausters_document_resolve` | `JsDocument.resolve` | |
| `clausters_document_snapshot` | `JsDocument.snapshot` | the one call still the size of the document, and asked for rather than paid per edit |
| `clausters_document_version` | `JsDocument.version` | `idiom` — a getter |
| `clausters_document_inverse` | `JsDocument.inverse` | the edit that would put a node back, read before anything lands — what a caller recording its **own** entry needs |
| `clausters_document_coalesce_key` | `JsDocument.coalesce_key` | `idiom` — a free function in C, a static on the class in JS; it is the *multitrack's* sentence for "the same thing done the same way", so it lives with the document and not with the history. The Python client offers **both** spellings (`document_coalesce_key` and `Document.coalesce_key`, as it does for `inverse`), so a reader who learned one client's shape finds it in the other |
| `clausters_domain_coalesce_key` | `domain_coalesce_key` | the same sentence for **any** vocabulary the crate speaks — the multitrack, a curve, a span of samples, a timeline. Free in both, since a caller across either seam holds no structure to ask: a curve, a span of samples and a timeline live in the caller's own memory and only their *vocabulary* is the crate's. An unknown domain, or a payload not written in that one, answers nothing (`0` in C, `""` in JS) |
| `clausters_domain_edit` | `domain_edit` | applying a payload to a structure held **as its own state**, answering what it now is and the payload that puts it back — both in one call, since the inverse has to be read before the edit lands. The other half of the row above: the coalesce sentence and the inverse are one vocabulary's rule, and a client computing the second itself is what the first was given a door to prevent. Two domains answer nothing on purpose — the multitrack's tree (it needs a version and a grid, and has `clausters_document_apply`) and a span of samples (a borrowed view whose frames are in a buffer, never in a string) |
| `clausters_domain_edit_all` | `domain_edit_all` | the edits one gesture came to, applied **as one: all of them or none**, with the payload that puts back each that landed. The rule is the crate's so no handler spells it: an edit refused, or unreadable, refuses the whole gesture and hands the state back unchanged, and a gesture half applied is never left standing. The same two domains answer nothing |
| `clausters_session_format` | `session_format` | the session format this build writes (the crate's `session::FORMAT`). Both clients keep the number as a literal, because a `Session` is plain data and writing one must not need a native load or an `await loadCore()` — so what keeps the two literals the crate's is a **test** in each client that asks this and compares. That check did not exist when joins moved the format to 2 and both clients went on stamping 1 onto files that could carry a source whose samples are spans of other sources |
| `clausters_session_migrate` | `session_migrate` | a session written in an older format, as this build writes it — the crate's `session::migrate` over the session's JSON. Format 2 placed the multitrack in beats and 3 places it in seconds, so each client's `Session.read` calls this for an older file and every reader opens it through one migration. C sizes and fills a buffer where wasm returns the string |
| `clausters_view_not_an_edit` | `view_not_an_edit` | the `/gui_event` tags that are **screen state** rather than edits of the structure — what a view is looking at, and where the hand is. A client routes an event by its tag, answers these generically and lets nothing else near a domain; which tags they are was a literal list in each client's editor until this row existed, and a table that small is exactly the kind that drifts unread, since a tag missing from one client makes it *edit* with a gesture the other one merely looks at. What a client does with them stays the client's — screen state is each window's by the four-layer rule and the crate holds none of it. Read once and kept, never called per event: it answers a constant |
| `clausters_view_props` | `view_props` | the widget a **catalogue view** is, and what is on it: `"waveform"` is a `signal` drawn as a `trace` with the sample editor's three-gesture plan, `"bpf"` is a `curve` on the axis it is handed, `"pianoroll"` is `notes` with the pitch window fitted to them; a kind it does not draw, or facts that do not read, answer `{"error": reason}` naming the field, and both clients raise it. Each was assembled three times — once per client and once more in the standalone host — and the three agreed only because they were written to. One door with the kind named rather than one symbol per view, as `domain_edit` names its vocabulary, so a view the crate learns to draw needs no new symbol. The answer carries no `id`: which number a widget gets is the caller's, and nothing here knows it |
| `clausters_mixer_defs` | `mixer_defs` | the defs a multitrack of these widths is **played by** — the reader, the channel strip, and the clip, track and multitrack graphs that are that strip over readers, over clips and over tracks. One shape three times, which is why a clip's gain and a track's gain are both real and are different stages. Here rather than per client for the reason `multitrack_picture` is: two clients writing their own channel strips is two mixers, and the same multitrack would sound different in two places with nothing to point at. The lists come back in send order, because a graph naming one that has not been sent is a failure in another process at instantiation time. A width past stereo answers nothing on purpose — a downmix is a table, and which table is a decision |
| `clausters_multitrack_plan` | `multitrack_plan` | **which** of those a given multitrack needs, wired to which buffer, at which frame, with which level. Three rules live in it and each was written twice before it did: seconds crossed to frames at the rate, the source's width picking the clip's wiring (a mono take is panned into the track and a stereo one is balanced), and what a solo anywhere does to everything else — which the document deliberately does not hold, since it records that a track was *marked* and what a mark does to the others is a mixer's question. The source table is the caller's: a buffer number is a running server's fact and not a document's, so a region whose source nobody can find is left out rather than planned as silence

**Size-then-fill needs two rules here**, because the surface mutates. A mutating
call commits **only when the bytes are written**, so a sizing pass changes
nothing and repeating one is harmless — which matters more now than it did, since
the tree is no longer the caller's to compare against. A **pure read caches**
between the pair instead: `snapshot` keeps what the sizing pass serialized and
the fill copies it out, so a document is serialized once per pair rather than
twice. Caching a mutating call that way would be wrong — the mutation would land
on the sizing pass, and a caller that sized and then gave up would have edited
without knowing.

The **history** crosses as a handle too, for its own reason: a bulk payload
*leaves* the pile for its spill store on purpose, so passing one by value would
carry every spilled span on every call, which is the cost spilling exists to
avoid. One handle is one editing context — the structures registered in it and
one ordered pile over them — so what a caller decides by choosing a handle is
what shares an undo order. `register` mints a structure's identity and every
call that names one takes it.

Three things the pile refuses to promise, and says so rather than pretending:
an act with **no inverse** is recorded with no `backward`, kept, marked, and
walked past in both directions with the walk naming it in `skipped`; deleting a
structure (`forget`) invalidates the entries that name it and **defers the
free**, since undoing a deletion has to be able to give the data back —
`released` is what says the last entry holding it has retired; and the **save
mark** (`mark_saved`, `dirty`, `saved_reachable`) is the pile's, because a save
is an event of the whole editing context.

`apply` is the multitrack's door alone, and takes the document handle: it is
the one state this surface can reach, and applying and recording are one call
because the inverse has to be read before the edit lands. Everything else —
a destructive write, and every domain that is not the multitrack — is applied
by the caller and recorded through `record`. `undo` and `redo` apply nothing and
take no document: they hand back the payloads with the structure each belongs
to, in the order they must be applied.

| C ABI | wasm | Note |
|---|---|---|
| `clausters_history_new` | `JsHistory.new` | `idiom` — a constructor where C mints a handle |
| `clausters_history_free` | — | `n/a` — wasm frees by `Drop` |
| `clausters_history_register` | `JsHistory.register` | |
| `clausters_history_apply` | `JsHistory.apply` | |
| `clausters_history_record` | `JsHistory.record` | `idiom` — C returns 0/-1, wasm a boolean. One call carries the whole entry, legs and all: several legs are one transaction, and half of one is worse than none |
| `clausters_history_walk` | `JsHistory.walk` | one step of the pile, **routed**: the legs each structure has to apply, in order, and what only its owner can re-run. One door with the direction named rather than an `undo` and a `redo`, because what a caller was writing per client was two rules and not plumbing — which side of an entry a direction reads, and which legs a structure owns. Four callers wrote both (two editing clients and both document logs). The order kept is the order *within* a structure, since a caller applies through one vocabulary at a time |
| `clausters_history_can_undo` | `JsHistory.can_undo` | `idiom` — a getter |
| `clausters_history_can_redo` | `JsHistory.can_redo` | `idiom` — a getter |
| `clausters_history_undo_label` | `JsHistory.undo_label` | `idiom` — a getter returning a string, where C sizes and fills |
| `clausters_history_redo_label` | `JsHistory.redo_label` | `idiom` — as above |
| `clausters_history_len` | `JsHistory.len` | `idiom` — a getter |
| — | `JsHistory.is_empty` | `idiom` — `len == 0`, as `JsScheduler` and `JsRegistry` already spell it |
| `clausters_history_forget` | `JsHistory.forget` | the data behind a structure is gone; the answer says whether its memory may go now |
| `clausters_history_released` | `JsHistory.released` | `idiom` — C sizes and fills a JSON array, wasm returns one. Drains: each is reported once |
| `clausters_history_mark_saved` | `JsHistory.mark_saved` | |
| `clausters_history_dirty` | `JsHistory.dirty` | `idiom` — a getter |
| `clausters_history_saved_reachable` | `JsHistory.saved_reachable` | `idiom` — a getter |
| `clausters_history_clear` | `JsHistory.clear` | |

## The projections

What an editable structure owes its three endpoints, out of `clausters-editing`:
the props a host draws it with, the payloads an edit becomes, the operations
that make a server sound it. One implementation, so a page and a script send
the same props for the same structure — a projection written twice is how one
curve comes to be drawn two ways, and it is not a difference any compiler or
test sees.

**The rule and the projection are different layers and both are here.**
`clausters_core_curve_axis` above answers what a curve is *drawn against*;
`clausters_editing_points_props` assembles the whole payload a `bpf` widget is
set with, axis included. The rule stays in the core because it serves views
that are not projections (a clip's curve body); the assembly is a projection and
sits over it. Neither client's points editor calls either door any more: the
editor is the crate's application (`clausters_apps::points`), which calls the
projection itself, so both doors are kept for a caller drawing a curve of its
own.

| C ABI (`clausters-ffi`) | wasm (`clausters-core-web`) | Note |
|---|---|---|
| `clausters_editing_conversation_read` | `conversation_read` | `idiom` — **what one message from the host is**: a close, a history step, an edit made against a picture that is gone, or an edit to route. Takes the conversation's two integers and the event's *envelope* — never the payload, which crosses once through the intake — and hands back the turn and the two integers as they now stand. Pure, so a client keeps the pair and nothing else |
| `clausters_editing_conversation_answer` | `conversation_answer` | `idiom` — **what to answer the host with**: `silent`, `ack` or `push`. There is no success flag in it — applied, transformed and refused are one message, and a refusal is simply the previous value among the corrections |
| `clausters_editing_multitrack_names` | `multitrack_names` | `idiom` — what a multitrack calls its rows, its boxes and its **curves**, by the names the wire carries them under. The minting correction's half that is a fact about the multitrack: a host that made a track or split a box minted the *word* while the document minted the *id*. The curves are the other half of the same fact and were missing from it: an automation the **owner** made is one the host cannot have drawn, so a toggle asking a track for its gain curve changed nothing on screen until the next gesture that added a row happened to fire the correction |
| `clausters_editing_instance_new` | `JsInstance.new` | `idiom` — C hands back an opaque pointer, wasm a class; the instance projection is the one with **state**, so it is a handle rather than a function |
| `clausters_editing_instance_free` | `JsInstance` `Drop` | `idiom` — wasm frees by `Drop`, C needs an explicit `_free`. It frees the bookkeeping and not the nodes: a caller that means to stop the sound tears down first |
| `clausters_editing_instance_reconcile` | `JsInstance.reconcile` | `idiom` — what is sounding against what the multitrack says, as the JSON list of operations a client applies. Takes the same three arguments the instance plan does, plus the master's own level; C sizes and fills a buffer where wasm returns the string. An operation names what it acts on by a **handle**, never by a node id, a bus index or a buffer number — this allocates none of those |
| `clausters_editing_instance_teardown` | `JsInstance.teardown` | `idiom` — the operations that give back everything it made |
| `clausters_editing_instance_meters` | `JsInstance.meters` | `idiom` — which control bus run each track's meters write, by track |
| `clausters_editing_default_tempo` | `editing_default_tempo` | `idiom` — the tempo, in beats per second, a multitrack that states none is drawn at: what its ruler reads, and nothing it places; a plain function on both sides |
| `clausters_editing_runner_new` | `JsStepRunner.new` | `idiom` — C hands back an opaque pointer, wasm a class; **steps being carried out**: the one queue a playback's answers are walked through, whatever waits on the reply |
| `clausters_editing_runner_free` | `n/a` | wasm frees by `Drop` |
| `clausters_editing_runner_call` | `JsStepRunner.call` | `idiom` — **one verb of the runner**, as JSON: `push` steps for a server, what is `ready` to go out (the awaited message last, with what it awaits), a `reply` offered back, and whether it is `idle`. **One door rather than one per verb**, as the editor's. A script blocks on the last message's reply and a page awaits it; both hand the reply back the same way. C sizes and fills a buffer where wasm returns the string |
| `clausters_editing_audio_playback_new` | `JsAudioEditorPlayback.new` | `idiom` — C hands back an opaque pointer, wasm a class; **the audio editor as it is playing** — its structure, its open files and its own transport, answering every verb as steps. It takes the chunk a fill may carry and the transport, negative for one of its own, which its `open` verb or its first sync takes from the id spaces and its `close` gives back |
| `clausters_editing_audio_playback_free` | `JsAudioEditorPlayback` `Drop` | `idiom` — wasm frees by `Drop`, C needs an explicit `_free`; the bookkeeping, not the nodes |
| `clausters_editing_audio_playback_call` | `JsAudioEditorPlayback.call` | `idiom` — one JSON door for every verb (`sync`, `closeFile`, `play`, `resume`, `pause`, `stop`, `locate`, `cue`, `close`, `setRolling`, `state`, `space`), allocating from the client's id spaces; C sizes and fills (changing the playback and the spaces only on the fill) where wasm returns the string |
| `clausters_editing_notes_playback_new` | `JsNotesPlayback.new` | `idiom` — C returns a handle; wasm wraps the playback in an object. It takes the transport, negative for one of its own, which its `open` verb or its first play takes from the id spaces and its `close` gives back — one playback per sequence, so two sequences sound together |
| `clausters_editing_notes_playback_free` | — | `n/a` — wasm frees by `Drop` |
| `clausters_editing_notes_playback_call` | `JsNotesPlayback.call` | `idiom` — the notes editor's playback, every verb as steps over the sequence handed in; C fills a buffer size-then-fill, running on a copy so the sizing pass changes nothing, and wasm returns the string |
| `clausters_editing_playback_new` | `JsMultitrackPlayback.new` | `idiom` — C hands back an opaque pointer, wasm a class; **one multitrack as it is playing** — its instance, its applier and its transport, answering every verb as steps. It takes only the chunk a fill may carry: the transport's group, made at the top and following the transport, with the multitrack inside it and its tracks' group governed, is the same for every endpoint |
| `clausters_editing_playback_free` | `JsMultitrackPlayback` `Drop` | `idiom` — wasm frees by `Drop`, C needs an explicit `_free`; the bookkeeping, not the nodes |
| `clausters_editing_playback_sync` | `JsMultitrackPlayback.sync` | `idiom` — the steps that make what sounds be what the multitrack says, allocating from the client's id spaces; C sizes and fills (changing the playback and the spaces only on the fill) where wasm returns the string |
| `clausters_editing_playback_notes` | `JsMultitrackPlayback.notes` | `idiom` — the notes regions as the event lane's data, as steps; C sizes and fills where wasm returns the string |
| `clausters_editing_playback_play` | `JsMultitrackPlayback.play` | `idiom` — the steps that roll the transport |
| `clausters_editing_playback_pause` | `JsMultitrackPlayback.pause` | `idiom` — the steps that freeze the multitrack and zero its meters |
| `clausters_editing_playback_stop` | `JsMultitrackPlayback.stop` | `idiom` — the steps that halt and go back to the mark |
| `clausters_editing_playback_locate` | `JsMultitrackPlayback.locate` | `idiom` — the steps that put the transport at a second of the multitrack |
| `clausters_editing_playback_cue` | `JsMultitrackPlayback.cue` | `idiom` — a locate for a stopped transport, nothing for a rolling one |
| `clausters_editing_playback_play_pass` | `JsMultitrackPlayback.play_pass` | `idiom` — the steps of the space bar's play: from a time range's start to its end, or the loop switch over the range or the whole multitrack, the pass as JSON text |
| `clausters_editing_playback_set_loop` | `JsMultitrackPlayback.set_loop` | `idiom` — the steps the loop switch asks while the multitrack plays: the pass in progress loops over the range or the whole multitrack, or goes on to its end, the pass as JSON text |
| `clausters_editing_playback_set_end` | `JsMultitrackPlayback.set_end` | `idiom` — the steps that set where a pass ends, back to the position cursor: `null` (open), `"contents"` or seconds (an end marker), as JSON text |
| `clausters_editing_playback_close` | `JsMultitrackPlayback.close` | `idiom` — the steps that free everything the muece made |
| `clausters_editing_playback_meters` | `JsMultitrackPlayback.meters` | `idiom` — the meter bus runs by track, as JSON |
| `clausters_editing_playback_set_rolling` | `JsMultitrackPlayback.set_rolling` | `idiom` — whether the transport is rolling, when the caller learned it from the engine |
| `clausters_editing_playback_rolling` | `JsMultitrackPlayback.rolling` | `idiom` — whether the transport was last told to roll; C answers 1 or 0 |
| `clausters_editing_playback_transport` | `JsMultitrackPlayback.transport` | `idiom` — the transport the multitrack plays on, which its commands name and its play cursor is drawn from |
| `clausters_editing_playback_end` | `JsMultitrackPlayback.end` | `idiom` — where a pass ends, as that JSON; C sizes and fills where wasm returns the string |
| `clausters_editing_playback_secs_to_samples` | `JsMultitrackPlayback.secs_to_samples` | `idiom` — a second of the multitrack as a sample, at the rate it was planned at; wasm answers a double |
| `clausters_editing_playback_samples_to_secs` | `JsMultitrackPlayback.samples_to_secs` | `idiom` — a sample as a second of the multitrack |
| `clausters_editing_intake` | `editing_intake` | `idiom` — **what a gesture means**, in a structure's own vocabulary: a tag and a flat list of values become the payloads an edit is. One door over all four domains (`points`, `samples`, `events`, `multitrack`), because a host reports every gesture the same way — and because a fifth vocabulary then has nowhere to grow. Both take the domain, the tag and one JSON request; C sizes and fills a buffer where wasm returns the string |
| `clausters_editing_stitch` | `editing_stitch` | `idiom` — **what a source made of spans comes to**: the buffer a join is, as `/buffer_stitch` takes it — the width, the rate, the length, and each part's buffer, span, fades and full channel map (a part narrower than the join repeats). One reading for every endpoint that builds a minted join: the GUI host at open and at edit, and both clients at edit, which had read the recipe three ways and drifted on the channel map. Both take the source entry and the caller's table as JSON; C sizes and fills a buffer where wasm returns the string |
| `clausters_editing_load` | `editing_load` | `idiom` — **a session's sources, loaded**: every take read and every join stitched into buffer numbers the caller set aside, as steps for the runner — each read and each stitch followed by the `/done` of its buffer, so a join waits for the takes it is over. The GUI host plans the same load and walked it alone, which left a saved session one only it could open and sound. Both take `{"session", "beside", "buffers"}` and answer what each source became, the steps, what will not load and why, and the numbers left over; the door assumes every file is there, since a client cannot see its server's disk and a page never can, so a missing file is the server's refusal of the read. C sizes and fills a buffer where wasm returns the string |
| `clausters_editing_event_curves` | `editing_event_curves` | `idiom` — **a played event's curves, as the values a client sends**: the `/node_set`s of every instant of one window, for the notes that sound and the channels' curves over them. An event a client plays has no lane to read its curves on the server, so the client sets the controls they drive, and what each value is — a note's own curve over a channel's, a bend adding the two onto the note's frequency, a channel's curve glided as the lane's reader glides it — is this one function's rather than each client's. A curve reaches a note from its start to its off, the last instant a client knows the node is there. Stateless: a channel's glide goes out in the answer and comes back in the next request. Both take the request as JSON; C sizes and fills a buffer where wasm returns the string |
| `clausters_editing_multitrack_props` | `multitrack_props` | `idiom` — a multitrack as the props its widget is drawn with: the rows, the boxes, the automations over both, their break-points, which are hidden and which boxes loop. Both take the multitrack and the source table as JSON; C sizes and fills a buffer where wasm returns the string. It takes **the same source table the instance plan does** rather than a second one shaped for drawing — what a box is drawn from and what it is played from are the same samples |
| `clausters_editing_points_props` | `points_props` | `idiom` — the props a break-point curve is drawn with. C sizes and fills a JSON buffer and takes the kept axis as a flag plus two `f64`s; wasm returns the JSON string and takes the kept axis as optional arguments — the same split `clausters_core_curve_axis` makes, and for the same reason |

## The applications

What an editor opens and what it answers, out of `clausters-apps`. A projection
is a function of one structure; an application is what puts a structure beside
the controls that act on it — the window, and the props every widget of it is
corrected with. Each client composed that window for itself and the standalone
host composed a third, so one editor looked three ways.

| C ABI (`clausters-ffi`) | wasm (`clausters-core-web`) | Note |
|---|---|---|
| `clausters_apps_editing_new` | `JsEditing.new` | `idiom` — **an editing context**: one undo order over every editor opened in it. No application holds a history of its own; one opened alone is a context of one. It carries no transport bound: a step hands a take's writes back as payloads, and the member turns them into steps with the bound of the server the take is on |
| `clausters_apps_editing_free` | `n/a` | wasm frees by `Drop`; freeing a context frees the editors opened in it |
| `clausters_apps_editing_call` | `JsEditing.call` | `idiom` — **one verb of the context**, as JSON: open a multitrack or an audio editor under a key and answer its member and structure (a second member with the same key is the same structure, and a client names that structure's widgets by it), take in an `external` structure the crate does not apply, read and record a member's `event`, `step` the history, `record` an external member's entry, limit the `bytes` of takes only the history holds, reach a `member`'s own verbs, and read the `state`. A turn and a step answer the takes nothing reaches any more as `freed`, under the member whose server they are on. **C runs a verb once** across its sizing and filling calls, keeping the answer between them, because a history is not copied for a sizing pass the way an editor is; wasm returns the string |

The context also exposes internal service verbs through that same JSON door: `resources` reads registered source metadata; `retain` replaces a consumer root; `retainMember` retains a member's current sources for playback or pending work; `dropRoot` ends that lease and returns release/storage effects; `invalidate` forgets one server incarnation without freeing its reused ids. `openAudio` accepts `server: {connection, generation}`. Resource ids are `{server: {connection, generation}, buffer}`; roots are `{kind, owner}`, with independent `contents`, `history`, `clipboard`, `playback` and `pending` kinds. Audio history effects include `file: {id, display, channels, frames, rate, preview}` so an adapter updates the edited object's playback even when another file is viewed. These extend the existing JSON payload; C and wasm still call the same Rust implementation.

The same door's internal `exchange` verb reaches a clipboard service without opening an editor. It takes `action: publish|event|acquire|release|clear|invalidate` and `server: {connection, generation}`. `publish` receives a completed copy's `document` and returns its immutable scoped `document`; `event` receives GUI `args`, returns validated/transformed `args` and an optional reader `lease`; `acquire` accepts a document directly. `release` ends a reader lease, and `clear` ends publication ownership. Every answer includes `released: [{server, buffer}]` for the adapter to execute exactly once. Buffer documents add optional `location: {connection, generation, publication}`; application paste refuses an absent, foreign or expired location. `clipboardCut` in an audio outcome carries the `finish_cut` continuation, file and contents checkpoint; the adapter submits it through the existing `act` door only after copy/publication succeed. No C symbol or wasm export is added.

| `clausters_apps_samples_measures` | `samples_measures` | a **visual layer stack, checked**: names or objects (`draw`, `visible`, `solo`, `alpha`, `y`), returned in canonical array form, or the reason it is refused. The existing door also checks that different domains do not claim one axis. A stack is written by hand, so a typo is a layer that quietly does not appear. C sizes and fills a buffer where wasm returns the string |

The audio member's existing JSON call door also carries `layer` (active edit
body), and `window` accepts a caller-named `pane` id. These use the same
correction/Outcome path as visual `layers`; no binary symbol is added.

Audio `binding` also answers a stable instance-local file `id`, distinct from
its `display` buffer, and a view-binding `generation`. File-list entries and
`focus_file`/`close_file` use the object id; their public client methods still
accept file-list indices. Both clients use the id for playback association.
The existing `playing` JSON verb accepts the playback's `file` and `meters`
and answers corrections for the playhead, file mark and meter shape. No C
signature or wasm export changes.


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

The engraver used to be the one part of the core a browser could not have, and
the reason was packaging rather than design: `libverovio` is C++ and nothing
here built it for wasm. That changed on 2026-08-21. The pinned sources are now
compiled twice — natively by `third_party/build-verovio.sh` and by Emscripten
through `third_party/build-verovio-wasm.sh`, **with the same importer options** —
and the score model moved into `clausters_core::notation` over an `Engraver`
port, so both clients run one state machine over one engraver
(`docs/decisions.md`).

What that leaves in this table is a shape difference and nothing else: C hands
out an opaque handle and takes it back to every call, and wasm has a class.

| C ABI | wasm | Note |
|---|---|---|
| `clausters_core_svg_to_display_list` | `svg_to_display_list` | |
| `clausters_core_voice_to_mei` | `voice_to_mei` | |
| `clausters_core_voice_to_sheet` | `voice_to_sheet` | `idiom` — the envelope-versus-throw difference below. The bridge from the v1 wire form to the model: the reduction to slots stays in each client, because it reads client-native types, and everything above the slot is one implementation |
| `clausters_core_sheet_apply` | `sheet_apply` | `idiom` — C answers in an envelope (`{"ok": …}` / `{"error": …}`), wasm throws. Same behaviour, and a refusal keeps its reason either way. **One symbol for every operation there will ever be**: the verb is inside the payload, so the table below cannot see the verbs — that is what `clausters_core_sheet_ops` is contrasted against |
| `clausters_core_sheet_to_mei` | `sheet_to_mei` | `idiom` — the same envelope-versus-throw difference |
| `clausters_core_sheet_ops` | `sheet_ops` | the operation catalog. **The verbs it lists are not symbols and are not in this table**: each client offers one helper per catalog entry, named in that language's spelling — `set_meter` is `setMeter`, and `delete` is `del`, since it is a reserved word in JavaScript. Casing is idiom; the one renamed verb is written into each client's own contrast test so a *missing* verb still fails. the operation catalog. It exists **because** this table is blind to a data-carried verb: each client is read against this list, so an operation that reaches only one of them fails a test there instead of drifting |
| `clausters_core_mei_to_sheet` | `mei_to_sheet` | `idiom` — the same envelope-versus-throw difference. The **other** return path: `sheet_perform` turns a model into sound, this turns a *document* into a model, which is what a score opened from typed text needs before any verb can touch it. One input format rather than four, because the engraver normalizes whatever it loaded to MEI |
| `clausters_core_sheet_perform` | `sheet_perform` | `idiom` — the same envelope-versus-throw difference. The path back out of the score: what the symbols *mean*, read into sounding notes. Each note carries **two lengths** (`dur` written, `sustain` heard) and the `staff`/`voice` it came from, which is what a caller binds an instrument to — the notation does not say what plays it |
| `clausters_core_sheet_render_events` | `sheet_render_events` | `idiom` — the same envelope-versus-throw difference. A sheet as the sequence that plays it, one way: events with their notation keys, a channel to a voice, the dynamics as lanes, and the `notation` section. |
| `clausters_core_sheet_read_events` | `sheet_read_events` | `idiom` — the same envelope-versus-throw difference. The way back: a sequence read into a sheet, with which item each event became. What the events do not say is decided by the transcription that crosses with them as data — its fields are not symbols, and every one left out keeps its default |
| `clausters_core_item_id` | `item_id` | which **model item** an engraved element belongs to: `n7`, `n7-2` (a pare split across a barline) and `n7-p1` (one pitch of a chord) are all item 7. It is the step between a page's selection and a model verb, and it is a symbol rather than a line of each client because the answer is the emitter's — a client spelling it out again would disagree the first time the emitter spelled a split differently. `-1` where the element was not written from a model |
| `clausters_core_interpretation` | `interpretation` | the default reading, and **the parity surface for it**. Its fields are not symbols and are not in this table: they ride inside the payload `sheet_perform` takes, so nothing structural notices if one client's idea of `mf` drifts from the other's. Both clients are read against this value instead. It is also why neither writes those numbers down — a client with its own copy of the dynamics table plays the same score at a different amplitude |
| — | `engrave_options` | `idiom` — the engraver's options JSON. A native caller builds it from `EngraveOptions` inside `clausters-notation`, which is where its resource path also lives; a page has no such struct and calls the core's builder directly, so both configure verovio identically |
| `clausters_score_open` | `JsScore.new` | `idiom` — a constructor where C mints a handle. The engraver is built by the binding either way: natively from a resource path, in a page from the loaded module |
| `clausters_score_free` | — | `n/a` — wasm frees by `Drop` |
| `clausters_score_display_list` | `JsScore.display_list` | |
| `clausters_score_mei` | `JsScore.mei` | |
| `clausters_score_transpose` | `JsScore.transpose` | |
| `clausters_score_transpose_to` | `JsScore.transpose_to` | the absolute form, and the one an edit travels in: `transpose` is verovio's relative call and stays for a caller that has the delta |
| `clausters_score_edit` | `JsScore.edit` | |
| `clausters_score_apply` | `JsScore.apply` | the **edit path**: one model operation as one undo step. It is not the engraver's editor, because there is one implementation of what an edit to a score means and it is the vocabulary both clients already bind — a standalone host holding a sheet performs the same operation through the same code |
| `clausters_score_sheet` | `JsScore.sheet` | `idiom` — the envelope-versus-throw difference. The open document as the model, or a refusal when it could not be read into one, which is a state rather than a failure: the page still draws and still plays |
| `clausters_score_load` | `JsScore.load` | the door for an owner that keeps the order **somewhere else**: a client whose editing context holds one history over several structures records a score's edit as the MEI it produced and puts a previous one back through here, which is the same absolute, idempotent shape every other editable structure has. It clears the score's own stack, so one score has one history |
| `clausters_score_undo` | `JsScore.undo` | the score's own stack, for a caller with no such context — a standalone host holding a page and nothing else |
| `clausters_score_redo` | `JsScore.redo` | |
| `clausters_score_can_undo` | `JsScore.can_undo` | `idiom` — a getter |
| `clausters_score_can_redo` | `JsScore.can_redo` | `idiom` — a getter |

**The engraver itself is not in this table and cannot be**: it is not a core
symbol but a C++ library, reached through `tools/c_wrapper.h` in a process and
through `cwrap` over the same exports in a page. What holds *those* two together
is not this file but the pin: one commit, one set of build options, and
`clients/web/tests/notation-parity.test.ts`, which engraves the Python client's
fixtures in the browser stack and compares the drawing primitive by primitive.

## MIDI files

The one section whose C counterpart is **not in `clausters-ffi`**: the MIDI file
writers live in `clausters-midi`, a cdylib of its own with its own ABI counter,
because the Python client loads them separately and because half that crate
(`live`, a virtual OS port over midir) has no business inside the core's door.
The left column is therefore empty on both rows and the C symbols are named here
instead: `clausters_midi_write_smf`, `clausters_midi_write_smf_tempo`,
`clausters_midi_write_clip`, `clausters_midi_write_clip_ump`,
`clausters_midi_read_smf` and `clausters_midi_read_clip`, argument for
argument the same call.

What a page needs is exactly the half that is pure Rust. It has no virtual port
to open — Web MIDI hands out the ports that already exist and lets a page create
none — but it does have a score to write, so `MidiServer` over an NRT interface
accumulates `(beat, message)` and writes the file in the tab. The alternative was
a TypeScript SMF writer, which is a second implementation of a byte format, and
the whole point of a shared core is not having one.

The outgoing half of **MPE** crosses the same way and for the same reason: which
member channel a note of a zone goes on, and the bytes a zone and a note's
expression are written as, are rules, and a `MidiServer` that is a zone has to
put the same pattern on the same channels from a page and from a script.

| C ABI (`clausters-ffi`) | wasm (`clausters-core-web`) | Note |
|---|---|---|
| — | `midi_write_smf` | `idiom` — `clausters-midi`'s `clausters_midi_write_smf`, not `clausters-ffi`'s, so this table's left column cannot name it. Same flat arguments (n ticks, 3n bytes, ppq); C returns a malloc'd buffer freed by `clausters_midi_free`, wasm returns the bytes and JS sees it as `midiWriteSmf` |
| — | `midi_write_clip` | `idiom` — `clausters_midi_write_clip` on the same terms, `midiWriteClip` in JS |
| — | `midi_write_smf_tempo` | `idiom` — `clausters_midi_write_smf_tempo`: the writer with the file's tempo marks, as two flat arrays (ticks, microseconds per quarter); `midiWriteSmfTempo` in JS |
| — | `midi_read_smf` | `idiom` — `clausters_midi_read_smf`: a file read back as JSON, which C hands over in a malloc'd buffer and wasm as a string; `midiReadSmf` in JS |
| — | `midi_write_clip_ump` | `idiom` — `clausters_midi_write_clip_ump`: a MIDI 2.0 clip from UMP packets, flat as ticks, sizes and words; `midiWriteClipUmp` in JS |
| — | `midi_read_clip` | `idiom` — `clausters_midi_read_clip`: a clip read back as JSON, on the terms of the row above's reader; `midiReadClip` in JS |
| — | `mpe_zone_messages` | `idiom` — `clausters-midi`'s `clausters_mpe_zone_messages`: the messages that declare a zone. C fills a caller's buffer and returns the byte count; wasm returns the bytes, flat, three per message; `mpeZoneMessages` in JS |
| — | `mpe_expression_messages` | `idiom` — `clausters_mpe_expression_messages`: a note's bend, pressure and timbre ahead of its note-on, on the terms of the row above. C spells a dimension the note does not state as a negative number, wasm as an absent argument; `mpeExpressionMessages` in JS |
| — | `JsMpeAssigner.new` | `idiom` — `clausters_mpe_assigner_new`: which member channel an outgoing note goes on. C hands back a pointer freed by `clausters_mpe_assigner_free`; wasm wraps the assigner in an object (`MpeAssigner` in JS) and frees it by `Drop` |
| — | `JsMpeAssigner.note_on` | `idiom` — `clausters_mpe_assigner_note_on`: C answers a negative number for a zone of no members, wasm `undefined` |
| — | `JsMpeAssigner.note_off` | `clausters_mpe_assigner_note_off` |
| — | — | `clausters_mpe_decoder_*` and `clausters_mpe_bend_message` are bound by neither client: the decoder reads an MPE stream *coming in*, which is the server's and the host's work, and the bend is one of the three messages `mpe_expression_messages` already writes |

## The shared-memory segment

A peer maps the segment in its own language — that part is the language's — and
then needs to know where everything is. These are the numbers and the small
parts of *logic* it would otherwise transcribe, which is how one binding came
to declare 1024 control buses against a server that had had 16 384 for months:
wrong, unused, and invisible to every test.

Nothing here reaches wasm, and the reason is the same for every row: **a browser
cannot map a file**. A page has no segment to attach to, so it keeps talking OSC
over the WebSocket carrier and fetching what it needs to draw — the same split
every bulk path already has, recorded in `docs/ipc.md`.

| C ABI | wasm | Note |
|---|---|---|
| `clausters_core_shm_abi_version` | — | `n/a` — the segment layout version; a page has no segment to check it against |
| `clausters_core_shm_shape` | — | `n/a` — every count and byte offset in one call, so a binding stops carrying half the layout |
| `clausters_core_shm_segment_size` | — | `n/a` — how big a segment with these counts is, for a peer sizing a file to create one |
| `clausters_core_shm_init` | — | `n/a` — writes a fresh header, for a peer that **creates** a segment rather than attaching; the editor's arrangement makes that ordinary, since whoever owns the samples owns the segment |
| `clausters_core_shm_buffer_info` | — | `n/a` — the buffer directory's row, read under its seqlock |
| `clausters_core_shm_region_suffix` | — | `n/a` — the name a buffer's region file carries; three processes name that file |
| `clausters_core_shm_push` | — | `n/a` — the command ring's framing (length, peer tag, padding) |
| `clausters_core_shm_pop` | — | `n/a` — the same, inbound, including the resync a malformed frame forces |

## Transport and versioning

| C ABI | wasm | Note |
|---|---|---|
| `clausters_ws_connect` | — | `n/a` — the browser provides WebSocket; this exists so a browser-less binding does not re-implement the framing per language |
| `clausters_ws_send` | — | `n/a` — as above |
| `clausters_ws_recv` | — | `n/a` — as above |
| `clausters_ws_close` | — | `n/a` — as above |
| `clausters_ws_last_error` | — | `n/a` — as above |
| `clausters_core_abi_version` | — | `n/a` — the cdylib is loaded by a client built separately, so it answers a version handshake (`CORE_ABI_VERSION`); the wasm module ships inside the npm package and is rebuilt with it, so there are never two versions to reconcile |


### What each core ABI version changed

`CORE_ABI_VERSION` is refused on mismatch rather than negotiated, so what a number means is written down here rather than beside the constant.

v2 added
the `clausters_ws_*` WebSocket client transport; v3 the `clausters_core_peaks_*`
peak-pyramid cache builder; v4 the `clausters_core_window` smoothing windows
(shared with the server's FFT chain for bit-identical analysis); v5 the seam
audit pass -- the `clausters_sched_*` beat queue, the `clausters_clocksync_*`
sample-clock model, the `clausters_rng_*` value stream, NTP timetag packing,
`quant_delay` and `degree_to_midinote` -- so no value/time logic remains
per-language; v6 `clausters_rng_next_u64` (child-stream seed derivation for
the per-routine random context); v7 the `clausters_core_correlation` /
`clausters_core_lissajous` stereo-field measurements (shared with the GUI
phasescope so a headless client reads the identical numbers); v8 the
`clausters_core_peaks_multi_*` multichannel peak-pyramid cache (one cache
resource per buffer, all channels -- the editor-grade waveform's format);
v9 the ruler/axis scalars -- `clausters_core_hz_to_mel`/`_mel_to_hz`/
`_hz_to_bark`/`_bark_to_hz` (perceptual frequency scales, shared with the
GUI spectrogram axis) and `clausters_core_bar`/`_beat_in_bar` (the bar:beat
read of a quant grid, the display complement of `quant_delay`); v10 the
`clausters_registry_*` finite-resource id registry (node ids, buses,
buffers -- every client's allocator and the server's reserved ranges share
the one occupancy-map model, internally locked per handle); v11 the
`clausters_core_patch_compile` cord->bus pass (a directed patch JSON in, its
GraphDef wiring JSON out -- the GUI patcher's translation, shared so every
client compiles a patch identically); v12 the notation surface
(feature-gated, see `clausters_core::notation`) -- the pure
`clausters_core_svg_to_display_list` and `clausters_core_voice_to_mei`,
plus, behind `verovio`, the editable
`clausters_score_*` handle, so a client binds the notation layer instead of
reimplementing it; v13 the `clausters_core_bundle_*` component-bundle pass
(a manifest's requirements, one mounted instance's resolution, and the
writers' pre-flight -- shared so a bundle authored in any language mounts
identically in a tab, on the desktop and over loopback); v14
`clausters_core_stats`, the peak/RMS of one channel of an interleaved
buffer (what a render reports back, so no client writes the loop); v15 the
document surface -- `clausters_document_apply` and
`clausters_document_resolve` -- which is how every client binds one
implementation of what an edit *means* instead of three: the document and
the intent cross by value and the new document comes back, rather than each
client holding handles into a Rust object graph; v16 the undo log
(`clausters_log_*`), which crosses as a **handle** where the document
crosses by value -- a bulk inverse leaves the log on purpose, so sending one
by value would carry every spilled span on every call, which is the cost
spilling exists to avoid. (v32 renamed these `clausters_history_*`; see
below.) **v19 is a format rather than a symbol**: the peak
cache the `clausters_core_peaks_*` builders emit is CLPK v3, which carries a
mean square beside each bucket's min/max, so a cache built by this surface is
longer than a v18 one and a reader that predates it cannot parse it (the
converse holds: v1 and v2 caches still load). v21 the shared-memory segment
(`clausters_core_shm_*`): a peer maps the file in its own language and asks
here for every offset and count, for the directory's seqlock, for the ring
framing and for a region file's name -- the numbers a binding used to
transcribe, which is how one of them came to declare 1024 control buses
against a server that had 16 384. **v21 also carries**
`clausters_core_peaks_multi_write_buckets`, the receiving half of
`/buffer_stream`: a run of buckets somebody else measured, folded into a
cache in place, so a client that cannot map the memory a recording is
filling still draws it. **v23 the score model** -- `clausters_core_sheet_apply`,
`clausters_core_sheet_to_mei` and `clausters_core_sheet_ops`: notation as
data a client holds and operations as data it sends, so one implementation
of what an edit to a score *means* serves every client and, more to the
point, serves a standalone host that has no client language in the process
at all. It is the same by-value shape the document surface took at v15, for
the same reason. The verbs are **not** symbols -- they ride inside the
payload -- so the catalog is what says which exist, and adding one moves
nothing here. **v24 the interpreter** -- `clausters_core_sheet_perform` and
`clausters_core_interpretation`: the path back out of the score, reading
what the symbols *mean* into sounding notes, and the default reading a
caller starts from when it wants another one. Two symbols rather than one
because an override has to be able to read the defaults before editing them,
and a client that wrote those numbers down for itself would play the same
score at a different amplitude than the other client does. **v25 the
reader** -- `clausters_core_mei_to_sheet`: a *document* back into the model,
which is the other return path and the one that makes a score opened from
typed text editable at all. One symbol for every notation format there is,
because the engraver normalizes whatever it loaded to MEI before this sees
it. **v26 the score's edit path** -- `clausters_score_apply` and
`clausters_score_sheet`: an open document is edited through the *model's*
verbs rather than through the engraver's editor, so there is one
implementation of what an edit to a score means and a standalone host
performs the same one. The engraver's editor stays as the escape hatch for a
document that has no model behind it. **v27 which item a page element is** --
`clausters_core_item_id`: the step between a selection on the page and a
model verb, answered by the emitter that spelled the element rather than by
each client working the spelling out again. **v28 a take's length is
seconds** -- `clausters_document_resolve` takes `frames_per_second` beside
`frames_per_beat`, because the document now measures a placement in beats
and what it places in the unit of that element's own data, so one ratio can no
longer answer both questions. **v29 the multitrack's time map** --
`clausters_tempomap_*`: a beat is a logical coordinate and the tempo that
turns it into a second can change along the multitrack, so the conversion stops
being a scalar and becomes an integral. Additive: the affine functions stay
exactly as they were, and a one-segment map computes their expression.

**v30 a tempo curve has a shape** -- `clausters_tempomap_segment` writes
**seven** `f64` instead of six (the seventh is the curvature), and
`clausters_tempomap_shaped`/`_env` write a shaped ramp and a whole finite
envelope. Breaking rather than additive: the segment payload widened, and
its fourth number is now an envelope shape number rather than a flag, so a
reader that does not know shape 2 or 5 misreads a segment it can see.

**v31 the map is a value** -- `clausters_tempomap_version`, `_dump` and
`_load`: the edit counter a holder of a *shared* map compares, and the map
written out as its breakpoints and read back through the ordinary writers.
Additive, and the counter still moves: the ctypes binding declares every
symbol eagerly, so a staged library missing these fails at load -- with
*"speaks ABI v30, this binding v31"* rather than an `AttributeError` on a
name nobody was looking at.

**v32 the log became a history**, and the rename is the honest part of a
breaking change: the handle no longer holds one document's undo but one
*editing context* -- the structures registered in it and one ordered pile
over them -- so `clausters_log_*` became `clausters_history_*`, with
`clausters_history_register` minting a structure's identity and every call
that names one taking it. `undo` and `redo` lost their document argument and
apply nothing: a history holds structures this surface cannot reach, so they
hand back each payload with the structure it belongs to (`{"inverses": ...}`,
`{"edits": ..., "remaining": ...}`) and the caller applies them through whatever
door each domain has. `record` gained the coalesce **key**, because "the
same thing done the same way" is a sentence in a vocabulary the pile does
not read.

**v33 an entry is a transaction.** `clausters_history_record` takes the
whole entry as one JSON request -- a label, a coalesce flag and a list of
legs, each naming its structure -- because a gesture may touch more than one
structure and has to undo as one step, and a leg at a time would let half a
transaction land. `clausters_document_inverse` came with it: a caller
recording its own entry needs the inverse read *before* the edit lands, and
only the arrangement can state one for the arrangement.

**v34 what a history refuses to promise.** A leg may carry no `backward`:
an act with no inverse is recorded, marked, and walked past in both
directions, so `clausters_history_undo`/`_redo` now answer with the entry's
`label` and the `skipped` labels beside the payloads. Deleting a structure
is `clausters_history_forget`, which invalidates the entries naming it and
defers the free -- undoing a deletion has to be able to give the data back --
with `clausters_history_released` saying when the last entry holding it has
retired. And the save mark is the pile's: `clausters_history_mark_saved`,
`_dirty` and `_saved_reachable`.

**v35 the vocabularies are named once.** `clausters_domain_coalesce_key`
answers the coalesce sentence for any domain the crate speaks -- the
arrangement, a curve, a span of samples, a timeline -- because a caller
recording its own entry has to state a key the pile cannot compute, and
spelling four vocabularies' rules again in ctypes and again in TypeScript is
the divergence `clausters_document_coalesce_key` was given a door to
prevent. Additive, and the counter still moves for the reason v31 states:
the ctypes binding declares every symbol eagerly, so a staged library
missing this one fails at load with a version mismatch rather than an
`AttributeError` on a name nobody was looking at. One behaviour moved with
it and is not additive: an **empty** `writesamples` -- the inverse the
document can state for a destructive edit -- now bumps the source's
generation instead of applying as a no-op, so a reader's copy is marked
stale by an undo as it is by the edit.

**v36 a domain inverts its own edits.** `clausters_domain_edit` applies a
payload to a structure held as its own state -- a curve's points, a
timeline's events -- and answers with what it now is *and* the payload that
puts it back, both in one call because the inverse has to be read before the
edit lands. It is the other half of v35's argument: the coalesce sentence
and the inverse are the same vocabulary's rule, and a client computing the
second itself is the divergence the first was given a door to prevent. Two
domains are deliberately not served: the arrangement's tree, which needs a
version to check against and a grid to snap to and has
`clausters_document_apply` of its own, and a span of samples, whose frames
live in a buffer rather than in a value. Additive, and the counter moves for
v31's reason.

**v37 a score can be put back.** `clausters_score_load` (`JsScore.load` in a
page) loads a MEI state into an engraved score and clears the shared layer's
own stack, which is what an undo of a page needs once the history is the
editing context's rather than the engraver's: one score, one order.

**v38 a curve's axis is not the view's to invent.**
`clausters_core_curve_axis` answers what a break-point curve is *drawn*
against -- its range with a tenth of headroom, and, given the axis already in
hand, only widened where the data stopped fitting inside it. It is a drawing
rule rather than a signal one, and it is here for the reason the project
gives for all of them: it was written twice, once per client, and was about
to be written a third time for the standalone curve editor. Recomputed per
redraw it makes an edit rescale the picture, so the two clients agreeing
about it is the difference between one curve drawn one way and one curve
drawn two. Additive, and the counter moves for v31's reason.

**v39 a widget id names what it draws.** `clausters_widgetids_*` is the GUI
namespace with two doors over one occupancy map: the anonymous lease a
hand-built tree takes, and a **keyed** id asked for by naming the structure,
the role and which one it is -- the same name getting the same number for as
long as it keeps being drawn. It is here rather than in each client because
a leased id is what makes an edit-back in flight across a redraw land on the
wrong widget, and a table written twice would agree about that in one
language and not the other. `_begin`/`_retire` are the draw cycle: what was
not asked for is taken back, ascending, so two clients free the same widgets
in the same order. Additive, and the counter moves for v31's reason.

**v40 a redraw is the difference.** `clausters_gui_difference` answers what
to send so a host drawing one picture draws another: one `/gui_set` per
widget whose props moved, or the word that says the shape changed and the
tree has to go whole. A redefine frees the old subtree, so it takes every
widget's screen state with it and drops what the host had pending -- and a
walk deciding when that is necessary is a rule, not a convenience, which is
why it is here rather than once per client. Additive, and the counter moves
for v31's reason.

**v41 a change of shape costs a subtree, not the window.**
`clausters_gui_difference` answers `{whole, redefine, sets}` where it
answered `{define}` or `{sets}`: `/gui_def` names any widget, so a clip that
appeared in one lane is that lane's definition and every other lane keeps
the zoom, the scroll and the selection it had. **Not additive** -- the same
symbol answers a different document -- which is exactly why the counter has
to move: a staged library one version behind would otherwise be read as
saying "nothing changed" for every redraw.

**v42 the difference goes, because the host reconciles.**
`clausters_gui_difference` is removed. It answered *what to send so a host
drawing one picture draws another*, which is only a question a caller
holding a copy of the host's picture can ask -- and no caller can hold one:
the host mutates on its own (a drag writes an offset per frame, a wheel
writes a window, a marquee writes a mark) and screen state is reported by
nothing, correctly. A `/gui_def` now means *make it look like this*: the
host walks the tree it was handed beside the tree it draws, matches widget
to widget by the id that names what it draws, and keeps what is its own. So
the decision moved to the only place that has the true copy, and it did not
move as this symbol -- it is a comparison of a document with a **widget
tree**, not of two documents. **Removing a symbol**, so the counter moves and
a caller of it fails to link rather than diffing against a picture nobody is
drawing.

**v43 a multitrack's picture is the crate's.** `clausters_multitrack_picture`
answers the rows and boxes a multitrack draws as, and
`clausters_multitrack_read` answers what a report of those boxes *means* in
the multitrack's own vocabulary. Both are in beats and seconds, because the shape
is the format's and the time is `tempomap`'s; a caller crosses to its own
axis with the calls it already binds. **Additive**, and the reason the
counter moves at all is that a client which cannot find them has no way to
draw a multitrack without writing the mapping again -- which is the thing
they exist to prevent.

**v44 the routing table is the crate's.** `clausters_view_not_an_edit`
answers the `/gui_event` tags that are screen state rather than edits, which
each client held as its own literal list. **Additive**, and the counter
moves for the same reason v43 did: a client that cannot find it writes the
list again, which is the divergence the symbol exists to end.

**v45 the catalogue views are the crate's.** `clausters_view_props` answers
the widget a waveform, a curve or a roll is and what is on it, from the
facts a caller states -- one door with the kind named, so a view the crate
learns to draw needs no new symbol. **Additive**, and it moves the counter
because a client that cannot find it assembles the picture itself, which is
the third implementation this ends.

**v46 a history walk is one door.** `clausters_history_walk` takes the
direction and answers the legs already gathered per structure, replacing
`clausters_history_undo` and `clausters_history_redo`. **Removing symbols**,
so a caller of the old pair fails to link rather than keeping its own copy
of the two rules that moved: which side of an entry a direction reads, and
which legs a structure owns.

**v47 a multitrack's curves are in its picture, and a report of them is read
here.** `clausters_multitrack_picture` answers `curves` (a track's
automations, each a row of its own) and `layers` (a region's, each inside
its box) beside the rows and boxes, and `clausters_multitrack_read_points`
answers what a report of every curve *means* -- one `SetAutomation` per
curve whose break-points moved. **Additive**, and the counter moves for the
reason v43's pair did: a client that cannot find them writes the mapping
again in its own language.

**v48 a report of a multitrack's rows is read here too.**
`clausters_multitrack_read_rows` answers what a report of every row *means*
-- one `SetTracks` whatever changed, so adding a track, removing one with
its boxes, reordering the stack and moving a fader are one verb and one
entry. **Additive**, and the counter moves for the reason the other two
readers did: the client that could not find it was writing the mixer's half
of this mapping in its own language already, and the half it did not have
(a track added, a track gone) is exactly where two clients drift.

**v50 a projection is bound, not re-derived.** `clausters_editing_*` is the
first door onto `clausters-editing`: what an editable structure owes its
three endpoints -- the props a host draws it with, and in time the payloads
an edit becomes and the operations that make a server sound it. The first of
them is a break-point curve's props, which is deliberately the smallest
payload there is: what it establishes is that a *projection* crosses here at
all, beside the *rules* that already did. A projection written once per
client is one no compiler and no test reads against its twin, and that is
where a page and a script come to draw one curve two ways. **Additive**, and
the counter moves for v31's reason.

**v51 a multitrack's props are one answer.** `clausters_editing_multitrack_props`
hands back the rows, the boxes, the automations over both, their
break-points, which are hidden and which boxes loop -- everything a multitrack
has from the document alone. It was written three times before it was
written here, and the third was already in Rust: a standalone host draws the
same picture with no client in the process, so `clients/gui` linked its own
copy of the same sextuple, the same septuple and the same row height.
**Additive**, and the counter moves for v31's reason.

**v52 a gesture is read once, for every domain there is.**
`clausters_editing_intake` is the second projection: a tag and a flat list
of values become payloads in a structure's own vocabulary -- a curve's
points, a stroke over samples, a roll's two lanes, a multitrack's boxes, rows and
break-points. There were sixteen small readers before this, eight per
language, each able to disagree with its twin about what a septuple means;
it is **one** door for all four because a host reports every gesture the
same way, which is also what keeps a client from quietly growing a fifth
vocabulary. **Additive**, and the counter moves for v31's reason.

**v53 what is sounding is one reconciler.** `clausters_editing_instance_*`
is the third projection and the one with state: it holds what was made of
the last plan and answers the **difference** as a list of operations --
send this def, add this slot, set these ports, free that node. It opens no
socket, awaits nothing and allocates nothing, which is what makes it
testable with no server in the room and identical under NRT; an operation
names what it acts on by a handle rather than by a node id, a bus index or a
buffer number, because those are a running session's facts and the client's
to allocate. A handle rather than a function, for the same reason
`clausters_history_*` is one. **Additive**, and the counter moves for v31's
reason.

**v54 the conversation is one algorithm.** `clausters_editing_conversation_*`
is the protocol every editor speaks whatever it edits: what a message from
the host *is* (a close, a history step, an edit made against a picture that
is gone, or an edit to route), and what to answer it with. The floor and the
staleness rule are in it, which is where the version defects were, and the
envelope crosses rather than the payload -- what a report means already
crosses once through `clausters_editing_intake`. Pure, both of them: the
conversation's whole state is two integers, so a client keeps the pair and
hands it back. `clausters_editing_multitrack_names` goes with them, the
minting correction's half that is a fact about the multitrack. **Additive**, and
the counter moves for v31's reason.

**v55 the superseded readers go, and the format is askable.** The four
superseded doors -- `clausters_multitrack_picture` and the
three `clausters_multitrack_read*` -- are **removed**: the view is
`clausters_editing_multitrack_props` and the reading is
`clausters_editing_intake`, both clients call those instead, and
a door that answers half of what its replacement answers is a door that
invites the wrong call. `clausters_session_format` arrives in their place,
so a client can ask what format the crate writes instead of only knowing.
**Breaking**: a caller of the four is a caller of symbols that no longer
exist.

**v56 an editor's window is one composition.** `clausters_apps_multitrack_*`
is the first door of the applications crate: the window the multitrack
editor opens -- the time ruler above the multitrack, the multitrack, the transport row
-- and the props any widget of it is corrected with. Each client composed it
for itself and the standalone host composed a third, which had no ruler and
no transport at all. **Additive**, and the counter moves for v31's reason.

**v57 the editor is a handle.** `clausters_apps_multitrack_editor_*` holds
the multitrack editor between messages -- the conversation's floor, the
window's ids, the cursor -- and answers every turn: a gesture read and
applied with its inverse, the entry to record, the acknowledgement and the
corrections. Its verbs cross through one JSON door (`_call`), and it composes
the window itself, so the two stateless v56 doors are **removed** --
**breaking** for a caller of those two.

**v58 steps are carried out once.** `clausters_editing_runner_*` holds the
queue a playback's answers are walked through: the messages that may go out
now, the one awaited last, and what a reply releases. Each client walked
the steps its own way and the standalone host had a third walk, which could
not order a join's reads before its stitch. **Additive**, and the counter
moves for v31's reason.

**v59 one transport.** `clausters_editing_playback_new` takes only `chunk`:
the playback makes the transport's group at the top, binds it and makes the
multitrack inside it, the same for every endpoint. A client used to bind the
multitrack's own graph and the GUI host a group of its own, and said which by
passing `target` and `bind_transport`. **Breaking**: a caller of the
three-argument door is a caller of a signature that no longer exists.

**v60 a session is loaded once.** `clausters_editing_load` plans the reads
and the stitches that put a saved session's sources into a server, as steps
for the runner. Only the GUI host could open a session and sound it; a
client loaded each take by hand and could not load a join at all.
**Additive**, and the counter moves for v31's reason.

**v61 the samples editor's window is the crate's.**
`clausters_apps_samples_editor_*` holds a take, the measures its picture
stacks and the window's chrome, and composes and corrects the window; each
client composed it for itself. `clausters_apps_samples_measures` checks a
measure stack. **Additive**, and the counter moves for v31's reason.

**v62 one undo order across applications.** `clausters_apps_editing_*` holds
an editing context: the history, the version and the editors opened in it,
whose turns and history steps it takes. A verb runs once across its sizing
and filling calls, since a history is not copied for a sizing pass.
**Additive**, and the counter moves for v31's reason.

**v63 a take's step is its writes.** `clausters_apps_editing_new` takes no
argument: a step hands a take's writes back as payloads, which the member
turns into steps with the bound of the server the take is on, so a context
carries no bound of its own. An open answers the structure, and `record`
takes `coalesce` and answers the version. **Breaking** for a caller of the
one-argument door.

**v64 an editor is only a member.** `clausters_apps_multitrack_editor_*` and
`clausters_apps_samples_editor_*` are **removed**: both clients open their
editors in an editing context (`clausters_apps_editing_*`), which holds the
history they share. **Breaking** for a caller of the editor handles.

**v65 true peak.** `clausters_core_true_peak` measures the *reconstructed*
peak of one channel of an interleaved buffer -- the ITU-R BS.1770-4 Annex 2
filter at 4*, which is what makes a reading dBTP -- where
`clausters_core_stats` reports the largest sample. **Additive**, and the
counter moves for v31's reason.

**v66 loudness.** `clausters_core_loudness` measures an interleaved buffer
as ITU-R BS.1770 and EBU R 128 do: the gated integrated loudness, the
loudness range and the maximum momentary and short-term loudness.
**Additive**, and the counter moves for v31's reason.

**v67 the multitrack is in seconds.** A region, a fade, a curve point, a
marker and a span are seconds, so nothing that plans, draws or reads a
multitrack takes a tempo: `clausters_multitrack_plan`,
`clausters_editing_multitrack_props` and
`clausters_editing_instance_reconcile` lose `default_bpm`, a playback's
`locate` and `cue` take seconds, `clausters_editing_playback_beats_to_samples`
and `_samples_to_beats` become `_secs_to_samples` and `_samples_to_secs`, and
`clausters_editing_default_bpm` becomes `clausters_editing_default_tempo`, in
beats per second, which only a ruler reads. `clausters_session_migrate` reads
a session written in an older format as this one writes it. **Breaking**.

**v68 the structure is named, and a refusal says why.** The editing request
and outcome key `piece` is `multitrack`, the instance handle a playback
makes is `"multitrack"`, the mixer's graph is `clausters.multitrack.N`, and
the multitrack window's transport controls are `transport_rewind`,
`transport_play`, `transport_stop` and `transport_clock`.
`clausters_view_props` answers `{"error": reason}` for a kind it does not
draw or facts that will not read, where it answered nothing -- which both
clients stamped into a widget with nothing on it. **Breaking**.

**v69 a server has several transports.** `Shape` grows `transports` and
`transport_stride`, appended; its two transport offsets are transport 0's,
and transport `i`'s are `i * transport_stride` further on. The editing
playback's transport commands carry the transport id first, as every
`/transport_*` command now does. **Breaking**.

**v70 the audio editor's playback.** `clausters_editing_audio_playback_new`,
`_free` and `_call`: the audio editor as it is playing, every verb through
one JSON door. **Additive**, and the counter moves for v31's reason.

**v71 an event's keys are the core's.** `clausters_core_degree_to_midinote`
takes `alter` after `degree` and reads a fractional degree as an
alteration; the `clausters_core_pitch_*`, `_level_*`, `_split_degree`,
`_midinote_to_degree`, `_event_delta` and `_event_sustain` doors are new.
**Breaking**.

**v72 an event renders in the core.** `clausters_core_event_synth`,
`_event_midi` and `_event_of_midi`: a note's synth messages, an event's
MIDI messages, and MIDI bytes as the event that plays them back.
**Additive**, and the counter moves for v31's reason.

**v73 a sequence of events.** `clausters_document_sequence_new`, `_free` and
`_call`: the document's `EventSequence`, every verb through one JSON door.
**Additive**, and the counter moves for v31's reason.

**v74 the notes editor.** `clausters_apps_editing_open_notes` opens one over
a sequence handle, and the context's `openNotes` verb over data.
**Additive**, and the counter moves for v31's reason.

**v75 the notes editor's playback.** `clausters_editing_notes_playback_new`,
`_free` and `_call`. A blob in a step that is not samples travels as
`{"x": hex}`. **Additive**, and the counter moves for v31's reason.

**v76 the transport plays event lanes.** The notes editor's playback is an
event lane on its transport: `clausters_editing_notes_playback_call`'s
`replan` verb is gone (an edit is `update`, and nothing asks for the
transport's clock). New: `clausters_editing_playback_notes`, the
multitrack's notes regions as its lane's data, and
`clausters_apps_editing_bind_sequence`, a multitrack member's source bound
to a sequence handle. **Not additive** -- a verb went.

**v77 where a pass ends.** `clausters_editing_playback_set_stop_at_end` and
`_stops_at_end` are replaced by `clausters_editing_playback_set_end` and
`_end`, whose JSON says `null` (the transport rolls on), `"contents"` or an
end marker; the notes editor's playback takes the same through its `end`
verb and no longer marks the sequence's end on every play. New:
`clausters_editing_playback_play_pass`, the space bar's pass over a time
range or the loop switch, which the notes editor's `play` verb takes as
`range` and `loop`. **Not additive** -- two symbols went.

**v78 a note's curves are heard.** `clausters_editing_playback_notes`
takes the id spaces after `placed`, since the graphs a sequence's curves
play through allocate nodes, buses and buffers; `placed` is the events and
the lanes over their channels (a bare list of events is still read).
New: `clausters_editing_playback_set_loop`, the loop switch while the
multitrack plays. **Not additive** -- a signature moved.

**v81 an event's curves are sent by the client.**
`clausters_editing_event_curves`: the `/node_set`s a played event's curves
come to, one window at a time. Additive.

**v82 the notation keys are one table.**
`clausters_core_event_notation_keys`: what an event says about the page it
is written on, each key with what it holds, its unit and which way it is
read; and `clausters_core_event_written_midinote`, the MIDI note a
`pitches` value sounds. Additive. An event that states no sounding pitch
now sounds the first of its `pitches`, and `pitches`, `value`, `staff` and
`voice` no longer reach a synth as controls.

**v83 a score renders into a sequence.**
`clausters_core_sheet_render_events`: a sheet as the sequence that plays
it -- events with their notation keys, a channel to a voice, the dynamics
as lanes, the `notation` section. Additive. The interpretation gained
`dynamics_as` and `dynamics_cc`, which a caller that sends neither does not
notice.

**v84 a client writes a spectrogram cache.**
`clausters_core_stft_cache_size` and `clausters_core_stft_build`: the
short-time Fourier transform the GUI host draws a spectrogram from, as the
cache it maps. The analysis moved to the core from the host, which read
a format nothing else could write. Additive.

**v85 a sequence is read into a score.**
`clausters_core_sheet_read_events`: the way back from v83's render -- a
sequence's events read into a sheet, what they do not say decided by the
transcription that crosses with them. Additive. The render's `notation`
section gained `controls`, `marks` and `groups` and writes each staff as
it is written; a sequence with one curve of a note's own and one line is
now named MPE where it was named MIDI 2.0.

**v86 a score editor opens over a sequence.**
`clausters_apps_editing_open_score_over`: the score editor on the page a
sequence is read into, its edits written back to the sequence and
recorded as the sequence's. Additive.

**v87 a multitrack editor is handed an engraver.**
`clausters_apps_editing_bind_engraver`: the score a multitrack editor
engraves its boxes of notes with, each drawn as its page. Additive.

**v88 a selection is resolved against a tempo map.**
`clausters_document_resolve` takes a tempo-map handle where it took a
frames-per-beat ratio, so a selection may cross a tempo change. Breaking:
one parameter changed type.

**v89 a gesture is applied as one.** `clausters_domain_edit_all`: the
edits one gesture came to, all of them or none, with the payload that puts
each back. Additive.

**v90 the plugin catalog.** `clausters_core_plugin_catalog`,
`clausters_core_plugin_search` and `clausters_core_plugin_reference`: a
catalog read from scan rows, searched, and a plugin def's reference built
with its ports routed. Additive.

**v91 a note plays at a node, and event buses are allocated.**
`clausters_core_event_node_note`: a note event whose instrument is a node
(`{"node": id}`) rendered as the `/node_event` messages of one note,
identified by the caller. `clausters_ids_new` takes the server's event-bus
count after its transports, and space 5 is the event buses below the MIDI
bindings' half. Breaking: one parameter added.

**v92 a node's generic editor.** `clausters_core_node_editor`: the GuiDef of
a node's controls, generated from their description as `/def_query`
reports it, searched and paged. Additive.


## The client surface: one capability, two shapes

The tables above are the core's doors. This section is the other half of the
same rule, one level up: **the two clients have one surface**, and where a
language needs a different *shape* for the same capability, the difference is
written here as `idiom` rather than discovered by reading both. A difference of
*capability* is not idiom; it is a gap, and goes in a plan.

The shapes repeat, so they are grouped by cause.

**A mapping read with an operator in Python is read with methods in
TypeScript**, which has no indexing operator for an object:

- an event of a sequence (`SeqEvent`): Python `event["midinote"]`,
  `event["midinote"] = 62`, `"midinote" in event` and iterating its keys;
  TypeScript `event.get("midinote")`, `event.set("midinote", 62)`,
  `event.has("midinote")` and `event.keys()`;
- a live collection (`seq.events`, `seq.automation`, an event's
  `automation`): Python `collection[i]`; TypeScript `collection.item(i)`.
  Both iterate, both answer `len(...)` / `.length`.

**A `with` block is a callback**: Python `with seq.history("humanize"): ...`;
TypeScript `seq.history.entry("humanize", () => ...)`, the shape
`session.use(...)` already has for `with session:`.

**What asks the engine or the host is asynchronous in TypeScript**, so a
Python property that reads becomes a method, a property that writes becomes a
`set…` method, and a verb that sends answers a promise:

- the transport (`Transport`): Python `t.playing`, `t.end`, `t.end = 4`,
  `t.span = (0, 2)`; TypeScript `await t.playing()`, `await t.end()`,
  `await t.setEnd(4)`, `await t.setSpan([0, 2])`. `t.span` and `t.looping`
  read local state and stay properties in both;
- what the hand marked (`Marking`, the mixin `NotesEditor`,
  `MultitrackEditor` and `AudioEditor` take): Python `editor.selected`,
  `editor.select(marked)`, `editor.unselect()`; TypeScript
  `await editor.selected()`, `await editor.select(marked)`,
  `await editor.unselect()`. One shape for the three, since the words are
  written once: a roll's and a multitrack's marks are the host's to answer,
  and an `AudioEditor`'s, though its own state, are awaited as the others
  are — marking there moves the transport's span, which sends;
- playing a sequence: Python `play(seq)` and `seq.play()` answer the
  `Transport`; TypeScript answers a promise of it;
- the default session's server: Python `main.server_or_boot()`; TypeScript
  `await main.serverOrBoot()`.

**Options are one object in TypeScript**, as everywhere in that client:
Python `seq.play(at, server)`; TypeScript `seq.play({ at, server })`.


The existing editing JSON door also carries audio gain curves: `envelope`
reads seconds-based quads, `show_envelope` selects the curve body, and
`setautomation` / `apply_envelope` run through `Editing::act` to record the
same undo order as host gestures. The window request accepts the caller-owned
`envelope` widget id beside `widget` and `pane`. These are JSON payloads over
the existing door; no C or wasm symbol or ABI counter changes.

The existing audio editor/playback JSON doors carry preview without new
binding symbols: application `preview_on`, `preview_off`, `free_preview`
and the `preview` query; playback `gain` (`file`, seconds-based point quads,
`on`, contents `revision`) and `removeEffect` (`file`). Playback `state` adds
`previews`, a file-id keyed map of visible processed buffers. Python
`preview_on`/`preview_off`/`free_preview` and TypeScript
`previewOn`/`previewOff`/`freePreview` carry those same turns. Signal builders
spell the wire `preview_buffer` as Python `preview_buffer` and TypeScript
`previewBuffer`.
