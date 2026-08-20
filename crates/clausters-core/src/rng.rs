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

/// The stride between successive seeds: the odd golden-ratio constant, whose
/// low bits change on every step.
///
/// A server hands out one seed per stochastic UGen by walking this stride from
/// wherever the sequence *starts*, so two generators in one graph never share a
/// stream — correlated "noise" sums to a comb filter rather than to more noise.
/// It lives here, with the generator, so a client can reproduce a server-side
/// stream exactly. Where the walk starts is [`entropy_seed`]'s business, not
/// this constant's.
pub const SEED_STRIDE: u64 = 0x9E37_79B9_7F4A_7C15;

/// A fresh starting seed, drawn from the platform's entropy.
///
/// **A random process is unpredictable first and reproducible on request.**
/// Anything that starts a seed sequence without being told which one — a
/// server booting, a render with no seed configured — calls this, so the same
/// score sounds different every time it is played, the way a random process in
/// a piece is meant to. Fixing the seed is the *caller's* deliberate act, and
/// what it gives is the replay: `--seed`, `RenderConfig::seed`, `seed=` in the
/// clients. The client's own random context (`clausters.base.rand`) has always
/// worked this way; this is the server side of the same rule.
///
/// Whoever draws one must **report it back** (`RenderStats::seed`), or the take
/// you just liked is unrepeatable.
///
/// On `wasm32` there is no entropy reachable from this crate — `SystemTime` is
/// not implemented there — so this returns a fixed value and the JS door takes
/// a seed explicitly, from `crypto.getRandomValues` at the edge that has it.
pub fn entropy_seed() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::hash::{BuildHasher, Hasher};
        use std::time::{SystemTime, UNIX_EPOCH};

        // Two independent sources, mixed: the wall clock (different on every
        // run) and std's own OS-seeded hasher (different within the same
        // nanosecond, and on platforms with a coarse clock).
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(nanos);
        splitmix64(nanos ^ h.finish())
    }
    #[cfg(target_arch = "wasm32")]
    {
        SEED_STRIDE
    }
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

/// A seeded value-level generator for the sequencing layer (`Pwhite`, `Prand`,
/// …): the same splitmix64 seeding and xorshift64 step as [`WhiteNoise`], but
/// yielding `f64` uniforms and bounded integers instead of audio samples. It
/// lives here so a seeded pattern replays the **same stream in every client
/// language** — the host language's own RNG (e.g. Python's Mersenne Twister)
/// must never leak into sequenced values.
#[derive(Clone, Copy)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// Seeds exactly like [`WhiteNoise::from_seed`] (state never zero).
    #[inline]
    pub fn from_seed(seed: u64) -> Self {
        Self {
            state: splitmix64(seed) | 1,
        }
    }

    /// A generator resuming from a raw `state` word (the flat form that
    /// crosses the C ABI between calls). Only zero is illegal for xorshift —
    /// an even state is a normal mid-stream value, so it must pass unchanged.
    #[inline]
    pub fn from_state(state: u64) -> Self {
        Self {
            state: if state == 0 { 1 } else { state },
        }
    }

    /// The raw state word to persist between boundary crossings.
    #[inline]
    pub fn state(&self) -> u64 {
        self.state
    }

    /// One xorshift64 step; the full-width random word.
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        self.state
    }

    /// Uniform in `[0, 1)` with 53-bit resolution.
    #[inline]
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)
    }

    /// Uniform in `[lo, hi)` (degenerate to `lo` when `hi <= lo`).
    #[inline]
    pub fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
        lo + self.next_f64() * (hi - lo).max(0.0)
    }

    /// Uniform integer in `[0, n)`; 0 when `n == 0`. Uses the 53-bit uniform,
    /// so it is bias-free for any collection a pattern realistically indexes.
    #[inline]
    pub fn next_below(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        let v = (self.next_f64() * n as f64) as u64;
        v.min(n - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_rng_is_deterministic_and_in_range() {
        let mut a = Rng::from_seed(1);
        let mut b = Rng::from_seed(1);
        for _ in 0..1000 {
            let x = a.next_f64();
            assert_eq!(x, b.next_f64());
            assert!((0.0..1.0).contains(&x));
        }
        let mut r = Rng::from_seed(7);
        for _ in 0..1000 {
            let k = r.next_below(3);
            assert!(k < 3);
        }
        // Resuming from a persisted state continues the same stream.
        let mut c = Rng::from_seed(5);
        let s = c.state();
        let expect = c.next_f64();
        assert_eq!(Rng::from_state(s).next_f64(), expect);
    }

    #[test]
    fn deterministic_for_a_seed() {
        let mut a = WhiteNoise::from_seed(42);
        let mut b = WhiteNoise::from_seed(42);
        for _ in 0..1000 {
            assert_eq!(a.next_sample(), b.next_sample());
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn entropy_seeds_differ_between_calls() {
        // The point of the default: two starts are two different takes.
        let seeds: std::collections::HashSet<u64> = (0..32).map(|_| entropy_seed()).collect();
        assert!(
            seeds.len() > 30,
            "entropy_seed repeats: {} distinct",
            seeds.len()
        );
        // And a drawn seed still seeds a usable generator.
        let mut n = WhiteNoise::from_seed(entropy_seed());
        assert!((-1.0..1.0).contains(&n.next_sample()));
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
