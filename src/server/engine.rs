//! Processing engine, independent of the audio backend.

use crate::dsp::sinosc::SinOsc;

/// Frames per processing block, like scsynth.
pub const BLOCK_SIZE: usize = 64;

/// Engine state. In M0 it holds a hardcoded sine; from M2 onwards this is
/// where the node tree, the buses and the command FIFOs live.
pub struct Engine {
    sample_rate: f32,
    channels: usize,
    sine: SinOsc,
    mono: [f32; BLOCK_SIZE],
}

impl Engine {
    pub fn new(sample_rate: f32, channels: usize) -> Self {
        assert!(channels > 0);
        Self {
            sample_rate,
            channels,
            sine: SinOsc::new(440.0, 0.2),
            mono: [0.0; BLOCK_SIZE],
        }
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Processes one block. `out` is interleaved and its length must be
    /// `BLOCK_SIZE * channels`. Runs on the audio thread: does not allocate.
    pub fn process_block(&mut self, out: &mut [f32]) {
        debug_assert_eq!(out.len(), BLOCK_SIZE * self.channels);
        self.sine.process(self.sample_rate, &mut self.mono);
        for (frame, &s) in out.chunks_exact_mut(self.channels).zip(self.mono.iter()) {
            frame.fill(s);
        }
    }
}
