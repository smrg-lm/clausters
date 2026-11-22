//! Granular synthesis, scsynth's from its `GrainUGens.cpp` and
//! `DelayUGens.cpp`: grains of an input (`GrainIn`), of a sine (`GrainSin`),
//! of an FM pair (`GrainFM`) and of a buffer (`GrainBuf`, `TGrains`), a
//! buffer stretched in time (`Warp1`), and grains of a delay line
//! (`GrainTap`).
//!
//! **A grain is started by a trigger and reads its parameters there**: its
//! duration, its source's, its pan; then it plays on its own under its
//! window -- a Hann made by a sine recurrence, or a buffer read across the
//! grain's length -- until its duration is out. A trigger at audio rate
//! starts a grain at its sample, at control rate at the block's start.
//!
//! **One row a channel**, the channel count and the index last on the wire,
//! as the pan family's ([`super::pan`]). A grain is panned between two
//! neighbouring channels at equal power, as scsynth's are; each row keeps
//! the grains that sound in its channel and only counts the others, so a
//! ring of many costs about what two channels do and the grain limit is the
//! whole UGen's. A parameter that scsynth would pull from a demand stream
//! is a signal here: the rows cannot each pull one stream, and a `Demand`
//! on the same trigger gives the grain its value.
//!
//! The limit is `maxGrains` grains at once, a constant that sizes the rows;
//! scsynth's holds one fewer.

use crate::dsp::buffer::Buffer;
use crate::dsp::noise::{cubic32, sc_wrap};
use crate::dsp::rand::SynthRand;
use crate::dsp::{ProcessCtx, UGen, at};
use std::f64::consts::{FRAC_PI_2, PI, TAU};

// --- the window --------------------------------------------------------------

/// Where a grain's window is.
#[derive(Clone, Copy)]
enum Window {
    /// The built-in Hann, `sin^2`, by the recurrence `y = b1 y' - y''`.
    Hann { b1: f64, y1: f64, y2: f64 },
    /// A buffer read from its start to its end across the grain, linearly.
    Table { buf: usize, pos: f64, inc: f64 },
}

/// A grain's envelope: its window and the level it gives this sample.
#[derive(Clone, Copy)]
struct Env {
    window: Window,
    amp: f32,
}

impl Env {
    /// The envelope of a grain `len` samples long (its length before it is
    /// truncated to whole samples, as scsynth's window takes it), from window
    /// buffer `wintype` (negative for the Hann). `None` where the buffer has
    /// no samples, scsynth's grain that never starts.
    fn new(ctx: &ProcessCtx, wintype: f32, len: f64) -> Option<Self> {
        if wintype < 0.0 {
            let w = PI / len;
            let y1 = w.sin();
            return Some(Self {
                window: Window::Hann {
                    b1: 2.0 * w.cos(),
                    y1,
                    y2: 0.0,
                },
                amp: (y1 * y1) as f32,
            });
        }
        let buf = wintype as usize;
        let b = ctx.buffer(buf)?;
        if b.is_empty() || b.cells().is_none() {
            return None;
        }
        Some(Self {
            window: Window::Table {
                buf,
                pos: 0.0,
                inc: b.len() as f64 / len,
            },
            amp: b.at(0),
        })
    }

    /// The window's buffer this block, if it has one.
    #[inline]
    fn table<'a>(&self, ctx: &ProcessCtx<'a>) -> Option<&'a Buffer> {
        match self.window {
            Window::Table { buf, .. } => ctx.buffer(buf),
            Window::Hann { .. } => None,
        }
    }

    /// Moves to the next sample's level.
    #[inline]
    fn step(&mut self, table: Option<&Buffer>) {
        match &mut self.window {
            Window::Hann { b1, y1, y2 } => {
                let y0 = *b1 * *y1 - *y2;
                *y2 = *y1;
                *y1 = y0;
                self.amp = (y0 * y0) as f32;
            }
            Window::Table { pos, inc, .. } => {
                let Some(t) = table else {
                    self.amp = 0.0;
                    return;
                };
                *pos += *inc;
                let len = t.len();
                let i = *pos as usize;
                let frac = (*pos - i as f64) as f32;
                let a = t.at(i.min(len - 1));
                let j = if i + 1 >= len { i + 1 - len } else { i + 1 };
                let b = t.at(j.min(len - 1));
                self.amp = a + frac * (b - a);
            }
        }
    }
}

