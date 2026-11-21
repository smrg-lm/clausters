//! The seeded generators: scsynth's [`RGen`], which the server's UGens draw
//! from, and the sequencing layer's [`Rng`], which a client's patterns draw
//! from, with the distributions both read ([`Draw`]).
//!
//! [`WhiteNoise`] is the server's white noise, kept here so a client can
//! reproduce a server noise stream sample for sample from the same seed.
//! Allocation-free.

/// SplitMix64 -- mixes a raw seed into a well-distributed state word. The same
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
/// stream -- correlated "noise" sums to a comb filter rather than to more noise.
/// It lives here, with the generator, so a client can reproduce a server-side
/// stream exactly. Where the walk starts is [`entropy_seed`]'s business, not
/// this constant's.
pub const SEED_STRIDE: u64 = 0x9E37_79B9_7F4A_7C15;

/// A fresh starting seed, drawn from the platform's entropy.
///
/// **A random process is unpredictable first and reproducible on request.**
/// Anything that starts a seed sequence without being told which one -- a
/// server booting, a render with no seed configured -- calls this, so the same
/// score sounds different every time it is played, the way a random process in
/// a take is meant to. Fixing the seed is the *caller's* deliberate act, and
/// what it gives is the replay: `--seed`, `RenderConfig::seed`, `seed=` in the
/// clients. The client's own random context (`clausters.base.rand`) has always
/// worked this way; this is the server side of the same rule.
///
/// Whoever draws one must **report it back** (`RenderStats::seed`), or the take
/// you just liked is unrepeatable.
///
/// On `wasm32` there is no entropy reachable from this crate -- `SystemTime` is
/// not implemented there -- so this returns a fixed value and the JS door takes
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

/// White noise in `[-1, 1)`, the server's: scsynth's `frand2` per sample, from
/// the generator a server seeded with `seed` gives its synths first
/// (`dsp::rand` in the server). A `WhiteNoise` UGen drawing alone from that
/// generator plays exactly this stream.
#[derive(Clone, Copy)]
pub struct WhiteNoise {
    g: RGen,
}

impl WhiteNoise {
    /// The stream of a server seeded with `seed`: its first generator, which
    /// takes the seed's low word with the high one folded in.
    #[inline]
    pub fn from_seed(seed: u64) -> Self {
        Self {
            g: RGen::new((seed ^ (seed >> 32)) as u32),
        }
    }

    /// Next sample in [-1, 1).
    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        self.g.frand2()
    }

    /// Fills a slice with successive samples. Allocation-free.
    #[inline]
    pub fn fill(&mut self, out: &mut [f32]) {
        for s in out.iter_mut() {
            *s = self.next_sample();
        }
    }
}

/// A seeded value-level generator for the sequencing layer (`pwhite`, `prand`,
/// ...): splitmix64 seeding and a xorshift64 step, but
/// yielding `f64` uniforms and bounded integers instead of audio samples. It
/// lives here so a seeded pattern replays the **same stream in every client
/// language** -- the host language's own RNG (e.g. Python's Mersenne Twister)
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
    /// crosses the C ABI between calls). Only zero is illegal for xorshift --
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

