//! Reverberators, scsynth's from its `ReverbUGens.cpp`: Jezar's Freeverb,
//! mono (`FreeVerb`) and stereo (`FreeVerb2`), and Juhana Sadeharju's GVerb
//! (`GVerb`), a feedback delay network.
//!
//! **Freeverb's tunings are for 44.1 kHz**, as its author's code says, and
//! scsynth's are the same samples at every rate, so its room shrinks as
//! the rate grows -- by half at 88.2 kHz. Here each line is the tuning
//! scaled to the server's rate, the same lines as scsynth's at 44.1 kHz.
//! The rest is scsynth's to the order of its sums: eight lowpassed combs in
//! parallel into four allpasses in series, each line read a sample before
//! its feedback is taken, as the Faust program scsynth's was compiled from
//! reads them.

use crate::dsp::{ProcessCtx, Ramp, UGen, at};

/// The Freeverb tunings in samples at 44.1 kHz, scsynth's order: the combs,
/// then the allpasses from the last of the chain to the first.
const COMBS: [usize; 8] = [1617, 1557, 1491, 1422, 1277, 1116, 1188, 1356];
const ALLPASSES: [usize; 4] = [556, 441, 341, 225];
/// How much longer each of the right channel's lines is, Freeverb's
/// stereo spread.
const SPREAD: usize = 23;

/// A tuning at the server's rate.
fn scaled(len: usize, sample_rate: f32) -> usize {
    ((len as f64 * sample_rate as f64 / 44_100.0).round() as usize).max(1)
}

/// One line of a Freeverb tank: its samples, where it is, the sample it
/// read last and, for a comb, its lowpass.
struct Line {
    buf: Vec<f32>,
    idx: usize,
    last: f32,
    lp: f32,
}

impl Line {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len],
            idx: 0,
            last: 0.0,
            lp: 0.0,
        }
    }

    /// The sample `len` behind, the write position moved onto it.
    #[inline]
    fn read(&mut self) -> f32 {
        self.idx += 1;
        if self.idx == self.buf.len() {
            self.idx = 0;
        }
        self.buf[self.idx]
    }

    fn clear(&mut self) {
        self.buf.fill(0.0);
        self.idx = 0;
        self.last = 0.0;
        self.lp = 0.0;
    }
}

/// One channel of Freeverb: eight combs and four allpasses.
struct Tank {
    combs: Vec<Line>,
    allpasses: Vec<Line>,
}

impl Tank {
    fn new(sample_rate: f32, extra: usize) -> Self {
        Self {
            combs: COMBS
                .iter()
                .map(|&l| Line::new(scaled(l + extra, sample_rate)))
                .collect(),
            allpasses: ALLPASSES
                .iter()
                .map(|&l| Line::new(scaled(l + extra, sample_rate)))
                .collect(),
        }
    }

    fn clear(&mut self) {
        for l in self.combs.iter_mut().chain(&mut self.allpasses) {
            l.clear();
        }
    }

    /// One sample of the wet signal for an input already scaled by the
    /// tank's gain, with the comb feedback `room` and lowpass `damp`.
    #[inline]
    fn tick(&mut self, x: f32, room: f32, damp: f32) -> f32 {
        let t: [f32; 4] = std::array::from_fn(|k| self.allpasses[k].read());
        for c in &mut self.combs {
            let v = c.read();
            c.lp = (1.0 - damp) * c.last + damp * c.lp;
            c.buf[c.idx] = x + room * c.lp;
            c.last = v;
        }
        let c: [f32; 8] = std::array::from_fn(|k| self.combs[k].last);
        // scsynth's sums, in its order, so the rounding is its.
        let tail = c[6] + c[7];
        let a = &mut self.allpasses;
        let first = &mut a[0];
        let i = first.idx;
        first.buf[i] =
            ((0.5 * first.last + c[0]) + (c[1] + c[2])) + ((c[3] + c[4]) + (c[5] + tail));
        first.last = t[0];
        let mut y = first.last - (((c[0] + c[1]) + (c[2] + c[3])) + ((c[4] + c[5]) + tail));
        for (line, &t) in a[1..].iter_mut().zip(&t[1..]) {
            let i = line.idx;
            line.buf[i] = 0.5 * line.last + y;
            line.last = t;
            y = line.last - y;
        }
        y
    }
}