// --- the pan -----------------------------------------------------------------

/// This row's gain for a grain at `pan` on `n` channels, scsynth's law: on
/// two, equal power from left (-1) to right (1); on more, between the two
/// channels either side of `pan` round the ring (-1 to 1, a turn), the
/// channel at 0 half-way between the first two. `None` if the grain does
/// not sound in this row's channel.
fn pan_gain(pan: f32, n: usize, chan: usize) -> Option<f32> {
    if n <= 1 {
        return Some(1.0);
    }
    let (first, angle) = if n > 2 {
        let p = sc_wrap(pan * 0.5, 0.0, 1.0);
        let c = n as f32 * p + 0.5;
        let i = c.floor();
        ((i as usize) % n, ((c - i) as f64) * FRAC_PI_2)
    } else {
        let p = (pan * 0.5 + 0.5).clamp(0.0, 1.0);
        (0, p as f64 * FRAC_PI_2)
    };
    let second = (first + 1) % n;
    if chan == first {
        Some(angle.cos() as f32)
    } else if chan == second {
        Some(angle.sin() as f32)
    } else {
        None
    }
}

/// How long a grain of `dur` seconds lasts, in samples: at least four.
#[inline]
fn grain_len(dur: f32, sr: f32) -> f64 {
    (dur as f64 * sr as f64).max(4.0)
}

/// `x` wrapped into `[0, hi)`, scsynth's `sc_gloop`.
#[inline]
fn gloop(x: f64, hi: f64) -> f64 {
    if x >= hi {
        let y = x - hi;
        if y < hi {
            return y;
        }
    } else if x < 0.0 {
        let y = x + hi;
        if y >= 0.0 {
            return y;
        }
    } else {
        return x;
    }
    x - hi * (x / hi).floor()
}

/// How many of a buffer's frames a sample moves through at a rate of 1,
/// in `f32` as scsynth's.
#[inline]
fn rate_scale(b: &Buffer, sr: f32) -> f32 {
    (b.sample_rate() as f32 as f64 / sr as f64) as f32
}

/// A mono buffer's sample at a position, wrapped round it, with no
/// interpolation (`interp` 1), linear (2) or cubic (4).
#[inline]
fn read(buf: &Buffer, chans: usize, chan: usize, phase: f64, interp: i32) -> f32 {
    let frames = buf.frames();
    let i = phase as usize;
    let s = |f: usize| buf.at((f % frames) * chans + chan);
    let frac = (phase - i as f64) as f32;
    if interp >= 4 {
        cubic32(frac, s(i + frames - 1), s(i), s(i + 1), s(i + 2))
    } else if interp >= 2 {
        let (b, c) = (s(i), s(i + 1));
        b + frac * (c - b)
    } else {
        s(i)
    }
}

// --- the grain sets ----------------------------------------------------------

/// What a grain plays.
#[derive(Clone, Copy)]
enum Voice {
    /// The UGen's input.
    Input,
    /// A sine: its phase and increment, in cycles.
    Sine { phase: f64, inc: f64 },
    /// An FM pair: the carrier's phase and base increment, the modulator's
    /// phase and increment, and the deviation in increments.
    Fm {
        car: f64,
        car_inc: f64,
        modp: f64,
        mod_inc: f64,
        dev: f64,
    },
    /// A mono buffer: which, where, how fast, how read.
    Buf {
        buf: usize,
        phase: f64,
        rate: f64,
        interp: i32,
    },
}

