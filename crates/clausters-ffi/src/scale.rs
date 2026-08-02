//! Perceptual frequency scales, and the degree-to-midinote map.

use clausters_core::builtins;
use clausters_core::scale;

/// Hertz → mel (O'Shaughnessy), the perceptual frequency scale shared with the
/// GUI spectrogram axis.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_hz_to_mel(hz: f64) -> f64 {
    scale::hz_to_mel(hz)
}

/// Mel → hertz, the exact inverse of [`clausters_core_hz_to_mel`].
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_mel_to_hz(mel: f64) -> f64 {
    scale::mel_to_hz(mel)
}

/// Hertz → bark (Traunmüller closed form; −0.53 at 0 Hz, the axis floor).
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_hz_to_bark(hz: f64) -> f64 {
    scale::hz_to_bark(hz)
}

/// Bark → hertz, the analytic inverse of [`clausters_core_hz_to_bark`].
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_bark_to_hz(bark: f64) -> f64 {
    scale::bark_to_hz(bark)
}

/// Scale-degree → MIDI note number in the pitch space `octave`/`root`, with
/// floored octave wrapping (sclang semantics). `scale` is `n` semitone offsets;
/// `n == 0` (or a null `scale`) yields middle C.
///
/// # Safety
/// `scale` must be readable for `n` `f32`s (or null with `n == 0`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_degree_to_midinote(
    degree: f64,
    octave: f64,
    root: f64,
    scale: *const f32,
    n: usize,
) -> f64 {
    if scale.is_null() || n == 0 {
        return builtins::degree_to_midinote(degree, octave, root, &[]);
    }
    // SAFETY: caller guarantees `scale` is readable for `n`.
    let s = unsafe { std::slice::from_raw_parts(scale, n) };
    builtins::degree_to_midinote(degree, octave, root, s)
}
