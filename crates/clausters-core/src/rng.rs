//! Seeded white noise, identical to the server's `dsp::noise`.
//!
//! Keeping the generator here lets a client reproduce a server noise stream
//! sample for sample from the same seed. Both the splitmix64 seed mixer and
//! the xorshift step / sample scaling are the exact code the server's
//! `WhiteNoise` UGen now calls. Allocation-free.

/// SplitMix64 — mixes a raw seed into a well-distributed state word. The same
/// constant the server uses to seed successive `WhiteNoise` instances.
#[inline]
pub fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// White noise in [-1, 1], one xorshift64 step per sample. No inputs.
#[derive(Clone, Copy)]
pub struct WhiteNoise {
    state: u64,
}

impl WhiteNoise {
    /// Builds a generator from a raw seed exactly as the server does:
    /// `splitmix64(seed) | 1` (the state is never zero).
    #[inline]
    pub fn from_seed(seed: u64) -> Self {
        Self {
            state: splitmix64(seed) | 1,
        }
    }

    /// Next sample in [-1, 1).
    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        (self.state as i32 as f32) * (1.0 / 2_147_483_648.0)
    }

    /// Fills a slice with successive samples. Allocation-free.
    #[inline]
    pub fn fill(&mut self, out: &mut [f32]) {
        for s in out.iter_mut() {
            *s = self.next_sample();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_for_a_seed() {
        let mut a = WhiteNoise::from_seed(42);
        let mut b = WhiteNoise::from_seed(42);
        for _ in 0..1000 {
            assert_eq!(a.next_sample(), b.next_sample());
        }
    }

    #[test]
    fn stays_in_range() {
        let mut n = WhiteNoise::from_seed(0xdead_beef);
        for _ in 0..10_000 {
            let s = n.next_sample();
            assert!((-1.0..1.0).contains(&s), "out of range: {s}");
        }
    }
}