/// One grain sounding in this row.
#[derive(Clone, Copy)]
struct Grain {
    env: Env,
    voice: Voice,
    counter: i64,
    gain: f32,
}

/// One row's grains: those in its channel, and how many the others hold.
struct Grains {
    max: usize,
    mine: Vec<Grain>,
    others: Vec<i64>,
    prev: f32,
}

impl Grains {
    fn new(max: f32) -> Self {
        let max = (max.max(1.0) as usize).min(1 << 16);
        Self {
            max,
            mine: Vec::with_capacity(max),
            others: Vec::with_capacity(max),
            prev: 0.0,
        }
    }

    fn clear(&mut self) {
        self.mine.clear();
        self.others.clear();
        self.prev = 0.0;
    }

    /// Plays grain `g` into `out` from block sample `from`, returning
    /// whether it is still sounding.
    fn play(g: &mut Grain, ctx: &ProcessCtx, input: &[f32], out: &mut [f32], from: usize) -> bool {
        let n = (g.counter.max(0) as usize).min(out.len() - from);
        let table = g.env.table(ctx);
        if matches!(g.env.window, Window::Table { .. }) && table.is_none() {
            // A window buffer freed under it: scsynth's grain stops.
            return false;
        }
        let buf = match g.voice {
            Voice::Buf { buf, .. } => ctx.buffer(buf),
            _ => None,
        };
        for (j, s) in out[from..from + n].iter_mut().enumerate() {
            let v = match &mut g.voice {
                Voice::Input => at(input, from + j),
                Voice::Sine { phase, inc } => {
                    let v = (TAU * *phase).sin() as f32;
                    *phase += *inc;
                    v
                }
                Voice::Fm {
                    car,
                    car_inc,
                    modp,
                    mod_inc,
                    dev,
                } => {
                    let m = (TAU * *modp).sin() * *dev;
                    let v = (TAU * *car).sin() as f32;
                    *car += *car_inc + m;
                    *modp += *mod_inc;
                    v
                }
                Voice::Buf {
                    phase,
                    rate,
                    interp,
                    ..
                } => match buf {
                    Some(b) if b.frames() > 0 => {
                        *phase = gloop(*phase, b.frames() as f64);
                        let v = read(b, 1, 0, *phase, *interp);
                        *phase += *rate;
                        v
                    }
                    _ => 0.0,
                },
            };
            *s += g.env.amp * v * g.gain;
            g.env.step(table);
        }
        g.counter -= n as i64;
        g.counter > 0
    }

    /// Every sounding grain over the block, and the others counted down.
    fn play_all(&mut self, ctx: &ProcessCtx, input: &[f32], out: &mut [f32]) {
        let mut i = 0;
        while i < self.mine.len() {
            if Self::play(&mut self.mine[i], ctx, input, out, 0) {
                i += 1;
            } else {
                self.mine.swap_remove(i);
            }
        }
        let n = out.len() as i64;
        self.others.retain_mut(|c| {
            *c -= n;
            *c > 0
        });
    }

    /// Starts a grain at block sample `from` if the limit allows: its
    /// envelope, its voice and its pan, played to the block's end.
    #[allow(clippy::too_many_arguments)]
    fn start(
        &mut self,
        ctx: &ProcessCtx,
        input: &[f32],
        out: &mut [f32],
        from: usize,
        len: f64,
        wintype: f32,
        voice: Voice,
        gain: Option<f32>,
    ) {
        if self.mine.len() + self.others.len() >= self.max {
            return;
        }
        let Some(env) = Env::new(ctx, wintype, len) else {
            return;
        };
        let counter = len as i64;
        let Some(gain) = gain else {
            // Sounding elsewhere: counted, from the rest of this block on.
            let left = counter - (out.len() - from) as i64;
            if left > 0 {
                self.others.push(left);
            }
            return;
        };
        let mut g = Grain {
            env,
            voice,
            counter,
            gain,
        };
        if Self::play(&mut g, ctx, input, out, from) {
            self.mine.push(g);
        }
    }

