//! Musical time: the clock's arithmetic, and its queue.
//!
//! Every door here is `clausters-core`'s own function reached
//! from JS, the same way `clausters-ffi` reaches it from Python: the sequencing
//! layer computes no time of its own in either language, so a beat resolves to
//! the same second, and a second to the same sample, in the client and in the
//! server.

#[cfg(target_arch = "wasm32")]
use clausters_core::osc;
#[cfg(target_arch = "wasm32")]
use clausters_core::tempoclock;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// Seconds at `beats` for the affine clock `(tempo, base_beats, base_seconds)`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn beats_to_secs(tempo: f64, base_beats: f64, base_seconds: f64, beats: f64) -> f64 {
    base_seconds + (beats - base_beats) / tempo
}

/// Beats at `secs` for the affine clock `(tempo, base_beats, base_seconds)`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn secs_to_beats(tempo: f64, base_beats: f64, base_seconds: f64, secs: f64) -> f64 {
    base_beats + (secs - base_seconds) * tempo
}

/// Seconds -> sample count at `sample_rate` (ties to even).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn secs_to_samples(secs: f64, sample_rate: f64) -> f64 {
    tempoclock::secs_to_samples(secs, sample_rate) as f64
}

/// Sample count -> seconds at `sample_rate`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn samples_to_secs(samples: f64, sample_rate: f64) -> f64 {
    tempoclock::samples_to_secs(samples as i64, sample_rate)
}

/// Beats to wait so a routine starts on the next `quant` boundary of the grid
/// (`quant <= 0` -> now). The snapping rule every client shares.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn quant_delay(pos: f64, quant: f64) -> f64 {
    tempoclock::quant_delay(pos, quant)
}

/// The 0-based bar index `beats` falls in on a grid of `quant` beats per bar.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn bar(beats: f64, quant: f64) -> f64 {
    tempoclock::bar(beats, quant)
}

/// The beat within its bar, in `[0, quant)`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn beat_in_bar(beats: f64, quant: f64) -> f64 {
    tempoclock::beat_in_bar(beats, quant)
}

/// A Unix timestamp -> the 64 NTP timetag bits, as a `BigInt` (the wire value
/// is a full 64-bit word; JS numbers would lose its low bits).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn unix_to_ntp(unix_secs: f64) -> u64 {
    osc::timetag_bits(osc::unix_to_ntp(unix_secs))
}

/// A Unix timestamp -> the server's absolute sample, through a `/clock_query` anchor
/// (`anchor_unix`, `anchor_sample`) and the measured `rate`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn unix_to_sample(unix_secs: f64, anchor_unix: f64, anchor_sample: f64, rate: f64) -> f64 {
    osc::unix_to_sample(unix_secs, anchor_unix, anchor_sample as i64, rate) as f64
}
