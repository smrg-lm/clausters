//! The peak and RMS a render reports back.

/// Peak magnitude and RMS of channel `channel` of the **interleaved** buffer
/// `samples` (`n` `f32`s across `channels` channels), written to `out[0]` and
/// `out[1]`. Returns 0, or -1 on a null pointer or an out-of-range channel.
///
/// The stride walk means a caller measures a render without deinterleaving it
/// first, and reads the same numbers the server would.
///
/// # Safety
/// `samples` must be readable for `n` `f32`s and `out` writable for 2.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_stats(
    samples: *const f32,
    n: usize,
    channels: usize,
    channel: usize,
    out: *mut f32,
) -> i32 {
    if samples.is_null() || out.is_null() || channels == 0 || channel >= channels {
        return -1;
    }
    // SAFETY: caller guarantees `samples` is readable for `n` and `out` for 2.
    let s = unsafe { std::slice::from_raw_parts(samples, n) };
    let (peak, rms) = clausters_core::measure::channel_stats(s, channels, channel);
    // SAFETY: caller contract.
    unsafe {
        *out = peak;
        *out.add(1) = rms;
    }
    0
}