/// The random distributions, named as sclang names them: what a draw reads
/// off [`Rng::next_f64`] and how. One table so a client crosses its boundary
/// once per draw with the distribution by number, and the formulas live here
/// once. The discriminants are the stable C-ABI contract: append only.
///
/// Every one takes up to two numbers, `a` and `b`, and returns an `f64`; the
/// integer draws return a whole number, and [`Draw::Coin`] returns `1.0` or
/// `0.0`. The generator is this crate's, not scsynth's, so a seed does not
/// replay sclang's stream -- the **shapes** are sclang's.
#[repr(u32)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Draw {
    /// Uniform in `[0, a)`.
    Rand = 0,
    /// Uniform in `[-a, a)`.
    Rand2 = 1,
    /// Uniform between `a` and `b`, either order.
    Rrand = 2,
    /// Exponentially distributed between `a` and `b` (same sign, nonzero):
    /// equal chances per octave rather than per hertz.
    Exprand = 3,
    /// Linearly distributed in `[0, a)`, more likely toward 0: the lesser of
    /// two uniforms.
    Linrand = 4,
    /// Bilinear in `(-a, a)`, more likely toward 0: one uniform minus another.
    Bilinrand = 5,
    /// The sum of three uniforms, scaled to `(-a, a)`: a bell, sclang's loose
    /// approximation of a Gaussian.
    Sum3rand = 6,
    /// `1.0` with probability `a`, else `0.0`.
    Coin = 7,
    /// A whole number in `[0, a)` (`floor(a * u)`).
    Irand = 8,
    /// A whole number in `[-a, a]`, both ends included.
    Irand2 = 9,
    /// A whole number between `a` and `b`, both ends included.
    Irrand = 10,
    /// Gaussian with mean `a` and standard deviation `b` (Box-Muller, as
    /// sclang's `gauss` is).
    Gauss = 11,
    /// The lower of two uniform draws between `a` and `b`: more likely toward
    /// `a` (sclang's `Plprand`).
    Lprand = 12,
    /// The higher of two uniform draws between `a` and `b`: more likely
    /// toward `b` (sclang's `Phprand`).
    Hprand = 13,
    /// The mean of two uniform draws between `a` and `b`: more likely toward
    /// the middle (sclang's `Pmeanrand`).
    Meanrand = 14,
    /// The beta distribution in `[0, 1]` with the probabilities `a` and `b`
    /// of its two ends, by sclang's rejection loop (`Pbeta`).
    Betarand = 15,
    /// Cauchy with median `a` and spread `b` (sclang's `Pcauchy`).
    Cauchy = 16,
    /// Poisson with mean `a`: a whole number of events (sclang's
    /// `Ppoisson`).
    Poisson = 17,
}

impl Draw {
    /// Maps a C-ABI discriminant back to the enum.
    pub fn from_u32(v: u32) -> Option<Draw> {
        use Draw::*;
        Some(match v {
            0 => Rand,
            1 => Rand2,
            2 => Rrand,
            3 => Exprand,
            4 => Linrand,
            5 => Bilinrand,
            6 => Sum3rand,
            7 => Coin,
            8 => Irand,
            9 => Irand2,
            10 => Irrand,
            11 => Gauss,
            12 => Lprand,
            13 => Hprand,
            14 => Meanrand,
            15 => Betarand,
            16 => Cauchy,
            17 => Poisson,
            _ => return None,
        })
    }

    /// The distribution's name, sclang's selector lowercased.
    pub fn name(self) -> &'static str {
        use Draw::*;
        match self {
            Rand => "rand",
            Rand2 => "rand2",
            Rrand => "rrand",
            Exprand => "exprand",
            Linrand => "linrand",
            Bilinrand => "bilinrand",
            Sum3rand => "sum3rand",
            Coin => "coin",
            Irand => "irand",
            Irand2 => "irand2",
            Irrand => "irrand",
            Gauss => "gauss",
            Lprand => "lprand",
            Hprand => "hprand",
            Meanrand => "meanrand",
            Betarand => "betarand",
            Cauchy => "cauchy",
            Poisson => "poisson",
        }
    }

    /// Resolves a name to the distribution (the inverse of [`name`](Self::name)).
    pub fn from_name(name: &str) -> Option<Draw> {
        (0..).map_while(Draw::from_u32).find(|d| d.name() == name)
    }
}

/// A source of uniform draws in `[0, 1)`, which is all a [`Draw`] reads: the
/// sequencing layer's [`Rng`], or scsynth's [`RGen`] on the server.
pub trait Uniform {
    fn next_f64(&mut self) -> f64;
}

impl Uniform for Rng {
    #[inline]
    fn next_f64(&mut self) -> f64 {
        Rng::next_f64(self)
    }
}

impl Uniform for RGen {
    #[inline]
    fn next_f64(&mut self) -> f64 {
        self.drand()
    }
}

