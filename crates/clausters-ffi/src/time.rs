//! Beats, seconds, samples and NTP timetags — the conversions every client must agree on.

use super::*;

/// Seconds at `beats` for the affine clock `(tempo, base_beats, base_seconds)`.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_beats_to_secs(
    tempo: f64,
    base_beats: f64,
    base_seconds: f64,
    beats: f64,
) -> f64 {
    base_seconds + (beats - base_beats) / tempo
}

/// Beats at `secs` for the affine clock `(tempo, base_beats, base_seconds)`.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_secs_to_beats(
    tempo: f64,
    base_beats: f64,
    base_seconds: f64,
    secs: f64,
) -> f64 {
    base_beats + (secs - base_seconds) * tempo
}

/// Seconds → sample count at `sample_rate` (ties to even).
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_secs_to_samples(secs: f64, sample_rate: f64) -> i64 {
    tempoclock::secs_to_samples(secs, sample_rate)
}

/// Sample count → seconds at `sample_rate`.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_samples_to_secs(samples: i64, sample_rate: f64) -> f64 {
    tempoclock::samples_to_secs(samples, sample_rate)
}

/// The server's sample counter at Unix instant `unix_secs`, from an anchor
/// (`anchor_sample` at `anchor_unix`) and the sample rate — the `/sched_at`
/// target conversion.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_unix_to_sample(
    unix_secs: f64,
    anchor_unix: f64,
    anchor_sample: i64,
    sample_rate: f64,
) -> i64 {
    clausters_core::osc::unix_to_sample(unix_secs, anchor_unix, anchor_sample, sample_rate)
}

/// Beats to wait so a routine starts on the next `quant` boundary of a grid
/// currently at `pos` beats (`quant <= 0` → 0, i.e. now).
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_quant_delay(pos: f64, quant: f64) -> f64 {
    tempoclock::quant_delay(pos, quant)
}

/// The bar index a beat position falls in on a grid of `quant` beats per bar
/// (0-based; `quant <= 0` → 0, no bar grid) — the display complement of
/// [`clausters_core_quant_delay`].
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_bar(beats: f64, quant: f64) -> f64 {
    tempoclock::bar(beats, quant)
}

/// The beat within its bar for a beat position on a grid of `quant` beats per
/// bar (0-based, in `[0, quant)`; `quant <= 0` returns the position itself).
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_beat_in_bar(beats: f64, quant: f64) -> f64 {
    tempoclock::beat_in_bar(beats, quant)
}

/// Packs raw NTP-scale seconds (any epoch: Unix + offset for wire timetags,
/// seconds-from-start for an NRT score) into the 64 timetag bits
/// (`seconds << 32 | fractional`), rounding the fraction — the one packing rule
/// every client shares.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_ntp_timetag(ntp_secs: f64) -> u64 {
    clausters_core::osc::timetag_bits(clausters_core::osc::pack_timetag(ntp_secs))
}

/// A Unix timestamp → the 64 NTP timetag bits (adds the 1900→1970 offset,
/// then packs like [`clausters_core_ntp_timetag`]).
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_unix_to_ntp(unix_secs: f64) -> u64 {
    clausters_core::osc::timetag_bits(clausters_core::osc::unix_to_ntp(unix_secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_scalars() {
        // 120 bpm = 2 bps, beat 0 at second 0.
        assert_eq!(clausters_core_beats_to_secs(2.0, 0.0, 0.0, 2.0), 1.0);
        assert_eq!(clausters_core_secs_to_samples(1.0, 48_000.0), 48_000);
    }

    #[test]
    fn ruler_axis_scalars_cross_the_boundary() {
        // bar:beat reads of the quant grid (the quant_delay complement).
        assert_eq!(clausters_core_bar(9.5, 4.0), 2.0);
        assert!((clausters_core_beat_in_bar(9.5, 4.0) - 1.5).abs() < 1e-12);
        assert_eq!(clausters_core_beat_in_bar(9.5, 0.0), 9.5);
        // Perceptual frequency scales round-trip through the C surface.
        for hz in [100.0, 1000.0, 12_000.0] {
            assert!((clausters_core_mel_to_hz(clausters_core_hz_to_mel(hz)) - hz).abs() < 1e-6);
            assert!((clausters_core_bark_to_hz(clausters_core_hz_to_bark(hz)) - hz).abs() < 1e-6);
        }
        assert!((clausters_core_hz_to_mel(1000.0) - 1000.0).abs() < 0.1);
        assert!((clausters_core_hz_to_bark(1000.0) - 8.53).abs() < 0.05);
    }
}
