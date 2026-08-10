//! The **rolling analysis** a retained live time-frequency view draws.
//!
//! A stored spectrogram is analyzed once: the samples are all there, the
//! transform is computed, the texture is uploaded. A live one has no such
//! moment — the samples arrive a tick at a time, and the picture is the last
//! `retention` seconds of them. Recomputing the whole transform each tick would
//! redo hundreds of FFTs to learn what the newest one or two say, so the
//! **columns are what is kept**: each whole hop that becomes available is
//! analyzed once, pushed on the back, and the oldest falls off the front.
//!
//! The state is keyed by widget rather than by bus, because two views of one
//! bus may analyze it differently (a different `fft_size`, a different `hop`) —
//! the history they read is shared, the transform of it is not.
//!
//! What this module deliberately does *not* do is own a texture. It hands back
//! an ordinary [`Stft`], the same type the stored path produces, so the
//! renderer, the frequency ruler and the cursor readout stay one implementation
//! and a retained waterfall is a spectrogram in every respect but where its
//! samples came from.

use crate::spectrogram::{MAX_FRAMES, Stft, analysis_window, column_into};

/// One retaining time-frequency view's rolling transform.
#[derive(Clone, Debug)]
pub(crate) struct Waterfall {
    /// Frame-major magnitudes, oldest column first.
    mags: Vec<f32>,
    n_bins: usize,
    window_size: usize,
    hop: usize,
    sample_rate: f32,
    /// The stream position of the first sample of the **next** column to
    /// analyze. The analysis is anchored to the bus's own stream rather than to
    /// the history's start, so a column covers the same samples however much
    /// history happens to be retained around it.
    next: Option<u64>,
    /// How many columns the span asks for.
    capacity: usize,
    /// Whether a column landed since the last time the renderer asked.
    dirty: bool,
    /// The analysis window and its gain, plus the per-column scratch — held so
    /// a landing column allocates nothing.
    hann: Vec<f32>,
    gain: f32,
    windowed: Vec<f32>,
    spectrum: Vec<f32>,
}

impl Waterfall {
    /// A view analyzing at `window_size`/`hop`, retaining `capacity` columns.
    pub fn new(window_size: usize, hop: usize, sample_rate: f32, capacity: usize) -> Waterfall {
        let (hann, gain) = analysis_window(window_size);
        Waterfall {
            mags: Vec::new(),
            n_bins: window_size / 2,
            window_size,
            hop: hop.max(1),
            sample_rate,
            next: None,
            capacity: capacity.max(1),
            dirty: false,
            hann,
            gain,
            windowed: vec![0.0; window_size],
            spectrum: vec![0.0; window_size / 2],
        }
    }

    /// Whether the analysis parameters still match — a `/gui_set` of any of
    /// them restarts the roll rather than splicing two transforms, since the
    /// columns of one are not the columns of the other.
    pub fn matches(&self, window_size: usize, hop: usize, sample_rate: f32) -> bool {
        self.window_size == window_size && self.hop == hop.max(1) && self.sample_rate == sample_rate
    }

    /// Resizes the retained span in columns, dropping the oldest when it
    /// shrinks (a live `/gui_set retention`).
    pub fn set_capacity(&mut self, capacity: usize) {
        let capacity = capacity.clamp(1, MAX_FRAMES);
        if capacity != self.capacity {
            self.capacity = capacity;
            self.trim();
            self.dirty = true;
        }
    }

    /// Analyzes every whole window that `history` now holds and this view has
    /// not seen, where `end` is the stream position just past its newest
    /// sample. Returns how many columns landed.
    ///
    /// The history is a moving window of the bus, so a column can be *missed*
    /// rather than merely late: when the retained span no longer reaches back
    /// to where the next column would start, the analysis skips forward to the
    /// oldest sample still held. That is a real discontinuity in the picture
    /// and it is the honest one — the samples it would have covered are gone.
    pub fn advance(&mut self, history: &[f32], end: u64) -> usize {
        if history.len() < self.window_size {
            return 0;
        }
        let start = end - history.len() as u64; // position of `history[0]`
        let mut next = match self.next {
            Some(next) if next >= start => next,
            _ => start,
        };
        let mut landed = 0;
        while next + self.window_size as u64 <= end {
            let at = (next - start) as usize;
            self.push(&history[at..at + self.window_size]);
            next += self.hop as u64;
            landed += 1;
        }
        self.next = Some(next);
        if landed > 0 {
            self.dirty = true;
        }
        landed
    }

