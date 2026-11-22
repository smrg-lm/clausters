//! Delay lines beyond the nine of [`super::delay`], scsynth's from its
//! `DelayUGens.cpp`: a plucked string (`Pluck`), a granular pitch shifter
//! (`PitchShift`), and a tap pair over a buffer, one writing it and any
//! number reading it back (`DelTapWr`, `DelTapRd`).
//!
//! Each private line is allocated at build, sized from a constant input as
//! scsynth's constructor sizes it, and is zeroed, which scsynth's is not: its
//! first pass through a line guards every read against memory not yet
//! written, and a zeroed line reads the same without the guard.

use crate::dsp::noise::cubic32;
use crate::dsp::rand::SynthRand;
use crate::dsp::{ProcessCtx, Ramp, UGen, at};

/// `ln(0.001)`: a decay time is the time to fall by 60 dB.
const LOG001: f32 = -6.907_755;

/// The gain a line of `delay` seconds is fed back with to fall by 60 dB in
/// `decay` seconds, negated for a negative decay; scsynth's
/// `sc_CalcFeedback`.
#[inline]
fn feedback(delay: f32, decay: f32) -> f32 {
    if delay == 0.0 || decay == 0.0 {
        return 0.0;
    }
    (LOG001 * delay / decay.abs()).exp().copysign(decay)
}

/// A value too small or too large to keep feeding back, flushed to 0:
/// scsynth's `zapgremlins`.
#[inline]
fn zap(x: f32) -> f32 {
    let a = x.abs();
    if a > 1e-15 && a < 1e15 { x } else { 0.0 }
}

/// A line of at least `frames` samples whose length is a power of two, so a
/// position wraps by a mask.
fn line(frames: f32) -> Vec<f32> {
    vec![0.0; (frames.max(1.0).ceil() as usize).next_power_of_two()]
}

/// `Pluck(in, trig, maxdelaytime, delaytime, decaytime, coef)`: the
/// Karplus-Strong string. A rising `trig` lets one period of `in` (the
/// delay, in samples) into a line that feeds back through a one-pole
/// lowpass, `y = (1 - |coef|) x + coef y'`, with the gain that decays by
/// 60 dB in `decaytime`; the delay is read with cubic interpolation. A
/// control-rate delay, decay or coefficient moves in a line across the
/// block, as scsynth's does.
///
/// The line is sized by `maxdelaytime`, a constant, and the delay is held
/// to it: scsynth's holds it only to the line's length rounded up to a
/// power of two, past which its interpolation reads newer samples. And
/// scsynth's, with an audio-rate trigger, moves a control-rate coefficient
/// only in a block where the delay or the decay moves too, jumping to it
/// otherwise; here it moves in every block, as its other forms move it.
pub struct Pluck {
    line: Vec<f32>,
    mask: usize,
    write: usize,
    /// The longest delay the line serves, in samples.
    max: f32,
    state: Option<PluckState>,
}

#[derive(Clone, Copy)]
struct PluckState {
    dsamp: f32,
    feedbk: f32,
    delay: f32,
    decay: f32,
    coef: f32,
    last: f32,
    prev_trig: f32,
    inputs: u64,
}

impl Pluck {
    pub fn new(max_delay: f32, sample_rate: f32) -> Self {
        let max = (max_delay * sample_rate).max(2.0);
        let line = line(max + 1.0 + crate::dsp::BLOCK_SIZE as f32);
        Self {
            mask: line.len() - 1,
            line,
            write: 0,
            max,
            state: None,
        }
    }

    #[inline]
    fn dsamp(&self, delay: f32, sr: f32) -> f32 {
        (delay * sr).clamp(2.0, self.max)
    }
}

