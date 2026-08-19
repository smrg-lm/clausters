//! The numeric builtins: the op tables, noise, smoothing windows, the peak-pyramid caches and the stereo-field measurements.

use super::*;
use clausters_core::builtins::{self, BinaryOp, UnaryOp};
use clausters_core::measure;

/// Applies unary `op` to `input` (broadcast if `in_len == 1`) into `out`,
/// writing `n` samples. Returns 0 on success, -1 on an unknown op, -2 on a
/// null pointer or a non-broadcast length mismatch.
///
/// # Safety
/// `input` must be readable for `in_len` `f32`s and `out` writable for `n`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_unary(
    op: u32,
    input: *const f32,
    in_len: usize,
    out: *mut f32,
    n: usize,
) -> i32 {
    let Some(op) = UnaryOp::from_u32(op) else {
        return -1;
    };
    if input.is_null() || out.is_null() || (in_len != 1 && in_len != n) {
        return -2;
    }
    // SAFETY: caller guarantees the ranges above.
    let a = unsafe { std::slice::from_raw_parts(input, in_len) };
    let o = unsafe { std::slice::from_raw_parts_mut(out, n) };
    builtins::unary_slice(op, a, o);
    0
}

/// Applies binary `op` to `a` and `b` (each broadcast if its length is 1) into
/// `out`, writing `n` samples. Same return codes as
/// [`clausters_core_unary`].
///
/// # Safety
/// `a`/`b` must be readable for `a_len`/`b_len` `f32`s, `out` writable for `n`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_binary(
    op: u32,
    a: *const f32,
    a_len: usize,
    b: *const f32,
    b_len: usize,
    out: *mut f32,
    n: usize,
) -> i32 {
    let Some(op) = BinaryOp::from_u32(op) else {
        return -1;
    };
    let ok_len = |l: usize| l == 1 || l == n;
    if a.is_null() || b.is_null() || out.is_null() || !ok_len(a_len) || !ok_len(b_len) {
        return -2;
    }
    // SAFETY: caller guarantees the ranges above.
    let av = unsafe { std::slice::from_raw_parts(a, a_len) };
    let bv = unsafe { std::slice::from_raw_parts(b, b_len) };
    let o = unsafe { std::slice::from_raw_parts_mut(out, n) };
    builtins::binary_slice(op, av, bv, o);
    0
}

/// Fills `out` with `n` white-noise samples from `seed`, identical to the
/// server's `WhiteNoise` UGen seeded the same way.
///
/// # Safety
/// `out` must be writable for `n` `f32`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_whitenoise(seed: u64, out: *mut f32, n: usize) {
    if out.is_null() {
        return;
    }
    // SAFETY: caller guarantees `out` is writable for `n`.
    let o = unsafe { std::slice::from_raw_parts_mut(out, n) };
    WhiteNoise::from_seed(seed).fill(o);
}

/// Fills `out` (`n` samples) with the smoothing window of type `wintype` — the
/// **same** `clausters_core::window::Window` the server's `FFT`/`IFFT` UGens
/// apply, so a client that pre-windows audio matches the server bit for bit.
/// `wintype`: -1 rectangular, 0 Hann, 1 sine, 2 Welch, 3 Hamming, 4 Blackman
/// (any other value falls back to Hann, as the server does).
///
/// # Safety
/// `out` must be writable for `n` `f32`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_window(wintype: i32, out: *mut f32, n: usize) {
    if out.is_null() {
        return;
    }
    // SAFETY: caller guarantees `out` is writable for `n`.
    let o = unsafe { std::slice::from_raw_parts_mut(out, n) };
    Window::from_wintype(wintype).fill(o);
}

/// The exact byte length of the peak-pyramid cache for `n` samples at
/// `base_bucket` — call it to size the buffer for [`clausters_core_peaks_build`]
/// without building the pyramid. Returns 0 if `base_bucket == 0`.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_peaks_cache_size(n: usize, base_bucket: usize) -> usize {
    if base_bucket == 0 {
        return 0;
    }
    peaks::cache_size(n, base_bucket)
}

