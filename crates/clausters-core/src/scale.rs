//! Perceptual frequency-scale conversions: hertz ↔ mel and hertz ↔ bark.
//!
//! General audio measurement math shared by every process that draws or
//! analyzes a frequency axis (the GUI host's spectrogram ruler and shader
//! mapping, a client laying out the same axis), kept here so the formulas
//! live once. Both scales use closed forms with exact analytic inverses:
//!
//! - **Mel** (O'Shaughnessy): `m = 2595 · log10(1 + f/700)`.
//! - **Bark** (Traunmüller): `z = 26.81 · f / (1960 + f) − 0.53`, chosen over
//!   Zwicker's arctan fit precisely because it inverts analytically —
//!   `f = 1960 · (z + 0.53) / (26.28 − z)` — so a display mapping and its
//!   ruler can round-trip without a numeric solve. The raw closed form is
//!   kept (no low/high-end corrections, no clamp to 0): it is exactly
//!   invertible on the whole axis, and `hz_to_bark(0) = −0.53` is simply the
//!   axis floor a display normalizes against.
//!
//! It also carries the **MIDI note ↔ name** mapping (scientific pitch
//! notation, `C4` = middle C = note 60), the pitch-axis analogue of the
//! frequency scales: the piano-roll's pitch ruler and keyboard labels read it,
//! and a client authoring notes can reuse the same names. Kept here so the
//! musical spelling lives once alongside the perceptual scales.

/// Hertz → mel (O'Shaughnessy). Negative input is treated as 0.
#[inline]
pub fn hz_to_mel(hz: f64) -> f64 {
    2595.0 * (1.0 + hz.max(0.0) / 700.0).log10()
}

/// Mel → hertz, the exact inverse of [`hz_to_mel`]. Negative input maps to 0.
#[inline]
pub fn mel_to_hz(mel: f64) -> f64 {
    700.0 * (10f64.powf(mel.max(0.0) / 2595.0) - 1.0)
}

/// Hertz → bark (Traunmüller). Negative input is treated as 0; the value at
/// 0 Hz is −0.53 (the formula's own floor — normalize a display axis against
/// it rather than clamping, so the inverse stays exact).
#[inline]
pub fn hz_to_bark(hz: f64) -> f64 {
    let hz = hz.max(0.0);
    26.81 * hz / (1960.0 + hz) - 0.53
}

/// Bark → hertz, the analytic inverse of [`hz_to_bark`]. Input is clamped to
/// the formula's range `[−0.53, 26.28)` so the result stays finite and
/// non-negative.
#[inline]
pub fn bark_to_hz(bark: f64) -> f64 {
    let z = bark.clamp(-0.53, 26.28 - 1e-9);
    1960.0 * (z + 0.53) / (26.28 - z)
}

/// The twelve pitch-class names, sharp spelling, index 0 = C.
pub const PITCH_CLASSES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// The pitch class (`0..12`, `0` = C) of a MIDI note number, wrapping every
/// octave. Uses Euclidean remainder so negative note numbers still map into
/// `0..12`.
#[inline]
pub fn pitch_class(midi: i32) -> usize {
    midi.rem_euclid(12) as usize
}

/// `true` if the MIDI note is a black key (a sharp/flat) — the accidental
/// pitch classes C#, D#, F#, G#, A#. The piano-roll keyboard and its row
/// shading read this.
#[inline]
pub fn is_black_key(midi: i32) -> bool {
    matches!(pitch_class(midi), 1 | 3 | 6 | 8 | 10)
}

/// A MIDI note number's name in scientific pitch notation (`60` → `"C4"`,
/// `69` → `"A4"`, `61` → `"C#4"`). The octave is `midi/12 − 1`, so middle C
/// (60) is `C4` and A440 (69) is `A4`; sharps are used for the black keys.
pub fn note_name(midi: i32) -> String {
    let octave = midi.div_euclid(12) - 1;
    format!("{}{}", PITCH_CLASSES[pitch_class(midi)], octave)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_names_follow_scientific_pitch() {
        assert_eq!(note_name(60), "C4"); // middle C
        assert_eq!(note_name(69), "A4"); // A440
        assert_eq!(note_name(61), "C#4");
        assert_eq!(note_name(21), "A0"); // lowest piano key
        assert_eq!(note_name(108), "C8"); // highest piano key
        assert_eq!(note_name(0), "C-1"); // MIDI floor
    }

    #[test]
    fn pitch_class_and_black_keys() {
        assert_eq!(pitch_class(60), 0);
        assert_eq!(pitch_class(69), 9);
        assert!(!is_black_key(60)); // C
        assert!(is_black_key(61)); // C#
        assert!(is_black_key(66)); // F#
        assert!(!is_black_key(65)); // F
    }

    #[test]
    fn mel_landmarks() {
        // 1 kHz sits at ~1000 mel by construction of the scale.
        assert!((hz_to_mel(1000.0) - 999.99).abs() < 0.1);
        assert_eq!(hz_to_mel(0.0), 0.0);
    }

    #[test]
    fn bark_landmarks() {
        // 1 kHz is ~8.5 bark (Traunmüller); 0 Hz sits at the −0.53 floor.
        assert!((hz_to_bark(1000.0) - 8.53).abs() < 0.05);
        assert!((hz_to_bark(0.0) + 0.53).abs() < 1e-12);
        assert_eq!(bark_to_hz(hz_to_bark(0.0)), 0.0);
    }

    #[test]
    fn round_trips_across_the_audible_range() {
        for hz in [0.0, 20.0, 100.0, 440.0, 1000.0, 4000.0, 12_000.0, 20_000.0] {
            assert!(
                (mel_to_hz(hz_to_mel(hz)) - hz).abs() < 1e-6,
                "mel round trip at {hz}"
            );
            assert!(
                (bark_to_hz(hz_to_bark(hz)) - hz).abs() < 1e-6,
                "bark round trip at {hz}"
            );
        }
    }

    #[test]
    fn both_scales_are_monotonic() {
        let freqs = [0.0, 50.0, 200.0, 1000.0, 5000.0, 20_000.0];
        for w in freqs.windows(2) {
            assert!(hz_to_mel(w[0]) < hz_to_mel(w[1]));
            assert!(hz_to_bark(w[0]) < hz_to_bark(w[1]));
        }
    }
}
