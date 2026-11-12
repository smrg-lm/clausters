//! The data paths' analysis.
//!
//! What a page measures from the data it reads off the server --
//! control buses, tap windows, buffer samples. Every one of them is a
//! measurement of the *signal*, and it is the same function the GUI host
//! measures with, so a figure a script reports and a figure a widget draws are
//! one number rather than two implementations of it. Nothing of the *screen*
//! belongs here: a display window, a trigger's framing, a decibel curve
//! and a row of pixel columns are drawing, and the host is what draws. Nothing
//! here keeps state except the peak pyramid, which is a cache by definition.

#[cfg(target_arch = "wasm32")]
use clausters_core::loudness;
#[cfg(target_arch = "wasm32")]
use clausters_core::measure;
#[cfg(target_arch = "wasm32")]
use clausters_core::resample;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// JS face: the **peak and RMS** of one channel of an interleaved buffer, as
/// `[peak, rms]` -- what a render reports back about what it produced. The
/// stride walk measures a render without deinterleaving it first, so a page
/// reads the same two numbers the server and the Python client report.
///
/// An empty pair for a channel the buffer does not have.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn channel_stats(samples: &[f32], channels: usize, channel: usize) -> Vec<f32> {
    if channels == 0 || channel >= channels {
        return Vec::new();
    }
    let (peak, rms) = measure::channel_stats(samples, channels, channel);
    vec![peak, rms]
}

/// JS face: the **true peak** of one channel of an interleaved buffer, in
/// linear amplitude -- the reconstructed peak rather than the largest sample.
///
/// The ITU-R BS.1770-4 Annex 2 filter at 4*, which is what makes the reading
/// dBTP: a signal whose samples all read below full scale can still reconstruct
/// above it, by up to about 3 dB, and every converter sees that peak. Always at
/// or above [`channel_stats`]'s peak; `-1` for a channel the buffer does not
/// have.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn true_peak(samples: &[f32], channels: usize, channel: usize) -> f32 {
    if channels == 0 || channel >= channels {
        return -1.0;
    }
    resample::true_peak(samples, channels, channel)
}

/// JS face: **the loudness** of an interleaved buffer at `rate` Hz, as
/// `[integrated, range, momentaryMax, shortTermMax]` -- LUFS, LU, LUFS, LUFS,
/// as ITU-R BS.1770 and EBU R 128 measure them.
///
/// `weights` is one per channel (`0` leaves one out, `1.41` is a surround), or
/// absent for the weights BS.1770 gives a layout known by its count. A reading
/// with nothing to measure is `-Infinity`, a range with no spread `0`. Empty for
/// no channels, a rate under 10 Hz, or weights that are not one per channel.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn loudness(
    samples: &[f32],
    channels: usize,
    rate: f64,
    weights: Option<Vec<f64>>,
) -> Vec<f64> {
    let measured = match weights {
        Some(w) if w.len() != channels => None,
        Some(w) => loudness::loudness_with_weights(samples, &w, rate),
        None => loudness::loudness(samples, channels, rate),
    };
    measured.map_or_else(Vec::new, |l| {
        vec![l.integrated, l.range, l.momentary_max, l.short_term_max]
    })
}
