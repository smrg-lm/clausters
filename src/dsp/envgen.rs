use crate::dsp::{at, DoneAction, ProcessCtx, UGen};

pub struct EnvGen {
    current_segment: usize,
    segment_phase: usize,
    gate_prev: f32,
    finished: bool,
    start_level: f32,
    last_val: f32,
    done_action: DoneAction,
}

impl EnvGen {
    pub fn new() -> Self {
        Self {
            current_segment: 0,
            segment_phase: 0,
            gate_prev: 0.0,
            finished: false,
            start_level: 0.0,
            last_val: 0.0,
            done_action: DoneAction::None,
        }
    }
}

impl UGen for EnvGen {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        // inputs:
        // 0: gate, 1: levelScale, 2: levelBias, 3: timeScale, 4: doneAction
        // 5: initLevel, 6: numSegments, 7: releaseNode, 8: loopNode
        // 9...: [target, duration, shape, curve] per segment
        if inputs.len() < 9 {
            output.fill(0.0);
            return;
        }

        let done_act = at(inputs[4], 0) as i32;
        self.done_action = match done_act {
            0 => DoneAction::None,
            1 => DoneAction::PauseSelf,
            2 => DoneAction::FreeSelf,
            14 => DoneAction::FreeGroup,
            _ => DoneAction::None,
        };

        let num_segments = at(inputs[6], 0) as usize;
        let release_node = at(inputs[7], 0) as i32;

        for i in 0..ctx.frames {
            let gate = at(inputs[0], i);
            let level_scale = at(inputs[1], i);
            let level_bias = at(inputs[2], i);
            let time_scale = at(inputs[3], i);

            let trig = gate > 0.0 && self.gate_prev <= 0.0;
            let released = gate <= 0.0 && self.gate_prev > 0.0;
            self.gate_prev = gate;

            if trig {
                self.current_segment = 0;
                self.segment_phase = 0;
                self.finished = false;
                self.start_level = at(inputs[5], i);
                self.last_val = self.start_level;
            } else if released && !self.finished {
                if release_node >= 0 && release_node < num_segments as i32 {
                    self.start_level = self.last_val;
                    self.current_segment = release_node as usize;
                    self.segment_phase = 0;
                }
            }

            if self.finished {
                output[i] = self.last_val * level_scale + level_bias;
                continue;
            }

            // Advance segments if completed
            loop {
                if self.current_segment >= num_segments {
                    self.finished = true;
                    break;
                }
                let base_idx = 9 + self.current_segment * 4;
                if base_idx + 1 >= inputs.len() {
                    self.finished = true;
                    break;
                }
                let duration = at(inputs[base_idx + 1], i) * time_scale;
                let duration_samples = (duration * ctx.sample_rate).max(1.0) as usize;

                if self.segment_phase < duration_samples {
                    break;
                }

                // Move to next segment
                self.start_level = at(inputs[base_idx], i);
                self.last_val = self.start_level;
                self.current_segment += 1;
                self.segment_phase -= duration_samples;
            }

            if self.finished {
                output[i] = self.last_val * level_scale + level_bias;
                continue;
            }

            // Interpolate current segment
            let base_idx = 9 + self.current_segment * 4;
            let target_level = at(inputs[base_idx], i);
            let duration = at(inputs[base_idx + 1], i) * time_scale;
            let duration_samples = (duration * ctx.sample_rate).max(1.0) as usize;
            let shape = at(inputs[base_idx + 2], i) as i32;

            let frac = self.segment_phase as f32 / duration_samples as f32;

            let val = match shape {
                2 => { // Exponential
                    let st = if self.start_level.abs() < 1e-5 { 1e-5 * self.start_level.signum().max(1.0) } else { self.start_level };
                    let en = if target_level.abs() < 1e-5 { 1e-5 * target_level.signum().max(1.0) } else { target_level };
                    if st.signum() == en.signum() {
                        st * (en / st).powf(frac)
                    } else {
                        self.start_level + frac * (target_level - self.start_level)
                    }
                }
                _ => { // Linear (1) or others
                    self.start_level + frac * (target_level - self.start_level)
                }
            };

            self.last_val = val;
            output[i] = val * level_scale + level_bias;
            self.segment_phase += 1;
        }
    }

    fn done(&self) -> DoneAction {
        if self.finished {
            self.done_action
        } else {
            DoneAction::None
        }
    }
}