    /// The samples of the block a rising trigger starts a grain at.
    fn triggers(&mut self, trig: &[f32], n: usize, mut each: impl FnMut(&mut Self, usize)) {
        if trig.len() == 1 {
            if self.prev <= 0.0 && trig[0] > 0.0 {
                each(self, 0);
            }
            self.prev = trig[0];
            return;
        }
        for (i, &t) in trig[..n].iter().enumerate() {
            if self.prev <= 0.0 && t > 0.0 {
                each(self, i);
            }
            self.prev = t;
        }
    }
}

/// Which source a [`Granular`] row plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrainKind {
    /// `GrainIn(trigger, dur, in, pan, envbufnum, maxGrains, ...)`.
    In,
    /// `GrainSin(trigger, dur, freq, pan, envbufnum, maxGrains, ...)`.
    Sin,
    /// `GrainFM(trigger, dur, carfreq, modfreq, index, pan, envbufnum,
    /// maxGrains, ...)`: a carrier at `carfreq` deviated by `index *
    /// modfreq`.
    Fm,
    /// `GrainBuf(trigger, dur, sndbuf, rate, pos, interp, pan, envbufnum,
    /// maxGrains, ...)`: a mono buffer from `pos` (0 to 1 of it) at `rate`
    /// times its own rate, wrapping round it.
    Buf,
}

impl GrainKind {
    /// Where this kind's `pan`, `envbufnum` and `maxGrains` are.
    fn slots(self) -> (usize, usize, usize) {
        match self {
            GrainKind::In | GrainKind::Sin => (3, 4, 5),
            GrainKind::Fm => (5, 6, 7),
            GrainKind::Buf => (6, 7, 8),
        }
    }
}

/// `GrainIn`, `GrainSin`, `GrainFM`, `GrainBuf`: one channel of `numchans`,
/// the two last inputs.
pub struct Granular {
    kind: GrainKind,
    grains: Grains,
}

impl Granular {
    pub fn new(kind: GrainKind, max: f32) -> Self {
        Self {
            kind,
            grains: Grains::new(max),
        }
    }
}

impl UGen for Granular {
    fn reset(&mut self) {
        self.grains.clear();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
        let kind = self.kind;
        let (pan_at, win_at, max_at) = kind.slots();
        let nchans = (inputs[max_at + 1][0].max(1.0)) as usize;
        let chan = inputs[max_at + 2][0].max(0.0) as usize;
        let input = if kind == GrainKind::In {
            inputs[2]
        } else {
            &[0.0][..]
        };
        let sr = ctx.sample_rate;
        self.grains.play_all(ctx, input, output);
        let n = output.len();
        let ctx = &*ctx;
        self.grains.triggers(inputs[0], n, |g, i| {
            let len = grain_len(at(inputs[1], i), sr);
            let voice = match kind {
                GrainKind::In => Voice::Input,
                GrainKind::Sin => Voice::Sine {
                    phase: 0.0,
                    inc: at(inputs[2], i) as f64 / sr as f64,
                },
                GrainKind::Fm => {
                    let modf = at(inputs[3], i) as f64;
                    Voice::Fm {
                        car: 0.0,
                        car_inc: at(inputs[2], i) as f64 / sr as f64,
                        modp: 0.0,
                        mod_inc: modf / sr as f64,
                        dev: at(inputs[4], i) as f64 * modf / sr as f64,
                    }
                }
                GrainKind::Buf => {
                    let bufnum = at(inputs[2], i).max(0.0) as usize;
                    let Some(b) = ctx.buffer(bufnum) else {
                        return;
                    };
                    // scsynth's precisions: the start and the rate in `f32`.
                    let pos = (at(inputs[4], i) * b.frames() as f32) as f64;
                    if b.channels() != 1 || b.frames() == 0 || !pos.is_finite() {
                        return;
                    }
                    Voice::Buf {
                        buf: bufnum,
                        phase: pos,
                        rate: (at(inputs[3], i) * rate_scale(b, sr)) as f64,
                        interp: at(inputs[5], i) as i32,
                    }
                }
            };
            let gain = pan_gain(at(inputs[pan_at], i), nchans, chan);
            g.start(
                ctx,
                input,
                output,
                i,
                len,
                at(inputs[win_at], i),
                voice,
                gain,
            );
        });
    }
}

