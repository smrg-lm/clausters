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
    unsafe { crate::out::fill(answer.as_bytes(), out, out_cap) }
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
        unsafe { crate::out::text(multitrack, multitrack_len) },
        unsafe { crate::out::text(sources, sources_len) },
    ) else {
        return 0;
    };
    let answer = clausters_editing::multitrack::props_json(&multitrack, rate, &sources);
    // SAFETY: forwarded from this function's own contract. A pure read, so
    // there is nothing to commit.
    unsafe { crate::out::fill(answer.as_bytes(), out, out_cap) }
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
        unsafe { crate::out::text(domain, domain_len) },
        unsafe { crate::out::text(tag, tag_len) },
        unsafe { crate::out::text(request, request_len) },
    ) else {
        return 0;
    };
    let answer = clausters_editing::intake_json(&domain, &tag, &request);
    // SAFETY: forwarded from this function's own contract. A pure read, so
    // there is nothing to commit.
    unsafe { crate::out::fill(answer.as_bytes(), out, out_cap) }
}

/// **A played event's curves, as the values a client sends**: the
/// `/node_set`s of every instant of one window.
///
/// `request` is `clausters_editing::event_curves::Request` as JSON -- the
/// step, the stretch of time, the notes that sound with their own curves and
/// the channels' curves -- and the answer is
/// `clausters_editing::event_curves::window_json`'s: `{"bundles", "states",
/// "over", "idle"}`.
///
/// Sizes with a null `out` and fills with a second call.
///
/// # Safety
/// `request` must be null or readable for `request_len` bytes, and `out` null
/// or writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_event_curves(
    request: *const u8,
    request_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let Some(request) = (unsafe { crate::out::text(request, request_len) }) else {
        return 0;
    };
    let answer = clausters_editing::event_curves::window_json(&request);
    // SAFETY: forwarded from this function's own contract. A pure read.
    unsafe { crate::out::fill(answer.as_bytes(), out, out_cap) }
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
    let Some(request) = (unsafe { crate::out::text(request, request_len) }) else {
        return 0;
    };
    let Ok(mut held) = runner.0.lock() else {
        return 0;
    };
    let mut next = held.clone();
    let answer = clausters_editing::run::call_json(&mut next, &request);
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        crate::out::fill_then(answer.as_bytes(), out, out_cap, || {
            *held = next;
        })
    }
}

/// **The notes editor, as it is playing**
/// (`clausters_editing::notes_playback::NotesPlayback`): its structure and its
/// plan on its own transport. Free it with
/// [`clausters_editing_notes_playback_free`].
pub struct FfiNotesPlayback(std::sync::Mutex<clausters_editing::notes_playback::NotesPlayback>);

/// A new notes playback on `transport` -- negative for one of its own, which
/// it takes from the id spaces with its `open` verb or its first play and its
/// `close` verb gives back.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_editing_notes_playback_new(transport: i32) -> *mut FfiNotesPlayback {
    use clausters_editing::notes_playback::NotesPlayback;
    let playback = if transport < 0 {
        NotesPlayback::new()
    } else {
        NotesPlayback::on(transport)
    };
    Box::into_raw(Box::new(FfiNotesPlayback(std::sync::Mutex::new(playback))))
}

/// Frees a playback created by [`clausters_editing_notes_playback_new`] (null
/// is a no-op). The bookkeeping, not the nodes: `close` answers the steps that
/// free those.
///
/// # Safety
/// `p` must be a pointer from `clausters_editing_notes_playback_new`, not yet
/// freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_notes_playback_free(p: *mut FfiNotesPlayback) {
    if !p.is_null() {
        // SAFETY: caller guarantees `p` came from Box::into_raw above.
        drop(unsafe { Box::from_raw(p) });
    }
}

/// **One verb of the notes playback**, as JSON
/// (`clausters_editing::notes_playback::call_json`), over the sequence
/// `sequence` names: `{"steps": [...]}`, a query's own object, or
/// `{"error": ...}`. Allocates from `ids`. Sizes with a null `out` and fills
/// with a second call; only the call that fills changes anything.
///
/// # Safety
/// `p`, `sequence` and `ids` null or live, `request` null or readable for
/// `request_len` bytes, `out` null or writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_notes_playback_call(
    p: *mut FfiNotesPlayback,
    sequence: *mut crate::document::FfiEventSequence,
    request: *const u8,
    request_len: usize,
    ids: *mut crate::registry::FfiIdSpaces,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let Some(request) = (unsafe { crate::out::text(request, request_len) }) else {
        return 0;
    };
    // SAFETY: forwarded from this function's own contract.
    let (Some(playback), Some(sequence), Some(spaces)) = (
        unsafe { p.as_ref() },
        unsafe { sequence.as_ref() },
        unsafe { ids.as_ref() },
    ) else {
        return 0;
    };
    let (Ok(mut held), Ok(mut spaces), Ok(sequence)) =
        (playback.0.lock(), spaces.0.lock(), sequence.0.lock())
    else {
        return 0;
    };
    let (mut next, mut next_ids) = (held.clone(), spaces.clone());
    let answer =
        clausters_editing::notes_playback::call_json(&mut next, &sequence, &request, &mut next_ids);
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        crate::out::fill_then(answer.as_bytes(), out, out_cap, || {
            *held = next;
            *spaces = next_ids;
        })
    }
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
    let (Some(state), Some(message)) = (unsafe { crate::out::text(state, state_len) }, unsafe {
        crate::out::text(message, message_len)
    }) else {
        return 0;
    };
    let answer = clausters_editing::conversation::read_json(&state, &message);
    // SAFETY: forwarded from this function's own contract. The conversation's
    // state comes back in the answer rather than being kept here, so this is a
    // pure read and a sizing pass changes nothing.
    unsafe { crate::out::fill(answer.as_bytes(), out, out_cap) }
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
    let Some(request) = (unsafe { crate::out::text(request, request_len) }) else {
        return 0;
    };
    let answer = clausters_editing::conversation::answer_json(&request);
    // SAFETY: forwarded from this function's own contract. A pure read.
    unsafe { crate::out::fill(answer.as_bytes(), out, out_cap) }
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
    let Some(multitrack) = (unsafe { crate::out::text(multitrack, multitrack_len) }) else {
        return 0;
    };
    let answer = clausters_editing::multitrack::names_json(&multitrack);
    // SAFETY: forwarded from this function's own contract. A pure read.
    unsafe { crate::out::fill(answer.as_bytes(), out, out_cap) }
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
}
