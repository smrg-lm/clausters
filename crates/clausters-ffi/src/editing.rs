//! The projections an editable structure owes its endpoints.
//!
//! The C half of [`clausters_editing`]. Sizes with a null `out` and fills with
//! a second call, like the rest of the JSON surface here -- a projection is a
//! pure read, so a sizing pass changes nothing and can be repeated.

/// The props a break-point curve is drawn with: `{"points": [...], "min": ..,
/// "max": .., "duration": ..}` as JSON, for the `n` flat `t v shape curve`
/// values at `points`.
///
/// With `hold` non-zero, `kept_lo`/`kept_hi` are the value axis the view
/// already has and `held` the time span it already has; both are widened and
/// never narrowed, which is what keeps an edit from rescaling the picture under
/// the hand. `duration` is absent when the curve spans nothing.
///
/// Returns the byte count the answer needs, or 0 for a null `points` with a
/// non-zero `n`.
///
/// # Safety
/// `points` must be null or readable for `n` `f64`s, and `out` null or
/// writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_points_props(
    points: *const f64,
    n: usize,
    hold: i32,
    kept_lo: f64,
    kept_hi: f64,
    held: f64,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    if points.is_null() && n != 0 {
        return 0;
    }
    // SAFETY: caller guarantees `points` is readable for `n`.
    let slice = if n == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(points, n) }
    };
    let kept = (hold != 0).then_some((kept_lo, kept_hi));
    let answer = clausters_editing::points::props_json(slice, kept, held);
    // SAFETY: forwarded from this function's own contract. A pure read, so
    // there is nothing to commit.
    unsafe { crate::document::fill(answer.as_bytes(), out, out_cap, || {}) }
}

/// **A multitrack as the props the multitrack widget is drawn with**, as JSON.
///
/// The rows, the boxes, the automations over both, their break-points, which
/// are hidden and which boxes loop -- everything a multitrack has from the document
/// alone. What a caller adds is what is a function of something else: the
/// position cursor, the meter buses, the widget's own chrome.
///
/// `sources` is the same table [`crate::document`]'s instance plan takes,
/// source id to `{"buffer", "channels"}`. Sizes with a null `out` and fills
/// with a second call.
///
/// # Safety
/// `multitrack` and `sources` must be null or readable for their lengths, and `out`
/// null or writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_multitrack_props(
    multitrack: *const u8,
    multitrack_len: usize,
    rate: f64,
    sources: *const u8,
    sources_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let (Some(multitrack), Some(sources)) = (
        unsafe { crate::document::text(multitrack, multitrack_len) },
        unsafe { crate::document::text(sources, sources_len) },
    ) else {
        return 0;
    };
    let answer = clausters_editing::multitrack::props_json(&multitrack, rate, &sources);
    // SAFETY: forwarded from this function's own contract. A pure read, so
    // there is nothing to commit.
    unsafe { crate::document::fill(answer.as_bytes(), out, out_cap, || {}) }
}

/// **What a gesture means, in a structure's own vocabulary** -- the edit
/// ingestion, for every domain there is.
///
/// A host reports a gesture as a tag and a flat list of values, and what an
/// editor needs from that is payloads [`clausters_document::intent`] will
/// apply. `domain` is `"points"`, `"samples"`, `"events"` or `"multitrack"`;
/// `request` is the one JSON object [`clausters_editing::intake_json`]
/// documents, carrying the report and whatever that domain needs beside it.
///
/// The answer is `{"payloads": [...], "label": "...."}`, with `inverse` where
/// the gesture carried one and `refusal` where the gesture is this domain's and
/// cannot be written. A domain or a tag nobody answers for comes back with no
/// payloads and no refusal, which is "nothing to say" and not a failure.
///
/// Sizes with a null `out` and fills with a second call.
///
/// # Safety
/// `domain`, `tag` and `request` must be null or readable for their lengths,
/// and `out` null or writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_intake(
    domain: *const u8,
    domain_len: usize,
    tag: *const u8,
    tag_len: usize,
    request: *const u8,
    request_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let (Some(domain), Some(tag), Some(request)) = (
        unsafe { crate::document::text(domain, domain_len) },
        unsafe { crate::document::text(tag, tag_len) },
        unsafe { crate::document::text(request, request_len) },
    ) else {
        return 0;
    };
    let answer = clausters_editing::intake_json(&domain, &tag, &request);
    // SAFETY: forwarded from this function's own contract. A pure read, so
    // there is nothing to commit.
    unsafe { crate::document::fill(answer.as_bytes(), out, out_cap, || {}) }
}

