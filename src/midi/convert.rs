//! MIDI message-type conversions to `f32` control values.
//!
//! One named helper per standard channel-voice message type, so the binding's
//! control map references them by name and the server and client agree on the
//! same curves. Inputs are already normalized to MIDI 2.0 / UMP resolution
//! (16-bit velocity, 32-bit controllers/pressure/bend) — the wire parser
//! widens classic MIDI 1.0 7/14-bit values up to these (see
//! [`super::widen_7_to_16`] / [`super::widen_7_to_32`]), so this module is
//! independent of the wire version. The `f32` results land directly on a
//! node's named control zones, keeping the high resolution that 7-bit MIDI 1.0
//! would have quantized away.

/// **Note on/off** — note number (with optional microtonal fraction) to
/// frequency in Hz. The `f32` server counterpart of the client's `midicps`
/// (12-TET, A4 = note 69 = 440 Hz).
#[inline]
pub fn midi2freq(note: f32) -> f32 {
    440.0 * 2.0f32.powf((note - 69.0) / 12.0)
}

/// **Note on/off** — 16-bit velocity (0..=65535) to linear amplitude (0..=1).
#[inline]
pub fn velocity2amp(velocity: u16) -> f32 {
    velocity as f32 / u16::MAX as f32
}

/// **Aftertouch** (poly or channel pressure) — 32-bit value to 0..=1.
#[inline]
pub fn aftertouch2control(pressure: u32) -> f32 {
    pressure as f32 / u32::MAX as f32
}

/// **Control change** — 32-bit value to 0..=1. The caller scales this unit
/// value into the target control's range.
#[inline]
pub fn cc2control(value: u32) -> f32 {
    value as f32 / u32::MAX as f32
}

/// **Pitch bend** — 32-bit value, center `0x8000_0000`, to bipolar -1..=1.
#[inline]
pub fn bend2control(value: u32) -> f32 {
    const CENTER: f64 = 0x8000_0000u32 as f64;
    ((value as f64 - CENTER) / CENTER).clamp(-1.0, 1.0) as f32
}

/// **Program change** — program number as an `f32` selector (0..=127).
#[inline]
pub fn program2control(program: u8) -> f32 {
    program as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[inline]
    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn note_to_freq_is_12tet() {
        assert!(close(midi2freq(69.0), 440.0));
        assert!(close(midi2freq(57.0), 220.0));
        assert!(close(midi2freq(81.0), 880.0));
        // Microtonal: a quarter-tone above A4 is between 440 and the next semitone.
        assert!(midi2freq(69.5) > 440.0 && midi2freq(69.5) < midi2freq(70.0));
    }

    #[test]
    fn velocity_full_scale() {
        assert!(close(velocity2amp(0), 0.0));
        assert!(close(velocity2amp(u16::MAX), 1.0));
        assert!(close(velocity2amp(u16::MAX / 2), 0.5));
    }

    #[test]
    fn bend_is_bipolar_centered() {
        assert!(close(bend2control(0x8000_0000), 0.0));
        assert!(close(bend2control(0), -1.0));
        assert!(close(bend2control(u32::MAX), 1.0));
    }

    #[test]
    fn unit_ranged_conversions() {
        assert!(close(aftertouch2control(0), 0.0));
        assert!(close(aftertouch2control(u32::MAX), 1.0));
        assert!(close(cc2control(0), 0.0));
        assert!(close(cc2control(u32::MAX), 1.0));
    }
}