/// The mix, room and damping, each clipped to 0..1, as the gains they
/// are: the dry and wet levels, the comb feedback, the lowpass.
fn freeverb_params(mix: f32, room: f32, damp: f32) -> (f32, f32, f32, f32) {
    let mix = mix.clamp(0.0, 1.0);
    let room = 0.7 + 0.28 * room.clamp(0.0, 1.0);
    let damp = 0.4 * damp.clamp(0.0, 1.0);
    (1.0 - mix, mix, room, damp)
}

/// `FreeVerb(in, mix, room, damp)`: Freeverb on one channel. `mix` is the
/// wet part of the output, `room` the size (the combs' feedback, 0.7 to
/// 0.98) and `damp` how fast the highs die (their lowpass).
pub struct FreeVerb {
    tank: Tank,
}

impl FreeVerb {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            tank: Tank::new(sample_rate, 0),
        }
    }
}

impl UGen for FreeVerb {
    fn reset(&mut self) {
        self.tank.clear();
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let (dry, wet, room, damp) = freeverb_params(inputs[1][0], inputs[2][0], inputs[3][0]);
        for (i, s) in output.iter_mut().enumerate() {
            let x = at(inputs[0], i);
            let y = self.tank.tick(0.015 * x, room, damp);
            *s = dry * x + wet * y;
        }
    }
}

/// `FreeVerb2(in, in2, mix, room, damp, chan)`: Freeverb on a pair, both
/// channels' tanks fed their sum, the right's lines 23 samples longer.
/// One row a channel: `chan` 0 is the left, 1 the right.
pub struct FreeVerb2 {
    tank: Tank,
    right: bool,
}

impl FreeVerb2 {
    pub fn new(sample_rate: f32, chan: f32) -> Self {
        let right = chan >= 0.5;
        Self {
            tank: Tank::new(sample_rate, if right { SPREAD } else { 0 }),
            right,
        }
    }
}

impl UGen for FreeVerb2 {
    fn reset(&mut self) {
        self.tank.clear();
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let (dry, wet, room, damp) = freeverb_params(inputs[2][0], inputs[3][0], inputs[4][0]);
        let own = if self.right { inputs[1] } else { inputs[0] };
        for (i, s) in output.iter_mut().enumerate() {
            let x = 0.015 * (at(inputs[0], i) + at(inputs[1], i));
            let y = self.tank.tick(x, room, damp);
            *s = dry * at(own, i) + wet * y;
        }
    }
}

// --- GVerb ----------------------------------------------------------------

/// A value too small or too large to keep, flushed to 0: scsynth's
/// `zapgremlins`.
#[inline]
fn zap(x: f32) -> f32 {
    let a = x.abs();
    if a > 1e-15 && a < 1e15 { x } else { 0.0 }
}

/// GVerb's own flush, of values below about `2^-112`.
#[inline]
fn flush(x: f32) -> f32 {
    if (x.to_bits() & 0x7f80_0000) < 0x0800_0000 {
        0.0
    } else {
        x
    }
}

/// Rounding to the nearest integer, half to even, as GVerb's `f_round`.
#[inline]
fn f_round(x: f32) -> i64 {
    x.round_ties_even() as i64
}

/// Whether `n` is prime.
fn is_prime(n: i64) -> bool {
    if n < 2 {
        return false;
    }
    if n < 4 {
        return true;
    }
    if n % 2 == 0 {
        return false;
    }
    let mut i = 3;
    while i * i <= n {
        if n % i == 0 {
            return false;
        }
        i += 2;
    }
    true
}