// --- TGrains -----------------------------------------------------------------

/// `TGrains(trigger, bufnum, rate, centerPos, dur, pan, amp, interp,
/// numchans, chan)`: grains of a mono buffer centred on `centerPos`
/// seconds, under the Hann, at most 64 at once as scsynth's.
pub struct TGrains {
    grains: Grains,
}

impl Default for TGrains {
    fn default() -> Self {
        Self {
            grains: Grains::new(64.0),
        }
    }
}

impl UGen for TGrains {
    fn reset(&mut self) {
        self.grains.clear();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
        let nchans = (inputs[8][0].max(1.0)) as usize;
        let chan = inputs[9][0].max(0.0) as usize;
        let sr = ctx.sample_rate;
        self.grains.play_all(ctx, &[], output);
        let n = output.len();
        let ctx = &*ctx;
        self.grains.triggers(inputs[0], n, |g, i| {
            let bufnum = at(inputs[1], i).max(0.0) as usize;
            let Some(b) = ctx.buffer(bufnum) else {
                return;
            };
            if b.channels() != 1 || b.frames() == 0 {
                return;
            }
            let len = (at(inputs[4], i) as f64 * sr as f64).floor().max(4.0);
            let rate = (at(inputs[2], i) * rate_scale(b, sr)) as f64;
            let center = (at(inputs[3], i) * b.sample_rate() as f32) as f64;
            let voice = Voice::Buf {
                buf: bufnum,
                phase: center - 0.5 * len * rate,
                rate,
                interp: at(inputs[7], i) as i32,
            };
            let gain = pan_gain(at(inputs[5], i), nchans, chan).map(|p| p * at(inputs[6], i));
            g.start(ctx, &[], output, i, len, -1.0, voice, gain);
        });
    }
}

// --- Warp1 -------------------------------------------------------------------

/// `Warp1(bufnum, pointer, freqScale, windowSize, envbufnum, overlaps,
/// windowRandRatio, interp, chan)`: channel `chan` of a buffer stretched in
/// time -- grains `windowSize` long (moved at random by up to
/// `windowRandRatio` of it), `overlaps` of them at once, each read from
/// `pointer` (0 to 1 of the buffer) at `freqScale` times its rate. At most
/// 64 grains at once, as scsynth's.
///
/// scsynth's, once its grains reach the limit or a window buffer is
/// missing, never starts another (its next-grain count passes 0 and goes
/// on down); here the next is scheduled as if it had started. And it keeps
/// a grain's first block's position in `f32`, here in `f64`.
pub struct Warp1 {
    rand: SynthRand,
    grains: Grains,
    next: i64,
}

impl Warp1 {
    pub fn new(rand: SynthRand) -> Self {
        Self {
            rand,
            grains: Grains::new(64.0),
            next: 1,
        }
    }
}

impl UGen for Warp1 {
    fn reset(&mut self) {
        self.grains.clear();
        self.next = 1;
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
        let Some(b) = ctx.buffer(inputs[0][0].max(0.0) as usize) else {
            return;
        };
        let chan = inputs[8][0].max(0.0) as usize;
        if b.frames() == 0 || chan >= b.channels() {
            return;
        }
        let sr = ctx.sample_rate;
        // The grains read a channel of a multichannel buffer: the voice's
        // read takes the channel from the row.
        self.play_channel(ctx, b, chan, output, sr, inputs);
    }
}