impl Rng {
    /// One draw from distribution `d` with its arguments `a` and `b` (see
    /// [`Draw`] for what each reads).
    pub fn draw(&mut self, d: Draw, a: f64, b: f64) -> f64 {
        d.sample(self, a, b)
    }
}

impl Draw {
    /// One draw of this distribution from `u`, with its arguments `a` and `b`.
    pub fn sample<U: Uniform + ?Sized>(self, u: &mut U, a: f64, b: f64) -> f64 {
        let d = self;
        use Draw::*;
        match d {
            Rand => u.next_f64() * a,
            Rand2 => (2.0 * u.next_f64() - 1.0) * a,
            Rrand => a + u.next_f64() * (b - a),
            Exprand => a * ((b / a).ln() * u.next_f64()).exp(),
            Linrand => {
                let (x, y) = (u.next_f64(), u.next_f64());
                x.min(y) * a
            }
            Bilinrand => {
                let (x, y) = (u.next_f64(), u.next_f64());
                (x - y) * a
            }
            Sum3rand => {
                let sum = u.next_f64() + u.next_f64() + u.next_f64();
                (sum - 1.5) * (2.0 / 3.0) * a
            }
            Coin => (u.next_f64() < a) as u8 as f64,
            Irand => (a * u.next_f64()).floor(),
            Irand2 => ((2.0 * a + 1.0) * u.next_f64() - a).floor(),
            Irrand => {
                let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                lo + ((hi - lo + 1.0) * u.next_f64()).floor()
            }
            // `1 - u` rather than sclang's `u`: the uniform reaches 0 and
            // never 1, and `ln(0)` would be an infinity for one draw in 2^53.
            Gauss => {
                let (u, v) = (u.next_f64(), u.next_f64());
                (-2.0 * (1.0 - u).ln()).sqrt() * (core::f64::consts::TAU * v).sin() * b + a
            }
            Lprand | Hprand | Meanrand => {
                let x = a + u.next_f64() * (b - a);
                let y = a + u.next_f64() * (b - a);
                match d {
                    Lprand => x.min(y),
                    Hprand => x.max(y),
                    _ => (x + y) * 0.5,
                }
            }
            Betarand => {
                let (ra, rb) = (1.0 / a, 1.0 / b);
                // sclang's loop: draw until the two powers sum to at most 1.
                loop {
                    let x = u.next_f64().powf(ra);
                    let sum = x + u.next_f64().powf(rb);
                    if sum <= 1.0 {
                        break if sum > 0.0 { x / sum } else { 0.0 };
                    }
                }
            }
            Cauchy => {
                // A uniform of exactly one half would be the pole of tan.
                let mut x = 0.5;
                while x == 0.5 {
                    x = u.next_f64();
                }
                b * (x * core::f64::consts::PI).tan() + a
            }
            Poisson => {
                let limit = (-a).exp();
                let mut count = 0.0;
                let mut test = u.next_f64();
                while test > limit {
                    count += 1.0;
                    test *= u.next_f64();
                }
                count
            }
        }
    }
}

impl Rng {
    /// One draw shaped by a table [`ListOp::Asrandomtable`](crate::numeric::ListOp)
    /// built: the table read at a uniform position, sclang's `tableRand`. An
    /// empty table gives NaN.
    pub fn tablerand(&mut self, table: &[f64]) -> f64 {
        let at = self.draw(Draw::Rand, table.len().saturating_sub(1) as f64, 0.0);
        crate::numeric::blend_at(table, at, crate::builtins::BoundOp::Clip)
    }

    /// A uniformly random permutation of `0..out.len()` into `out` --
    /// sclang's `scramble`, Fisher-Yates with this stream's draws.
    pub fn scramble(&mut self, out: &mut [u32]) {
        for (i, o) in out.iter_mut().enumerate() {
            *o = i as u32;
        }
        for i in (1..out.len()).rev() {
            let j = self.next_below(i as u64 + 1) as usize;
            out.swap(i, j);
        }
    }

