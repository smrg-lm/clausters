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
| `clausters_core_map` | `map` | `idiom` — the range maps (`linlin`, `linexp`, `lincurve`, …). C takes a whole sequence and writes it through an out pointer, as it does for the op tables above; wasm maps one value, and the client loops — the same split `unary`/`binary` already have, for the same reason (a ctypes call is worth batching, a wasm-bindgen call is not) |
| `clausters_core_whitenoise` | — | **gap** — `JsRng` steps values one at a time; there is no buffer fill, so a browser cannot reproduce a server noise stream in one call |
| `clausters_core_window` | — | **gap** — the FFT smoothing windows. Nothing in the browser transforms anything: a spectrum is a view and the GUI host draws it, reaching `spectrum` and `window` as a Rust caller of the core |
| `clausters_core_true_peak` | `true_peak` | both — the **reconstructed** peak of one channel of an interleaved buffer, where the row below reports the largest sample. The ITU-R BS.1770-4 Annex 2 filter at 4×, which is what makes a reading dBTP; C and wasm both return the amplitude, there being one number to return |
| `clausters_core_loudness` | `loudness` | `idiom` — the **loudness** of an interleaved buffer as ITU-R BS.1770 and EBU R 128 measure it: integrated, range, and the maximum momentary and short-term readings. C writes the four through an out pointer and takes the weights as a nullable pointer; wasm returns them as an array and takes the weights as an optional one — the split `clausters_core_stats` makes. The streaming `LoudnessMeter` has no door on either side: a live meter runs in the host, a Rust caller of the core |
| `clausters_core_stats` | `channel_stats` | `idiom` — the peak and RMS of one channel of an interleaved buffer; C writes the pair through an out pointer, wasm returns it as a two-element array |
| `clausters_core_correlation` | `correlation` | |
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
| `clausters_ids_new` | `JsIdSpaces.new` | `idiom` — **a client's id spaces**, sized from the server and sliced by a share: the node table's client range, the audio buses above the server's own outputs, both bus spaces clear of their GraphDef windows, the buffers. One constructor with a `score` flag in C; two in wasm, because a constructor that cannot fail and one that can are two JS shapes. The policy every endpoint used to restate over its own registries, and had drifted on (two output buses by default against the server's output count) |
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
| `clausters_document_apply` | `JsDocument.apply` | |
| `clausters_document_resolve` | `JsDocument.resolve` | |
| `clausters_document_snapshot` | `JsDocument.snapshot` | the one call still the size of the document, and asked for rather than paid per edit |
| `clausters_document_version` | `JsDocument.version` | `idiom` — a getter |
| `clausters_document_inverse` | `JsDocument.inverse` | the edit that would put a node back, read before anything lands — what a caller recording its **own** entry needs |
| `clausters_document_coalesce_key` | `JsDocument.coalesce_key` | `idiom` — a free function in C, a static on the class in JS; it is the *multitrack's* sentence for "the same thing done the same way", so it lives with the document and not with the history. The Python client offers **both** spellings (`document_coalesce_key` and `Document.coalesce_key`, as it does for `inverse`), so a reader who learned one client's shape finds it in the other |
| `clausters_domain_coalesce_key` | `domain_coalesce_key` | the same sentence for **any** vocabulary the crate speaks — the multitrack, a curve, a span of samples, a timeline. Free in both, since a caller across either seam holds no structure to ask: a curve, a span of samples and a timeline live in the caller's own memory and only their *vocabulary* is the crate's. An unknown domain, or a payload not written in that one, answers nothing (`0` in C, `""` in JS) |
| `clausters_domain_edit` | `domain_edit` | applying a payload to a structure held **as its own state**, answering what it now is and the payload that puts it back — both in one call, since the inverse has to be read before the edit lands. The other half of the row above: the coalesce sentence and the inverse are one vocabulary's rule, and a client computing the second itself is what the first was given a door to prevent. Two domains answer nothing on purpose — the multitrack's tree (it needs a version and a grid, and has `clausters_document_apply`) and a span of samples (a borrowed view whose frames are in a buffer, never in a string) |
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
sits over it.

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
| `clausters_editing_playback_new` | `JsMultitrackPlayback.new` | `idiom` — C hands back an opaque pointer, wasm a class; **one multitrack as it is playing** — its instance, its applier and its transport, answering every verb as steps. It takes only the chunk a fill may carry: the transport's group, made at the top and bound with the multitrack inside it, is the same for every endpoint |
| `clausters_editing_playback_free` | `JsMultitrackPlayback` `Drop` | `idiom` — wasm frees by `Drop`, C needs an explicit `_free`; the bookkeeping, not the nodes |
| `clausters_editing_playback_sync` | `JsMultitrackPlayback.sync` | `idiom` — the steps that make what sounds be what the multitrack says, allocating from the client's id spaces; C sizes and fills (changing the playback and the spaces only on the fill) where wasm returns the string |
| `clausters_editing_playback_play` | `JsMultitrackPlayback.play` | `idiom` — the steps that roll the transport |
| `clausters_editing_playback_pause` | `JsMultitrackPlayback.pause` | `idiom` — the steps that freeze the multitrack and zero its meters |
| `clausters_editing_playback_stop` | `JsMultitrackPlayback.stop` | `idiom` — the steps that halt and go back to the mark |
| `clausters_editing_playback_locate` | `JsMultitrackPlayback.locate` | `idiom` — the steps that put the transport at a second of the multitrack |
| `clausters_editing_playback_cue` | `JsMultitrackPlayback.cue` | `idiom` — a locate for a stopped transport, nothing for a rolling one |
| `clausters_editing_playback_close` | `JsMultitrackPlayback.close` | `idiom` — the steps that free everything the muece made |
| `clausters_editing_playback_meters` | `JsMultitrackPlayback.meters` | `idiom` — the meter bus runs by track, as JSON |
| `clausters_editing_playback_set_rolling` | `JsMultitrackPlayback.set_rolling` | `idiom` — whether the transport is rolling, when the caller learned it from the engine |
| `clausters_editing_playback_rolling` | `JsMultitrackPlayback.rolling` | `idiom` — whether the transport was last told to roll; C answers 1 or 0 |
| `clausters_editing_playback_secs_to_samples` | `JsMultitrackPlayback.secs_to_samples` | `idiom` — a second of the multitrack as a sample, at the rate it was planned at; wasm answers a double |
| `clausters_editing_playback_samples_to_secs` | `JsMultitrackPlayback.samples_to_secs` | `idiom` — a sample as a second of the multitrack |
| `clausters_editing_intake` | `editing_intake` | `idiom` — **what a gesture means**, in a structure's own vocabulary: a tag and a flat list of values become the payloads an edit is. One door over all four domains (`points`, `samples`, `events`, `multitrack`), because a host reports every gesture the same way — and because a fifth vocabulary then has nowhere to grow. Both take the domain, the tag and one JSON request; C sizes and fills a buffer where wasm returns the string |
| `clausters_editing_stitch` | `editing_stitch` | `idiom` — **what a source made of spans comes to**: the buffer a join is, as `/buffer_stitch` takes it — the width, the rate, the length, and each part's buffer, span, fades and full channel map (a part narrower than the join repeats). One reading for every endpoint that builds a minted join: the GUI host at open and at edit, and both clients at edit, which had read the recipe three ways and drifted on the channel map. Both take the source entry and the caller's table as JSON; C sizes and fills a buffer where wasm returns the string |
| `clausters_editing_load` | `editing_load` | `idiom` — **a session's sources, loaded**: every take read and every join stitched into buffer numbers the caller set aside, as steps for the runner — each read and each stitch followed by the `/done` of its buffer, so a join waits for the takes it is over. The GUI host plans the same load and walked it alone, which left a saved session one only it could open and sound. Both take `{"session", "beside", "buffers"}` and answer what each source became, the steps, what will not load and why, and the numbers left over; the door assumes every file is there, since a client cannot see its server's disk and a page never can, so a missing file is the server's refusal of the read. C sizes and fills a buffer where wasm returns the string |
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
| `clausters_apps_samples_measures` | `samples_measures` | a **measure stack, checked**: the stack back, or the reason it is refused. A stack is written by hand, so a typo is a layer that quietly does not appear. C sizes and fills a buffer where wasm returns the string |

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
instead: `clausters_midi_write_smf` and `clausters_midi_write_clip`, argument for
argument the same call.

What a page needs is exactly the half that is pure Rust. It has no virtual port
to open — Web MIDI hands out the ports that already exist and lets a page create
none — but it does have a score to write, so `MidiServer` over an NRT interface
accumulates `(beat, message)` and writes the file in the tab. The alternative was
a TypeScript SMF writer, which is a second implementation of a byte format, and
the whole point of a shared core is not having one.

| C ABI (`clausters-ffi`) | wasm (`clausters-core-web`) | Note |
|---|---|---|
| — | `midi_write_smf` | `idiom` — `clausters-midi`'s `clausters_midi_write_smf`, not `clausters-ffi`'s, so this table's left column cannot name it. Same flat arguments (n ticks, 3n bytes, ppq); C returns a malloc'd buffer freed by `clausters_midi_free`, wasm returns the bytes and JS sees it as `midiWriteSmf` |
| — | `midi_write_clip` | `idiom` — `clausters_midi_write_clip` on the same terms, `midiWriteClip` in JS |

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