/// The prime nearest `n`, the one above first on a tie, searched within
/// half of `n`.
///
/// scsynth's truncates that half to an integer before it multiplies, so
/// it searches nowhere and answers -1 for any `n` not already prime: its
/// first line then holds one sample, its feedback a sample long.
fn nearest_prime(n: i64) -> i64 {
    if is_prime(n) {
        return n;
    }
    for k in 1..=(n / 2).max(1) {
        if is_prime(n + k) {
            return n + k;
        }
        if is_prime(n - k) {
            return n - k;
        }
    }
    n.max(1)
}

/// A circular line read any number of samples behind its write, up to its
/// length.
struct Ring {
    buf: Vec<f32>,
    idx: usize,
}

impl Ring {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len.max(1)],
            idx: 0,
        }
    }

    /// The sample written `n` samples ago.
    #[inline]
    fn read(&self, n: usize) -> f32 {
        let len = self.buf.len();
        self.buf[(self.idx + len - n % len) % len]
    }

    #[inline]
    fn write(&mut self, x: f32) {
        self.buf[self.idx] = zap(x);
        self.idx += 1;
        if self.idx == self.buf.len() {
            self.idx = 0;
        }
    }

    fn clear(&mut self) {
        self.buf.fill(0.0);
        self.idx = 0;
    }
}

/// A Schroeder allpass of a fixed length within a line allocated for the
/// longest the room allows.
struct Diffuser {
    buf: Vec<f32>,
    size: usize,
    idx: usize,
    coef: f32,
}

impl Diffuser {
    fn new(capacity: usize, coef: f32) -> Self {
        Self {
            buf: vec![0.0; capacity.max(1)],
            size: 1,
            idx: 0,
            coef,
        }
    }

    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let b = self.buf[self.idx];
        let w = flush(x - b * self.coef);
        let y = b + w * self.coef;
        self.buf[self.idx] = zap(w);
        self.idx += 1;
        if self.idx == self.size {
            self.idx = 0;
        }
        y
    }

    fn clear(&mut self) {
        self.buf.fill(0.0);
        self.idx = 0;
    }
}

/// A one-pole lowpass, `y = (1 - d) x + d y'`.
#[derive(Default, Clone, Copy)]
struct Damper {
    damping: f32,
    delay: f32,
}

impl Damper {
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let y = x * (1.0 - self.damping) + self.delay * self.damping;
        self.delay = zap(y);
        y
    }
}

/// The parameters a block of GVerb reads.
#[derive(Clone, Copy, PartialEq)]
struct GParams {
    roomsize: f32,
    revtime: f32,
    damping: f32,
    bandwidth: f32,
    dry: f32,
    early: f32,
    tail: f32,
}

/// `GVerb(in, roomsize, revtime, damping, inputbw, spread, drylevel,
/// earlyreflevel, taillevel, maxroomsize, chan)`: a room `roomsize` metres
/// across (sound at 340 m/s) whose tail falls 60 dB in `revtime` seconds --
/// four lines in a feedback network mixed by a Hadamard matrix, early
/// reflections tapped from a line before it, each output channel through
/// allpasses of its own. `damping` lowpasses the tail and `inputbw` the
/// input; `spread` (read at the start) sets the outputs' allpasses apart.
/// `maxroomsize` is a constant and sizes the lines. One row a channel:
/// `chan` 0 left, 1 right.
///
/// Where scsynth's departs from the network it is: its first line's length,
/// the prime nearest the room, is -1 unless the room is already prime
/// ([`nearest_prime`]), and a change of room moves the other three lines
/// but never the first; its tap line is 44000 samples whatever the room,
/// and a room grown past the one it started at reads its lines outside
/// them. Its levels start at 0 and rise across the first block. Here every
/// line follows the room within lines sized for `maxroomsize`, and the
/// levels start at theirs.
pub struct GVerb {
    sr: f32,
    right: bool,
    max_room: f32,
    input: Damper,
    fdn: [Ring; 4],
    fdn_len: [usize; 4],
    fdn_gain: [f32; 4],
    fdn_slope: [f32; 4],
    fdn_damp: [Damper; 4],
    /// The input's allpass, then this channel's three.
    difs: [Diffuser; 4],
    tap: Ring,
    taps: [usize; 4],
    tap_gain: [f32; 4],
    tap_slope: [f32; 4],
    alpha: f64,
    d: [f32; 4],
    levels: [Ramp; 3],
    params: Option<GParams>,
}

