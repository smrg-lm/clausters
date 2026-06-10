//! The hardcoded "default" synth of M2: a single SinOsc. It stands in for
//! SynthDef-built instances until the SynthDef interpreter arrives in M3.

use crate::dsp::sinosc::SinOsc;

/// Control indices, scsynth-style: settable by index or by name.
pub const CTL_FREQ: u32 = 0;
pub const CTL_AMP: u32 = 1;

pub fn control_index(name: &str) -> Option<u32> {
    match name {
        "freq" => Some(CTL_FREQ),
        "amp" => Some(CTL_AMP),
        _ => None,
    }
}

pub struct DefaultSynth {
    osc: SinOsc,
}

impl DefaultSynth {
    pub fn new(freq: f32, amp: f32) -> Self {
        Self {
            osc: SinOsc::new(freq, amp),
        }
    }

    /// Unknown indices are ignored, like scsynth does with unknown controls.
    pub fn set_control(&mut self, index: u32, value: f32) {
        match index {
            CTL_FREQ => self.osc.set_freq(value),
            CTL_AMP => self.osc.set_amp(value),
            _ => {}
        }
    }

    pub fn process(&mut self, sample_rate: f32, out: &mut [f32]) {
        self.osc.process(sample_rate, out);
    }
}