/// Builds a min/max peak pyramid from `samples` (mono, `n` `f32`s) at
/// `base_bucket` and writes its cache bytes — the memory-mappable format the GUI
/// host maps to render a waveform without re-sending the samples — into `out`
/// (capacity `out_cap`). Returns the number of bytes written, or 0 on a null
/// pointer, `base_bucket == 0`, or `out_cap` below
/// [`clausters_core_peaks_cache_size`]`(n, base_bucket)`.
///
/// # Safety
/// `samples` must be readable for `n` `f32`s and `out` writable for `out_cap`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_peaks_build(
    samples: *const f32,
    n: usize,
    base_bucket: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    if samples.is_null() || out.is_null() || base_bucket == 0 {
        return 0;
    }
    // SAFETY: caller guarantees `samples` is readable for `n` `f32`s.
    let s = unsafe { std::slice::from_raw_parts(samples, n) };
    let cache = Pyramid::build(s, base_bucket).to_bytes();
    if cache.len() > out_cap {
        return 0;
    }
    // SAFETY: out is writable for out_cap >= cache.len().
    let o = unsafe { std::slice::from_raw_parts_mut(out, cache.len()) };
    o.copy_from_slice(&cache);
    cache.len()
}

/// The exact byte length of the **multichannel** peak-pyramid cache for
/// `frames` samples per channel across `channels` channels at `base_bucket` —
/// sizes the buffer for [`clausters_core_peaks_multi_build`] without building.
/// Returns 0 if `base_bucket == 0`.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_peaks_multi_cache_size(
    frames: usize,
    channels: usize,
    base_bucket: usize,
) -> usize {
    if base_bucket == 0 {
        return 0;
    }
    peaks::multi_cache_size(frames, channels, base_bucket)
}

/// Builds the multichannel peak-pyramid cache from `samples` (`n` `f32`s
/// holding `channels` interleaved channels; a trailing partial frame is
/// ignored) at `base_bucket`, writing the version-2 cache bytes — the single
/// mappable resource an editor-grade waveform names as its `cache` — into
/// `out` (capacity `out_cap`). Returns the bytes written, or 0 on a null
/// pointer, `base_bucket == 0` / `channels == 0`, or a too-small `out_cap`.
///
/// # Safety
/// `samples` must be readable for `n` `f32`s and `out` writable for `out_cap`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_peaks_multi_build(
    samples: *const f32,
    n: usize,
    channels: usize,
    base_bucket: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    if samples.is_null() || out.is_null() || base_bucket == 0 || channels == 0 {
        return 0;
    }
    // SAFETY: caller guarantees `samples` is readable for `n` `f32`s.
    let s = unsafe { std::slice::from_raw_parts(samples, n) };
    let cache = MultiPyramid::build_interleaved(s, channels, base_bucket).to_bytes();
    if cache.len() > out_cap {
        return 0;
    }
    // SAFETY: out is writable for out_cap >= cache.len().
    let o = unsafe { std::slice::from_raw_parts_mut(out, cache.len()) };
    o.copy_from_slice(&cache);
    cache.len()
}