impl UGen for Pluck {
    fn reset(&mut self) {
        self.line.fill(0.0);
        self.write = 0;
        self.state = None;
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let sr = ctx.sample_rate;
        let (x, trig, coef_in) = (inputs[0], inputs[1], inputs[5]);
        let (delay, decay) = (inputs[3][0], inputs[4][0]);
        let mut st = match self.state {
            Some(st) => st,
            None => PluckState {
                dsamp: self.dsamp(delay, sr),
                feedbk: feedback(delay, decay),
                delay,
                decay,
                coef: coef_in[0],
                last: 0.0,
                prev_trig: 0.0,
                inputs: 0,
            },
        };
        let n = output.len();
        let moved = delay != st.delay || decay != st.decay;
        let (dsamp_step, feedbk_step) = if moved {
            (
                (self.dsamp(delay, sr) - st.dsamp) / n as f32,
                (feedback(delay, decay) - st.feedbk) / n as f32,
            )
        } else {
            (0.0, 0.0)
        };
        // A control-rate coefficient moves in a line from the last block's.
        let coef_step = (coef_in[0] - st.coef) / n as f32;
        let period = (delay * sr + 0.5) as u64;
        if trig.len() == 1 {
            if st.prev_trig <= 0.0 && trig[0] > 0.0 {
                st.inputs = period;
            }
            st.prev_trig = trig[0];
        }
        let mask = self.mask;
        for (i, s) in output.iter_mut().enumerate() {
            if trig.len() > 1 {
                if st.prev_trig <= 0.0 && trig[i] > 0.0 {
                    st.inputs = period;
                }
                st.prev_trig = trig[i];
            }
            if moved {
                st.dsamp += dsamp_step;
            }
            let idsamp = st.dsamp as usize;
            let frac = st.dsamp - idsamp as f32;
            let r1 = self.write.wrapping_sub(idsamp);
            let d0 = self.line[r1.wrapping_add(1) & mask];
            let d1 = self.line[r1 & mask];
            let d2 = self.line[r1.wrapping_sub(1) & mask];
            let d3 = self.line[r1.wrapping_sub(2) & mask];
            let input = if st.inputs > 0 {
                st.inputs -= 1;
                at(x, i)
            } else {
                0.0
            };
            let value = cubic32(frac, d0, d1, d2, d3);
            let c = if coef_in.len() > 1 {
                coef_in[i]
            } else {
                st.coef + coef_step * i as f32
            };
            let onepole = ((1.0 - c.abs() as f64) * value as f64 + (c * st.last) as f64) as f32;
            self.line[self.write & mask] = input + st.feedbk * onepole;
            *s = onepole;
            st.last = onepole;
            if moved {
                st.feedbk += feedbk_step;
            }
            self.write = self.write.wrapping_add(1);
        }
        if moved {
            st.delay = delay;
            st.decay = decay;
        }
        st.coef = coef_in[coef_in.len() - 1];
        st.last = zap(st.last);
        self.state = Some(st);
    }
}

/// One of [`PitchShift`]'s four read heads: its delay, how fast the delay
/// moves, and its window's level and slope.
#[derive(Clone, Copy, Default)]
struct Head {
    dsamp: f32,
    dsamp_slope: f32,
    ramp: f32,
    ramp_slope: f32,
}

/// `PitchShift(in, windowSize, pitchRatio, pitchDispersion,
/// timeDispersion)`: four read heads a quarter of a window apart, each
/// reading the line at `pitchRatio` times the speed it is written and
/// faded by a triangle a window long, so the four always sum to a constant
/// (halved, they sum to 1). A head starts a quarter window after the last,
/// its ratio moved by up to `pitchDispersion` and its start by up to
/// `timeDispersion` seconds, at random. The ratio is held to 0..4.
///
/// `windowSize` is read at the start and is at least three samples. The
/// line holds a window at a ratio of 4 plus the longest time dispersion,
/// which scsynth's does not: it sizes the line for the ratio alone, and a
/// head that starts late at a high ratio reads past it onto new samples.
pub struct PitchShift {
    rand: SynthRand,
    line: Vec<f32>,
    mask: usize,
    write: usize,
    framesize: i64,
    slope: f32,
    counter: i64,
    stage: usize,
    heads: [Head; 4],
    /// Before the first block, which stores the first input sample as
    /// scsynth's constructor does.
    first: bool,
}

impl PitchShift {
    pub fn new(rand: SynthRand, winsize: f32, sample_rate: f32) -> Self {
        let winsize = winsize.max(3.0 / sample_rate);
        let line = line(winsize * sample_rate * 4.0 + 4.0 + crate::dsp::BLOCK_SIZE as f32);
        let framesize = (((winsize * sample_rate) as i64) + 2) & !3;
        let mut me = Self {
            rand,
            mask: line.len() - 1,
            line,
            write: 0,
            framesize,
            slope: 2.0 / framesize as f32,
            counter: 0,
            stage: 0,
            heads: [Head::default(); 4],
            first: true,
        };
        me.start();
        me
    }

    /// The state scsynth's constructor leaves: the last quarter of a cycle
    /// under way, every head two samples behind the write.
    fn start(&mut self) {
        let s = self.slope;
        self.stage = 3;
        self.counter = self.framesize >> 2;
        let levels = [(0.5, -s), (1.0, -s), (0.5, s), (0.0, s)];
        for (h, (ramp, ramp_slope)) in self.heads.iter_mut().zip(levels) {
            *h = Head {
                dsamp: 2.0,
                dsamp_slope: 1.0,
                ramp,
                ramp_slope,
            };
        }
    }
}

