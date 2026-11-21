use crate::dsp::{ProcessCtx, UGen};

/// Band-unlimited impulse train: a single-sample `1.0` every `1/freq`
/// seconds, `0.0` in between -- scsynth's `Impulse`, ported from its code.
///
/// Inputs: 0 the frequency in Hz, 1 the **phase offset** in cycles. The phase
/// starts at the offset, and an offset of `0` starts it "due", so the
/// **first** output sample is an impulse. Combined with a `/sched_at`'d
/// `/synth_new` -- which splits the processing block at the target sample, so
/// the synth's first sample *is* the target -- this places one pristine
/// impulse on an exact sample of the clock, and a frequency of `0` then emits
/// that single impulse and silence forever after, which is how
/// `examples/clock_recorder.py` marks each scheduled instant.
///
/// What scsynth's does and this keeps:
///
/// - **A negative frequency runs the phase backwards**, and an impulse fires
///   each time it wraps below 0.
/// - **The offset is a running offset.** Moving it moves the phase by the
///   same amount, and an impulse fires if that carries the phase across a
///   wrap -- so a modulated offset can place impulses between the ones the
///   frequency gives.
/// - **A control-rate frequency or offset is interpolated** across the slice
///   ([`Ramp`](crate::dsp::Ramp)), so a swept train does not step at the block
///   rate.
/// - A phase that jumps more than a cycle in one sample fires once and lands
///   inside the cycle.
pub struct Impulse {
    /// Where the train is in its cycle.
    phase: f64,
    /// The last increment, in cycles per sample.
    inc: f64,
    /// The last offset, in cycles.
    offset: f64,
    started: bool,
}

impl Impulse {
    pub fn new() -> Self {
        Self {
            phase: 0.0,
            inc: 0.0,
            offset: 0.0,
            started: false,
        }
    }
}

impl Default for Impulse {
    fn default() -> Self {
        Self::new()
    }
}

/// scsynth's `Impulse_testWrapPhase`: whether the phase crossed a wrap in the
/// direction `inc` runs, wrapping it back into the cycle if so.
#[inline]
fn wrapped(inc: f64, phase: &mut f64) -> bool {
    if inc < 0.0 {
        if *phase <= 0.0 {
            *phase += 1.0;
            if *phase <= 0.0 {
                *phase -= phase.ceil();
            }
            return true;
        }
    } else if *phase >= 1.0 {
        *phase -= 1.0;
        if *phase >= 1.0 {
            *phase -= phase.floor();
        }
        return true;
    }
    false
}

impl UGen for Impulse {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let freq = inputs[0];
        let offset = inputs.get(1).copied().unwrap_or(&[0.0]);
        let mul = 1.0 / ctx.sample_rate as f64;
        let frames = output.len();
        if !self.started {
            self.started = true;
            self.inc = freq[0] as f64 * mul;
            self.offset = offset[0] as f64;
            let p = self.offset - self.offset.floor();
            self.phase = if p == 0.0 && self.inc >= 0.0 { 1.0 } else { p };
        }
        // A control-rate offset spreads its change across the slice; an
        // audio-rate one moves by its own per-sample difference.
        let off_step = if offset.len() == 1 {
            (offset[0] as f64 - self.offset) / frames.max(1) as f64
        } else {
            0.0
        };
        let inc_step = if freq.len() == 1 {
            (freq[0] as f64 * mul - self.inc) / frames.max(1) as f64
        } else {
            0.0
        };
        for (i, s) in output.iter_mut().enumerate() {
            *s = if wrapped(self.inc, &mut self.phase) {
                1.0
            } else {
                0.0
            };
            let moved = if offset.len() == 1 {
                off_step
            } else {
                let o = offset[i] as f64;
                let d = o - self.offset;
                self.offset = o;
                d
            };
            if moved != 0.0 {
                self.phase += moved;
                wrapped(self.inc, &mut self.phase);
            }
            if freq.len() == 1 {
                self.inc += inc_step;
            } else {
                self.inc = freq[i] as f64 * mul;
            }
            self.phase += self.inc;
        }
        if offset.len() == 1 {
            self.offset = offset[0] as f64;
        }
        if freq.len() == 1 {
            self.inc = freq[0] as f64 * mul;
        }
    }
}
