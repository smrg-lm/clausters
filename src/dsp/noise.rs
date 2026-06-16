use std::sync::atomic::{AtomicU64, Ordering};

use crate::dsp::{ProcessCtx, UGen};

/// Seeds successive instances differently without `rand` (which may allocate
/// or lock — forbidden on the audio thread; construction happens off it, but
/// the RNG state must live inside the UGen anyway).
static SEED: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// White noise in [-1, 1], xorshift per sample. No inputs.
pub struct WhiteNoise {
    state: u64,
}

impl WhiteNoise {
    pub fn new() -> Self {
        let seed = SEED.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
        Self {
            state: splitmix64(seed) | 1, // never zero
        }
    }
}

impl Default for WhiteNoise {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for WhiteNoise {
    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        for s in output.iter_mut() {
            self.state ^= self.state << 13;
            self.state ^= self.state >> 17;
            self.state ^= self.state << 5;
            *s = (self.state as i32 as f32) * (1.0 / 2_147_483_648.0);
        }
    }
}