impl Warp1 {
    fn play_channel(
        &mut self,
        ctx: &ProcessCtx,
        b: &Buffer,
        chan: usize,
        out: &mut [f32],
        sr: f32,
        inputs: &[&[f32]],
    ) {
        let chans = b.channels();
        let frames = b.frames() as f64;
        let mut i = 0;
        while i < self.grains.mine.len() {
            let g = &mut self.grains.mine[i];
            if play_warp(g, ctx, b, chans, chan, frames, out, 0) {
                i += 1;
            } else {
                self.grains.mine.swap_remove(i);
            }
        }
        for i in 0..out.len() {
            self.next -= 1;
            if self.next != 0 {
                continue;
            }
            let overlaps = at(inputs[5], i);
            let rand = self.rand.draw(|r| r.frand2()) as f64 * at(inputs[6], i) as f64;
            let len = (at(inputs[3], i) as f64 * sr as f64) as f32 as f64;
            let length = (len + len * rand).floor().max(4.0) as f32;
            let counter = length as i64;
            // At least a sample on: scsynth's, at 0, never starts another.
            self.next = ((length / overlaps) as i64).max(1);
            if self.grains.mine.len() >= self.grains.max {
                continue;
            }
            let Some(env) = Env::new(ctx, at(inputs[4], i) as i32 as f32, length as f64) else {
                continue;
            };
            let mut g = Grain {
                env,
                voice: Voice::Buf {
                    buf: 0,
                    phase: at(inputs[1], i) as f64 * frames,
                    rate: (at(inputs[2], i) * rate_scale(b, sr)) as f64,
                    interp: at(inputs[7], i) as i32,
                },
                counter,
                gain: 1.0,
            };
            if play_warp(&mut g, ctx, b, chans, chan, frames, out, i) {
                self.grains.mine.push(g);
            }
        }
    }
}

/// One `Warp1` grain from block sample `from`: a channel of a multichannel
/// buffer, so read here rather than by [`Grains::play`].
#[allow(clippy::too_many_arguments)]
fn play_warp(
    g: &mut Grain,
    ctx: &ProcessCtx,
    b: &Buffer,
    chans: usize,
    chan: usize,
    frames: f64,
    out: &mut [f32],
    from: usize,
) -> bool {
    let n = (g.counter.max(0) as usize).min(out.len() - from);
    let table = g.env.table(ctx);
    if matches!(g.env.window, Window::Table { .. }) && table.is_none() {
        return false;
    }
    let Voice::Buf {
        phase,
        rate,
        interp,
        ..
    } = &mut g.voice
    else {
        return false;
    };
    for s in &mut out[from..from + n] {
        *phase = gloop(*phase, frames);
        *s += g.env.amp * read(b, chans, chan, *phase, *interp);
        *phase += *rate;
        g.env.step(table);
    }
    g.counter -= n as i64;
    g.counter > 0
}

// --- GrainTap ----------------------------------------------------------------

/// The most grains a `GrainTap` holds at once, scsynth's.
const TAP_GRAINS: usize = 32;

#[derive(Clone, Copy, Default)]
struct TapGrain {
    pos: f32,
    rate: f32,
    level: f32,
    slope: f32,
    curve: f32,
    counter: i64,
}

/// `GrainTap(bufnum, grainDur, pchRatio, pchDispersion, timeDispersion,
/// overlap)`: grains of a delay line in a buffer whose length is a power of
/// two, which a delay writes as this reads (a `BufDelay` from the same
/// block on): `overlap` grains of `grainDur` at once, each read at
/// `pchRatio` (moved at random by up to `pchDispersion`) from up to
/// `timeDispersion` seconds back, under a parabola. Silent over a buffer
/// whose length is not a power of two, as scsynth's.
///
/// scsynth's plays a grain started in a block to the block's end whatever
/// its length, so a grain shorter than what is left of the block runs past
/// its parabola into negative levels; here it ends where it lasts.
pub struct GrainTap {
    rand: SynthRand,
    grains: [TapGrain; TAP_GRAINS],
    active: usize,
    write: usize,
    next: i64,
    size: usize,
}