/// **What a source made of spans comes to**: the buffer a join is, resolved.
///
/// `source` is a source-table entry as JSON (a minted source as an intent
/// carries it reads the same) and `held` the caller's table, source id to
/// `{"buffer", "channels", "frames"}`. The answer is
/// `clausters_editing::sources::stitch_json`'s: the join's width, rate, length
/// and parts with their full channel maps, or `null` where there is nothing to
/// make.
///
/// Sizes with a null `out` and fills with a second call.
///
/// # Safety
/// `source` and `held` must be null or readable for their lengths, and `out`
/// null or writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_stitch(
    source: *const u8,
    source_len: usize,
    held: *const u8,
    held_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let (Some(source), Some(held)) = (
        unsafe { crate::document::text(source, source_len) },
        unsafe { crate::document::text(held, held_len) },
    ) else {
        return 0;
    };
    let answer = clausters_editing::sources::stitch_json(&source, &held);
    // SAFETY: forwarded from this function's own contract. A pure read.
    unsafe { crate::document::fill(answer.as_bytes(), out, out_cap, || {}) }
}

/// **A session's sources, loaded**: the steps that read every take and stitch
/// every join into the buffers the caller set aside.
///
/// `request` is `{"session", "beside", "buffers"}` as JSON and the answer is
/// `clausters_editing::load::plan_json`'s: what each source became, the steps to
/// walk through a runner, what will not load and why, and the numbers left
/// over.
///
/// Sizes with a null `out` and fills with a second call.
///
/// # Safety
/// `request` must be null or readable for `request_len` bytes, and `out` null
/// or writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_load(
    request: *const u8,
    request_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let Some(request) = (unsafe { crate::document::text(request, request_len) }) else {
        return 0;
    };
    let answer = clausters_editing::load::plan_json(&request);
    // SAFETY: forwarded from this function's own contract. A pure read.
    unsafe { crate::document::fill(answer.as_bytes(), out, out_cap, || {}) }
}

/// **What is sounding of a multitrack**, held across edits.
///
/// The instance projection's state: an opaque handle, because it is the one
/// projection with memory and shipping it out and back in on every edit would
/// carry every curve's table twice for nothing. Free it with
/// [`clausters_editing_instance_free`].
pub struct FfiInstance(std::sync::Mutex<clausters_editing::instance::Instance>);

/// A new instance: nothing of the multitrack is sounding yet.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_editing_instance_new() -> *mut FfiInstance {
    Box::into_raw(Box::new(FfiInstance(std::sync::Mutex::new(
        clausters_editing::instance::Instance::new(),
    ))))
}

/// Frees an instance created by [`clausters_editing_instance_new`] (null is a
/// no-op).
///
/// **It frees the bookkeeping and not the nodes**: what a server holds is the
/// server's, so a caller that means to stop the sound calls
/// [`clausters_editing_instance_teardown`] first and applies what it answers.
///
/// # Safety
/// `i` must be a pointer from `clausters_editing_instance_new`, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_instance_free(i: *mut FfiInstance) {
    if !i.is_null() {
        // SAFETY: caller guarantees `i` came from Box::into_raw above.
        drop(unsafe { Box::from_raw(i) });
    }
}

/// **The difference between what is sounding and what the multitrack says**, as the
/// JSON list of operations a client applies.
///
/// The same four arguments the instance plan takes -- the multitrack, the rate, the
/// tempo a multitrack that states none is read at, and the source table -- plus the
/// master's own level, which is the caller's and not the multitrack's. Everything
/// already right is left alone, which is what lets a hand drag a box without
/// hearing the rest of the multitrack restart.
///
/// An operation names what it acts on by a **handle**, never by a node id, a
/// bus index or a buffer number: this allocates none of those, and the client
/// keeps the one table from handle to whatever it made.
///
/// Sizes with a null `out` and fills with a second call.
///
/// # Safety
/// `i` must be null or a live pointer from `clausters_editing_instance_new`,
/// `multitrack` and `sources` null or readable for their lengths, and `out` null or
/// writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_instance_reconcile(
    i: *mut FfiInstance,
    multitrack: *const u8,
    multitrack_len: usize,
    sample_rate: f64,
    sources: *const u8,
    sources_len: usize,
    gain: f32,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    if i.is_null() {
        return 0;
    }
    // SAFETY: forwarded from this function's own contract.
    let (Some(multitrack), Some(sources)) = (
        unsafe { crate::document::text(multitrack, multitrack_len) },
        unsafe { crate::document::text(sources, sources_len) },
    ) else {
        return 0;
    };
    // SAFETY: caller guarantees `i` is live.
    let instance = unsafe { &*i };
    let Ok(mut held) = instance.0.lock() else {
        return 0;
    };
    // **A sizing pass must change nothing.** This is the first door on this
    // surface whose work is not a pure read -- reconciling is what teaches the
    // instance what it now believes is sounding -- so the diff is taken against
    // a copy and the copy is adopted only when the answer was actually written.
    // Without that, the sizing call did the reconciling and the filling call
    // found nothing left to do.
    let mut next = held.clone();
    let answer = clausters_editing::instance::reconcile_json(
        &mut next,
        &multitrack,
        sample_rate,
        &sources,
        gain,
    );
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        crate::document::fill(answer.as_bytes(), out, out_cap, || {
            *held = next;
        })
    }
}

