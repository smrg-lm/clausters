//! `VDiskIn`: a file streamed from disk at a variable rate, scsynth's from
//! its `DiskIO_UGens.cpp`, over the same stream a [`DiskIn`] reads natively
//! and in a page.
//!
//! The stream is a `DiskIn`'s, asked for as many frames as the block's
//! rate moves through; what is read is a window of four frames, and the
//! output is the cubic between its middle two at the fraction the rate has
//! reached. So the rate and the interpolation are written once, and the
//! reader under them is whichever the platform has.

use crate::dsp::disk::DiskIn;
use crate::dsp::noise::cubic32;
use crate::dsp::registry::{BuildCtx, UGenConfig};
use crate::dsp::{BLOCK_SIZE, ProcessCtx, Ramp, UGen};

/// Frames asked of the stream at once.
const CHUNK: usize = 4 * BLOCK_SIZE;

/// `VDiskIn(chan, rate)`: a channel of a file streamed from disk, `rate`
/// frames of the file a sample (none backwards: a negative rate is 0),
/// read with cubic interpolation. The file, its loop and its start are the
/// static fields `DiskIn` takes. A control-rate `rate` moves in a line
/// across the block, as scsynth's does.
///
/// scsynth's caps the rate by the size of the cue buffer it reads through
/// and is silent past it; this stream has no cue buffer, and any rate
/// reads. Its `sendID`, which reports how many cue buffers it has read, is
/// not taken, for the same reason. And its first block plays the file's
/// first frame twice (it writes it, then reads it again before moving);
/// here frame `k` plays at sample `k / rate`.
pub struct VDiskIn {
    inner: DiskIn,
    chunk: Vec<f32>,
    have: usize,
    next: usize,
    /// The four frames the cubic reads: the one before, the two either side
    /// of the position, the one after.
    window: [f32; 4],
    frac: f64,
    rate: Ramp,
    started: bool,
}

impl VDiskIn {
    pub fn open(config: &UGenConfig, ctx: &BuildCtx) -> Self {
        Self {
            inner: DiskIn::open(config, ctx),
            chunk: vec![0.0; CHUNK],
            have: 0,
            next: 0,
            window: [0.0; 4],
            frac: 0.0,
            rate: Ramp::default(),
            started: false,
        }
    }

    /// The stream's next frame, `left` more still wanted this block.
    #[inline]
    fn frame(&mut self, ctx: &mut ProcessCtx, chan: &[f32], left: usize) -> f32 {
        if self.next == self.have {
            let k = left.clamp(1, CHUNK);
            self.inner.process(ctx, &[chan], &mut self.chunk[..k]);
            self.have = k;
            self.next = 0;
        }
        self.next += 1;
        self.chunk[self.next - 1]
    }
}

impl UGen for VDiskIn {
    fn reset(&mut self) {
        // The stream is its reader's and cannot be restarted, as `DiskIn`'s.
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let chan = inputs[0];
        let r = inputs[1];
        let n = output.len();
        let (start, step) = if r.len() == 1 {
            self.rate.toward(r[0].max(0.0) as f64, n)
        } else {
            (0.0, 0.0)
        };
        let rate_at = |i: usize| {
            if r.len() == 1 {
                start + step * (i + 1) as f64
            } else {
                r[i].max(0.0) as f64
            }
        };
        // How many frames the block moves through, by the same arithmetic as
        // the loop below, so the stream is asked for them at once.
        let mut wanted = 0usize;
        let mut frac = self.frac;
        for i in 0..n {
            frac += rate_at(i);
            let whole = frac.floor();
            wanted += whole as usize;
            frac -= whole;
        }
        if !self.started {
            self.started = true;
            wanted += 3;
            // The frame before the file's first is silence.
            self.window = [0.0; 4];
            for k in 1..4 {
                self.window[k] = self.frame(ctx, chan, wanted);
                wanted -= 1;
            }
        }
        for (i, s) in output.iter_mut().enumerate() {
            let w = &self.window;
            *s = cubic32(self.frac as f32, w[0], w[1], w[2], w[3]);
            self.frac += rate_at(i);
            while self.frac >= 1.0 {
                self.frac -= 1.0;
                let f = self.frame(ctx, chan, wanted);
                wanted = wanted.saturating_sub(1);
                // A rate over 1 moves more than one frame a sample.
                self.window.copy_within(1.., 0);
                self.window[3] = f;
            }
        }
    }
}
