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
| `clausters_core_stats` | `channel_stats` | `idiom` — the peak and RMS of one channel of an interleaved buffer; C writes the pair through an out pointer, wasm returns it as a two-element array |
| `clausters_core_correlation` | `correlation` | |
| `clausters_core_lissajous` | `lissajous` | |
| `clausters_core_hz_to_mel` | — | **gap** — the perceptual axes a spectrogram labels |
| `clausters_core_mel_to_hz` | — | **gap** — as above |
| `clausters_core_hz_to_bark` | — | **gap** — as above |
| `clausters_core_bark_to_hz` | — | **gap** — as above |

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
ms** for one placement on a 10240-event composition (3.3 MB of JSON), against 6
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
| `clausters_document_snapshot` | `JsDocument.snapshot` | the one call still the size of the composition, and asked for rather than paid per edit |
| `clausters_document_version` | `JsDocument.version` | `idiom` — a getter |

**Size-then-fill needs two rules here**, because the surface mutates. A mutating
call commits **only when the bytes are written**, so a sizing pass changes
nothing and repeating one is harmless — which matters more now than it did, since
the tree is no longer the caller's to compare against. A **pure read caches**
between the pair instead: `snapshot` keeps what the sizing pass serialized and
the fill copies it out, so a composition is serialized once per pair rather than
twice. Caching a mutating call that way would be wrong — the mutation would land
on the sizing pass, and a caller that sized and then gave up would have edited
without knowing.

The **log** crosses as a handle too, for its own reason: a bulk inverse *leaves*
the log for its spill store on purpose, so passing one by value would carry
every spilled span on every call, which is the cost spilling exists to avoid.
Its document-carrying calls take the document handle.

| C ABI | wasm | Note |
|---|---|---|
| `clausters_log_new` | `JsLog.new` | `idiom` — a constructor where C mints a handle |
| `clausters_log_free` | — | `n/a` — wasm frees by `Drop` |
| `clausters_log_apply` | `JsLog.apply` | |
| `clausters_log_record` | `JsLog.record` | |
| `clausters_log_undo` | `JsLog.undo` | |
| `clausters_log_redo` | `JsLog.redo` | |
| `clausters_log_can_undo` | `JsLog.can_undo` | `idiom` — a getter |
| `clausters_log_can_redo` | `JsLog.can_redo` | `idiom` — a getter |
| `clausters_log_undo_label` | `JsLog.undo_label` | `idiom` — a getter returning a string, where C sizes and fills |
| `clausters_log_redo_label` | `JsLog.redo_label` | `idiom` — as above |
| `clausters_log_len` | `JsLog.len` | `idiom` — a getter |
| — | `JsLog.is_empty` | `idiom` — `len == 0`, as `JsScheduler` and `JsRegistry` already spell it |
| `clausters_log_clear` | `JsLog.clear` | |

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
| `clausters_core_sheet_perform` | `sheet_perform` | `idiom` — the same envelope-versus-throw difference. The path back out of the score: what the symbols *mean*, read into sounding notes. Each note carries **two lengths** (`dur` written, `sustain` heard) and the `staff`/`voice` it came from, which is what a caller binds an instrument to — the notation does not say what plays it |
| `clausters_core_interpretation` | `interpretation` | the default reading, and **the parity surface for it**. Its fields are not symbols and are not in this table: they ride inside the payload `sheet_perform` takes, so nothing structural notices if one client's idea of `mf` drifts from the other's. Both clients are read against this value instead. It is also why neither writes those numbers down — a client with its own copy of the dynamics table plays the same score at a different amplitude |
| — | `engrave_options` | `idiom` — the engraver's options JSON. A native caller builds it from `EngraveOptions` inside `clausters-notation`, which is where its resource path also lives; a page has no such struct and calls the core's builder directly, so both configure verovio identically |
| `clausters_score_open` | `JsScore.new` | `idiom` — a constructor where C mints a handle. The engraver is built by the binding either way: natively from a resource path, in a page from the loaded module |
| `clausters_score_free` | — | `n/a` — wasm frees by `Drop` |
| `clausters_score_display_list` | `JsScore.display_list` | |
| `clausters_score_mei` | `JsScore.mei` | |
| `clausters_score_transpose` | `JsScore.transpose` | |
| `clausters_score_transpose_to` | `JsScore.transpose_to` | the absolute form, and the one an edit travels in: `transpose` is verovio's relative call and stays for a caller that has the delta |
| `clausters_score_edit` | `JsScore.edit` | |
| `clausters_score_undo` | `JsScore.undo` | |
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
pieces of *logic* it would otherwise transcribe, which is how one binding came
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