/// **Everything this made, given back** -- the operations that stop the multitrack.
///
/// The multitrack itself is untouched: what an instance holds is nodes, and nodes
/// are not the document. Afterwards it believes nothing is sounding, and the
/// defs it sent stay known, since they are on the server whatever this holds.
///
/// # Safety
/// `i` must be null or a live pointer from `clausters_editing_instance_new`,
/// and `out` null or writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_instance_teardown(
    i: *mut FfiInstance,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    if i.is_null() {
        return 0;
    }
    // SAFETY: caller guarantees `i` is live.
    let Ok(mut held) = (unsafe { &*i }).0.lock() else {
        return 0;
    };
    // Against a copy, for [`clausters_editing_instance_reconcile`]'s reason: a
    // sizing pass that forgot what is sounding would leave the nodes on the
    // server with nothing holding their handles.
    let mut next = held.clone();
    let answer = next.teardown_json();
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        crate::document::fill(answer.as_bytes(), out, out_cap, || {
            *held = next;
        })
    }
}

/// **Which control bus run each track's meters write**, by track -- what a host
/// reads every frame, and the reason a level that moves every block costs no
/// message.
///
/// `[{"track": id, "bus": handle, "channels": n}]`, a run of `2 * channels`:
/// the level first and the mark that waits after it.
///
/// # Safety
/// `i` must be null or a live pointer from `clausters_editing_instance_new`,
/// and `out` null or writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_instance_meters(
    i: *mut FfiInstance,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    if i.is_null() {
        return 0;
    }
    // SAFETY: caller guarantees `i` is live.
    let Ok(held) = (unsafe { &*i }).0.lock() else {
        return 0;
    };
    let answer = held.meters_json();
    // SAFETY: forwarded from this function's own contract. A pure read, so
    // there is nothing to commit.
    unsafe { crate::document::fill(answer.as_bytes(), out, out_cap, || {}) }
}

/// **The tempo a multitrack that states none is drawn at**, in beats per
/// second -- the one default every endpoint's ruler takes. Nothing a multitrack
/// places reads it.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_editing_default_tempo() -> f64 {
    clausters_editing::multitrack::DEFAULT_TEMPO
}

/// **Steps being carried out**: the queue a playback's answers are walked
/// through, the messages that may go out now and what a reply releases. Free it
/// with [`clausters_editing_runner_free`].
pub struct FfiStepRunner(std::sync::Mutex<clausters_editing::run::Runner>);

/// A runner holding nothing.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_editing_runner_new() -> *mut FfiStepRunner {
    Box::into_raw(Box::new(FfiStepRunner(std::sync::Mutex::new(
        clausters_editing::run::Runner::new(),
    ))))
}

/// Frees a runner created by [`clausters_editing_runner_new`] (null is a
/// no-op).
///
/// # Safety
/// `r` must be a pointer from `clausters_editing_runner_new`, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_runner_free(r: *mut FfiStepRunner) {
    if !r.is_null() {
        // SAFETY: caller guarantees `r` came from Box::into_raw above.
        drop(unsafe { Box::from_raw(r) });
    }
}

