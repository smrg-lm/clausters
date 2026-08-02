//! The sample-clock model: anchors in, a linear map out.

use super::*;

// ---- sample-clock tracking model ----

/// A new least-squares model at `nominal_rate` keeping `window` anchors.
/// Free with [`clausters_clocksync_free`].
#[unsafe(no_mangle)]
pub extern "C" fn clausters_clocksync_new(
    nominal_rate: f64,
    window: usize,
) -> *mut SampleClockModel {
    Box::into_raw(Box::new(SampleClockModel::new(nominal_rate, window)))
}

/// Frees a model created by [`clausters_clocksync_new`] (null is a no-op).
///
/// # Safety
/// `h` must be a pointer from `clausters_clocksync_new`, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_clocksync_free(h: *mut SampleClockModel) {
    if !h.is_null() {
        // SAFETY: caller guarantees `h` came from Box::into_raw above.
        drop(unsafe { Box::from_raw(h) });
    }
}

/// Adds an anchor `(t_local, sample)` and refits; a positive finite `rate`
/// updates the nominal rate (pass `<= 0` to keep it).
///
/// # Safety
/// `h` must be a live model handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_clocksync_add_anchor(
    h: *mut SampleClockModel,
    t_local: f64,
    sample: i64,
    rate: f64,
) {
    if let Some(m) = unsafe { h.as_mut() } {
        m.add_anchor(t_local, sample, rate);
    }
}

/// The predicted counter at local time `t_local` (0 for a null handle).
///
/// # Safety
/// `h` must be a live model handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_clocksync_sample_at(
    h: *mut SampleClockModel,
    t_local: f64,
) -> i64 {
    unsafe { h.as_ref() }.map_or(0, |m| m.sample_at(t_local))
}

/// Inverse: the local time the counter reaches `sample`.
///
/// # Safety
/// `h` must be a live model handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_clocksync_local_time_of(
    h: *mut SampleClockModel,
    sample: i64,
) -> f64 {
    unsafe { h.as_ref() }.map_or(0.0, |m| m.local_time_of(sample))
}

/// Fitted-slope deviation from the nominal rate, in ppm.
///
/// # Safety
/// `h` must be a live model handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_clocksync_drift_ppm(h: *mut SampleClockModel) -> f64 {
    unsafe { h.as_ref() }.map_or(0.0, SampleClockModel::drift_ppm)
}

/// Local-time span covered by the anchor window (0 below two anchors).
///
/// # Safety
/// `h` must be a live model handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_clocksync_span(h: *mut SampleClockModel) -> f64 {
    unsafe { h.as_ref() }.map_or(0.0, SampleClockModel::span)
}

/// The nominal (or last reported) sample rate.
///
/// # Safety
/// `h` must be a live model handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_clocksync_rate(h: *mut SampleClockModel) -> f64 {
    unsafe { h.as_ref() }.map_or(0.0, SampleClockModel::rate)
}

/// Fitted slope `b` (samples per local second).
///
/// # Safety
/// `h` must be a live model handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_clocksync_slope(h: *mut SampleClockModel) -> f64 {
    unsafe { h.as_ref() }.map_or(0.0, SampleClockModel::slope)
}

/// Fitted intercept `a` (samples at local time 0).
///
/// # Safety
/// `h` must be a live model handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_clocksync_intercept(h: *mut SampleClockModel) -> f64 {
    unsafe { h.as_ref() }.map_or(0.0, SampleClockModel::intercept)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clocksync_and_rng_and_timetags_over_the_abi() {
        let h = clausters_clocksync_new(48_000.0, 64);
        unsafe {
            for i in 0..6 {
                let t = i as f64 * 0.05;
                clausters_clocksync_add_anchor(h, t, (1000.0 + 48_000.0 * t) as i64, 48_000.0);
            }
            assert!((clausters_clocksync_sample_at(h, 1.0) - 49_000).abs() <= 1);
            assert!(clausters_clocksync_drift_ppm(h).abs() < 1.0);
            assert_eq!(clausters_clocksync_rate(h), 48_000.0);
            clausters_clocksync_free(h);
        }

        // The flat-state RNG resumes the same stream as the library type.
        let mut state = clausters_rng_seed(1);
        let mut expect = Rng::from_seed(1);
        for _ in 0..100 {
            assert_eq!(
                unsafe { clausters_rng_next_f64(&mut state) },
                expect.next_f64()
            );
        }
        // ...including the raw word used to derive child-stream seeds.
        assert_eq!(
            unsafe { clausters_rng_next_u64(&mut state) },
            expect.next_u64()
        );

        // Timetag packing matches the core's rounding rule.
        assert_eq!(
            clausters_core_ntp_timetag(10.75),
            (10u64 << 32) | ((3u64) << 30)
        );
        let bits = clausters_core_unix_to_ntp(0.0);
        assert_eq!(bits >> 32, 2_208_988_800);

        assert_eq!(clausters_core_quant_delay(3.5, 4.0), 0.5);
        let major = [0.0f32, 2.0, 4.0, 5.0, 7.0, 9.0, 11.0];
        assert_eq!(
            unsafe { clausters_core_degree_to_midinote(-1.0, 5.0, 0.0, major.as_ptr(), 7) },
            59.0
        );
    }
}
