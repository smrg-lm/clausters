//! Perceptual frequency scales.

use clausters_core::scale;

/// Hertz -> mel (O'Shaughnessy), the perceptual frequency scale shared with the
/// GUI spectrogram axis.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_hz_to_mel(hz: f64) -> f64 {
    scale::hz_to_mel(hz)
}

/// Mel -> hertz, the exact inverse of [`clausters_core_hz_to_mel`].
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_mel_to_hz(mel: f64) -> f64 {
    scale::mel_to_hz(mel)
}

/// Hertz -> bark (Traunmuller closed form; -0.53 at 0 Hz, the axis floor).
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_hz_to_bark(hz: f64) -> f64 {
    scale::hz_to_bark(hz)
}

/// Bark -> hertz, the analytic inverse of [`clausters_core_hz_to_bark`].
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_bark_to_hz(bark: f64) -> f64 {
    scale::bark_to_hz(bark)
}