    /// sclang's `sputter`: the indices of a list of `n` values read in order,
    /// each one repeated while a `probability` coin keeps landing, until the
    /// list is read through or `maxlen` indices are out. A probability of 0
    /// reads the list once; one of 1 repeats its first value `maxlen` times.
    pub fn sputter(&mut self, n: usize, probability: f64, maxlen: usize) -> Vec<u32> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < n && out.len() < maxlen {
            out.push(i as u32);
            // sclang advances on a coin of the complement.
            if self.draw(Draw::Coin, 1.0 - probability, 0.0) != 0.0 {
                i += 1;
            }
        }
        out
    }

    /// The index `wchoose` picks: `weights` read as relative chances (they
    /// need not sum to 1), the first index whose running sum reaches a uniform
    /// draw over the total. An empty or all-zero list answers 0.
    pub fn windex(&mut self, weights: &[f64]) -> usize {
        let total: f64 = weights.iter().sum();
        let r = self.next_f64() * total;
        let mut sum = 0.0;
        for (i, &w) in weights.iter().enumerate() {
            sum += w;
            if r < sum {
                return i;
            }
        }
        weights.len().saturating_sub(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_are_in_their_ranges_and_reproducible() {
        let mut r = Rng::from_seed(7);
        for _ in 0..2000 {
            let x = r.draw(Draw::Rand, 3.0, 0.0);
            assert!((0.0..3.0).contains(&x));
            let x = r.draw(Draw::Rand2, 2.0, 0.0);
            assert!((-2.0..2.0).contains(&x));
            let x = r.draw(Draw::Rrand, 5.0, 1.0);
            assert!((1.0..=5.0).contains(&x));
            let x = r.draw(Draw::Exprand, 20.0, 20000.0);
            assert!((20.0..=20000.0).contains(&x));
            let x = r.draw(Draw::Linrand, 1.0, 0.0);
            assert!((0.0..1.0).contains(&x));
            let x = r.draw(Draw::Bilinrand, 1.0, 0.0);
            assert!(x > -1.0 && x < 1.0);
            let x = r.draw(Draw::Sum3rand, 1.0, 0.0);
            assert!((-1.0..1.0).contains(&x));
            assert!([0.0, 1.0].contains(&r.draw(Draw::Coin, 0.5, 0.0)));
            let n = r.draw(Draw::Irand, 4.0, 0.0);
            assert!([0.0, 1.0, 2.0, 3.0].contains(&n));
            let n = r.draw(Draw::Irand2, 2.0, 0.0);
            assert!(n.fract() == 0.0 && (-2.0..=2.0).contains(&n));
            let n = r.draw(Draw::Irrand, 6.0, 3.0);
            assert!(n.fract() == 0.0 && (3.0..=6.0).contains(&n));
            assert!(r.draw(Draw::Gauss, 0.0, 1.0).is_finite());
        }
        let (mut a, mut b) = (Rng::from_seed(3), Rng::from_seed(3));
        for d in (0..).map_while(Draw::from_u32) {
            assert_eq!(a.draw(d, 2.0, 9.0), b.draw(d, 2.0, 9.0), "{}", d.name());
            assert_eq!(Draw::from_name(d.name()), Some(d));
        }
    }

    /// The shapes: the linear draw's mean is a third, the bilinear's and the
    /// three-sum's are zero, an even coin lands half the time, and a Gaussian
    /// has its mean and its deviation.
    #[test]
    fn draws_have_sclangs_shapes() {
        let mut r = Rng::from_seed(11);
        let n = 200_000;
        let mean = |r: &mut Rng, d: Draw, a: f64, b: f64| {
            (0..n).map(|_| r.draw(d, a, b)).sum::<f64>() / n as f64
        };
        assert!((mean(&mut r, Draw::Linrand, 1.0, 0.0) - 1.0 / 3.0).abs() < 0.01);
        assert!(mean(&mut r, Draw::Bilinrand, 1.0, 0.0).abs() < 0.01);
        assert!(mean(&mut r, Draw::Sum3rand, 1.0, 0.0).abs() < 0.01);
        assert!((mean(&mut r, Draw::Coin, 0.5, 0.0) - 0.5).abs() < 0.01);
        let g: Vec<f64> = (0..n).map(|_| r.draw(Draw::Gauss, 3.0, 2.0)).collect();
        let m = g.iter().sum::<f64>() / n as f64;
        let sd = (g.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / n as f64).sqrt();
        assert!(
            (m - 3.0).abs() < 0.02 && (sd - 2.0).abs() < 0.02,
            "{m} {sd}"
        );
        // wchoose reads relative weights.
        let mut counts = [0usize; 3];
        for _ in 0..n {
            counts[r.windex(&[1.0, 2.0, 1.0])] += 1;
        }
        assert!(
            (counts[1] as f64 / n as f64 - 0.5).abs() < 0.01,
            "{counts:?}"
        );
        assert_eq!(r.windex(&[]), 0);
        // A permutation, and a table draw within the table's range.
        let mut p = [0u32; 10];
        r.scramble(&mut p);
        let mut sorted = p;
        sorted.sort_unstable();
        assert_eq!(sorted, [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
        let t = [0.0, 0.25, 0.5, 1.0];
        assert!((0..100).all(|_| (0.0..=1.0).contains(&r.tablerand(&t))));
        // sputter reads in order and only ever repeats; its two ends.
        let s = r.sputter(8, 0.5, 100);
        assert!(
            s.windows(2).all(|w| w[1] == w[0] || w[1] == w[0] + 1),
            "{s:?}"
        );
        assert_eq!(s.first(), Some(&0));
        assert_eq!(s.last(), Some(&7));
        assert_eq!(r.sputter(4, 0.0, 100), [0, 1, 2, 3]);
        assert_eq!(r.sputter(4, 1.0, 5), [0, 0, 0, 0, 0]);
        assert!(r.sputter(0, 0.5, 10).is_empty());
    }

    #[test]
    fn the_pattern_distributions_have_sclangs_shapes() {
        let mut r = Rng::from_seed(23);
        let n = 100_000;
        let mut mean =
            |d: Draw, a: f64, b: f64| (0..n).map(|_| r.draw(d, a, b)).sum::<f64>() / n as f64;
        // Two draws: the expected min, max and mean of two uniforms on 0..1.
        assert!((mean(Draw::Lprand, 0.0, 1.0) - 1.0 / 3.0).abs() < 0.01);
        assert!((mean(Draw::Hprand, 0.0, 1.0) - 2.0 / 3.0).abs() < 0.01);
        assert!((mean(Draw::Meanrand, 0.0, 1.0) - 0.5).abs() < 0.01);
        // Beta(1, 1) is uniform; beta(a, b) has mean a / (a + b).
        assert!((mean(Draw::Betarand, 1.0, 1.0) - 0.5).abs() < 0.01);
        assert!((mean(Draw::Betarand, 2.0, 6.0) - 0.25).abs() < 0.01);
        // Poisson's mean is its parameter.
        assert!((mean(Draw::Poisson, 3.0, 0.0) - 3.0).abs() < 0.05);
        // Cauchy has no mean; its median is the parameter.
        let mut c: Vec<f64> = (0..n).map(|_| r.draw(Draw::Cauchy, 10.0, 2.0)).collect();
        c.sort_by(f64::total_cmp);
        assert!((c[n / 2] - 10.0).abs() < 0.05);
        assert!((0..1000).all(|_| (0.0..=1.0).contains(&r.draw(Draw::Betarand, 0.5, 0.5))));
        assert!((0..1000).all(|_| r.draw(Draw::Poisson, 1.0, 0.0).fract() == 0.0));
    }

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

/// **scsynth's generator**: the three-word Tausworthe generator (L'Ecuyer's
/// `taus88`) every UGen of scsynth's draws from, with its seeding and its
/// conversions -- `SC_RGen.h`, ported exactly, so a seed gives the stream
/// scsynth gives for it.
///
/// The server's UGens draw from this rather than from [`Rng`] for that
/// reason: a def ported from SuperCollider and run from the seed it was run
/// from in SuperCollider (`RandSeed`, or the server's seed) gives the same
/// numbers, and the server's own reference tests can hold the random UGens to
/// scsynth's samples rather than to their statistics. [`Rng`] stays the
/// sequencing layer's: a client's patterns are not scsynth's and never drew
/// from this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RGen {
    s1: u32,
    s2: u32,
    s3: u32,
}

/// Thomas Wang's 32-bit integer hash, scsynth's `Hash`: what `RGen::init`
/// passes a seed through, since people tend to use small ones.
#[inline]
pub fn sc_hash(key: i32) -> i32 {
    let mut hash = key as u32;
    hash = hash.wrapping_add(!(hash << 15));
    hash ^= hash >> 10;
    hash = hash.wrapping_add(hash << 3);
    hash ^= hash >> 6;
    hash = hash.wrapping_add(!(hash << 11));
    hash ^= hash >> 16;
    hash as i32
}

impl RGen {
    /// A generator seeded as scsynth's `RGen::init` seeds one.
    pub fn new(seed: u32) -> Self {
        let mut g = Self {
            s1: 0,
            s2: 0,
            s3: 0,
        };
        g.init(seed);
        g
    }

    /// Reseeds in place, scsynth's `RGen::init`.
    pub fn init(&mut self, seed: u32) {
        let seed = sc_hash(seed as i32) as u32;
        self.s1 = 1_243_598_713 ^ seed;
        if self.s1 < 2 {
            self.s1 = 1_243_598_713;
        }
        self.s2 = 3_093_459_404 ^ seed;
        if self.s2 < 8 {
            self.s2 = 3_093_459_404;
        }
        self.s3 = 1_821_928_721 ^ seed;
        if self.s3 < 16 {
            self.s3 = 1_821_928_721;
        }
    }

    /// One step: a random 32-bit word.
    #[inline]
    pub fn trand(&mut self) -> u32 {
        self.s1 = ((self.s1 & 0xFFFF_FFFE) << 12) ^ (((self.s1 << 13) ^ self.s1) >> 19);
        self.s2 = ((self.s2 & 0xFFFF_FFF8) << 4) ^ (((self.s2 << 2) ^ self.s2) >> 25);
        self.s3 = ((self.s3 & 0xFFFF_FFF0) << 17) ^ (((self.s3 << 3) ^ self.s3) >> 11);
        self.s1 ^ self.s2 ^ self.s3
    }

    /// `[1, 2)`, the 23 high bits of a word as a float's mantissa.
    #[inline]
    pub fn frand0(&mut self) -> f32 {
        f32::from_bits(0x3F80_0000 | (self.trand() >> 9))
    }

    /// `[0, 1)`.
    #[inline]
    pub fn frand(&mut self) -> f32 {
        f32::from_bits(0x3F80_0000 | (self.trand() >> 9)) - 1.0
    }

    /// `[-1, 1)`.
    #[inline]
    pub fn frand2(&mut self) -> f32 {
        f32::from_bits(0x4000_0000 | (self.trand() >> 9)) - 3.0
    }

    /// `[-0.125, 0.125)`.
    #[inline]
    pub fn frand8(&mut self) -> f32 {
        f32::from_bits(0x3E80_0000 | (self.trand() >> 9)) - 0.375
    }

    /// `-1` or `+1`.
    #[inline]
    pub fn fcoin(&mut self) -> f32 {
        f32::from_bits(0x3F80_0000 | (0x8000_0000 & self.trand()))
    }

    /// `[0, 1)` with a word's 32 bits of resolution.
    #[inline]
    pub fn drand(&mut self) -> f64 {
        self.trand() as f64 / 4_294_967_296.0
    }

    /// `[-1, 1)`.
    #[inline]
    pub fn drand2(&mut self) -> f64 {
        self.drand() * 2.0 - 1.0
    }

    /// `[0, 1)`, the lower of two: a linear distribution.
    #[inline]
    pub fn flinrand(&mut self) -> f32 {
        let (a, b) = (self.frand(), self.frand());
        a.min(b)
    }

    /// `(-1, 1)`, the difference of two: a triangular distribution.
    #[inline]
    pub fn fbilinrand(&mut self) -> f32 {
        let (a, b) = (self.frand(), self.frand());
        a - b
    }

    /// Three uniforms summed and centred: a rough bell in about `(-1, 1)`.
    #[inline]
    pub fn fsum3rand(&mut self) -> f32 {
        let sum = self.frand() + self.frand() + self.frand();
        ((sum as f64 - 1.5) * 0.666_666_667) as f32
    }

    /// `[0, scale)` in a linear distribution.
    #[inline]
    pub fn linrand(&mut self, scale: f64) -> f64 {
        let (a, b) = (self.drand(), self.drand());
        a.min(b) * scale
    }

    /// `(-scale, scale)` in a triangular distribution.
    #[inline]
    pub fn bilinrand(&mut self, scale: f64) -> f64 {
        let (a, b) = (self.drand(), self.drand());
        (a - b) * scale
    }

    /// `[lo, hi)` log-uniformly, scsynth's `exprandrng` (`lo` and `hi`
    /// positive).
    #[inline]
    pub fn exprandrng(&mut self, lo: f64, hi: f64) -> f64 {
        lo * ((hi / lo).ln() * self.drand()).exp()
    }

    /// Three uniforms summed and centred, scaled.
    #[inline]
    pub fn sum3rand(&mut self, scale: f64) -> f64 {
        (self.drand() + self.drand() + self.drand() - 1.5) * 0.666_666_667 * scale
    }

    /// An integer in `[0, scale)`.
    #[inline]
    pub fn irand(&mut self, scale: i32) -> i32 {
        (scale as f64 * self.drand()).floor() as i32
    }

    /// An integer in `[-scale, scale]`.
    #[inline]
    pub fn irand2(&mut self, scale: i32) -> i32 {
        ((2.0 * scale as f64 + 1.0) * self.drand() - scale as f64).floor() as i32
    }
}

#[cfg(test)]
mod rgen_tests {
    use super::*;

    #[test]
    fn the_stream_is_scsynths() {
        // Computed with scsynth's own `Hash` and `RGen` (SC_RGen.h, Hash.h).
        assert_eq!(sc_hash(0), 1_177_991_625);
        assert_eq!(sc_hash(1956), -2_019_139_805);
        let mut g = RGen::new(1956);
        let words: Vec<u32> = (0..4).map(|_| g.trand()).collect();
        assert_eq!(
            words,
            [19_990_754, 741_962_571, 1_167_491_941, 3_662_646_931]
        );
        assert_eq!(g.drand(), 0.016_599_499_154_835_94);
        assert_eq!(g.frand2(), 0.155_036_21);
        assert_eq!(g.frand(), 0.153_249_74);
    }

    #[test]
    fn a_seed_replays_and_two_seeds_part() {
        let (mut a, mut b, mut c) = (RGen::new(1956), RGen::new(1956), RGen::new(1957));
        let xa: Vec<u32> = (0..8).map(|_| a.trand()).collect();
        let xb: Vec<u32> = (0..8).map(|_| b.trand()).collect();
        let xc: Vec<u32> = (0..8).map(|_| c.trand()).collect();
        assert_eq!(xa, xb);
        assert_ne!(xa, xc);
    }

    #[test]
    fn the_conversions_stay_in_their_ranges() {
        let mut g = RGen::new(7);
        for _ in 0..10_000 {
            let f = g.frand();
            assert!((0.0..1.0).contains(&f));
            let f2 = g.frand2();
            assert!((-1.0..1.0).contains(&f2));
            let f8 = g.frand8();
            assert!((-0.125..0.125).contains(&f8));
            let d = g.drand();
            assert!((0.0..1.0).contains(&d));
            assert!(g.fcoin().abs() == 1.0);
        }
    }
}
