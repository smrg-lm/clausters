//! The seeded value stream.
//!
//! The random patterns draw from here, so one root seed replays a whole script
//! identically in every client language. `spawn` is the sclang-style
//! inheritance a routine's own stream is built from; the u64 state never
//! crosses to JS, which keeps the surface free of BigInt.

#[cfg(target_arch = "wasm32")]
use clausters_core::rng::Rng;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// A resumable seeded value stream, the JS face of
/// [`clausters_core::rng::Rng`].
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = Rng)]
pub struct JsRng(Rng);

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = Rng)]
impl JsRng {
    /// The stream for `seed` (splitmix64-mixed, never zero) -- the same seeding
    /// as the server's `WhiteNoise`.
    #[wasm_bindgen(constructor)]
    pub fn new(seed: f64) -> JsRng {
        JsRng(Rng::from_seed(seed as u64))
    }

    /// Uniform in `[0, 1)` with 53-bit resolution.
    #[wasm_bindgen(js_name = nextF64)]
    pub fn next_f64(&mut self) -> f64 {
        self.0.next_f64()
    }

    /// One draw from the distribution named `draw` (`"rand"`, `"rrand"`,
    /// `"exprand"`, `"coin"`, `"irand"`, `"gauss"` ...) with its arguments `a`
    /// and `b` -- `clausters_core::rng::Draw`, by name.
    pub fn draw(&mut self, draw: &str, a: f64, b: f64) -> Result<f64, JsError> {
        let d = clausters_core::rng::Draw::from_name(draw)
            .ok_or_else(|| JsError::new(&format!("unknown distribution '{draw}'")))?;
        Ok(self.0.draw(d, a, b))
    }

    /// The index `wchoose` picks from the relative `weights`.
    pub fn windex(&mut self, weights: &[f64]) -> f64 {
        self.0.windex(weights) as f64
    }

    /// One draw shaped by a table `asrandomtable` built.
    pub fn tablerand(&mut self, table: &[f64]) -> f64 {
        self.0.tablerand(table)
    }

    /// A uniformly random permutation of `0..n`.
    pub fn scramble(&mut self, n: usize) -> Vec<u32> {
        let mut out = vec![0; n];
        self.0.scramble(&mut out);
        out
    }

    /// sclang's `sputter` of a list of `n` values: its indices, each repeated
    /// while a `probability` coin lands, at most `maxlen` of them.
    pub fn sputter(&mut self, n: usize, probability: f64, maxlen: usize) -> Vec<u32> {
        self.0.sputter(n, probability, maxlen)
    }

    /// Uniform integer in `[0, n)`; 0 when `n` is 0.
    #[wasm_bindgen(js_name = nextBelow)]
    pub fn next_below(&mut self, n: f64) -> f64 {
        self.0.next_below(n.max(0.0) as u64) as f64
    }

    /// A child stream seeded from this one's next word: deterministic
    /// derivation, so seeding a root reproduces every stream created under it,
    /// in creation order.
    pub fn spawn(&mut self) -> JsRng {
        JsRng(Rng::from_seed(self.0.next_u64()))
    }
}