/// Rewrites an existing multichannel cache over the **frame span an edit
/// touched**, in place: `cache` is parsed, the buckets `[start, start+frames)`
/// overlaps are rebuilt from `samples`, and the bytes are written back over the
/// same buffer (the shape is unchanged, so the length is too).
///
/// This is what keeps an editor's overview true without re-summarizing the
/// take: the owner applies an edit to its working copy and updates the span it
/// touched, and the picture that reads the cache follows. The result is
/// identical to rebuilding the cache from the edited material, which is
/// asserted core-side rather than assumed.
///
/// `samples` is the **whole** buffer as it now stands, interleaved — a bucket
/// at either edge of the span holds untouched samples too. Returns the bytes
/// written, or 0 on a null pointer, an unparseable cache, a buffer that is not
/// the one the cache describes, or a cache whose re-serialization does not fit
/// (which cannot happen for an unchanged shape and is checked rather than
/// trusted).
///
/// There is no mono sibling: a one-channel cache *is* a multichannel one with
/// one channel (the format has said so since v3), so a second entry point would
/// be a second spelling of the same call.
///
/// # Safety
/// `cache` must be readable and writable for `cache_len` bytes, and `samples`
/// readable for `n` `f32`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_peaks_multi_update(
    cache: *mut u8,
    cache_len: usize,
    samples: *const f32,
    n: usize,
    start: usize,
    frames: usize,
) -> usize {
    if cache.is_null() || samples.is_null() || cache_len == 0 {
        return 0;
    }
    // SAFETY: caller guarantees the two ranges.
    let (bytes, s) = unsafe {
        (
            std::slice::from_raw_parts_mut(cache, cache_len),
            std::slice::from_raw_parts(samples, n),
        )
    };
    let Some(mut pyr) = MultiPyramid::from_bytes(bytes) else {
        return 0;
    };
    if !pyr.update_range(s, start, frames) {
        return 0;
    }
    let out = pyr.to_bytes();
    if out.len() != cache_len {
        return 0;
    }
    bytes.copy_from_slice(&out);
    cache_len
}

/// Writes the cache bytes of an **empty** multichannel pyramid over `frames`
/// frames of `channels` channels at `base_bucket` — the summary of a take that
/// has been allocated and not yet recorded into.
///
/// Its sibling [`clausters_core_peaks_multi_build`] needs the samples to
/// summarize; this one has none to read, which is the point: a client that
/// will fill the cache from `/buffer_stream` reports
/// ([`clausters_core_peaks_multi_write_buckets`]) would otherwise allocate the
/// whole take in silence to summarize what nobody wrote. Size the buffer with
/// [`clausters_core_peaks_multi_cache_size`], as for the builder.
///
/// Returns the bytes written, or 0 on a null pointer, `base_bucket == 0` /
/// `channels == 0`, or a too-small `out_cap`.
///
/// # Safety
/// `out` must be writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_peaks_multi_empty(
    frames: usize,
    channels: usize,
    base_bucket: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    if out.is_null() || base_bucket == 0 || channels == 0 {
        return 0;
    }
    let cache = MultiPyramid::empty(frames, channels, base_bucket).to_bytes();
    if cache.len() > out_cap {
        return 0;
    }
    // SAFETY: out is writable for out_cap >= cache.len().
    let o = unsafe { std::slice::from_raw_parts_mut(out, cache.len()) };
    o.copy_from_slice(&cache);
    cache.len()
}

/// Folds a run of **already-summarized buckets** into an existing multichannel
/// cache, in place — the receiving half of `/buffer_stream`, which sends the
/// overview of material as it is written instead of the material.
///
/// `stats` is the reply's blob read as `n` `f32`s, **bucket-major and
/// channel-minor**: for each bucket of `bucket` frames in order, for each
/// channel, `min`, `max` and mean square. `start_frame` is where the report
/// begins on the buffer's own sample axis. Nothing here measures anything: the
/// writer measured, and this puts the buckets where they belong and rebuilds
/// the levels above them, so a client that cannot reach the samples still
/// draws the picture the samples would have built.
///
/// Returns the bytes written, or 0 on a null pointer, an unparseable cache, a
/// report on another grid (a `bucket` that is not the cache's own, a
/// `start_frame` off a bucket boundary, a run past the end, or a length that
/// is not whole buckets across every channel), or a re-serialization that does
/// not fit — which cannot happen for an unchanged shape and is checked rather
/// than trusted. A refused report changes nothing.
///
/// # Safety
/// `cache` must be readable and writable for `cache_len` bytes, and `stats`
/// readable for `n` `f32`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_peaks_multi_write_buckets(
    cache: *mut u8,
    cache_len: usize,
    start_frame: usize,
    bucket: usize,
    stats: *const f32,
    n: usize,
) -> usize {
    if cache.is_null() || stats.is_null() || cache_len == 0 {
        return 0;
    }
    // SAFETY: caller guarantees the two ranges.
    let (bytes, s) = unsafe {
        (
            std::slice::from_raw_parts_mut(cache, cache_len),
            std::slice::from_raw_parts(stats, n),
        )
    };
    let Some(mut pyr) = MultiPyramid::from_bytes(bytes) else {
        return 0;
    };
    if !pyr.write_buckets(start_frame, bucket, s) {
        return 0;
    }
    let out = pyr.to_bytes();
    if out.len() != cache_len {
        return 0;
    }
    bytes.copy_from_slice(&out);
    cache_len
}