/// **One verb of the runner**, as `clausters_editing::run::call_json`
/// documents: `push`, `ready`, `reply`, `idle`.
///
/// The verb runs against a copy that is adopted when the answer is filled, so a
/// sizing pass changes nothing and can be repeated.
///
/// Returns the byte count the answer needs, or 0 for a null handle or a request
/// that is not UTF-8.
///
/// # Safety
/// `r` must be null or a live runner, `request` readable for `request_len`
/// bytes, and `out` null or writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_runner_call(
    r: *mut FfiStepRunner,
    request: *const u8,
    request_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: caller guarantees `r` is live or null.
    let Some(runner) = (unsafe { r.as_ref() }) else {
        return 0;
    };
    // SAFETY: forwarded from this function's own contract.
    let Some(request) = (unsafe { crate::document::text(request, request_len) }) else {
        return 0;
    };
    let Ok(mut held) = runner.0.lock() else {
        return 0;
    };
    let mut next = held.clone();
    let answer = clausters_editing::run::call_json(&mut next, &request);
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        crate::document::fill(answer.as_bytes(), out, out_cap, || {
            *held = next;
        })
    }
}

/// **The audio editor, as it is playing**: its structure, its files and its
/// transport, answering every verb as steps. Free it with
/// [`clausters_editing_audio_playback_free`].
pub struct FfiAudioPlayback(
    std::sync::Mutex<clausters_editing::audio_playback::AudioEditorPlayback>,
);

/// A new audio editor playback: `chunk` is how many samples one fill carries,
/// and `transport` the transport it plays on -- negative for the crate's own,
/// `AUDIO_EDITOR_TRANSPORT`.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_editing_audio_playback_new(
    chunk: usize,
    transport: i32,
) -> *mut FfiAudioPlayback {
    use clausters_editing::apply::Endpoint;
    use clausters_editing::audio_playback::{AUDIO_EDITOR_TRANSPORT, AudioEditorPlayback};
    let transport = if transport < 0 {
        AUDIO_EDITOR_TRANSPORT
    } else {
        transport
    };
    Box::into_raw(Box::new(FfiAudioPlayback(std::sync::Mutex::new(
        AudioEditorPlayback::new(
            Endpoint {
                chunk: chunk.max(1),
            },
            transport,
        ),
    ))))
}

/// Frees a playback created by [`clausters_editing_audio_playback_new`] (null
/// is a no-op). The bookkeeping, not the nodes: a caller that means to stop the
/// sound closes it first and sends what that answers.
///
/// # Safety
/// `p` must be a pointer from `clausters_editing_audio_playback_new`, not yet
/// freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_audio_playback_free(p: *mut FfiAudioPlayback) {
    if !p.is_null() {
        // SAFETY: caller guarantees `p` came from Box::into_raw above.
        drop(unsafe { Box::from_raw(p) });
    }
}

/// **One verb of the audio editor's playback**, as JSON
/// (`clausters_editing::audio_playback::call_json`): `request` is
/// `{"verb": ...}` and the answer is `{"steps": [...]}`, a query's own object,
/// or `{"error": ...}`. Allocates from `ids`. Sizes with a null `out` and
/// fills with a second call; only the call that fills changes anything.
///
/// # Safety
/// `p` and `ids` null or live, `request` null or readable for `request_len`
/// bytes, `out` null or writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_audio_playback_call(
    p: *mut FfiAudioPlayback,
    request: *const u8,
    request_len: usize,
    ids: *mut crate::registry::FfiIdSpaces,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let Some(request) = (unsafe { crate::document::text(request, request_len) }) else {
        return 0;
    };
    // SAFETY: forwarded from this function's own contract.
    let (Some(playback), Some(spaces)) = (unsafe { p.as_ref() }, unsafe { ids.as_ref() }) else {
        return 0;
    };
    let (Ok(mut held), Ok(mut spaces)) = (playback.0.lock(), spaces.0.lock()) else {
        return 0;
    };
    let (mut next, mut next_ids) = (held.clone(), spaces.clone());
    let answer = clausters_editing::audio_playback::call_json(&mut next, &request, &mut next_ids);
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        crate::document::fill(answer.as_bytes(), out, out_cap, || {
            *held = next;
            *spaces = next_ids;
        })
    }
}

/// **One multitrack, as it is playing**: its instance, its applier and its
/// transport, answering every verb as steps. Free it with
/// [`clausters_editing_playback_free`].
pub struct FfiPlayback(std::sync::Mutex<clausters_editing::playback::MultitrackPlayback>);

/// A new playback; `chunk` is how many samples one `/buffer_setRange` carries.
/// Where it makes the multitrack and how it binds the transport are the crate's, the
/// same for every endpoint: a group at the top, bound, with the multitrack inside.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_editing_playback_new(chunk: usize) -> *mut FfiPlayback {
    use clausters_editing::apply::Endpoint;
    use clausters_editing::playback::MultitrackPlayback;
    Box::into_raw(Box::new(FfiPlayback(std::sync::Mutex::new(
        MultitrackPlayback::new(Endpoint {
            chunk: chunk.max(1),
        }),
    ))))
}

