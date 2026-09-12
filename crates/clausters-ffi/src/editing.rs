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
