//! Sample buffers (M5).
//!
//! A [`Buffer`] is **immutable once built**: the NRT thread allocates and
//! fills it (zeroes or a sound file), the network thread installs it in the
//! engine's pool via `Cmd::SetBuffer`, and from then on it is only read.
//! That makes `Arc<Buffer>` freely shareable across threads with no locks:
//! the audio thread reads it inside `process`, while the network thread
//! keeps a mirror clone for `/b_query`, `/b_write` and `/b_zero`/`/b_read`
//! (which *replace* the buffer with a freshly built one instead of mutating
//! in place — scsynth mutates shared memory; we trade one copy for aliasing
//! safety). Recording UGens would need a different scheme.
//!
//! Replaced or freed buffers leave the audio thread through the garbage
//! FIFO, so the final `Arc` drop (the deallocation) never happens there.

use std::sync::Arc;

/// Size of the buffer pool, like scsynth's default `-b`.
pub const NUM_BUFFERS: usize = 1024;

/// The engine-side pool: index → installed buffer.
pub type BufferPool = Vec<Option<Arc<Buffer>>>;

pub fn empty_pool() -> BufferPool {
    (0..NUM_BUFFERS).map(|_| None).collect()
}

/// Interleaved f32 sample data plus its shape. See the module docs for the
/// immutability contract.
pub struct Buffer {
    data: Vec<f32>,
    channels: usize,
    frames: usize,
    sample_rate: f64,
}

impl std::fmt::Debug for Buffer {
    /// Shape only — buffers hold millions of samples.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Buffer")
            .field("frames", &self.frames)
            .field("channels", &self.channels)
            .field("sample_rate", &self.sample_rate)
            .finish_non_exhaustive()
    }
}

impl Buffer {
    /// `data` is interleaved; its length must be `frames * channels`.
    pub fn new(data: Vec<f32>, channels: usize, frames: usize, sample_rate: f64) -> Self {
        assert_eq!(data.len(), frames * channels);
        Self {
            data,
            channels,
            frames,
            sample_rate,
        }
    }

    pub fn zeroed(frames: usize, channels: usize, sample_rate: f64) -> Self {
        Self::new(vec![0.0; frames * channels], channels, frames, sample_rate)
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Interleaved samples, `frames * channels` long.
    pub fn data(&self) -> &[f32] {
        &self.data
    }

    /// One sample; out-of-range frames or channels read as 0.
    #[inline]
    pub fn sample(&self, frame: usize, channel: usize) -> f32 {
        if frame >= self.frames || channel >= self.channels {
            return 0.0;
        }
        self.data[frame * self.channels + channel]
    }
}
