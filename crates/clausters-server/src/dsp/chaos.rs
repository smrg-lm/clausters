//! The chaotic maps, scsynth's from its `NoiseUGens.cpp` and
//! `ChaosUGens.cpp`: iterated functions sampled at a rate, whose output is
//! deterministic from its initial state and sensitive to it.

use crate::dsp::{ProcessCtx, UGen, at};

/// `Logistic(chaos_param, freq, init)`: the logistic map `y = r y (1 - y)`,
/// iterated `freq` times a second and held between. `r` near 3.57 and above
/// is chaos; below it the map settles into cycles. The first output is the
/// first iterate of `init`.
///
/// scsynth's constructor iterates once before the first block, so its
/// output starts an iterate later.
pub struct Logistic {
    y: f64,
    counter: i64,
    started: bool,
}

impl Logistic {
    pub fn new() -> Self {
        Self {
            y: 0.0,
            counter: 0,
            started: false,
        }
    }
}

impl Default for Logistic {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for Logistic {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        if !self.started {
            self.started = true;
            self.y = at(inputs[2], 0) as f64;
        }
        let r = at(inputs[0], 0) as f64;
        let freq = at(inputs[1], 0);
        for s in output.iter_mut() {
            if self.counter <= 0 {
                self.counter = ((ctx.sample_rate as f64 / freq.max(0.001) as f64) as i64).max(1);
                self.y = r * self.y * (1.0 - self.y);
            }
            self.counter -= 1;
            *s = self.y as f32;
        }
    }
}

/// Which map a [`Chaos`] iterates. Each is scsynth's, its inputs after
/// `freq` in scsynth's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChaosMap {
    /// `Cusp(a, b, xi)`: `x = a - b sqrt(|x|)`.
    Cusp,
    /// `Gbman(xi, yi)`: the gingerbread man, `x = 1 - y + |x|`, `y` the last
    /// `x`.
    Gbman,
    /// `Henon(a, b, x0, x1)`: `x[n+1] = 1 - a x[n]^2 + b x[n-1]`, from `x0`
    /// and `x1`; the output starts at `x1`, `x0` the value before it.
    Henon,
    /// `Latoocarfian(a, b, c, d, xi, yi)`: `x = sin(b y) + c sin(b x)`,
    /// `y = sin(a x) + d sin(a y)`.
    Latoocarfian,
    /// `LinCong(a, c, m, xi)`: the linear congruential generator
    /// `x = (a x + c) mod m`, scaled to +/-1.
    LinCong,
    /// `Lorenz(s, r, b, h, xi, yi, zi)`: the Lorenz system integrated by
    /// fourth-order Runge-Kutta with step `h`; the output is `x` scaled by
    /// 0.04.
    Lorenz,
    /// `Quad(a, b, c, xi)`: `x = a x^2 + b x + c`.
    Quad,
    /// `Standard(k, xi, yi)`: Chirikov's standard map, `y = (y + k sin x)
    /// mod 2 pi`, `x = (x + y) mod 2 pi`, the output `x` scaled to +/-1.
    Standard,
    /// `FBSine(im, fb, a, c, xi, yi)`: `x = sin(im y + fb x)`,
    /// `y = (a y + c) mod 2 pi`.
    FbSine,
}

/// How a [`Chaos`] joins the map's values: scsynth's `N`, `L` and `C`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChaosInterp {
    /// Each value held for its period.
    Hold,
    /// A line from the last value to the new one across the period.
    Linear,
    /// A cubic through the last four values, between the second and third:
    /// two periods late, as scsynth's is.
    Cubic,
}

impl ChaosMap {
    /// The inputs that are initial conditions.
    fn initial(self) -> &'static [usize] {
        match self {
            ChaosMap::Cusp => &[3],
            ChaosMap::Gbman => &[1, 2],
            ChaosMap::Henon => &[3, 4],
            ChaosMap::Latoocarfian => &[5, 6],
            ChaosMap::LinCong => &[4],
            ChaosMap::Lorenz => &[5, 6, 7],
            ChaosMap::Quad => &[4],
            ChaosMap::Standard => &[2, 3],
            ChaosMap::FbSine => &[5, 6],
        }
    }
}

/// scsynth's `mod2pi`: into `[0, 2 pi)` by one step either way, else by a
/// truncated division.
#[inline]
fn mod2pi(x: f64) -> f64 {
    use std::f64::consts::TAU;
    if x >= TAU {
        let y = x - TAU;
        if y < TAU {
            return y;
        }
        y - TAU * ((y * (1.0 / TAU)) as i64) as f64
    } else if x < 0.0 {
        let y = x + TAU;
        if y >= 0.0 {
            return y;
        }
        y - TAU * ((y * (1.0 / TAU)) as i64) as f64
    } else {
        x
    }
}

