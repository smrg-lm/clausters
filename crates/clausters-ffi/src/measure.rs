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

/// **The true peak** of channel `channel` of the **interleaved** buffer
/// `samples` (`n` `f32`s across `channels` channels), in linear amplitude, or
/// a negative value on a null pointer or an out-of-range channel.
///
/// The reconstructed peak, not the largest sample: the ITU-R BS.1770-4 Annex 2
/// interpolation filter at 4×, which is what makes the reading dBTP. It is
/// always at or above [`clausters_core_stats`]'s peak, by up to about 3 dB.
///
/// # Safety
/// `samples` must be readable for `n` `f32`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_true_peak(
    samples: *const f32,
    n: usize,
    channels: usize,
    channel: usize,
) -> f32 {
    if samples.is_null() || channels == 0 || channel >= channels {
        return -1.0;
    }
    // SAFETY: caller guarantees `samples` is readable for `n`.
    let s = unsafe { std::slice::from_raw_parts(samples, n) };
    clausters_core::resample::true_peak(s, channels, channel)
}