impl GrainTap {
    pub fn new(rand: SynthRand) -> Self {
        Self {
            rand,
            grains: [TapGrain::default(); TAP_GRAINS],
            active: 0,
            write: 0,
            next: 0,
            size: 0,
        }
    }
}

impl UGen for GrainTap {
    fn reset(&mut self) {
        self.active = 0;
        self.write = 0;
        self.next = 0;
        self.size = 0;
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
        let Some(b) = ctx.buffer(inputs[0][0].max(0.0) as usize) else {
            return;
        };
        let len = b.len();
        if self.size == 0 {
            self.size = len;
        }
        if len != self.size || !len.is_power_of_two() || len < 4 * crate::dsp::BLOCK_SIZE {
            return;
        }
        let mask = len - 1;
        let n = output.len();
        let sr = ctx.sample_rate;
        let block = crate::dsp::BLOCK_SIZE as f32;
        let delay_len = len as f32 - 2.0 * block - 3.0;
        let density = inputs[5][0].max(0.0001);
        let tap = |g: &mut TapGrain, out: &mut [f32], from: usize, write0: usize| {
            let k = (g.counter.max(0) as usize).min(out.len() - from);
            let mut w = write0;
            for s in &mut out[from..from + k] {
                g.pos += g.rate;
                let id = g.pos as i64;
                let frac = g.pos - id as f32;
                w = (w + 1) & mask;
                let r = (w as i64 - id) as usize & mask;
                let (d1, d2) = (b.at(r), b.at(r.wrapping_sub(1) & mask));
                *s += (d1 + frac * (d2 - d1)) * g.level;
                g.level += g.slope;
                g.slope += g.curve;
            }
            g.counter -= k as i64;
            g.counter > 0
        };
        let write0 = self.write;
        let mut i = 0;
        while i < self.active {
            if tap(&mut self.grains[i], output, 0, write0) {
                i += 1;
            } else {
                self.active -= 1;
                self.grains[i] = self.grains[self.active];
            }
        }
        let mut remain = n as i64;
        while self.next <= remain {
            remain -= self.next;
            let sdur = (inputs[1][0] * sr).max(4.0);
            if self.active < TAP_GRAINS {
                let offset = n - remain as usize;
                let timedisp = inputs[4][0].max(0.0);
                let timedisp = self.rand.draw(|r| r.frand()) * timedisp * sr;
                let pitch = inputs[2][0] + self.rand.draw(|r| r.frand2()) * inputs[3][0];
                let (rate, pos) = if pitch >= 1.0 {
                    let pitch = pitch.min(1.0 + delay_len / sdur);
                    let rate = 1.0 - pitch;
                    let timedisp = timedisp.min(delay_len + sdur * rate);
                    let pos = block + offset as f32 + 2.0 + timedisp - sdur * rate;
                    (rate, pos.min(delay_len))
                } else {
                    let pitch = pitch.max(-(1.0 + delay_len / sdur));
                    let rate = 1.0 - pitch;
                    let timedisp = timedisp.min(delay_len - sdur * rate);
                    let pos = block + offset as f32 + 2.0 + timedisp;
                    (rate, pos.min(delay_len))
                };
                let rdur = 1.0 / sdur;
                let mut g = TapGrain {
                    pos,
                    rate,
                    level: 0.0,
                    slope: 4.0 * (rdur - rdur * rdur),
                    curve: -8.0 * rdur * rdur,
                    counter: sdur as i64,
                };
                let w = (write0 + offset) & mask;
                if tap(&mut g, output, offset, w) {
                    self.grains[self.active] = g;
                    self.active += 1;
                }
            }
            self.next = ((sdur / density) as i64).max(1);
        }
        self.write = (write0 + n) & mask;
        self.next = (self.next - remain).max(0);
    }
}