/// Frees a playback created by [`clausters_editing_playback_new`] (null is a
/// no-op). The bookkeeping, not the nodes: a caller that means to stop the
/// sound closes it first and sends what that answers.
///
/// # Safety
/// `p` must be a pointer from `clausters_editing_playback_new`, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_playback_free(p: *mut FfiPlayback) {
    if !p.is_null() {
        // SAFETY: caller guarantees `p` came from Box::into_raw above.
        drop(unsafe { Box::from_raw(p) });
    }
}

/// Runs a verb against a copy and adopts the copy only on the call that fills:
/// a sizing pass changes nothing.
///
/// # Safety
/// `p` must be null or a live playback, and `out` null or writable for
/// `out_cap` bytes.
unsafe fn playback_verb(
    p: *mut FfiPlayback,
    out: *mut u8,
    out_cap: usize,
    verb: impl FnOnce(&mut clausters_editing::playback::MultitrackPlayback) -> String,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let Some(playback) = (unsafe { p.as_ref() }) else {
        return 0;
    };
    let Ok(mut held) = playback.0.lock() else {
        return 0;
    };
    let mut next = held.clone();
    let answer = verb(&mut next);
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        crate::document::fill(answer.as_bytes(), out, out_cap, || {
            *held = next;
        })
    }
}

/// Runs a verb that allocates or releases ids, against copies of both.
///
/// # Safety
/// As [`playback_verb`], and `ids` null or a live id-spaces handle.
unsafe fn playback_ids_verb(
    p: *mut FfiPlayback,
    ids: *mut crate::registry::FfiIdSpaces,
    out: *mut u8,
    out_cap: usize,
    verb: impl FnOnce(
        &mut clausters_editing::playback::MultitrackPlayback,
        &mut clausters_core::ids::IdSpaces,
    ) -> String,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let (Some(playback), Some(spaces)) = (unsafe { p.as_ref() }, unsafe { ids.as_ref() }) else {
        return 0;
    };
    let (Ok(mut held), Ok(mut spaces)) = (playback.0.lock(), spaces.0.lock()) else {
        return 0;
    };
    let (mut next, mut next_ids) = (held.clone(), spaces.clone());
    let answer = verb(&mut next, &mut next_ids);
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        crate::document::fill(answer.as_bytes(), out, out_cap, || {
            *held = next;
            *spaces = next_ids;
        })
    }
}

/// **Makes what sounds be what the multitrack says**: the steps, as JSON
/// (`{"steps": [...]}` or `{"error": "..."}`), allocating from `ids`. The multitrack
/// is the document's JSON and `sources` the table of source id -> buffer and
/// channels. Sizes with a null `out` and fills with a second call.
///
/// # Safety
/// `p` and `ids` null or live, `multitrack` and `sources` null or readable for their
/// lengths, `out` null or writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_playback_sync(
    p: *mut FfiPlayback,
    multitrack: *const u8,
    multitrack_len: usize,
    sample_rate: f64,
    sources: *const u8,
    sources_len: usize,
    gain: f32,
    ids: *mut crate::registry::FfiIdSpaces,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let (Some(multitrack), Some(sources)) = (
        unsafe { crate::document::text(multitrack, multitrack_len) },
        unsafe { crate::document::text(sources, sources_len) },
    ) else {
        return 0;
    };
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        playback_ids_verb(p, ids, out, out_cap, |playback, ids| {
            clausters_editing::playback::sync_json(
                playback,
                &multitrack,
                sample_rate,
                &sources,
                gain,
                ids,
            )
        })
    }
}

/// The steps that roll the transport.
///
/// # Safety
/// As [`clausters_editing_playback_sync`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_playback_play(
    p: *mut FfiPlayback,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    use clausters_editing::playback::answer_json;
    // SAFETY: forwarded from this function's own contract.
    unsafe { playback_verb(p, out, out_cap, |pb| answer_json(Ok(pb.play()))) }
}

/// The steps that freeze the multitrack and zero its meters.
///
/// # Safety
/// As [`clausters_editing_playback_sync`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_playback_pause(
    p: *mut FfiPlayback,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    use clausters_editing::playback::answer_json;
    // SAFETY: forwarded from this function's own contract.
    unsafe { playback_verb(p, out, out_cap, |pb| answer_json(Ok(pb.pause()))) }
}