/// The stereo **correlation** (Pearson's r) of channels `left` and `right`
/// (each `n` `f32`s): `+1` mono/in-phase, `0` decorrelated, `-1` anti-phase —
/// the same measurement the GUI phasescope shows. Writes the coefficient into
/// `*out` and returns 0; returns -1 (leaving `*out` untouched) when it is
/// undefined (`n == 0` or a constant channel — silence/DC) or on a null pointer.
///
/// # Safety
/// `left`/`right` must be readable for `n` `f32`s and `out` writable for one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_correlation(
    left: *const f32,
    right: *const f32,
    n: usize,
    out: *mut f32,
) -> i32 {
    if left.is_null() || right.is_null() || out.is_null() {
        return -1;
    }
    // SAFETY: caller guarantees both channels are readable for `n` `f32`s.
    let l = unsafe { std::slice::from_raw_parts(left, n) };
    let r = unsafe { std::slice::from_raw_parts(right, n) };
    match measure::correlation(l, r) {
        Some(v) => {
            // SAFETY: `out` is writable for one `f32`.
            unsafe { *out = v };
            0
        }
        None => -1,
    }
}

/// Maps `n` stereo pairs (`left`, `right`) to their **Lissajous / goniometer**
/// coordinates, writing `2 * n` interleaved `f32`s `[x0, y0, x1, y1, …]` into
/// `out`, where `x` is the side component `(L − R)/√2` and `y` the mid
/// `(L + R)/√2` — the 45°-rotated stereo plane a goniometer draws. Returns 0, or
/// -1 (leaving `out` untouched) on a null pointer.
///
/// # Safety
/// `left`/`right` must be readable for `n` `f32`s and `out` writable for `2 * n`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_lissajous(
    left: *const f32,
    right: *const f32,
    n: usize,
    out: *mut f32,
) -> i32 {
    if left.is_null() || right.is_null() || out.is_null() {
        return -1;
    }
    // SAFETY: caller guarantees the ranges; `[f32; 2]` is two consecutive
    // `f32`s, so the `out` buffer of `2 * n` floats aliases `n` pairs.
    let l = unsafe { std::slice::from_raw_parts(left, n) };
    let r = unsafe { std::slice::from_raw_parts(right, n) };
    let o = unsafe { std::slice::from_raw_parts_mut(out as *mut [f32; 2], n) };
    if measure::lissajous_into(l, r, o) {
        0
    } else {
        -1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unary_and_binary_over_arrays() {
        let a = [1.0f32, 2.0, 3.0];
        let b = [10.0f32];
        let mut out = [0.0f32; 3];
        // add with a broadcast constant
        let rc = unsafe {
            clausters_core_binary(
                BinaryOp::Add as u32,
                a.as_ptr(),
                3,
                b.as_ptr(),
                1,
                out.as_mut_ptr(),
                3,
            )
        };
        assert_eq!(rc, 0);
        assert_eq!(out, [11.0, 12.0, 13.0]);

        let rc = unsafe {
            clausters_core_unary(UnaryOp::Neg as u32, a.as_ptr(), 3, out.as_mut_ptr(), 3)
        };
        assert_eq!(rc, 0);
        assert_eq!(out, [-1.0, -2.0, -3.0]);
    }

    #[test]
    fn rejects_unknown_op_and_bad_length() {
        let mut out = [0.0f32; 2];
        let a = [1.0f32, 2.0];
        assert_eq!(
            unsafe { clausters_core_unary(9999, a.as_ptr(), 2, out.as_mut_ptr(), 2) },
            -1
        );
        // in_len neither 1 nor n
        assert_eq!(
            unsafe {
                clausters_core_unary(UnaryOp::Abs as u32, a.as_ptr(), 2, out.as_mut_ptr(), 5)
            },
            -2
        );
    }

    #[test]
    fn whitenoise_matches_core() {
        let mut out = [0.0f32; 8];
        unsafe { clausters_core_whitenoise(42, out.as_mut_ptr(), 8) };
        let mut expect = [0.0f32; 8];
        WhiteNoise::from_seed(42).fill(&mut expect);
        assert_eq!(out, expect);
    }

    #[test]
    fn peaks_build_writes_a_parseable_cache() {
        let samples: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.01).sin()).collect();
        let base = 64;
        let size = clausters_core_peaks_cache_size(samples.len(), base);
        assert!(size > 0);
        let mut out = vec![0u8; size];
        let written = unsafe {
            clausters_core_peaks_build(
                samples.as_ptr(),
                samples.len(),
                base,
                out.as_mut_ptr(),
                out.len(),
            )
        };
        assert_eq!(written, size, "writes exactly the predicted size");
        // The bytes are the same cache the GUI host parses, identical to a
        // pyramid built in-process (the algorithm lives once in the core).
        let from_ffi = Pyramid::from_bytes(&out).expect("parse");
        assert_eq!(from_ffi.total_samples(), samples.len());
        assert_eq!(
            Pyramid::build(&samples, base).to_bytes(),
            out,
            "FFI cache is byte-identical to the in-process build"
        );
        // A buffer one byte short writes nothing.
        let mut tiny = vec![0u8; size - 1];
        assert_eq!(
            unsafe {
                clausters_core_peaks_build(
                    samples.as_ptr(),
                    samples.len(),
                    base,
                    tiny.as_mut_ptr(),
                    tiny.len(),
                )
            },
            0
        );
    }

    #[test]
    fn peaks_multi_write_buckets_folds_a_report_into_a_cache() {
        // A take being recorded: the cache starts as the silence the buffer was
        // allocated as, and the reports are all this side ever sees of the
        // material. What it ends up with must be the cache the samples build.
        let (frames, channels, base) = (512, 2, 64);
        let inter: Vec<f32> = (0..frames * channels)
            .map(|i| ((i / channels) as f32 * 0.021 + (i % channels) as f32).sin() * 0.7)
            .collect();
        let silent = vec![0.0f32; frames * channels];
        let mut cache = MultiPyramid::build_interleaved(&silent, channels, base).to_bytes();

        // The writer's own measurement of two buckets, bucket-major and
        // channel-minor -- the layout of `/buffer_stream.reply`'s blob.
        let mut stats: Vec<f32> = Vec::new();
        for b in 0..frames / base {
            for ch in 0..channels {
                let chunk: Vec<f32> = (b * base..(b + 1) * base)
                    .map(|f| inter[f * channels + ch])
                    .collect();
                let (lo, hi) = peaks::min_max(&chunk).unwrap();
                stats.extend([lo, hi, peaks::mean_square(&chunk).unwrap()]);
            }
        }

        let n = cache.len();
        let written = unsafe {
            clausters_core_peaks_multi_write_buckets(
                cache.as_mut_ptr(),
                n,
                0,
                base,
                stats.as_ptr(),
                stats.len(),
            )
        };
        assert_eq!(written, n, "the cache keeps its shape, so its length too");
        assert_eq!(
            cache,
            MultiPyramid::build_interleaved(&inter, channels, base).to_bytes(),
            "a streamed picture is the picture the samples would have built"
        );

        // A report on another grid is refused, and leaves the bytes alone.
        let keep = cache.clone();
        for (start, bucket) in [(0, base * 2), (base / 2, base), (frames, base)] {
            assert_eq!(
                unsafe {
                    clausters_core_peaks_multi_write_buckets(
                        cache.as_mut_ptr(),
                        n,
                        start,
                        bucket,
                        stats.as_ptr(),
                        channels * 3,
                    )
                },
                0,
                "start {start}, bucket {bucket}"
            );
        }
        assert_eq!(cache, keep, "a refused report changes nothing");
    }

    #[test]
    fn peaks_multi_build_writes_a_parseable_multichannel_cache() {
        // Interleaved stereo; the FFI cache must be byte-identical to the
        // in-process multichannel build (one algorithm, in the core).
        let inter: Vec<f32> = (0..2000).map(|i| (i as f32 * 0.01).sin()).collect();
        let (frames, channels, base) = (1000, 2, 64);
        let size = clausters_core_peaks_multi_cache_size(frames, channels, base);
        assert!(size > 0);
        let mut out = vec![0u8; size];
        let written = unsafe {
            clausters_core_peaks_multi_build(
                inter.as_ptr(),
                inter.len(),
                channels,
                base,
                out.as_mut_ptr(),
                out.len(),
            )
        };
        assert_eq!(written, size, "writes exactly the predicted size");
        let parsed = MultiPyramid::from_bytes(&out).expect("parse");
        assert_eq!(parsed.num_channels(), channels);
        assert_eq!(parsed.frames(), frames);
        assert_eq!(
            MultiPyramid::build_interleaved(&inter, channels, base).to_bytes(),
            out
        );
        // Refusals: zero channels/bucket and a too-small buffer.
        assert_eq!(
            clausters_core_peaks_multi_cache_size(frames, channels, 0),
            0
        );
        let mut tiny = vec![0u8; size - 1];
        assert_eq!(
            unsafe {
                clausters_core_peaks_multi_build(
                    inter.as_ptr(),
                    inter.len(),
                    channels,
                    base,
                    tiny.as_mut_ptr(),
                    tiny.len(),
                )
            },
            0
        );
    }

    #[test]
    fn correlation_matches_the_core_and_flags_the_undefined_case() {
        let l: Vec<f32> = (0..128).map(|i| (i as f32 * 0.2).sin()).collect();
        let neg: Vec<f32> = l.iter().map(|s| -s).collect();
        let mut r = 0.0f32;
        assert_eq!(
            unsafe { clausters_core_correlation(l.as_ptr(), l.as_ptr(), l.len(), &mut r) },
            0
        );
        assert!((r - 1.0).abs() < 1e-5, "mono reads +1");
        assert_eq!(
            unsafe { clausters_core_correlation(l.as_ptr(), neg.as_ptr(), l.len(), &mut r) },
            0
        );
        assert!((r + 1.0).abs() < 1e-5, "anti-phase reads -1");
        // A constant channel is undefined: -1, and `r` is left as it was.
        let flat = vec![0.5f32; 128];
        let before = r;
        assert_eq!(
            unsafe { clausters_core_correlation(flat.as_ptr(), l.as_ptr(), l.len(), &mut r) },
            -1
        );
        assert_eq!(r, before, "out untouched on the undefined case");
    }

    #[test]
    fn lissajous_writes_interleaved_side_mid_pairs() {
        let l = [1.0f32, 0.7, 0.0];
        let r = [1.0f32, -0.7, 0.0];
        let mut out = vec![0.0f32; l.len() * 2];
        assert_eq!(
            unsafe { clausters_core_lissajous(l.as_ptr(), r.as_ptr(), l.len(), out.as_mut_ptr()) },
            0
        );
        // Mono pair (1,1): side 0, mid √2. Anti pair (0.7,-0.7): side √2·0.7, mid 0.
        assert!(out[0].abs() < 1e-6 && (out[1] - std::f32::consts::SQRT_2).abs() < 1e-6);
        assert!((out[2] - std::f32::consts::SQRT_2 * 0.7).abs() < 1e-6 && out[3].abs() < 1e-6);
    }
}