/// scsynth's `double` `sc_mod` into `[0, m)`.
#[inline]
fn sc_mod(x: f64, m: f64) -> f64 {
    if x >= m {
        let y = x - m;
        if y < m {
            return y;
        }
        y - m * (y / m).floor()
    } else if x < 0.0 {
        let y = x + m;
        if y >= 0.0 {
            return y;
        }
        y - m * (y / m).floor()
    } else {
        x
    }
}

/// The chaotic maps of scsynth's `ChaosUGens.cpp`, one engine for the
/// twenty-four: a map stepped `freq` times a second (input 0; at the sample
/// rate and above, every sample), its value held, joined by lines or by a
/// cubic. The map's parameters are read once a block, as scsynth reads
/// them; an initial condition that changes restarts the map from it.
///
/// scsynth's, with its counter in `f32` so the steps fall on its samples.
/// Where scsynth's variants of one map disagree, the theory decides, and
/// the defs decision lists them: its `HenonN` and `HenonL` iterate with the
/// two initial values exchanged (only `HenonC` iterates the Henon map as it
/// is written), and its three diverge differently; its cubic variants and
/// `LinCongL` start from a state that is not the map's (zeros, or `xi`
/// unscaled, or `xi` in every coefficient) for the first period; its
/// `FBSineC` restarts `x` but not `y`. Here every variant starts at the
/// map's first value and a diverged Henon map holds its last value until a
/// parameter changes.
pub struct Chaos {
    map: ChaosMap,
    interp: ChaosInterp,
    state: [f64; 3],
    /// The initial conditions the state was started from.
    ics: [f32; 3],
    /// `Henon`: the parameters it diverged with, and whether it has.
    params: [f32; 2],
    diverged: bool,
    counter: f32,
    frac: f64,
    /// The last four values, oldest first.
    h: [f64; 4],
    coef: [f64; 4],
    started: bool,
}

impl Chaos {
    pub fn new(map: ChaosMap, interp: ChaosInterp) -> Self {
        Self {
            map,
            interp,
            state: [0.0; 3],
            ics: [0.0; 3],
            params: [0.0; 2],
            diverged: false,
            counter: 0.0,
            frac: 0.0,
            h: [0.0; 4],
            coef: [0.0; 4],
            started: false,
        }
    }

    /// The map's state from its initial conditions.
    fn init(&mut self, ics: [f32; 3]) {
        self.ics = ics;
        let [a, b, c] = ics.map(|x| x as f64);
        self.state = [a, b, c];
        self.diverged = false;
    }

    /// The value the map outputs from its state, with the block's
    /// parameters.
    fn value(&self, p: &[f64; 8]) -> f64 {
        let s = &self.state;
        match self.map {
            ChaosMap::LinCong => s[0] * (2.0 / p[3].max(0.001)) - 1.0,
            ChaosMap::Lorenz => s[0] * 0.04,
            ChaosMap::Standard => (s[0] - std::f64::consts::PI) * (1.0 / std::f64::consts::PI),
            // The newer of its two values.
            ChaosMap::Henon => s[1],
            _ => s[0],
        }
    }

    /// The value before the first, for a map whose state holds one: the
    /// gingerbread man's `y` and the Henon map's first value. Every other
    /// map starts from its first value alone.
    fn before(&self, p: &[f64; 8]) -> f64 {
        match self.map {
            ChaosMap::Gbman => self.state[1],
            ChaosMap::Henon => self.state[0],
            _ => self.value(p),
        }
    }