/// The steps that halt and go back to the mark at beat `mark`.
///
/// # Safety
/// As [`clausters_editing_playback_sync`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_playback_stop(
    p: *mut FfiPlayback,
    mark: f64,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    use clausters_editing::playback::answer_json;
    // SAFETY: forwarded from this function's own contract.
    unsafe { playback_verb(p, out, out_cap, |pb| answer_json(Ok(pb.stop(mark)))) }
}

/// The steps that put the transport at `secs` of the multitrack.
///
/// # Safety
/// As [`clausters_editing_playback_sync`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_playback_locate(
    p: *mut FfiPlayback,
    secs: f64,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    use clausters_editing::playback::answer_json;
    // SAFETY: forwarded from this function's own contract.
    unsafe { playback_verb(p, out, out_cap, |pb| answer_json(Ok(pb.locate(secs)))) }
}

/// The steps that cue a stopped transport at `secs` -- none for a rolling one.
///
/// # Safety
/// As [`clausters_editing_playback_sync`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_playback_cue(
    p: *mut FfiPlayback,
    secs: f64,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    use clausters_editing::playback::answer_json;
    // SAFETY: forwarded from this function's own contract.
    unsafe { playback_verb(p, out, out_cap, |pb| answer_json(Ok(pb.cue(secs)))) }
}

/// The steps that switch whether a pass stops at the end of the contents
/// (`on` non-zero), going back to the position cursor -- the transport's end
/// mark, sent only when it moves.
///
/// # Safety
/// As [`clausters_editing_playback_sync`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_playback_set_stop_at_end(
    p: *mut FfiPlayback,
    on: i32,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    use clausters_editing::playback::answer_json;
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        playback_verb(p, out, out_cap, |pb| {
            answer_json(Ok(pb.set_stop_at_end(on != 0)))
        })
    }
}

/// The steps that free everything the multitrack made, releasing into `ids`.
///
/// # Safety
/// As [`clausters_editing_playback_sync`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_playback_close(
    p: *mut FfiPlayback,
    ids: *mut crate::registry::FfiIdSpaces,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    use clausters_editing::playback::answer_json;
    // SAFETY: forwarded from this function's own contract.
    unsafe { playback_ids_verb(p, ids, out, out_cap, |pb, ids| answer_json(pb.close(ids))) }
}

/// The meters the multitrack writes, `[{"track", "bus", "channels"}]`.
///
/// # Safety
/// As [`clausters_editing_playback_sync`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_playback_meters(
    p: *mut FfiPlayback,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        playback_verb(p, out, out_cap, |pb| {
            clausters_editing::playback::meters_json(pb)
        })
    }
}

/// Says whether the transport is rolling, when the caller learned it from the
/// engine.
///
/// # Safety
/// `p` must be null or a live playback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_playback_set_rolling(p: *mut FfiPlayback, rolling: i32) {
    // SAFETY: caller guarantees `p` is null or live.
    if let Some(playback) = unsafe { p.as_ref() }
        && let Ok(mut held) = playback.0.lock()
    {
        held.set_rolling(rolling != 0);
    }
}

/// Whether the transport was last told to roll: 1 or 0.
///
/// # Safety
/// `p` must be null or a live playback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_playback_rolling(p: *mut FfiPlayback) -> i32 {
    // SAFETY: caller guarantees `p` is null or live.
    unsafe { p.as_ref() }
        .and_then(|playback| playback.0.lock().ok().map(|held| i32::from(held.rolling())))
        .unwrap_or(0)
}

/// The transport the multitrack plays on; 0 for a null playback.
///
/// # Safety
/// `p` must be null or a live playback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_playback_transport(p: *mut FfiPlayback) -> i32 {
    // SAFETY: caller guarantees `p` is null or live.
    unsafe { p.as_ref() }
        .and_then(|playback| playback.0.lock().ok().map(|held| held.transport()))
        .unwrap_or(0)
}

/// Whether a pass stops at the end of the contents: 1 or 0.
///
/// # Safety
/// `p` must be null or a live playback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_playback_stops_at_end(p: *mut FfiPlayback) -> i32 {
    // SAFETY: caller guarantees `p` is null or live.
    unsafe { p.as_ref() }
        .and_then(|playback| {
            playback
                .0
                .lock()
                .ok()
                .map(|held| i32::from(held.stops_at_end()))
        })
        .unwrap_or(0)
}

/// A second of the multitrack as a sample, at the rate it was planned at.
///
/// # Safety
/// `p` must be null or a live playback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_playback_secs_to_samples(
    p: *mut FfiPlayback,
    secs: f64,
) -> i64 {
    // SAFETY: caller guarantees `p` is null or live.
    unsafe { p.as_ref() }
        .and_then(|playback| {
            playback
                .0
                .lock()
                .ok()
                .map(|held| held.secs_to_samples(secs))
        })
        .unwrap_or(0)
}

