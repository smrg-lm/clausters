//! UGens and DSP algorithms.
//!
//! A UGen processes one block through `process`: inputs are slices that are
//! either a full block (a wire from an earlier UGen) or a single sample (a
//! constant or a control). Use [`at`] to read them uniformly. Everything here
//! runs on the audio thread: no allocation.

pub mod binop;
pub mod noise;
pub mod registry;
pub mod sinosc;

/// Maximum inputs per UGen; lets the synth build its input list on the stack.
pub const MAX_UGEN_INPUTS: usize = 8;

pub struct ProcessCtx {
    pub sample_rate: f32,
}

pub trait UGen: Send {
    /// Writes one block into `output`. `inputs` are full-block or length-1
    /// slices, already resolved by the synth.
    fn process(&mut self, ctx: &ProcessCtx, inputs: &[&[f32]], output: &mut [f32]);
}

/// Reads input `i` from a block or a single-sample slice.
#[inline(always)]
pub fn at(input: &[f32], i: usize) -> f32 {
    if input.len() == 1 { input[0] } else { input[i] }
}
