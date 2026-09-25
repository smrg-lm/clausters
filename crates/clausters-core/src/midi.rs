//! Widening a MIDI 1.0 value to the resolution MIDI 2.0 carries.
//!
//! The server's live MIDI input and the clip file reader both widen a 7-bit
//! velocity or controller (and a 14-bit bend) the same way: **bit-repeat
//! fill**, so 0 stays 0 and the 7-bit maximum reaches the wide maximum, which
//! a plain shift would miss by the width of the fill. One copy here, so a note
//! played live and the same note read from a file carry the same velocity.

/// Widen a 7-bit MIDI 1.0 value to 16 bits (0->0 and 127->65535).
#[inline]
pub fn widen_7_to_16(v: u8) -> u16 {
    let v = (v & 0x7f) as u16;
    (v << 9) | (v << 2) | (v >> 5)
}

/// Widen a 7-bit MIDI 1.0 value to 32 bits.
#[inline]
pub fn widen_7_to_32(v: u8) -> u32 {
    let v = (v & 0x7f) as u32;
    (v << 25) | (v << 18) | (v << 11) | (v << 4) | (v >> 3)
}

/// Widen a 14-bit MIDI 1.0 value (e.g. pitch bend) to 32 bits.
#[inline]
pub fn widen_14_to_32(v: u16) -> u32 {
    let v = (v & 0x3fff) as u32;
    (v << 18) | (v << 4) | (v >> 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widening_hits_full_scale() {
        assert_eq!(widen_7_to_16(0), 0);
        assert_eq!(widen_7_to_16(127), u16::MAX);
        assert_eq!(widen_7_to_32(0), 0);
        assert_eq!(widen_7_to_32(127), u32::MAX);
        assert_eq!(widen_14_to_32(0x3fff), u32::MAX);
    }
}