/// A sample as a second of the multitrack, at the same rate.
///
/// # Safety
/// `p` must be null or a live playback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_playback_samples_to_secs(
    p: *mut FfiPlayback,
    samples: i64,
) -> f64 {
    // SAFETY: caller guarantees `p` is null or live.
    unsafe { p.as_ref() }
        .and_then(|playback| {
            playback
                .0
                .lock()
                .ok()
                .map(|held| held.samples_to_secs(samples))
        })
        .unwrap_or(0.0)
}

/// **What one message from the host is** -- the conversation's first decision.
///
/// `state` is the conversation's two integers (`{"floor", "applied"}`) and
/// `message` the event's *envelope* -- the address, the stamp, the version it
/// was made against, the tag, and whether this editor owns the widget and the
/// window. The payload is deliberately not here: what a report means is
/// [`clausters_editing_intake`]'s and already crosses once, so a drag reporting
/// a thousand boxes costs this nothing.
///
/// The answer is `{"turn": {...}, "state": {...}}`: what to do, and the two
/// integers as they now stand.
///
/// Sizes with a null `out` and fills with a second call.
///
/// # Safety
/// `state` and `message` must be null or readable for their lengths, and `out`
/// null or writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_conversation_read(
    state: *const u8,
    state_len: usize,
    message: *const u8,
    message_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let (Some(state), Some(message)) =
        (unsafe { crate::document::text(state, state_len) }, unsafe {
            crate::document::text(message, message_len)
        })
    else {
        return 0;
    };
    let answer = clausters_editing::conversation::read_json(&state, &message);
    // SAFETY: forwarded from this function's own contract. The conversation's
    // state comes back in the answer rather than being kept here, so this is a
    // pure read and a sizing pass changes nothing.
    unsafe { crate::document::fill(answer.as_bytes(), out, out_cap, || {}) }
}

/// **What to answer the host with** -- the conversation's second decision.
///
/// `request` is `{"seq", "docVersion", "reason", "corrections"}`, and the
/// answer is one of `silent`, `ack` or `push`. It runs after the routing
/// because what an answer carries is collected while routing: the corrections
/// the gesture did not survive intact, and the reason when one is owed.
///
/// There is no success flag in it: applied, transformed and refused are **one
/// message**, and a refusal is simply the previous value among the corrections.
///
/// # Safety
/// `request` must be null or readable for `request_len` bytes, and `out` null
/// or writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_conversation_answer(
    request: *const u8,
    request_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let Some(request) = (unsafe { crate::document::text(request, request_len) }) else {
        return 0;
    };
    let answer = clausters_editing::conversation::answer_json(&request);
    // SAFETY: forwarded from this function's own contract. A pure read.
    unsafe { crate::document::fill(answer.as_bytes(), out, out_cap, || {}) }
}