impl UGen for PitchShift {
    fn reset(&mut self) {
        self.line.fill(0.0);
        self.write = 0;
        self.start();
        self.first = true;
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let sr = ctx.sample_rate;
        let x = inputs[0];
        if std::mem::take(&mut self.first) {
            // scsynth's constructor stores the first input sample.
            self.line[0] = at(x, 0);
        }
        let winsize = inputs[1][0];
        let ratio = inputs[2][0];
        let pdisp = inputs[3][0];
        let tdisp = inputs[4][0].clamp(0.0, winsize.max(0.0)) * sr;
        let mask = self.mask;
        let mut i = 0;
        while i < output.len() {
            if self.counter <= 0 {
                self.counter = self.framesize >> 2;
                self.stage = (self.stage + 1) & 3;
                let mut r = ratio;
                if pdisp != 0.0 {
                    r += pdisp * self.rand.draw(|g| g.frand2());
                }
                let r1 = r.clamp(0.0, 4.0) - 1.0;
                let mut start = if r1 < 0.0 {
                    2.0
                } else {
                    self.framesize as f32 * r1 + 2.0
                };
                start += tdisp * self.rand.draw(|g| g.frand());
                // The head starting rises; the one opposite it falls.
                let s = self.slope;
                let h = &mut self.heads[self.stage];
                h.dsamp_slope = -r1;
                h.dsamp = start;
                h.ramp = 0.0;
                h.ramp_slope = s;
                self.heads[(self.stage + 2) & 3].ramp_slope = -s;
            }
            let n = (output.len() - i).min(self.counter as usize);
            self.counter -= n as i64;
            for s in &mut output[i..i + n] {
                self.write = (self.write + 1) & mask;
                let mut value = 0.0;
                for h in &mut self.heads {
                    h.dsamp += h.dsamp_slope;
                    let id = h.dsamp as i64;
                    let frac = h.dsamp - id as f32;
                    let r = (self.write as i64 - id) as usize & mask;
                    let d1 = self.line[r];
                    let d2 = self.line[r.wrapping_sub(1) & mask];
                    value += (d1 + frac * (d2 - d1)) * h.ramp;
                    h.ramp += h.ramp_slope;
                }
                self.line[self.write] = at(x, i);
                *s = value * 0.5;
                i += 1;
            }
        }
    }
}

/// The buffer a tap names, if it is a mono one with samples: scsynth's
/// taps refuse any other.
fn tap_line<'a>(ctx: &ProcessCtx<'a>, bufnum: f32) -> Option<&'a crate::dsp::buffer::Buffer> {
    let buf = ctx.buffer(bufnum.max(0.0) as usize)?;
    (buf.channels() == 1 && buf.frames() > 0 && buf.cells().is_some()).then_some(buf)
}

/// `DelTapWr(buffer, in)`: writes `in` round a mono buffer, which it clears
/// at its start, and outputs where each sample went -- the phase a
/// [`DelTapRd`] reads behind.
///
/// The phase is the frame as a number, exact to 2^24 frames (5.8 minutes at
/// 48 kHz); scsynth's is the frame's integer bits in the float, which no
/// arithmetic may touch on the way to the reader and which nothing else can
/// read.
#[derive(Default)]
pub struct DelTapWr {
    phase: usize,
    cleared: bool,
}

impl UGen for DelTapWr {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let Some(buf) = tap_line(ctx, inputs[0][0]) else {
            output.fill(0.0);
            return;
        };
        let len = buf.len();
        if !self.cleared {
            self.cleared = true;
            for i in 0..len {
                buf.set_at(i, 0.0);
            }
        }
        let mut phase = self.phase % len;
        for (i, s) in output.iter_mut().enumerate() {
            buf.set_at(phase, at(inputs[1], i));
            *s = phase as f32;
            phase += 1;
            if phase == len {
                phase = 0;
            }
        }
        self.phase = phase;
    }
}

/// `DelTapRd(buffer, phase, delTime, interp)`: reads the buffer a
/// [`DelTapWr`] writes, `delTime` seconds behind the phase it reports for
/// the block's first sample, with no interpolation (`interp` 1), linear (2)
/// or cubic (4). A control-rate `delTime` moves in a line across the block.
///
/// Every read wraps round the buffer. scsynth's linear read at a
/// control-rate delay that does not move wraps the wrong variable at the
/// buffer's end and reads past it, and its truncating read rounds a
/// fractional position towards the newer sample before the buffer's start
/// and the older one after it; here the position is wrapped and then
/// truncated, the same everywhere.
#[derive(Default)]
pub struct DelTapRd {
    delay: Ramp,
}

impl UGen for DelTapRd {
    fn reset(&mut self) {
        self.delay = Ramp::default();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let sr = ctx.sample_rate as f64;
        let Some(buf) = tap_line(ctx, inputs[0][0]) else {
            output.fill(0.0);
            return;
        };
        let len = buf.len();
        let flen = len as f64;
        let interp = inputs[3][0] as i32;
        let dt = inputs[2];
        let (mut delay, step) = if dt.len() == 1 {
            self.delay.toward(dt[0] as f64 * sr, output.len())
        } else {
            (0.0, 0.0)
        };
        let mut phase_in = inputs[1][0].max(0.0) as f64;
        let get = |i: usize| buf.at(if i >= len { i - len } else { i });
        for (i, s) in output.iter_mut().enumerate() {
            let d = if dt.len() == 1 {
                delay
            } else {
                dt[i] as f64 * sr
            };
            let mut p = (phase_in - d).rem_euclid(flen);
            if p >= flen {
                p -= flen;
            }
            let ip = p as usize;
            let frac = (p - ip as f64) as f32;
            *s = match interp {
                2 => {
                    let (b, c) = (get(ip), get(ip + 1));
                    b + frac * (c - b)
                }
                4 => {
                    let a = get(if ip == 0 { len - 1 } else { ip - 1 });
                    cubic32(frac, a, get(ip), get(ip + 1), get(ip + 2))
                }
                _ => get(ip),
            };
            phase_in += 1.0;
            delay += step;
        }
    }
}