impl GVerb {
    pub fn new(sample_rate: f32, max_room: f32, chan: f32) -> Self {
        let max_room = max_room.max(1.0001);
        let max_delay = sample_rate * max_room / 340.0;
        let line = max_delay as usize + 1000;
        // The diffusers are a fraction of the shortest line, at most the
        // whole of it.
        let dif = (0.632_45 * max_delay) as usize + 2;
        Self {
            sr: sample_rate,
            right: chan >= 0.5,
            max_room,
            input: Damper::default(),
            fdn: std::array::from_fn(|_| Ring::new(line)),
            fdn_len: [1; 4],
            fdn_gain: [0.0; 4],
            fdn_slope: [0.0; 4],
            fdn_damp: [Damper::default(); 4],
            difs: [
                Diffuser::new(dif, 0.75),
                Diffuser::new(dif, 0.75),
                Diffuser::new(dif, 0.625),
                Diffuser::new(dif, 0.625),
            ],
            tap: Ring::new(44_000.max((0.41 * max_delay) as usize + 6)),
            taps: [5; 4],
            tap_gain: [0.0; 4],
            tap_slope: [0.0; 4],
            alpha: 1.0,
            d: [0.0; 4],
            levels: [Ramp::default(); 3],
            params: None,
        }
    }

    /// The lines' lengths and gains for a room and a reverberation time,
    /// moving the gains in a line across `n` samples from where they were.
    fn set_room(&mut self, roomsize: f32, revtime: f32, n: usize, first: bool) {
        let room = if roomsize <= 1.0 || roomsize.is_nan() {
            1.0
        } else if roomsize >= self.max_room {
            self.max_room - 1.0
        } else {
            roomsize
        };
        // scsynth's arithmetic: the delay in `f64`, kept as an `f32`.
        let largest = (self.sr as f64 * room as f64 / 340.0) as f32 as f64;
        // GVerb's ratios, four figures each: `0.7071` is its, not `1/sqrt(2)`.
        #[allow(clippy::approx_constant)]
        let ratios = [0.816_49, 0.707_1, 0.632_45];
        self.fdn_len = [
            nearest_prime(largest as i64) as usize,
            (ratios[0] * largest) as usize,
            (ratios[1] * largest) as usize,
            (ratios[2] * largest) as usize,
        ];
        let cap = self.fdn[0].buf.len();
        for l in &mut self.fdn_len {
            *l = (*l).clamp(1, cap);
        }
        self.taps = [
            5 + (0.41 * largest) as usize,
            5 + (0.3 * largest) as usize,
            5 + (0.155 * largest) as usize,
            5,
        ];
        let n_rev = self.sr as f64 * revtime as f64;
        self.alpha = 0.001f32.powf((1.0 / n_rev) as f32) as f64;
        for k in 0..4 {
            let g = -self.alpha.powf(self.fdn_len[k] as f64) as f32;
            let t = self.alpha.powf(self.taps[k] as f64) as f32;
            if first {
                (self.fdn_gain[k], self.tap_gain[k]) = (g, t);
                (self.fdn_slope[k], self.tap_slope[k]) = (0.0, 0.0);
            } else {
                self.fdn_slope[k] = (g - self.fdn_gain[k]) / n as f32;
                self.tap_slope[k] = (t - self.tap_gain[k]) / n as f32;
            }
        }
    }