/// **What a multitrack calls its rows and its boxes** -- `{"rows": [...], "boxes":
/// [...]}`, by the names the wire carries them under.
///
/// The minting correction's half that is a fact about the multitrack: a host that
/// made a track or split a box minted the *word* while the document minted the
/// *id*, so a view keeps what it was last told and answers with the picture
/// when the two stop agreeing. Reading it here rather than striding the props
/// is what keeps a flat array's shape out of a call site.
///
/// # Safety
/// `multitrack` must be null or readable for `multitrack_len` bytes, and `out` null or
/// writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_multitrack_names(
    multitrack: *const u8,
    multitrack_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let Some(multitrack) = (unsafe { crate::document::text(multitrack, multitrack_len) }) else {
        return 0;
    };
    let answer = clausters_editing::multitrack::names_json(&multitrack);
    // SAFETY: forwarded from this function's own contract. A pure read.
    unsafe { crate::document::fill(answer.as_bytes(), out, out_cap, || {}) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two-call shape: size with a null `out`, then fill.
    #[test]
    fn the_props_size_then_fill() {
        let points = [0.0f64, 0.5, 1.0, 0.0, 2.0, 1.0, 1.0, 0.0];
        let n = unsafe {
            clausters_editing_points_props(
                points.as_ptr(),
                points.len(),
                0,
                0.0,
                0.0,
                0.0,
                std::ptr::null_mut(),
                0,
            )
        };
        assert!(n > 0);
        let mut buf = vec![0u8; n];
        let wrote = unsafe {
            clausters_editing_points_props(
                points.as_ptr(),
                points.len(),
                0,
                0.0,
                0.0,
                0.0,
                buf.as_mut_ptr(),
                buf.len(),
            )
        };
        assert_eq!(wrote, n);
        let answer: serde_json::Value = serde_json::from_slice(&buf).expect("JSON");
        assert_eq!(answer["duration"], serde_json::json!(2.0));
        assert_eq!(answer["points"].as_array().expect("points").len(), 8);
    }

    /// An axis in hand is read and only widened.
    #[test]
    fn an_axis_in_hand_crosses_the_boundary() {
        let points = [0.0f64, 0.0, 1.0, 0.0, 1.0, 0.5, 1.0, 0.0];
        let mut buf = vec![0u8; 256];
        let n = unsafe {
            clausters_editing_points_props(
                points.as_ptr(),
                points.len(),
                1,
                -4.0,
                4.0,
                9.0,
                buf.as_mut_ptr(),
                buf.len(),
            )
        };
        let answer: serde_json::Value = serde_json::from_slice(&buf[..n]).expect("JSON");
        assert_eq!(answer["min"], serde_json::json!(-4.0));
        assert_eq!(answer["max"], serde_json::json!(4.0));
        assert_eq!(answer["duration"], serde_json::json!(9.0));
    }

    /// The ingestion crosses as one door, and a domain nobody has is quiet.
    #[test]
    fn a_gesture_is_read_through_the_one_door() {
        let request = br#"{"values":[0,4,0.5,0.0]}"#;
        let read = |domain: &str| {
            let mut buf = vec![0u8; 512];
            let n = unsafe {
                clausters_editing_intake(
                    domain.as_ptr(),
                    domain.len(),
                    "sample".as_ptr(),
                    "sample".len(),
                    request.as_ptr(),
                    request.len(),
                    buf.as_mut_ptr(),
                    buf.len(),
                )
            };
            serde_json::from_slice::<serde_json::Value>(&buf[..n]).expect("JSON")
        };
        let wrote = read("samples");
        assert_eq!(wrote["payloads"][0]["intent"], serde_json::json!("write"));
        assert_eq!(wrote["label"], serde_json::json!("draw the samples"));
        assert_eq!(read("clips")["payloads"], serde_json::json!([]));
    }

    /// The instance keeps what it made: a second pass over the same multitrack has
    /// nothing to do.
    #[test]
    fn an_instance_remembers_across_two_calls() {
        let multitrack = br#"{"version":1,"tracks":[{"id":1,"lanes":[{"id":2,"regions":[]}]}]}"#;
        let sources = b"{}";
        let instance = clausters_editing_instance_new();
        let read = |cap: usize| {
            let mut buf = vec![0u8; cap];
            let n = unsafe {
                clausters_editing_instance_reconcile(
                    instance,
                    multitrack.as_ptr(),
                    multitrack.len(),
                    48_000.0,
                    sources.as_ptr(),
                    sources.len(),
                    0.5,
                    buf.as_mut_ptr(),
                    buf.len(),
                )
            };
            serde_json::from_slice::<serde_json::Value>(&buf[..n]).expect("JSON")
        };
        // **A sizing pass changes nothing**, which on this door is not free:
        // reconciling is what teaches the instance what is sounding, so a
        // sizing call that committed would leave the filling call with nothing
        // to say.
        let sized = unsafe {
            clausters_editing_instance_reconcile(
                instance,
                multitrack.as_ptr(),
                multitrack.len(),
                48_000.0,
                sources.as_ptr(),
                sources.len(),
                0.5,
                std::ptr::null_mut(),
                0,
            )
        };
        assert!(sized > 0);
        let first = read(1 << 20);
        assert!(!first.as_array().expect("ops").is_empty());
        assert_eq!(read(1 << 20), serde_json::json!([]), "nothing moved");

        let mut buf = vec![0u8; 4096];
        let n = unsafe { clausters_editing_instance_meters(instance, buf.as_mut_ptr(), buf.len()) };
        let meters: serde_json::Value = serde_json::from_slice(&buf[..n]).expect("JSON");
        assert_eq!(meters[0]["track"], serde_json::json!(1));

        let n =
            unsafe { clausters_editing_instance_teardown(instance, buf.as_mut_ptr(), buf.len()) };
        let ops: serde_json::Value = serde_json::from_slice(&buf[..n]).expect("JSON");
        assert!(!ops.as_array().expect("ops").is_empty());
        unsafe { clausters_editing_instance_free(instance) };
    }
}