    /// Analyzes one window into a column, through the very function the
    /// stored transform uses — so a retained waterfall and an offline
    /// spectrogram of the same audio are the same picture.
    fn push(&mut self, frame: &[f32]) {
        let at = self.mags.len();
        self.mags.resize(at + self.n_bins, 0.0);
        column_into(
            frame,
            &self.hann,
            self.gain,
            &mut self.windowed,
            &mut self.spectrum,
            &mut self.mags[at..],
        );
        self.trim();
    }

    fn trim(&mut self) {
        let max = self.capacity * self.n_bins;
        if self.mags.len() > max {
            let excess = self.mags.len() - max;
            self.mags.drain(..excess);
        }
    }

    /// The retained columns as the transform the renderer uploads, or `None`
    /// before the first column landed. Clears the dirty flag.
    pub fn take_stft(&mut self) -> Option<Stft> {
        if self.mags.is_empty() {
            return None;
        }
        self.dirty = false;
        Some(Stft::from_columns(
            self.mags.clone(),
            self.n_bins,
            self.hop,
            self.window_size,
            self.sample_rate,
        ))
    }

    /// Whether a column landed (or the span moved) since the renderer last
    /// took the transform — what keeps the texture upload to the ticks that
    /// changed something.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// How many columns are retained. A test accessor: what the renderer
    /// wants is the transform, which carries its own frame count.
    #[cfg(test)]
    pub fn columns(&self) -> usize {
        self.mags.len().checked_div(self.n_bins).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A steady tone: every column that lands measures the same thing, and the
    /// bin it lands in is the tone's.
    #[test]
    fn a_column_lands_per_hop_and_measures_the_tone() {
        let sr = 48_000.0f32;
        let ws = 256;
        let hop = 64;
        let mut w = Waterfall::new(ws, hop, sr, 100);
        // A 3 kHz sine: bin = 3000 / (48000 / 256) = 16.
        let n = 1024;
        let history: Vec<f32> = (0..n)
            .map(|i| (std::f32::consts::TAU * 3000.0 * i as f32 / sr).sin())
            .collect();
        let landed = w.advance(&history, n as u64);
        // Windows at 0, hop, 2*hop, ... while a whole one still fits.
        assert_eq!(landed, 1 + (n - ws) / hop);
        assert_eq!(w.columns(), landed);
        let stft = w.take_stft().unwrap();
        let bins = stft.n_bins();
        let col = &stft.magnitudes()[..bins];
        let peak = col
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert_eq!(peak, 16, "the tone's bin");
        // ...and it is the same column the stored transform computes.
        let stored = Stft::compute(&history, ws, hop, sr);
        assert_eq!(&stored.magnitudes()[..bins], col);
    }

    /// The roll is anchored to the bus's stream, not to the history's start:
    /// feeding the same samples again as a longer history adds only the
    /// columns that are genuinely new.
    #[test]
    fn advancing_twice_does_not_re_analyze_what_it_saw() {
        let mut w = Waterfall::new(64, 32, 48_000.0, 100);
        let a: Vec<f32> = (0..128).map(|i| i as f32 * 0.001).collect();
        let first = w.advance(&a, 128);
        assert!(first > 0);
        // The same window again: nothing new past the position it reached.
        assert_eq!(w.advance(&a, 128), 0);
        // Sixty-four more samples: two more hops fit.
        let b: Vec<f32> = (0..192).map(|i| i as f32 * 0.001).collect();
        assert_eq!(w.advance(&b, 192), 2);
        assert_eq!(w.columns(), first + 2);
    }

    /// The span is a cap: the oldest columns fall off, and narrowing it live
    /// takes effect at once rather than when the roll next fills.
    #[test]
    fn the_span_caps_the_columns_and_narrows_live() {
        let mut w = Waterfall::new(64, 32, 48_000.0, 4);
        let s: Vec<f32> = (0..1024).map(|i| i as f32 * 0.001).collect();
        w.advance(&s, 1024);
        assert_eq!(w.columns(), 4, "the oldest fall off");
        w.set_capacity(2);
        assert_eq!(w.columns(), 2);
        assert!(w.is_dirty());
    }

    /// A history shorter than one window analyzes nothing rather than
    /// analyzing a partial one — a half-filled window is not a measurement.
    #[test]
    fn a_history_shorter_than_a_window_lands_nothing() {
        let mut w = Waterfall::new(256, 128, 48_000.0, 10);
        assert_eq!(w.advance(&[0.0; 100], 100), 0);
        assert_eq!(w.columns(), 0);
        assert!(w.take_stft().is_none());
    }
}