    /// The allpasses' lengths, from the room at the start and `spread`:
    /// scsynth's, as fractions of its shortest line.
    fn set_diffusers(&mut self, roomsize: f32, spread: f32) {
        let largest = (self.sr as f64 * roomsize as f64 / 340.0) as f32;
        let scale = (f_round(0.632_45 * largest) as f64 / 1341.0) as f32;
        let (r1, r2) = if self.right {
            (-0.568_366, -0.126_815)
        } else {
            (0.125_541, 0.854_046)
        };
        let c = 210 + 159 + (spread * r1) as i64;
        let d = 210 + 159 + 562 + ((3.0 * spread) * r2) as i64;
        let sizes = [210, c - 210, d - c, 1341 - d];
        for (dif, s) in self.difs.iter_mut().zip(sizes) {
            dif.size = (f_round(scale * s as f32).max(1) as usize).min(dif.buf.len());
        }
        // The input's allpass is the left's first, on either channel.
        self.difs[0].size = (f_round(scale * 210.0).max(1) as usize).min(self.difs[0].buf.len());
    }
}

impl UGen for GVerb {
    fn reset(&mut self) {
        for r in &mut self.fdn {
            r.clear();
        }
        self.tap.clear();
        for d in &mut self.difs {
            d.clear();
        }
        self.input.delay = 0.0;
        for d in &mut self.fdn_damp {
            d.delay = 0.0;
        }
        self.d = [0.0; 4];
        self.levels = [Ramp::default(); 3];
        self.params = None;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let n = output.len();
        let p = GParams {
            roomsize: inputs[1][0],
            revtime: inputs[2][0],
            damping: inputs[3][0],
            bandwidth: inputs[4][0],
            dry: inputs[6][0],
            early: inputs[7][0],
            tail: inputs[8][0],
        };
        match self.params {
            None => {
                self.set_diffusers(p.roomsize, inputs[5][0]);
                self.set_room(p.roomsize, p.revtime, n, true);
            }
            Some(old) if old != p => self.set_room(p.roomsize, p.revtime, n, false),
            _ => {}
        }
        if self.params != Some(p) {
            for d in &mut self.fdn_damp {
                d.damping = p.damping;
            }
            self.input.damping = 1.0 - p.bandwidth;
        }
        self.params = Some(p);
        let (dry0, ddry) = self.levels[0].toward(p.dry as f64, n);
        let (early0, dearly) = self.levels[1].toward(p.early as f64, n);
        let (tail0, dtail) = self.levels[2].toward(p.tail as f64, n);
        let (mut dry, mut early, mut tail) = (dry0 as f32, early0 as f32, tail0 as f32);
        let (ddry, dearly, dtail) = (ddry as f32, dearly as f32, dtail as f32);
        for (i, s) in output.iter_mut().enumerate() {
            let v = at(inputs[0], i);
            let x = if v.is_nan() { 0.0 } else { v };
            let z = self.input.tick(x);
            let z = self.difs[0].tick(z);
            let u: [f32; 4] =
                std::array::from_fn(|j| self.tap_gain[j] * self.tap.read(self.taps[j]));
            self.tap.write(z);
            for j in 0..4 {
                let r = self.fdn[j].read(self.fdn_len[j]);
                self.d[j] = self.fdn_damp[j].tick(self.fdn_gain[j] * r);
            }
            let mut sum = 0.0f32;
            let mut sign = 1.0f32;
            for (d, u) in self.d.iter().zip(&u) {
                sum += sign * (tail * d + early * u);
                sign = -sign;
            }
            sum += x * early;
            let d = self.d;
            let f = [
                0.5 * (d[0] + d[1] - d[2] - d[3]),
                0.5 * (d[0] - d[1] - d[2] + d[3]),
                0.5 * (-d[0] + d[1] - d[2] + d[3]),
                0.5 * (d[0] + d[1] + d[2] + d[3]),
            ];
            for j in 0..4 {
                self.fdn[j].write(u[j] + f[j]);
            }
            let mut y = sum;
            for dif in &mut self.difs[1..] {
                y = dif.tick(y);
            }
            *s = y + x * dry;
            dry += ddry;
            early += dearly;
            tail += dtail;
            for j in 0..4 {
                self.fdn_gain[j] += self.fdn_slope[j];
                self.tap_gain[j] += self.tap_slope[j];
            }
        }
        self.fdn_slope = [0.0; 4];
        self.tap_slope = [0.0; 4];
    }
}
