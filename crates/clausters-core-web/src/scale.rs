//! Perceptual frequency scales: mel and bark, both ways.

#[cfg(target_arch = "wasm32")]
use clausters_core::scale;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// JS face: hertz -> **mel** (O'Shaughnessy), the perceptual frequency scale a
/// spectrogram axis is labeled in. The client spells it `cpsmel`, following
/// SuperCollider's `<from><to>` conversions.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn hz_to_mel(hz: f64) -> f64 {
    scale::hz_to_mel(hz)
}

/// JS face: mel -> hertz, the exact inverse of [`hz_to_mel`]. Spelled `melcps`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn mel_to_hz(mel: f64) -> f64 {
    scale::mel_to_hz(mel)
}

/// JS face: hertz -> **bark** (Traunmuller), the critical-band scale. Spelled
/// `cpsbark`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn hz_to_bark(hz: f64) -> f64 {
    scale::hz_to_bark(hz)
}

/// JS face: bark -> hertz, the analytic inverse of [`hz_to_bark`]. Spelled
/// `barkcps`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn bark_to_hz(bark: f64) -> f64 {
    scale::bark_to_hz(bark)
}
