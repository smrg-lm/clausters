use std::f64::consts::TAU;

/// Sine oscillator by phase accumulation. The phase is kept in `f64` so the
/// tuning does not degrade over long sessions.
pub struct SinOsc {
    freq: f32,
    amp: f32,
    phase: f64,
}

impl SinOsc {
    pub fn new(freq: f32, amp: f32) -> Self {
        Self {
            freq,
            amp,
            phase: 0.0,
        }
    }

    pub fn set_freq(&mut self, freq: f32) {
        self.freq = freq;
    }

    pub fn set_amp(&mut self, amp: f32) {
        self.amp = amp;
    }

    pub fn process(&mut self, sample_rate: f32, out: &mut [f32]) {
        let inc = TAU * self.freq as f64 / sample_rate as f64;
        for s in out.iter_mut() {
            *s = self.phase.sin() as f32 * self.amp;
            self.phase += inc;
            if self.phase >= TAU {
                self.phase -= TAU;
            }
        }
    }
}
