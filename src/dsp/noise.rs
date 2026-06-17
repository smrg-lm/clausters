use std::sync::atomic::{AtomicU64, Ordering};

use clausters_core::rng;

use crate::dsp::{ProcessCtx, UGen};

/// Seeds successive instances differently without `rand` (which may allocate
/// or lock — forbidden on the audio thread; construction happens off it, but
/// the RNG state must live inside the UGen anyway).
static SEED: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);

/// White noise in [-1, 1], xorshift per sample. No inputs. The generator
/// itself is `clausters_core::rng::WhiteNoise`, so a client can reproduce the
/// stream from the same seed; only the per-instance seeding lives here.
pub struct WhiteNoise {
    noise: rng::WhiteNoise,
}

impl WhiteNoise {
    pub fn new() -> Self {
        let seed = SEED.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
        Self {
            noise: rng::WhiteNoise::from_seed(seed),
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
        self.noise.fill(output);
    }
}
