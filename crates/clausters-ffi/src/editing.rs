//! The projections an editable structure owes its endpoints.
//!
//! The C half of [`clausters_editing`]. Sizes with a null `out` and fills with
//! a second call, like the rest of the JSON surface here — a projection is a
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

/// **A piece as the props the multitrack widget is drawn with**, as JSON.
///
/// The rows, the boxes, the automations over both, their break-points, which
/// are hidden and which boxes loop — everything a piece has from the document
/// alone. What a caller adds is what is a function of something else: the
/// position cursor, the meter buses, the widget's own chrome.
///
/// `sources` is the same table [`crate::document`]'s instance plan takes,
/// source id to `{"buffer", "channels"}`. Sizes with a null `out` and fills
/// with a second call.
///
/// # Safety
/// `piece` and `sources` must be null or readable for their lengths, and `out`
/// null or writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_multitrack_props(
    piece: *const u8,
    piece_len: usize,
    rate: f64,
    default_bpm: f64,
    sources: *const u8,
    sources_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let (Some(piece), Some(sources)) =
        (unsafe { crate::document::text(piece, piece_len) }, unsafe {
            crate::document::text(sources, sources_len)
        })
    else {
        return 0;
    };
    let answer = clausters_editing::multitrack::props_json(&piece, rate, default_bpm, &sources);
    // SAFETY: forwarded from this function's own contract. A pure read, so
    // there is nothing to commit.
    unsafe { crate::document::fill(answer.as_bytes(), out, out_cap, || {}) }
}

/// **What a gesture means, in a structure's own vocabulary** — the edit
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

/// **What is sounding of a piece**, held across edits.
///
/// The instance projection's state: an opaque handle, because it is the one
/// projection with memory and shipping it out and back in on every edit would
/// carry every curve's table twice for nothing. Free it with
/// [`clausters_editing_instance_free`].
pub struct FfiInstance(std::sync::Mutex<clausters_editing::instance::Instance>);

/// A new instance: nothing of the piece is sounding yet.
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

/// **The difference between what is sounding and what the piece says**, as the
/// JSON list of operations a client applies.
///
/// The same four arguments the instance plan takes — the piece, the rate, the
/// tempo a piece that states none is read at, and the source table — plus the
/// master's own level, which is the caller's and not the piece's. Everything
/// already right is left alone, which is what lets a hand drag a box without
/// hearing the rest of the piece restart.
///
/// An operation names what it acts on by a **handle**, never by a node id, a
/// bus index or a buffer number: this allocates none of those, and the client
/// keeps the one table from handle to whatever it made.
///
/// Sizes with a null `out` and fills with a second call.
///
/// # Safety
/// `i` must be null or a live pointer from `clausters_editing_instance_new`,
/// `piece` and `sources` null or readable for their lengths, and `out` null or
/// writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_editing_instance_reconcile(
    i: *mut FfiInstance,
    piece: *const u8,
    piece_len: usize,
    sample_rate: f64,
    default_bpm: f64,
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
    let (Some(piece), Some(sources)) =
        (unsafe { crate::document::text(piece, piece_len) }, unsafe {
            crate::document::text(sources, sources_len)
        })
    else {
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
        &piece,
        sample_rate,
        default_bpm,
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

/// **Everything this made, given back** — the operations that stop the piece.
///
/// The piece itself is untouched: what an instance holds is nodes, and nodes
/// are not the composition. Afterwards it believes nothing is sounding, and the
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

/// **Which control bus run each track's meters write**, by track — what a host
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

    /// The instance keeps what it made: a second pass over the same piece has
    /// nothing to do.
    #[test]
    fn an_instance_remembers_across_two_calls() {
        let piece = br#"{"version":1,"tracks":[{"id":1,"lanes":[{"id":2,"regions":[]}]}]}"#;
        let sources = b"{}";
        let instance = clausters_editing_instance_new();
        let read = |cap: usize| {
            let mut buf = vec![0u8; cap];
            let n = unsafe {
                clausters_editing_instance_reconcile(
                    instance,
                    piece.as_ptr(),
                    piece.len(),
                    48_000.0,
                    60.0,
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
                piece.as_ptr(),
                piece.len(),
                48_000.0,
                60.0,
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