    /// One step of the map.
    fn step(&mut self, p: &[f64; 8]) {
        let s = &mut self.state;
        match self.map {
            ChaosMap::Cusp => s[0] = p[1] - p[2] * s[0].abs().sqrt(),
            ChaosMap::Gbman => {
                let x = s[0];
                s[0] = if x < 0.0 {
                    1.0 - s[1] - x
                } else {
                    1.0 - s[1] + x
                };
                s[1] = x;
            }
            ChaosMap::Henon => {
                if self.diverged {
                    return;
                }
                let next = 1.0 - p[1] * s[1] * s[1] + p[2] * s[0];
                if !(-1.5..=1.5).contains(&next) {
                    self.diverged = true;
                    return;
                }
                s[0] = s[1];
                s[1] = next;
            }
            ChaosMap::Latoocarfian => {
                let (x, y) = (s[0], s[1]);
                s[0] = (y * p[2]).sin() + p[3] * (x * p[2]).sin();
                s[1] = (x * p[1]).sin() + p[4] * (y * p[1]).sin();
            }
            ChaosMap::LinCong => s[0] = sc_mod(s[0] * p[1] + p[2], p[3].max(0.001)),
            ChaosMap::Lorenz => {
                let (sg, r, b, h) = (p[1], p[2], p[3], p[4]);
                let (x, y, z) = (s[0], s[1], s[2]);
                let hs = h * sg;
                let k1x = hs * (y - x);
                let k1y = h * (x * (r - z) - y);
                let k1z = h * (x * y - b * z);
                let (hx, hy, hz) = (k1x * 0.5, k1y * 0.5, k1z * 0.5);
                let k2x = hs * (y + hy - x - hx);
                let k2y = h * ((x + hx) * (r - z - hz) - (y + hy));
                let k2z = h * ((x + hx) * (y + hy) - b * (z + hz));
                let (hx, hy, hz) = (k2x * 0.5, k2y * 0.5, k2z * 0.5);
                let k3x = hs * (y + hy - x - hx);
                let k3y = h * ((x + hx) * (r - z - hz) - (y + hy));
                let k3z = h * ((x + hx) * (y + hy) - b * (z + hz));
                let k4x = hs * (y + k3y - x - k3x);
                let k4y = h * ((x + k3x) * (r - z - k3z) - (y + k3y));
                let k4z = h * ((x + k3x) * (y + k3y) - b * (z + k3z));
                let sixth = 1.0 / 6.0;
                s[0] = x + (k1x + 2.0 * (k2x + k3x) + k4x) * sixth;
                s[1] = y + (k1y + 2.0 * (k2y + k3y) + k4y) * sixth;
                s[2] = z + (k1z + 2.0 * (k2z + k3z) + k4z) * sixth;
            }
            ChaosMap::Quad => s[0] = p[1] * s[0] * s[0] + p[2] * s[0] + p[3],
            ChaosMap::Standard => {
                s[1] = mod2pi(s[1] + p[1] * s[0].sin());
                s[0] = mod2pi(s[0] + s[1]);
            }
            ChaosMap::FbSine => {
                s[0] = (p[1] * s[1] + p[2] * s[0]).sin();
                s[1] = mod2pi(p[3] * s[1] + p[4]);
            }
        }
    }

    /// Pushes `v` as the newest value, and the cubic's coefficients with it.
    fn push(&mut self, v: f64) {
        self.h.copy_within(1.., 0);
        self.h[3] = v;
        self.coefficients();
    }

    /// The cubic through the history, between its second and third values.
    fn coefficients(&mut self) {
        let [y0, y1, y2, y3] = self.h;
        self.coef = [
            y1,
            0.5 * (y2 - y0),
            y0 - 2.5 * y1 + y2 + y2 - 0.5 * y3,
            0.5 * (y3 - y0) + 1.5 * (y1 - y2),
        ];
    }

    /// The map from its initial conditions. At the start the history is the
    /// first value, and for a map that holds a value before it (`before`),
    /// that one where scsynth places it: before the first for the line, in
    /// the cubic's third place, which is the history scsynth's `HenonC`
    /// starts from. A restart pushes the first value.
    fn restart(&mut self, ics: [f32; 3], p: &[f64; 8]) {
        self.init(ics);
        let v = self.value(p);
        if self.started {
            self.push(v);
            return;
        }
        let b = self.before(p);
        self.h = if self.interp == ChaosInterp::Cubic {
            [v, v, b, v]
        } else {
            [b, b, b, v]
        };
        self.coefficients();
    }
}

impl UGen for Chaos {
    fn reset(&mut self) {
        self.started = false;
        self.counter = 0.0;
        self.frac = 0.0;
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let mut p = [0.0f64; 8];
        for (k, x) in p.iter_mut().enumerate().take(inputs.len()) {
            *x = at(inputs[k], 0) as f64;
        }
        let mut ics = [0.0f32; 3];
        for (slot, &k) in ics.iter_mut().zip(self.map.initial()) {
            *slot = at(inputs[k], 0);
        }
        if !self.started {
            self.restart(ics, &p);
            self.started = true;
            self.params = [p[1] as f32, p[2] as f32];
        } else if ics != self.ics || (self.diverged && self.params != [p[1] as f32, p[2] as f32]) {
            self.restart(ics, &p);
        }
        self.params = [p[1] as f32, p[2] as f32];
        let freq = at(inputs[0], 0);
        let sr = ctx.sample_rate;
        let (spc, slope) = if freq < sr {
            let spc = (sr as f64 / freq.max(0.001) as f64) as f32;
            (spc, (1.0 / spc) as f64)
        } else {
            (1.0, 1.0)
        };
        for s in output.iter_mut() {
            if self.counter >= spc {
                self.counter -= spc;
                self.frac = 0.0;
                self.step(&p);
                if !(self.map == ChaosMap::Henon && self.diverged) {
                    let v = self.value(&p);
                    self.push(v);
                }
            }
            self.counter += 1.0;
            let h = &self.h;
            *s = match self.interp {
                ChaosInterp::Hold => h[3],
                ChaosInterp::Linear => h[2] + (h[3] - h[2]) * self.frac,
                ChaosInterp::Cubic => {
                    let f = self.frac as f32 as f64;
                    let [c0, c1, c2, c3] = self.coef;
                    ((c3 * f + c2) * f + c1) * f + c0
                }
            } as f32;
            self.frac += slope;
        }
    }
}
