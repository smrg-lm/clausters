//! Physical models, scsynth's from its `PhysicalModelingUGens.cpp`: a mass
//! on a damped spring (`Spring`), and a ball bouncing on a moving floor
//! (`Ball`, its height; `TBall`, its impacts).
//!
//! Each is scsynth's arithmetic, in its units: a position is in samples'
//! worth of the input, a velocity a change per sample and gravity per
//! second, so a spring's frequency is `sqrt(spring * sampleRate) / 2 pi`,
//! as scsynth's is.

use crate::dsp::rand::SynthRand;
use crate::dsp::{ProcessCtx, UGen, at};

/// `Spring(in, spring, damp)`: a mass on a spring driven by the force `in`,
/// its velocity damped by `damp` a sample; the output is the force the
/// spring exerts, `in - spring * position`.
#[derive(Default)]
pub struct Spring {
    pos: f32,
    vel: f32,
}

impl UGen for Spring {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let c = 1.0 / ctx.sample_rate;
        let rc = ctx.sample_rate;
        let spring = inputs[1][0] * c;
        let damping = 1.0 - inputs[2][0];
        let (mut pos, mut vel) = (self.pos, self.vel);
        for (i, s) in output.iter_mut().enumerate() {
            let force = at(inputs[0], i) * c - pos * spring;
            vel = (force + vel) * damping;
            pos += vel;
            *s = force * rc;
        }
        (self.pos, self.vel) = (pos, vel);
    }
}

/// Which output a bouncing ball gives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BallKind {
    /// `Ball`: its height.
    Height,
    /// `TBall`: its velocity at each impact, 0 between.
    Impacts,
}

/// `Ball(in, g, damp, friction)` and `TBall(in, g, damp, friction)`: a ball
/// falling under gravity `g` onto a floor at the height `in`, bouncing off
/// it with its velocity damped by `damp`, and held to the floor by
/// `friction` (proportional to `g`) once it is close enough. Each bounce
/// adds a dither from the synth's generator, scsynth's guard against the
/// ball settling into a sampled rhythm.
pub struct Ball {
    kind: BallKind,
    rand: SynthRand,
    state: Option<(f64, f32, f64)>,
}

impl Ball {
    pub fn new(kind: BallKind, rand: SynthRand) -> Self {
        Self {
            kind,
            rand,
            state: None,
        }
    }
}

impl UGen for Ball {
    fn reset(&mut self) {
        self.state = None;
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let x0 = at(inputs[0], 0) as f64;
        let (mut pos, mut vel, mut prev) = *self.state.get_or_insert((x0, 0.0, x0));
        let g_in = inputs[1][0];
        let damping = 1.0 - inputs[2][0];
        let c = 1.0 / ctx.sample_rate;
        let maxvel = c * 1000.0;
        let impacts = self.kind == BallKind::Impacts;
        // `TBall` pulls a ball close to the floor onto it ten times faster.
        let inter = c * if impacts { 10_000.0 } else { 1000.0 };
        let g = c * g_in;
        let k = (inputs[3][0] as f64 * g_in as f64) as f32;
        let dither = if impacts { 0.001 } else { 0.000_05 };
        let kf = k as f64;
        for (i, s) in output.iter_mut().enumerate() {
            let mut out = 0.0f32;
            vel -= g;
            // scsynth's precisions: `TBall` keeps its height and the floor's
            // in `f64`, `Ball` everything in `f32`.
            if impacts {
                let floor = at(inputs[0], i) as f64;
                pos += vel as f64;
                let dist = pos - floor;
                let floorvel = ((floor - prev) as f32).clamp(-maxvel, maxvel);
                let vel_diff = floorvel - vel;
                if dist.abs() < kf {
                    if dist.abs() < (k * 0.005) as f64 {
                        vel = 0.0;
                        pos = floor + g as f64;
                    } else {
                        vel += vel_diff * inter;
                        pos += (floor - pos) * inter as f64;
                    }
                } else if dist <= 0.0 {
                    pos = floor - dist;
                    vel = vel_diff * damping;
                    out = vel;
                    vel += self.rand.draw(|r| r.frand()) * dither * g_in;
                }
                prev = floor;
                *s = out;
            } else {
                let floor = at(inputs[0], i);
                let mut p = pos as f32;
                p += vel;
                let dist = p - floor;
                let floorvel = (floor - prev as f32).clamp(-maxvel, maxvel);
                let vel_diff = floorvel - vel;
                if dist.abs() < k {
                    if dist.abs() < k * 0.005 {
                        vel = 0.0;
                        p = floor + g;
                    } else {
                        vel += vel_diff * inter;
                        p += (floor - p) * inter;
                    }
                } else if dist <= 0.0 {
                    p = floor - dist;
                    vel = vel_diff * damping;
                    vel += self.rand.draw(|r| r.frand()) * dither * g_in;
                }
                pos = p as f64;
                prev = floor as f64;
                *s = p;
            }
        }
        self.state = Some((pos, vel, prev));
    }
}
